//! `GET /api/servers/:id` — detail view of a single managed server.
//!
//! Reads the metadata from `SQLite` and concurrently fetches the live
//! StatefulSet/Pod/Service from k8s, tolerating any of them being
//! absent (returns `Ok(None)` to the join). Status and endpoint are
//! derived in [`crate::k8s_status`].

use axum::Json;
use axum::extract::{Path, State};
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::Api;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::AppState;
use crate::error::AppError;
use crate::k8s::{Endpoint, ServerStatus};
use crate::k8s_status::{derive_endpoint, derive_status};

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
    /// Provider discriminator (`vanilla` | `curseforge`).
    pub source_kind: String,
    /// Provider config JSON (parsed). `null` for vanilla.
    pub source_config: serde_json::Value,
    /// `true` when a newer modpack version is cached.
    pub update_available: bool,
    /// `CurseForge` file id of the cached latest, if any.
    pub latest_version_id: Option<i64>,
    /// Display name of the cached latest, if any.
    pub latest_version_name: Option<String>,
    /// First lines of the changelog for the cached latest, if any.
    pub latest_changelog_excerpt: Option<String>,
    /// `true` while the orchestrator is mid-update for this server.
    pub update_in_progress: bool,
}

/// Handler for `GET /api/servers/:id`.
///
/// # Errors
///
/// - 404 if the `SQLite` row is missing.
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

    let (replicas, ready) = sts.as_ref().map_or((0, 0), |s| {
        let r = s.spec.as_ref().and_then(|sp| sp.replicas).unwrap_or(0);
        let ready = s
            .status
            .as_ref()
            .and_then(|st| st.ready_replicas)
            .unwrap_or(0);
        (r, ready)
    });
    let status = derive_status(replicas, ready, pod.as_ref());
    let endpoint = derive_endpoint(
        svc.as_ref(),
        &row.exposure_mode,
        &state.node_host,
        &resource_name,
        &state.mc_namespace,
    );

    // Modpack-version JOIN + lock check; both are independent of the k8s
    // calls above so the fan-out is fine.
    let mv: Option<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT latest_id, latest_name, changelog_excerpt
         FROM modpack_versions WHERE server_id = ?",
    )
    .bind(id)
    .fetch_optional(&state.pool)
    .await?;
    let (source_kind, source_config_text): (String, String) =
        sqlx::query_as("SELECT source_kind, source_config FROM servers WHERE id = ?")
            .bind(id)
            .fetch_one(&state.pool)
            .await?;
    let source_config: serde_json::Value =
        serde_json::from_str(&source_config_text).unwrap_or(serde_json::Value::Null);
    let current_version_id = source_config
        .get("current_version_id")
        .and_then(serde_json::Value::as_i64);
    let (latest_version_id, latest_version_name, latest_changelog_excerpt) = match mv {
        Some((id, name, excerpt)) => (Some(id), Some(name), excerpt),
        None => (None, None, None),
    };
    let update_available = match (current_version_id, latest_version_id) {
        (Some(cur), Some(latest)) => cur != latest,
        _ => false,
    };
    let update_in_progress = state
        .update_locks
        .lock()
        .expect("update_locks poisoned")
        .contains(id);

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
        source_kind,
        source_config,
        update_available,
        latest_version_id,
        latest_version_name,
        latest_changelog_excerpt,
        update_in_progress,
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

/// Tuple shape returned by the SELECT in [`fetch_server_row`]. Aliased
/// to keep the function signature readable.
type ServerRowTuple = (
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
);

/// Fetches one row from `servers` by id. Returns `AppError::NotFound`
/// when no row exists.
pub(crate) async fn fetch_server_row(pool: &SqlitePool, id: &str) -> Result<ServerRow, AppError> {
    let opt: Option<ServerRowTuple> = sqlx::query_as(
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
