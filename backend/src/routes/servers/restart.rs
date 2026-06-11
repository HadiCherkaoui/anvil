//! `POST /api/servers/:id/restart` — stop, wait for the pod to terminate,
//! then start.
//!
//! Returns `202 Accepted` immediately; the actual stop / wait / start
//! sequence runs in a `tokio::spawn`ed task. The frontend polls
//! `GET /api/servers/:id` and observes the status transition through
//! `running` → `stopping` → `stopped` → `starting` → `running`.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::Utc;
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::Pod;
use kube::Api;
use kube::api::{Patch, PatchParams};
use serde_json::{Value, json};
use tracing::Level;
use tracing::event;

use crate::AppState;
use crate::error::AppError;
use crate::routes::servers::create::insert_audit;
use crate::routes::servers::get::fetch_server_row;

/// Maximum time to wait for the pod to terminate after scale-to-0.
const POD_TERMINATE_TIMEOUT: Duration = Duration::from_secs(90);
/// Poll interval while waiting for the pod to terminate.
const POD_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Handler for `POST /api/servers/:id/restart`.
///
/// Spawns a background task and returns `202` immediately.
///
/// # Errors
///
/// - 404 if the server does not exist.
#[utoipa::path(
    post,
    path = "/api/servers/{id}/restart",
    params(("id" = String, Path, description = "server UUID")),
    responses(
        (status = 202, description = "Restart accepted; poll GET /api/servers/{id} for status", body = Object),
        (status = 404, description = "Server not found")
    ),
    tag = "servers"
)]
pub async fn handle(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let _row = fetch_server_row(&state.pool, &id).await?;
    let now = Utc::now().timestamp();
    insert_audit(&state.pool, &id, "restart_requested", None, now).await?;

    let task_state = state.clone();
    let task_id = id.clone();
    tokio::spawn(async move {
        if let Err(err) = restart_inner(&task_state, &task_id).await {
            event!(
                name: "anvil.restart.failed",
                Level::ERROR,
                server.id = %task_id,
                err = %err,
                "restart task failed",
            );
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({ "id": id, "status": "restarting" })),
    ))
}

/// Stop, poll the pod until gone (or timeout), then start.
async fn restart_inner(state: &AppState, id: &str) -> anyhow::Result<()> {
    let resource_name = format!("mc-{id}");
    let pod_name = format!("{resource_name}-0");

    let stsets: Api<StatefulSet> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let pp = PatchParams::default();

    // Stop.
    stsets
        .patch_scale(
            &resource_name,
            &pp,
            &Patch::Merge(&json!({ "spec": { "replicas": 0 } })),
        )
        .await?;

    // Wait for the pod to be gone (or timeout).
    let deadline = tokio::time::Instant::now() + POD_TERMINATE_TIMEOUT;
    loop {
        let still_present = pods.get_opt(&pod_name).await?.is_some();
        if !still_present {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("pod {pod_name} did not terminate within {POD_TERMINATE_TIMEOUT:?}");
        }
        tokio::time::sleep(POD_POLL_INTERVAL).await;
    }

    // Start.
    stsets
        .patch_scale(
            &resource_name,
            &pp,
            &Patch::Merge(&json!({ "spec": { "replicas": 1 } })),
        )
        .await?;

    let now = Utc::now().timestamp();
    sqlx::query("UPDATE servers SET last_started_at = ? WHERE id = ?")
        .bind(now)
        .bind(id)
        .execute(&state.pool)
        .await?;
    insert_audit(&state.pool, id, "restarted", None, now).await?;
    Ok(())
}
