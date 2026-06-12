//! `PATCH /api/servers/:id/version` — change MC version on a non-modpack server.
//!
//! Validates the request synchronously, acquires the per-server update lock,
//! and spawns the [`version_change`] FSM in the background. The frontend
//! consumes phase progress through the existing `/api/servers/:id/update/stream`
//! WS the modpack update flow already uses.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::AppError;
use crate::modpack::guard::UpdateGuard;
use crate::modpack::version_change;
use crate::validation::validate_mc_version;

/// Request body for `PATCH /api/servers/:id/version`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct VersionRequest {
    /// New Minecraft version (e.g. `"1.21.5"`).
    pub mc_version: String,
    /// Forge / `NeoForge` loader version. Required on modded forge/neoforge,
    /// ignored on fabric and on vanilla/paper.
    #[serde(default)]
    pub loader_version: Option<String>,
}

/// Response body for `PATCH /api/servers/:id/version` (matches the modpack
/// update route's shape so the frontend can reuse the same poll/stream code).
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VersionResponse {
    pub status: &'static str,
    pub server_id: String,
}

/// Handler for `PATCH /api/servers/:id/version`.
///
/// # Errors
///
/// - 404 `not_found` if the server does not exist.
/// - 409 `version_change_unsupported` if `source_kind ∈ {curseforge, modrinth}`.
/// - 400 `mc_version_unknown` if the requested MC version isn't a known release.
/// - 400 `loader_version_required` if `source_kind == "modded"`, runtime is
///   forge/neoforge, and `loader_version` is missing or empty.
/// - 400 `nothing_to_change` if the requested mc + loader match the current row.
/// - 409 `update_in_progress` if another update / apply already holds the lock.
#[utoipa::path(
    patch,
    path = "/api/servers/{id}/version",
    params(("id" = String, Path, description = "server UUID")),
    request_body = VersionRequest,
    responses(
        (status = 202, description = "Version change started", body = VersionResponse),
        (status = 400, description = "Invalid version or missing loader version"),
        (status = 404, description = "Server not found"),
        (status = 409, description = "Version change unsupported or update already in progress")
    ),
    tag = "servers"
)]
pub async fn handle(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(req): Json<VersionRequest>,
) -> Result<(StatusCode, Json<VersionResponse>), AppError> {
    let row: Option<(String, String, String)> =
        sqlx::query_as("SELECT source_kind, source_config, mc_version FROM servers WHERE id = ?")
            .bind(&id)
            .fetch_optional(&state.pool)
            .await?;
    let (source_kind, source_config, current_mc) = row.ok_or(AppError::NotFound)?;

    if matches!(source_kind.as_str(), "curseforge" | "modrinth") {
        return Err(AppError::Conflict {
            code: "version_change_unsupported",
            message: format!(
                "{source_kind} servers update via the modpack flow, not version change"
            ),
        });
    }

    validate_mc_version(&state, &req.mc_version).await?;

    if source_kind == "paper"
        && !crate::routes::papermc::is_supported(&state.papermc_cache, &req.mc_version).await
    {
        return Err(AppError::BadRequest {
            code: "paper_unsupported_version",
            message: format!("paper does not ship builds for MC {}", req.mc_version),
        });
    }

    let loader_version = req.loader_version.filter(|s| !s.is_empty());

    if source_kind == "modded" {
        let cfg: serde_json::Value =
            serde_json::from_str(&source_config).map_err(|e| AppError::Internal(e.into()))?;
        let runtime = cfg
            .get("runtime")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if matches!(runtime, "forge" | "neoforge") && loader_version.is_none() {
            return Err(AppError::BadRequest {
                code: "loader_version_required",
                message: format!("{runtime} servers require a loader_version"),
            });
        }
    } else if loader_version.is_some() {
        return Err(AppError::BadRequest {
            code: "loader_version_unsupported",
            message: format!("loader_version is only valid for modded servers, not {source_kind}"),
        });
    }

    // No-op detection: mc unchanged AND loader unchanged.
    let current_loader = serde_json::from_str::<serde_json::Value>(&source_config)
        .ok()
        .and_then(|v| {
            v.get("loader_version")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        });
    if req.mc_version == current_mc && loader_version == current_loader {
        return Err(AppError::BadRequest {
            code: "nothing_to_change",
            message: "mc_version and loader_version match the current row".to_owned(),
        });
    }

    let Some(guard) = UpdateGuard::try_acquire(
        &id,
        state.update_locks.clone(),
        state.update_phase_buses.clone(),
        &state.update_errors,
    ) else {
        return Err(AppError::Conflict {
            code: "update_in_progress",
            message: "another update or apply is running for this server".to_owned(),
        });
    };

    let task_state = state.clone();
    let task_id = id.clone();
    let task_mc = req.mc_version;
    let task_loader = loader_version;
    tokio::spawn(async move {
        version_change::run(task_state, task_id, task_mc, task_loader, guard).await;
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(VersionResponse {
            status: "started",
            server_id: id,
        }),
    ))
}
