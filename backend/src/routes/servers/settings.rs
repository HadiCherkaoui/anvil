//! `PATCH /api/servers/:id/settings` — update modpack-related settings.
//!
//! Mutates `source_config` JSON in-place: `auto_update_mode`, `version_skip`,
//! `force_version`. Vanilla servers reject 400 `not_modded`. Other fields
//! are ignored to keep the payload forward-compatible.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::Value;

use crate::AppState;
use crate::error::AppError;
use crate::modpack::curseforge::AutoUpdateMode;
use crate::routes::servers::create::insert_audit;

/// Request body — every field optional, only present fields update.
#[derive(Debug, Default, Deserialize)]
pub struct SettingsRequest {
    pub auto_update_mode: Option<AutoUpdateMode>,
    pub version_skip: Option<Vec<String>>,
    pub force_version: Option<Option<String>>,
}

/// Handler for `PATCH /api/servers/:id/settings`.
///
/// # Errors
///
/// - 404 if the server doesn't exist.
/// - 400 `not_modded` if `source_kind == "vanilla"`.
pub async fn handle(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(req): Json<SettingsRequest>,
) -> Result<StatusCode, AppError> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT source_kind, source_config FROM servers WHERE id = ?")
            .bind(&id)
            .fetch_optional(&state.pool)
            .await?;
    let (source_kind, source_config) = row.ok_or(AppError::NotFound)?;
    if source_kind == "vanilla" {
        return Err(AppError::BadRequest {
            code: "not_modded",
            message: "vanilla servers have no modpack settings".to_owned(),
        });
    }

    let mut cfg: Value = serde_json::from_str(&source_config)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("source_config not JSON: {e}")))?;
    let obj = cfg.as_object_mut().ok_or(AppError::BadRequest {
        code: "source_config_invalid",
        message: "source_config is not a JSON object".to_owned(),
    })?;
    if let Some(m) = req.auto_update_mode {
        obj.insert(
            "auto_update_mode".into(),
            serde_json::to_value(m).unwrap_or(Value::Null),
        );
    }
    if let Some(skips) = req.version_skip {
        obj.insert("version_skip".into(), serde_json::json!(skips));
    }
    if let Some(force) = req.force_version {
        obj.insert(
            "force_version".into(),
            force.map_or(Value::Null, Value::String),
        );
    }
    let new_raw = serde_json::to_string(&cfg).map_err(|e| AppError::Internal(e.into()))?;

    sqlx::query("UPDATE servers SET source_config = ? WHERE id = ?")
        .bind(&new_raw)
        .bind(&id)
        .execute(&state.pool)
        .await?;
    let now = chrono::Utc::now().timestamp();
    insert_audit(&state.pool, &id, "settings_updated", Some(cfg), now).await?;

    Ok(StatusCode::NO_CONTENT)
}
