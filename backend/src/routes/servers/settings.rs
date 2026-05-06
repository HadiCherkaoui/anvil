//! `PATCH /api/servers/:id/settings` — update server settings.
//!
//! `memory_mi` applies on next start to every server type. Modpack-specific
//! fields (`auto_update_mode`, `version_skip`, `force_version`) are rejected
//! on vanilla rows.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use k8s_openapi::api::core::v1::EnvVar;
use serde::Deserialize;
use serde_json::Value;

use crate::AppState;
use crate::error::AppError;
use crate::k8s_patches::patch_statefulset_env;
use crate::modpack::curseforge::AutoUpdateMode;
use crate::modpack::{ProviderContext, VanillaProvider, from_db};
use crate::routes::servers::create::insert_audit;
use crate::validation::{validate_force_version, validate_memory_mi, validate_version_skip};

/// Request body — every field optional, only present fields update.
#[derive(Debug, Default, Deserialize)]
pub struct SettingsRequest {
    /// Memory budget (MiB). Applies on next start.
    pub memory_mi: Option<i64>,
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
/// - 400 `memory_invalid` on out-of-range memory.
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
    if let Some(skips) = req.version_skip.as_ref() {
        validate_version_skip(skips)?;
    }
    if let Some(Some(v)) = req.force_version.as_ref() {
        validate_force_version(v)?;
    }

    if let Some(m) = req.memory_mi {
        sqlx::query("UPDATE servers SET memory_mi = ? WHERE id = ?")
            .bind(m)
            .bind(&id)
            .execute(&state.pool)
            .await?;

        // Strategic-merge the StatefulSet env so the next pod start picks
        // up the new INIT/MAX_MEMORY without recreating the resource. The
        // running pod keeps the old budget; toast wording stays "applies on
        // next start". Missing StatefulSet (404) is logged + ignored — the
        // SQLite update already persisted, next start picks up the value.
        let new_env = build_full_env_for_running_runtime(&state.pool, &id, m).await?;
        if let Err(e) = patch_statefulset_env(&state.kube, &state.mc_namespace, &id, &new_env).await
        {
            tracing::warn!(
                server.id = %id,
                error = %e,
                "memory PATCH wrote SQLite but failed to patch StatefulSet env",
            );
        }
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

    let now = chrono::Utc::now().timestamp();
    insert_audit(&state.pool, &id, "settings_updated", Some(audit), now).await?;

    Ok(StatusCode::NO_CONTENT)
}

/// Rebuilds the full container env for a server's persisted runtime with a
/// new memory budget. The result is the *complete* env block to strategic-
/// merge onto the `StatefulSet` — strategic-merge keys env entries by `name`,
/// so partial blocks would only mutate the listed names; sending the full
/// block keeps the resource deterministic.
///
/// Vanilla rows store `mc_version` outside `source_config`, so this function
/// reads it from the row and routes through `VanillaProvider::build_env`
/// directly (the `from_db` constructor returns a vanilla provider with
/// `VanillaVersion::default()` which would emit `VERSION=LATEST`).
async fn build_full_env_for_running_runtime(
    pool: &sqlx::SqlitePool,
    server_id: &str,
    memory_mi: i64,
) -> Result<Vec<EnvVar>, AppError> {
    let row: (String, String, String) =
        sqlx::query_as("SELECT source_kind, source_config, mc_version FROM servers WHERE id = ?")
            .bind(server_id)
            .fetch_one(pool)
            .await?;
    let (source_kind, source_config, mc_version) = row;
    if source_kind == "vanilla" {
        return Ok(VanillaProvider::build_env(
            server_id,
            &mc_version,
            memory_mi,
        ));
    }
    let provider = from_db(&source_kind, &source_config)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("rebuild provider: {e}")))?;
    Ok(provider.extra_env(&ProviderContext {
        server_id,
        memory_mi,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn seed_pool() -> sqlx::SqlitePool {
        crate::db::init("sqlite::memory:").await.expect("migrate")
    }

    async fn insert_server(
        pool: &sqlx::SqlitePool,
        id: &str,
        kind: &str,
        source_config: &str,
        mc_version: &str,
        memory_mi: i64,
    ) {
        sqlx::query(
            "INSERT INTO servers (
                id, name, mc_version, memory_mi, source_kind, exposure_mode,
                storage_class, storage_size_gi, source_config, nodeport,
                created_at, last_started_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(format!("{id}-name"))
        .bind(mc_version)
        .bind(memory_mi)
        .bind(kind)
        .bind("loadbalancer")
        .bind(Option::<String>::None)
        .bind(10_i64)
        .bind(source_config)
        .bind(Option::<i64>::None)
        .bind(0_i64)
        .bind(Option::<i64>::None)
        .execute(pool)
        .await
        .expect("insert");
    }

    fn env_value<'a>(env: &'a [EnvVar], key: &str) -> Option<&'a str> {
        env.iter()
            .find(|e| e.name == key)
            .and_then(|e| e.value.as_deref())
    }

    #[tokio::test]
    async fn rebuild_env_for_vanilla_uses_row_mc_version_and_new_memory() {
        let pool = seed_pool().await;
        insert_server(&pool, "v1", "vanilla", "{}", "1.21.4", 4096).await;

        let env = build_full_env_for_running_runtime(&pool, "v1", 8192)
            .await
            .expect("rebuild");

        assert_eq!(env_value(&env, "VERSION"), Some("1.21.4"));
        assert_eq!(env_value(&env, "MAX_MEMORY"), Some("8192M"));
        assert_eq!(env_value(&env, "INIT_MEMORY"), Some("2048M"));
    }

    #[tokio::test]
    async fn rebuild_env_for_modded_carries_runtime_version() {
        let pool = seed_pool().await;
        let cfg = r#"{"runtime":"fabric","mc_version":"1.21.1","mods":[],"pending":[]}"#;
        insert_server(&pool, "m1", "modded", cfg, "1.21.1", 4096).await;

        let env = build_full_env_for_running_runtime(&pool, "m1", 6144)
            .await
            .expect("rebuild");

        assert_eq!(env_value(&env, "TYPE"), Some("FABRIC"));
        assert_eq!(env_value(&env, "VERSION"), Some("1.21.1"));
        assert_eq!(env_value(&env, "MAX_MEMORY"), Some("6144M"));
        assert_eq!(env_value(&env, "INIT_MEMORY"), Some("1536M"));
    }

    #[tokio::test]
    async fn rebuild_env_for_paper_carries_mc_version() {
        let pool = seed_pool().await;
        let cfg = r#"{"mc_version":"1.21.4"}"#;
        insert_server(&pool, "p1", "paper", cfg, "1.21.4", 4096).await;

        let env = build_full_env_for_running_runtime(&pool, "p1", 4096)
            .await
            .expect("rebuild");

        assert_eq!(env_value(&env, "TYPE"), Some("PAPER"));
        assert_eq!(env_value(&env, "VERSION"), Some("1.21.4"));
        assert_eq!(env_value(&env, "MAX_MEMORY"), Some("4096M"));
    }
}
