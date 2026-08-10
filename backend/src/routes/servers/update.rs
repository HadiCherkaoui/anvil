// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

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
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct UpdateRequest {
    /// Upstream version id to target. `None` ⇒ use cached latest.
    #[serde(default)]
    pub version_id: Option<String>,
}

/// Response body.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct UpdateResponse {
    pub status: &'static str,
    pub server_id: String,
    pub target_version_id: String,
}

/// Handler for `POST /api/servers/:id/update`.
///
/// # Errors
///
/// - 404 if the server doesn't exist.
/// - 400 `not_modded` if `source_kind == "vanilla"`.
/// - 409 `update_in_progress` if another update is running for the same id.
/// - 409 `no_update_target` if no `version_id` in body and no cached latest.
#[utoipa::path(
    post,
    path = "/api/servers/{id}/update",
    params(("id" = String, Path, description = "server UUID")),
    request_body(content = Option<UpdateRequest>, description = "Optional target version override"),
    responses(
        (status = 202, description = "Update started", body = UpdateResponse),
        (status = 400, description = "Server type does not support modpack updates"),
        (status = 404, description = "Server not found"),
        (status = 409, description = "Update already in progress or no update target available")
    ),
    tag = "servers"
)]
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
    if matches!(source_kind.as_str(), "vanilla" | "modded" | "paper") {
        return Err(AppError::BadRequest {
            code: "not_modded",
            message: format!("{source_kind} servers cannot be updated via this endpoint"),
        });
    }

    // Pick the target version: body override → modpack_versions.latest_id /
    // latest_name. CF rows store id as a number; Modrinth rows fall back to
    // latest_name (the real string id lives there per poller convention).
    let target_version_id: String = if let Some(v) = req.version_id {
        v
    } else {
        let row: Option<(i64, String)> = sqlx::query_as(
            "SELECT latest_id, latest_name FROM modpack_versions WHERE server_id = ?",
        )
        .bind(&id)
        .fetch_optional(&state.pool)
        .await?;
        let (latest_id, latest_name) = row.ok_or(AppError::Conflict {
            code: "no_update_target",
            message: "no version_id supplied and no cached latest version available".to_owned(),
        })?;
        if source_kind == "modrinth" {
            latest_name
        } else {
            latest_id.to_string()
        }
    };

    let Some(guard) = UpdateGuard::try_acquire(
        &id,
        state.update_locks.clone(),
        state.update_phase_buses.clone(),
        &state.update_errors,
    ) else {
        return Err(AppError::Conflict {
            code: "update_in_progress",
            message: "an update is already running for this server".to_owned(),
        });
    };

    let task_state = state.clone();
    let task_id = id.clone();
    let task_target = target_version_id.clone();
    tokio::spawn(async move {
        orchestrator::run(task_state, task_id, task_target, guard).await;
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
