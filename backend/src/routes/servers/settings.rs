//! `PATCH /api/servers/:id/settings` — update server settings.
//!
//! Resource fields (`memory_mi`, `cpu_millicores`) apply on next start to
//! every server type. Modpack-specific fields (`auto_update_mode`,
//! `version_skip`, `force_version`) are rejected on vanilla rows.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::Value;

use crate::AppState;
use crate::error::AppError;
use crate::modpack::curseforge::AutoUpdateMode;
use crate::routes::servers::create::insert_audit;
use crate::validation::{
    validate_cpu_millicores, validate_force_version, validate_memory_mi, validate_version_skip,
};

/// Request body — every field optional, only present fields update.
#[derive(Debug, Default, Deserialize)]
pub struct SettingsRequest {
    /// Memory budget (MiB). Applies on next start.
    pub memory_mi: Option<i64>,
    /// CPU budget (millicores). Applies on next start.
    pub cpu_millicores: Option<i64>,
    pub auto_update_mode: Option<AutoUpdateMode>,
    pub version_skip: Option<Vec<String>>,
    pub force_version: Option<Option<String>>,
}

/// Handler for `PATCH /api/servers/:id/settings`.
///
/// # Errors
///
/// - 404 if the server doesn't exist.
/// - 400 `not_modded` if a modpack-specific field is set on a vanilla server.
/// - 400 `memory_invalid` / `cpu_millicores_invalid` on out-of-range resources.
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

    let touches_modpack =
        req.auto_update_mode.is_some() || req.version_skip.is_some() || req.force_version.is_some();
    if touches_modpack && source_kind == "vanilla" {
        return Err(AppError::BadRequest {
            code: "not_modded",
            message: "vanilla servers have no modpack settings".to_owned(),
        });
    }

    if let Some(m) = req.memory_mi {
        validate_memory_mi(m)?;
    }
    if let Some(c) = req.cpu_millicores {
        validate_cpu_millicores(c)?;
    }
    if let Some(skips) = req.version_skip.as_ref() {
        validate_version_skip(skips)?;
    }
    if let Some(Some(v)) = req.force_version.as_ref() {
        validate_force_version(v)?;
    }

    // Resource fields land independently of source_config. COALESCE preserves
    // any column the request did not touch.
    if req.memory_mi.is_some() || req.cpu_millicores.is_some() {
        sqlx::query(
            "UPDATE servers SET
                memory_mi      = COALESCE(?, memory_mi),
                cpu_millicores = COALESCE(?, cpu_millicores)
             WHERE id = ?",
        )
        .bind(req.memory_mi)
        .bind(req.cpu_millicores)
        .bind(&id)
        .execute(&state.pool)
        .await?;
    }

    let mut audit = if touches_modpack {
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
        cfg
    } else {
        Value::Object(serde_json::Map::new())
    };

    if let Some(m) = req.memory_mi
        && let Some(obj) = audit.as_object_mut()
    {
        obj.insert("memory_mi".into(), serde_json::json!(m));
    }
    if let Some(c) = req.cpu_millicores
        && let Some(obj) = audit.as_object_mut()
    {
        obj.insert("cpu_millicores".into(), serde_json::json!(c));
    }

    let now = chrono::Utc::now().timestamp();
    insert_audit(&state.pool, &id, "settings_updated", Some(audit), now).await?;

    Ok(StatusCode::NO_CONTENT)
}
