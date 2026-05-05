//! Server management handlers.
//!
//! Submodules host one HTTP handler each (create / get / start / stop /
//! restart / delete / logs). The list handler in this `mod.rs` JOINs
//! `SQLite` metadata with the live `StatefulSet` / `Pod` / `Service`
//! triples, returning the M2 wire shape.

pub mod create;
pub mod delete;
pub mod files;
pub mod get;
pub mod logs;
pub mod logs_stream;
pub mod metrics;
pub mod mods;
pub mod players;
pub mod plugins;
pub mod rcon;
pub mod restart;
pub mod settings;
pub mod start;
pub mod stop;
pub mod update;
pub mod update_stream;

use std::collections::HashMap;

use axum::Json;
use axum::extract::State;
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::Api;
use kube::api::ListParams;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::AppState;
use crate::error::AppError;
use crate::k8s::{LABEL_SERVER, MANAGED_BY_LABEL, MANAGED_BY_VALUE, ServerStatus, ServerSummary};
use crate::k8s_status::{derive_endpoint, derive_status};

/// Body of `GET /api/servers` (spec §2.2).
#[derive(Debug, Serialize)]
pub struct ServersBody {
    pub servers: Vec<ServerSummary>,
}

/// Handler for `GET /api/servers`.
///
/// `SQLite` is the source of truth for metadata; k8s is consulted for
/// live status and endpoint resolution. Servers in `SQLite` but not in
/// k8s appear with status `error` (partial create failure or external
/// teardown). Servers in k8s but not in `SQLite` are silently filtered —
/// the panel only owns what it created.
///
/// # Errors
///
/// Returns [`AppError::DbUnavailable`] or [`AppError::KubeUnavailable`]
/// if either source is unreachable.
///
/// # Panics
///
/// Panics if the `update_locks` Mutex is poisoned (recoverable only by
/// restarting the panel).
pub async fn list(State(state): State<AppState>) -> Result<Json<ServersBody>, AppError> {
    let rows = fetch_summary_rows(&state.pool).await?;

    let stsets: Api<StatefulSet> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let services: Api<Service> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let selector = format!("{MANAGED_BY_LABEL}={MANAGED_BY_VALUE}");
    let params = ListParams::default().labels(&selector);

    let (sts_res, pod_res, svc_res) = tokio::join!(
        stsets.list(&params),
        pods.list(&params),
        services.list(&params),
    );
    let sts_list = sts_res?;
    let pod_list = pod_res?;
    let svc_list = svc_res?;

    let sts_by_id = index_by_server_label(sts_list.items.iter().map(|s| (&s.metadata, s)));
    let pod_by_id = index_by_server_label(pod_list.items.iter().map(|p| (&p.metadata, p)));
    // Each server has two Services with the same `server` label — the public
    // one (`mc-{id}`) and the headless one (`mc-{id}-headless`, used for
    // in-cluster RCON DNS). They both match the list selector; if the
    // headless ends up in the index slot, derive_endpoint sees a Service
    // with `clusterIP: None` and no LB ingress, so the address column is
    // blank. Skip headless entries by filtering on `clusterIP == None`.
    let svc_by_id = index_by_server_label(
        svc_list
            .items
            .iter()
            .filter(|s| s.spec.as_ref().and_then(|sp| sp.cluster_ip.as_deref()) != Some("None"))
            .map(|s| (&s.metadata, s)),
    );

    let servers = rows
        .into_iter()
        .map(|row| {
            let sts = sts_by_id.get(row.id.as_str()).copied();
            let pod = pod_by_id.get(row.id.as_str()).copied();
            let svc = svc_by_id.get(row.id.as_str()).copied();

            let (replicas, ready) = sts.map_or((0, 0), |s| {
                let r = s.spec.as_ref().and_then(|sp| sp.replicas).unwrap_or(0);
                let ready = s
                    .status
                    .as_ref()
                    .and_then(|st| st.ready_replicas)
                    .unwrap_or(0);
                (r, ready)
            });

            let status = if sts.is_none() {
                // Metadata exists but k8s does not — partial create failure
                // or someone deleted the StatefulSet out of band.
                ServerStatus::Error
            } else {
                derive_status(replicas, ready, pod)
            };

            let resource_name = format!("mc-{}", row.id);
            let endpoint = derive_endpoint(
                svc,
                &row.exposure_mode,
                &state.node_host,
                &resource_name,
                &state.mc_namespace,
            );
            let update_available = match (row.current_version_id, row.latest_id) {
                (Some(cur), Some(latest)) => cur != latest,
                _ => false,
            };
            let update_in_progress = state
                .update_locks
                .lock()
                .expect("update_locks poisoned")
                .contains(&row.id);

            ServerSummary {
                id: row.id,
                name: row.name,
                status,
                mc_version: row.mc_version,
                memory_mi: row.memory_mi,
                exposure_mode: row.exposure_mode,
                endpoint,
                created_at: row.created_at,
                source_kind: row.source_kind,
                update_available,
                latest_version_name: row.latest_version_name,
                update_in_progress,
            }
        })
        .collect();

    Ok(Json(ServersBody { servers }))
}

/// Slim row used by the list handler.
struct SummaryRow {
    id: String,
    name: String,
    mc_version: String,
    memory_mi: i64,
    exposure_mode: String,
    created_at: i64,
    source_kind: String,
    /// Latest version display name from the LEFT JOIN — `None` for vanilla
    /// or unbumped CF servers.
    latest_version_name: Option<String>,
    /// CF `current_version_id` from `source_config`, used to compare against
    /// `latest_id` and decide whether `update_available` is true.
    current_version_id: Option<i64>,
    /// CF `latest_id` from the LEFT JOIN — `None` when no upstream version cached.
    latest_id: Option<i64>,
}

/// Tuple shape returned by [`fetch_summary_rows`]; aliased so the
/// `query_as` annotation is not a clippy `type_complexity` violation.
type SummaryTuple = (
    String,
    String,
    String,
    i64,
    String,
    i64,
    String,
    String,
    Option<String>,
    Option<i64>,
);

async fn fetch_summary_rows(pool: &SqlitePool) -> Result<Vec<SummaryRow>, AppError> {
    let rows: Vec<SummaryTuple> = sqlx::query_as(
        "SELECT s.id, s.name, s.mc_version, s.memory_mi,
                s.exposure_mode, s.created_at, s.source_kind, s.source_config,
                mv.latest_name, mv.latest_id
         FROM servers s
         LEFT JOIN modpack_versions mv ON mv.server_id = s.id
         ORDER BY s.created_at ASC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                name,
                mc_version,
                memory_mi,
                exposure_mode,
                created_at,
                source_kind,
                source_config,
                latest_version_name,
                latest_id,
            )| {
                let current_version_id = serde_json::from_str::<serde_json::Value>(&source_config)
                    .ok()
                    .and_then(|v| {
                        v.get("current_version_id")
                            .and_then(serde_json::Value::as_i64)
                    });
                SummaryRow {
                    id,
                    name,
                    mc_version,
                    memory_mi,
                    exposure_mode,
                    created_at,
                    source_kind,
                    latest_version_name,
                    current_version_id,
                    latest_id,
                }
            },
        )
        .collect())
}

/// Indexes objects by their `app.anvil.io/server` label value (the uuid).
fn index_by_server_label<'a, T, I>(iter: I) -> HashMap<&'a str, &'a T>
where
    I: Iterator<
        Item = (
            &'a k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta,
            &'a T,
        ),
    >,
    T: 'a,
{
    let mut map = HashMap::new();
    for (meta, obj) in iter {
        if let Some(id) = meta.labels.as_ref().and_then(|l| l.get(LABEL_SERVER)) {
            map.insert(id.as_str(), obj);
        }
    }
    map
}
