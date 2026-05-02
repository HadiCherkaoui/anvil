//! `POST /api/servers/:id/stop` — scale the `StatefulSet` to 0 replicas.
//!
//! Same shape as `start.rs` but no `last_started_at` update.

use axum::Json;
use axum::extract::{Path, State};
use chrono::Utc;
use k8s_openapi::api::apps::v1::StatefulSet;
use kube::Api;
use kube::api::{Patch, PatchParams};
use serde_json::json;

use crate::AppState;
use crate::error::AppError;
use crate::routes::servers::create::insert_audit;
use crate::routes::servers::get::{ServerDetail, fetch_detail, fetch_server_row};

/// Handler for `POST /api/servers/:id/stop`.
///
/// # Errors
///
/// - 404 if the server does not exist.
/// - 500 on DB or k8s failure.
pub async fn handle(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<ServerDetail>, AppError> {
    let _row = fetch_server_row(&state.pool, &id).await?;

    let stsets: Api<StatefulSet> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let resource_name = format!("mc-{id}");
    let patch = json!({ "spec": { "replicas": 0 } });
    stsets
        .patch_scale(
            &resource_name,
            &PatchParams::apply("anvil"),
            &Patch::Merge(&patch),
        )
        .await?;

    let now = Utc::now().timestamp();
    insert_audit(&state.pool, &id, "stopped", None, now).await?;

    let detail = fetch_detail(&state, &id).await?;
    Ok(Json(detail))
}
