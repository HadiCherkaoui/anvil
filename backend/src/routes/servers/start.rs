//! `POST /api/servers/:id/start` — scale the `StatefulSet` to 1 replica.
//!
//! Patches the `/scale` subresource (per the M2 task constraint —
//! never strategic-merge on Spec). Updates `last_started_at` in
//! `SQLite` and writes an audit-log entry.

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

/// Handler for `POST /api/servers/:id/start`.
///
/// # Errors
///
/// - 404 if the server does not exist.
/// - 500 on DB or k8s failure.
pub async fn handle(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<ServerDetail>, AppError> {
    // Verify the server is registered before touching k8s.
    let _row = fetch_server_row(&state.pool, &id).await?;

    let stsets: Api<StatefulSet> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let resource_name = format!("mc-{id}");
    let patch = json!({ "spec": { "replicas": 1 } });
    stsets
        .patch_scale(
            &resource_name,
            &PatchParams::default(),
            &Patch::Merge(&patch),
        )
        .await?;

    let now = Utc::now().timestamp();
    sqlx::query("UPDATE servers SET last_started_at = ? WHERE id = ?")
        .bind(now)
        .bind(&id)
        .execute(&state.pool)
        .await?;
    insert_audit(&state.pool, &id, "started", None, now).await?;

    let detail = fetch_detail(&state, &id).await?;
    Ok(Json(detail))
}
