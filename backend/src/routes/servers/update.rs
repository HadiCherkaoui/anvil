//! `POST /api/servers/:id/update` — kick off an update FSM run.
//!
//! Validates the server is non-vanilla, picks the target version (body
//! `version_id` → cached `modpack_versions.latest_id`), acquires the
//! per-server lock, spawns the orchestrator task, and returns 202.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;
use crate::error::AppError;
use crate::modpack::guard::UpdateGuard;
use crate::modpack::orchestrator;

/// Request body for `POST /api/servers/:id/update`.
#[derive(Debug, Default, Deserialize)]
pub struct UpdateRequest {
    /// `CurseForge` file id to target. `None` ⇒ use cached latest.
    #[serde(default)]
    pub version_id: Option<u32>,
}

/// Response body.
#[derive(Debug, Serialize)]
pub struct UpdateResponse {
    pub status: &'static str,
    pub server_id: String,
    pub target_version_id: u32,
}

/// Handler for `POST /api/servers/:id/update`.
///
/// # Errors
///
/// - 404 if the server doesn't exist.
/// - 400 `not_modded` if `source_kind == "vanilla"`.
/// - 409 `update_in_progress` if another update is running for the same id.
/// - 409 `no_update_target` if no `version_id` in body and no cached latest.
pub async fn handle(
    Path(id): Path<String>,
    State(state): State<AppState>,
    body: Option<Json<UpdateRequest>>,
) -> Result<(StatusCode, Json<serde_json::Value>), AppError> {
    let req = body.map(|Json(r)| r).unwrap_or_default();

    let row: Option<(String,)> = sqlx::query_as("SELECT source_kind FROM servers WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.pool)
        .await?;
    let source_kind = row.ok_or(AppError::NotFound)?.0;
    if source_kind == "vanilla" {
        return Err(AppError::BadRequest {
            code: "not_modded",
            message: "vanilla servers cannot be updated via this endpoint".to_owned(),
        });
    }

    // Pick the target version: body override → modpack_versions.latest_id.
    let target_version_id = if let Some(v) = req.version_id {
        v
    } else {
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT latest_id FROM modpack_versions WHERE server_id = ?")
                .bind(&id)
                .fetch_optional(&state.pool)
                .await?;
        let raw = row
            .ok_or(AppError::Conflict {
                code: "no_update_target",
                message: "no version_id supplied and no cached latest version available".to_owned(),
            })?
            .0;
        u32::try_from(raw).map_err(|_| {
            AppError::Internal(anyhow::anyhow!(
                "modpack_versions.latest_id out of u32 range"
            ))
        })?
    };

    let Some(guard) = UpdateGuard::try_acquire(
        &id,
        state.update_locks.clone(),
        state.update_phase_buses.clone(),
    ) else {
        return Err(AppError::Conflict {
            code: "update_in_progress",
            message: "an update is already running for this server".to_owned(),
        });
    };

    let task_state = state.clone();
    let task_id = id.clone();
    tokio::spawn(async move {
        orchestrator::run(task_state, task_id, target_version_id, guard).await;
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(json!({
            "status": "started",
            "server_id": id,
            "target_version_id": target_version_id,
        })),
    ))
}
