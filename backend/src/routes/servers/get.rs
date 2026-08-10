// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

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
use crate::server_properties::ServerProperties;

/// Detail body for `GET /api/servers/:id`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ServerDetail {
    pub id: String,
    pub name: String,
    pub status: ServerStatus,
    pub mc_version: String,
    pub memory_mi: i64,
    pub exposure_mode: String,
    pub storage_class: Option<String>,
    pub storage_size_gi: i64,
    pub nodeport: Option<i32>,
    pub endpoint: Option<Endpoint>,
    pub created_at: i64,
    pub last_started_at: Option<i64>,
    /// Provider discriminator (`vanilla` | `curseforge` | `modrinth` | `modded` | `paper`).
    pub source_kind: String,
    /// Provider config JSON (parsed). `null` for vanilla.
    #[schema(value_type = Object)]
    pub source_config: serde_json::Value,
    /// `true` when a newer modpack version is cached.
    pub update_available: bool,
    /// `CurseForge` file id of the cached latest, if any.
    pub latest_version_id: Option<i64>,
    /// Display name of the cached latest, if any.
    pub latest_version_name: Option<String>,
    /// `true` while the orchestrator is mid-update for this server.
    pub update_in_progress: bool,
    /// `true` when the file-helper Pod (`mc-{id}-files`) exists and is not
    /// mid-deletion. The frontend uses this to surface a manual "stop file
    /// viewer" control on stopped servers.
    pub files_helper_running: bool,
    /// Per-mod / per-plugin updates the poller has detected. Empty for
    /// vanilla and modpack-driven servers.
    pub mod_updates: Vec<ModUpdateInfo>,
    /// Newer Forge / `NeoForge` loader version available for this
    /// server's MC version, if any. `None` for fabric / paper / vanilla
    /// / modpack-driven servers and for forge/neoforge servers that
    /// already pin the latest.
    pub loader_update: Option<LoaderUpdateInfo>,
    /// User-tunable subset of server.properties applied via env on the
    /// next pod start. Defaults match vanilla MC for legacy rows.
    pub properties: ServerProperties,
}

/// One row of `mod_updates` surfaced on the server detail.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ModUpdateInfo {
    pub provider: String,
    pub project_id: String,
    pub current_version_id: String,
    pub latest_version_id: String,
    pub latest_version_name: String,
}

/// Latest published Forge / `NeoForge` loader version for the server's
/// MC version, surfaced when it differs from the current pin.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LoaderUpdateInfo {
    pub current_loader: String,
    pub latest_loader: String,
}

/// Handler for `GET /api/servers/:id`.
///
/// # Errors
///
/// - 404 if the `SQLite` row is missing.
/// - 500 on DB or k8s failure.
#[utoipa::path(
    get,
    path = "/api/servers/{id}",
    params(("id" = String, Path, description = "server UUID")),
    responses(
        (status = 200, description = "Server detail", body = ServerDetail),
        (status = 404, description = "Server not found")
    ),
    tag = "servers"
)]
pub async fn handle(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<ServerDetail>, AppError> {
    let detail = fetch_detail(&state, &id).await?;
    Ok(Json(detail))
}

/// Handler for `GET /api/servers/by-name/:name`.
///
/// Resolves the user-facing server name (UNIQUE in `servers`) to its UUID
/// then returns the same shape as [`handle`].
///
/// # Errors
///
/// - 404 if no server with that name exists.
/// - 500 on DB or k8s failure.
#[utoipa::path(
    get,
    path = "/api/servers/by-name/{name}",
    params(("name" = String, Path, description = "server name")),
    responses(
        (status = 200, description = "Server detail", body = ServerDetail),
        (status = 404, description = "Server not found")
    ),
    tag = "servers"
)]
pub async fn handle_by_name(
    Path(name): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<ServerDetail>, AppError> {
    let id: Option<String> = sqlx::query_scalar("SELECT id FROM servers WHERE name = ?")
        .bind(&name)
        .fetch_optional(&state.pool)
        .await?;
    let id = id.ok_or(AppError::NotFound)?;
    let detail = fetch_detail(&state, &id).await?;
    Ok(Json(detail))
}

/// Shared by start/stop/restart handlers — fetches the same shape
/// `GET /api/servers/:id` returns.
pub(crate) async fn fetch_detail(state: &AppState, id: &str) -> Result<ServerDetail, AppError> {
    let row = fetch_server_row(&state.pool, id).await?;

    let resource_name = format!("mc-{id}");
    let pod_name = format!("{resource_name}-0");
    let helper_name = format!("{resource_name}-files");

    let stsets: Api<StatefulSet> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let services: Api<Service> = Api::namespaced(state.kube.clone(), &state.mc_namespace);

    let (sts_res, pod_res, svc_res, helper_res) = tokio::join!(
        stsets.get_opt(&resource_name),
        pods.get_opt(&pod_name),
        services.get_opt(&resource_name),
        pods.get_opt(&helper_name),
    );
    let sts = sts_res?;
    let pod = pod_res?;
    let svc = svc_res?;
    let files_helper_running = helper_res
        .ok()
        .flatten()
        .is_some_and(|p| p.metadata.deletion_timestamp.is_none());

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
    let mv: Option<(i64, String)> =
        sqlx::query_as("SELECT latest_id, latest_name FROM modpack_versions WHERE server_id = ?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    // `fetch_optional` rather than `fetch_one`: the row can vanish between
    // `fetch_server_row` above and this query (concurrent DELETE) — that's
    // a 404, not a 500.
    let (source_kind, source_config_text): (String, String) =
        sqlx::query_as("SELECT source_kind, source_config FROM servers WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?
            .ok_or(AppError::NotFound)?;
    let source_config: serde_json::Value =
        serde_json::from_str(&source_config_text).unwrap_or(serde_json::Value::Null);
    let (latest_version_id, latest_version_name) = match mv {
        Some((id, name)) => (Some(id), Some(name)),
        None => (None, None),
    };
    // The poller is the source of truth: it deletes the modpack_versions
    // row whenever current == latest, auto_mode is "never", or the version
    // is in the skip list (poller.rs:139). So a row's existence already
    // implies an update is available. Comparing current_version_id here
    // would be redundant and was previously buggy — as_i64 returned None
    // for both Modrinth (string ids) and post-upgrade CF rows whose ids
    // had been stringified by the pre-fix orchestrator.
    let update_available = latest_version_id.is_some();
    let update_in_progress = state
        .update_locks
        .lock()
        .expect("update_locks poisoned")
        .contains(id);

    let mod_updates = fetch_mod_updates(&state.pool, id).await;
    let loader_update = fetch_loader_update(&state.pool, id).await;

    Ok(ServerDetail {
        id: row.id,
        name: row.name,
        status,
        mc_version: row.mc_version,
        memory_mi: row.memory_mi,
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
        update_in_progress,
        files_helper_running,
        mod_updates,
        loader_update,
        properties: row.properties,
    })
}

/// Returns the per-server loader-update row, if the poller flagged one.
/// Logged-and-empty on DB failure so detail still renders.
async fn fetch_loader_update(pool: &SqlitePool, server_id: &str) -> Option<LoaderUpdateInfo> {
    let row: Result<Option<(String, String)>, _> = sqlx::query_as(
        "SELECT current_loader, latest_loader FROM loader_updates WHERE server_id = ?",
    )
    .bind(server_id)
    .fetch_optional(pool)
    .await;
    match row {
        Ok(Some((current_loader, latest_loader))) => Some(LoaderUpdateInfo {
            current_loader,
            latest_loader,
        }),
        Ok(None) => None,
        Err(err) => {
            tracing::warn!(server.id = %server_id, error = %err, "loader_updates query failed");
            None
        }
    }
}

/// Per-mod update rows; on DB failure we log and return empty rather
/// than fail the whole detail fetch (the panel still renders fine).
async fn fetch_mod_updates(pool: &SqlitePool, server_id: &str) -> Vec<ModUpdateInfo> {
    type Row = (String, String, String, String, String);
    let rows: Result<Vec<Row>, _> = sqlx::query_as(
        "SELECT provider, project_id, current_version_id, latest_version_id, latest_version_name
         FROM mod_updates WHERE server_id = ?",
    )
    .bind(server_id)
    .fetch_all(pool)
    .await;
    match rows {
        Ok(rows) => rows
            .into_iter()
            .map(
                |(
                    provider,
                    project_id,
                    current_version_id,
                    latest_version_id,
                    latest_version_name,
                )| ModUpdateInfo {
                    provider,
                    project_id,
                    current_version_id,
                    latest_version_id,
                    latest_version_name,
                },
            )
            .collect(),
        Err(err) => {
            tracing::warn!(server.id = %server_id, error = %err, "mod_updates query failed");
            Vec::new()
        }
    }
}

/// In-memory shape mirroring the `servers` table.
pub(crate) struct ServerRow {
    pub id: String,
    pub name: String,
    pub mc_version: String,
    pub memory_mi: i64,
    pub exposure_mode: String,
    pub storage_class: Option<String>,
    pub storage_size_gi: i64,
    pub nodeport: Option<i32>,
    pub created_at: i64,
    pub last_started_at: Option<i64>,
    pub properties: ServerProperties,
}

/// Tuple shape returned by the SELECT in [`fetch_server_row`]. Aliased
/// to keep the function signature readable.
type ServerRowTuple = (
    String,
    String,
    String,
    i64,
    String,
    Option<String>,
    i64,
    Option<i64>,
    i64,
    Option<i64>,
    String,
);

/// Fetches one row from `servers` by id. Returns `AppError::NotFound`
/// when no row exists.
pub(crate) async fn fetch_server_row(pool: &SqlitePool, id: &str) -> Result<ServerRow, AppError> {
    let opt: Option<ServerRowTuple> = sqlx::query_as(
        "SELECT id, name, mc_version, memory_mi, exposure_mode,
                storage_class, storage_size_gi, nodeport, created_at, last_started_at,
                properties
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
            exposure_mode,
            storage_class,
            storage_size_gi,
            nodeport,
            created_at,
            last_started_at,
            properties_json,
        )) => Ok(ServerRow {
            id,
            name,
            mc_version,
            memory_mi,
            exposure_mode,
            storage_class,
            storage_size_gi,
            nodeport: nodeport.and_then(|n| i32::try_from(n).ok()),
            created_at,
            last_started_at,
            // Corrupt JSON falls back to defaults rather than 500-ing the
            // whole detail fetch — the panel still renders, the user can
            // re-save to fix the column.
            properties: serde_json::from_str(&properties_json).unwrap_or_default(),
        }),
    }
}
