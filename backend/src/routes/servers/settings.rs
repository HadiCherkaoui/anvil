// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `PATCH /api/servers/:id/settings` — update server settings.
//!
//! `memory_mi` applies on next start to every server type. Modpack-specific
//! fields (`auto_update_mode`, `version_skip`) are rejected on vanilla rows.

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
use crate::server_properties::ServerProperties;
use crate::validation::{validate_memory_mi, validate_version_skip};

/// Request body — every field optional, only present fields update.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct SettingsRequest {
    /// Memory budget (MiB). Applies on next start.
    pub memory_mi: Option<i64>,
    /// Auto-update behavior: `"never"`, `"notify"`, or `"apply"`.
    #[schema(value_type = Option<String>)]
    pub auto_update_mode: Option<AutoUpdateMode>,
    pub version_skip: Option<Vec<String>>,
    /// Full-replacement when present. Validated and written verbatim to
    /// `servers.properties`; the `StatefulSet` env is rebuilt and patched
    /// in the same path as `memory_mi`.
    pub properties: Option<ServerProperties>,
}

/// Handler for `PATCH /api/servers/:id/settings`.
///
/// # Errors
///
/// - 404 if the server doesn't exist.
/// - 400 `not_modded` if a modpack-specific field is set on a vanilla server.
/// - 400 `memory_invalid` on out-of-range memory.
#[utoipa::path(
    patch,
    path = "/api/servers/{id}/settings",
    params(("id" = String, Path, description = "server UUID")),
    request_body = SettingsRequest,
    responses(
        (status = 204, description = "Settings updated"),
        (status = 400, description = "Invalid settings or modpack field on vanilla server"),
        (status = 404, description = "Server not found")
    ),
    tag = "servers"
)]
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

    let touches_modpack = req.auto_update_mode.is_some() || req.version_skip.is_some();
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
    if let Some(p) = req.properties.as_ref() {
        p.validate()?;
    }

    if let Some(m) = req.memory_mi {
        sqlx::query("UPDATE servers SET memory_mi = ? WHERE id = ?")
            .bind(m)
            .bind(&id)
            .execute(&state.pool)
            .await?;
    }

    if let Some(p) = req.properties.as_ref() {
        let raw = serde_json::to_string(p).map_err(|e| AppError::Internal(e.into()))?;
        sqlx::query("UPDATE servers SET properties = ? WHERE id = ?")
            .bind(&raw)
            .bind(&id)
            .execute(&state.pool)
            .await?;
    }

    if req.memory_mi.is_some() || req.properties.is_some() {
        // Strategic-merge the StatefulSet env so the next pod start picks
        // up the new memory budget and/or server.properties without
        // recreating the resource. The running pod keeps the old values;
        // toast wording stays "applies on next start". Missing StatefulSet
        // (404) is logged + ignored — the SQLite update already persisted,
        // next start picks up the value.
        let new_env = build_full_env_for_running_runtime(&state.pool, &id).await?;
        if let Err(e) = patch_statefulset_env(&state.kube, &state.mc_namespace, &id, &new_env).await
        {
            tracing::warn!(
                server.id = %id,
                error = %e,
                "settings PATCH wrote SQLite but failed to patch StatefulSet env",
            );
        }
    }

    let mut audit = if touches_modpack {
        // Optimistic-locking CAS on source_config: read raw → mutate →
        // `UPDATE … WHERE source_config = ?`. 0 rows-affected means a
        // concurrent writer landed; retry once on a fresh read.
        apply_modpack_patch_cas(
            &state.pool,
            &id,
            source_config,
            req.auto_update_mode,
            req.version_skip.clone(),
        )
        .await?
    } else {
        Value::Object(serde_json::Map::new())
    };

    if let Some(m) = req.memory_mi
        && let Some(obj) = audit.as_object_mut()
    {
        obj.insert("memory_mi".into(), serde_json::json!(m));
    }
    if let Some(p) = req.properties.as_ref()
        && let Some(obj) = audit.as_object_mut()
    {
        obj.insert("properties".into(), serde_json::json!(p));
    }

    let now = chrono::Utc::now().timestamp();
    insert_audit(&state.pool, &id, "settings_updated", Some(audit), now).await?;

    Ok(StatusCode::NO_CONTENT)
}

/// Applies the modpack-specific patch to `source_config` with optimistic
/// locking. Reads the row's `source_config` (initial caller-supplied value
/// `initial_raw` is the first attempt's CAS key), mutates the JSON, then
/// `UPDATE … WHERE source_config = ?`. On 0 rows-affected, re-reads and
/// retries once before surfacing a conflict.
async fn apply_modpack_patch_cas(
    pool: &sqlx::SqlitePool,
    id: &str,
    initial_raw: String,
    auto_update_mode: Option<AutoUpdateMode>,
    version_skip: Option<Vec<String>>,
) -> Result<Value, AppError> {
    let mut raw_before = initial_raw;
    for attempt in 0..2 {
        let mut cfg: Value = serde_json::from_str(&raw_before)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("source_config not JSON: {e}")))?;
        let obj = cfg.as_object_mut().ok_or(AppError::BadRequest {
            code: "source_config_invalid",
            message: "source_config is not a JSON object".to_owned(),
        })?;
        if let Some(m) = auto_update_mode {
            obj.insert(
                "auto_update_mode".into(),
                serde_json::to_value(m).unwrap_or(Value::Null),
            );
        }
        if let Some(ref skips) = version_skip {
            obj.insert("version_skip".into(), serde_json::json!(skips));
        }
        let new_raw = serde_json::to_string(&cfg).map_err(|e| AppError::Internal(e.into()))?;
        let res =
            sqlx::query("UPDATE servers SET source_config = ? WHERE id = ? AND source_config = ?")
                .bind(&new_raw)
                .bind(id)
                .bind(&raw_before)
                .execute(pool)
                .await?;
        if res.rows_affected() > 0 {
            return Ok(cfg);
        }
        if attempt == 1 {
            return Err(AppError::Conflict {
                code: "source_config_conflict",
                message: "concurrent settings write; retry".to_owned(),
            });
        }
        // Re-read for the retry.
        let row: Option<(String,)> =
            sqlx::query_as("SELECT source_config FROM servers WHERE id = ?")
                .bind(id)
                .fetch_optional(pool)
                .await?;
        raw_before = row.ok_or(AppError::NotFound)?.0;
    }
    unreachable!("loop returns or errors")
}

/// Rebuilds the full container env for a server's persisted runtime.
///
/// Reads memory and the properties JSON from the row alongside the provider
/// config so the rebuilt env block reflects the post-PATCH canonical state.
/// The result is the *complete* env block to strategic-merge onto the
/// `StatefulSet` — strategic-merge keys env entries by `name`, so partial
/// blocks would only mutate the listed names; sending the full block keeps
/// the resource deterministic.
///
/// Vanilla rows store `mc_version` outside `source_config`, so this function
/// reads it from the row and routes through `VanillaProvider::build_env`
/// directly (the `from_db` constructor returns a vanilla provider with
/// `VanillaVersion::default()` which would emit `VERSION=LATEST`).
async fn build_full_env_for_running_runtime(
    pool: &sqlx::SqlitePool,
    server_id: &str,
) -> Result<Vec<EnvVar>, AppError> {
    let row: (String, String, String, i64, String) = sqlx::query_as(
        "SELECT source_kind, source_config, mc_version, memory_mi, properties
         FROM servers WHERE id = ?",
    )
    .bind(server_id)
    .fetch_one(pool)
    .await?;
    let (source_kind, source_config, mc_version, memory_mi, properties_json) = row;
    let properties: ServerProperties = serde_json::from_str(&properties_json).unwrap_or_default();
    let mut env = if source_kind == "vanilla" {
        VanillaProvider::build_env(server_id, &mc_version, memory_mi)
    } else {
        let provider = from_db(&source_kind, &source_config)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("rebuild provider: {e}")))?;
        provider.extra_env(&ProviderContext {
            server_id,
            memory_mi,
        })
    };
    env.extend(properties.to_env());
    Ok(env)
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
    async fn rebuild_env_for_vanilla_uses_row_mc_version_and_memory() {
        let pool = seed_pool().await;
        insert_server(&pool, "v1", "vanilla", "{}", "1.21.4", 8192).await;

        let env = build_full_env_for_running_runtime(&pool, "v1")
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
        insert_server(&pool, "m1", "modded", cfg, "1.21.1", 6144).await;

        let env = build_full_env_for_running_runtime(&pool, "m1")
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

        let env = build_full_env_for_running_runtime(&pool, "p1")
            .await
            .expect("rebuild");

        assert_eq!(env_value(&env, "TYPE"), Some("PAPER"));
        assert_eq!(env_value(&env, "VERSION"), Some("1.21.4"));
        assert_eq!(env_value(&env, "MAX_MEMORY"), Some("4096M"));
    }

    #[tokio::test]
    async fn rebuild_env_includes_properties_overrides() {
        let pool = seed_pool().await;
        insert_server(&pool, "v1", "vanilla", "{}", "1.21.4", 4096).await;
        // Poke the properties column directly to simulate a prior PATCH.
        sqlx::query("UPDATE servers SET properties = ? WHERE id = ?")
            .bind(r#"{"difficulty":"hard","max_players":50}"#)
            .bind("v1")
            .execute(&pool)
            .await
            .unwrap();

        let env = build_full_env_for_running_runtime(&pool, "v1")
            .await
            .expect("rebuild");

        assert_eq!(env_value(&env, "DIFFICULTY"), Some("hard"));
        assert_eq!(env_value(&env, "MAX_PLAYERS"), Some("50"));
        // Provider env still present.
        assert_eq!(env_value(&env, "MAX_MEMORY"), Some("4096M"));
        // Other property fields fall back to defaults.
        assert_eq!(env_value(&env, "PVP"), Some("true"));
    }
}
