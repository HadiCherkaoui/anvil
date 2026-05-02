//! `GET /api/servers/:id` — detail view of a single managed server.
//!
//! Reads the metadata from SQLite and concurrently fetches the live
//! StatefulSet/Pod/Service from k8s, tolerating any of them being
//! absent (returns `Ok(None)` to the join). Status and endpoint are
//! derived in [`crate::k8s_status`].

use axum::extract::{Path, State};
use axum::Json;
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::Api;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::error::AppError;
use crate::k8s::{Endpoint, ServerStatus};
use crate::k8s_status::{derive_endpoint, derive_status};
use crate::AppState;

/// Detail body for `GET /api/servers/:id`.
#[derive(Debug, Serialize)]
pub struct ServerDetail {
    pub id: String,
    pub name: String,
    pub status: ServerStatus,
    pub mc_version: String,
    pub memory_mi: i64,
    pub server_type: String,
    pub exposure_mode: String,
    pub storage_class: Option<String>,
    pub storage_size_gi: i64,
    pub nodeport: Option<i32>,
    pub endpoint: Option<Endpoint>,
    pub created_at: i64,
    pub last_started_at: Option<i64>,
}

/// Handler for `GET /api/servers/:id`.
///
/// # Errors
///
/// - 404 if the SQLite row is missing.
/// - 500 on DB or k8s failure.
pub async fn handle(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<ServerDetail>, AppError> {
    let detail = fetch_detail(&state, &id).await?;
    Ok(Json(detail))
}

/// Shared by start/stop/restart handlers — fetches the same shape
/// `GET /api/servers/:id` returns.
pub(crate) async fn fetch_detail(state: &AppState, id: &str) -> Result<ServerDetail, AppError> {
    let row = fetch_server_row(&state.pool, id).await?;

    let resource_name = format!("mc-{id}");
    let pod_name = format!("{resource_name}-0");

    let stsets: Api<StatefulSet> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let services: Api<Service> = Api::namespaced(state.kube.clone(), &state.mc_namespace);

    let (sts_res, pod_res, svc_res) = tokio::join!(
        stsets.get_opt(&resource_name),
        pods.get_opt(&pod_name),
        services.get_opt(&resource_name),
    );
    let sts = sts_res?;
    let pod = pod_res?;
    let svc = svc_res?;

    let (replicas, ready) = sts
        .as_ref()
        .map(|s| {
            let r = s.spec.as_ref().and_then(|sp| sp.replicas).unwrap_or(0);
            let ready = s
                .status
                .as_ref()
                .and_then(|st| st.ready_replicas)
                .unwrap_or(0);
            (r, ready)
        })
        .unwrap_or((0, 0));
    let status = derive_status(replicas, ready, pod.as_ref());
    let endpoint = derive_endpoint(
        svc.as_ref(),
        &row.exposure_mode,
        &state.node_host,
        &resource_name,
        &state.mc_namespace,
    );

    Ok(ServerDetail {
        id: row.id,
        name: row.name,
        status,
        mc_version: row.mc_version,
        memory_mi: row.memory_mi,
        server_type: row.server_type,
        exposure_mode: row.exposure_mode,
        storage_class: row.storage_class,
        storage_size_gi: row.storage_size_gi,
        nodeport: row.nodeport,
        endpoint,
        created_at: row.created_at,
        last_started_at: row.last_started_at,
    })
}

/// In-memory shape mirroring the `servers` table.
pub(crate) struct ServerRow {
    pub id: String,
    pub name: String,
    pub mc_version: String,
    pub memory_mi: i64,
    pub server_type: String,
    pub exposure_mode: String,
    pub storage_class: Option<String>,
    pub storage_size_gi: i64,
    pub nodeport: Option<i32>,
    pub created_at: i64,
    pub last_started_at: Option<i64>,
}

/// Fetches one row from `servers` by id. Returns `AppError::NotFound`
/// when no row exists.
pub(crate) async fn fetch_server_row(pool: &SqlitePool, id: &str) -> Result<ServerRow, AppError> {
    let opt: Option<(
        String,
        String,
        String,
        i64,
        String,
        String,
        Option<String>,
        i64,
        Option<i64>,
        i64,
        Option<i64>,
    )> = sqlx::query_as(
        "SELECT id, name, mc_version, memory_mi, server_type, exposure_mode,
                storage_class, storage_size_gi, nodeport, created_at, last_started_at
         FROM servers WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    match opt {
        None => Err(AppError::NotFound),
        Some((
            id,
            name,
            mc_version,
            memory_mi,
            server_type,
            exposure_mode,
            storage_class,
            storage_size_gi,
            nodeport,
            created_at,
            last_started_at,
        )) => Ok(ServerRow {
            id,
            name,
            mc_version,
            memory_mi,
            server_type,
            exposure_mode,
            storage_class,
            storage_size_gi,
            nodeport: nodeport.and_then(|n| i32::try_from(n).ok()),
            created_at,
            last_started_at,
        }),
    }
}
