//! Hourly modpack version poller.
//!
//! On each tick, iterates every non-vanilla server, asks its provider for
//! the latest upstream version, and either upserts `modpack_versions` (so
//! the frontend banner shows up) or — when `auto_update_mode=apply` — fires
//! the update orchestrator inline. After the modpack-level pass, walks
//! every modded / paper server and records per-mod / per-plugin updates
//! in `mod_updates`.

use std::time::Duration;

use serde_json::Value;
use sqlx::Row as _;
use tokio::time::sleep;
use tracing::{event, Level};

use crate::modpack::guard::UpdateGuard;
use crate::modpack::{from_db, orchestrator, ModpackHttp};
use crate::AppState;

/// Defensive gap between upstream calls in the per-mod pass. Modrinth's
/// anonymous limit is 300/min, CF's limit depends on the API key tier;
/// 100ms keeps both well below the limit even on a 150-mod ATM-class
/// server.
const PER_MOD_RATE_LIMIT_GAP: Duration = Duration::from_millis(100);

/// Initial delay before the first poll — gives `axum::serve` time to bind
/// before we hammer the DB / k8s API.
const STARTUP_DELAY: Duration = Duration::from_secs(30);

/// Background loop that refreshes `modpack_versions` for every CF server.
pub async fn run(state: AppState) {
    sleep(STARTUP_DELAY).await;
    loop {
        if let Err(err) = tick(&state).await {
            event!(
                name: "anvil.modpack.poll.error",
                Level::WARN,
                err = %err,
                "modpack poll tick failed; continuing",
            );
        }
        sleep(state.modpack_poll_interval).await;
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "linear per-server fan-out: fetch latest, gate, upsert, optionally apply"
)]
async fn tick(state: &AppState) -> anyhow::Result<()> {
    let http = ModpackHttp {
        cf: state.cf_client.as_deref(),
        mr: state.mr_client.as_ref(),
    };

    let rows = sqlx::query(
        "SELECT id, source_kind, source_config FROM servers
         WHERE source_kind IN ('curseforge','modrinth')",
    )
    .fetch_all(&state.pool)
    .await?;

    for row in rows {
        let id: String = row.try_get("id")?;
        let source_kind: String = row.try_get("source_kind")?;
        let source_config: String = row.try_get("source_config")?;

        // CF rows can't poll without the API key — skip with a debug log.
        if source_kind == "curseforge" && http.cf.is_none() {
            continue;
        }

        // One slow upstream call per server, every poll_interval; serial is
        // fine at homelab scale.
        let provider = match from_db(&source_kind, &source_config) {
            Ok(p) => p,
            Err(err) => {
                event!(
                    name: "anvil.modpack.poll.bad_config",
                    Level::WARN,
                    server.id = %id,
                    err = %err,
                    "skipping server with malformed source_config",
                );
                continue;
            }
        };

        let latest = match provider.latest(&http).await {
            Ok(Some(v)) => v,
            Ok(None) => continue,
            Err(err) => {
                event!(
                    name: "anvil.modpack.poll.upstream_error",
                    Level::WARN,
                    server.id = %id,
                    err = %err,
                    "upstream lookup failed; skipping",
                );
                continue;
            }
        };

        // Decide: do we record this as an update available, do nothing, or
        // auto-apply?
        let cfg: Value =
            serde_json::from_str(&source_config).unwrap_or_else(|_| serde_json::json!({}));
        // CF persists `current_version_id` as a JSON number (u32 file id),
        // Modrinth as a string (8-char base62 version id). Normalise both
        // to a string for comparison against `latest.id`.
        let current_id_str: String = cfg
            .get("current_version_id")
            .map(|v| match v {
                Value::Number(n) => n.to_string(),
                Value::String(s) => s.clone(),
                _ => String::new(),
            })
            .unwrap_or_default();
        let auto_mode = cfg
            .get("auto_update_mode")
            .and_then(Value::as_str)
            .unwrap_or("notify");
        let skip_list: Vec<String> = cfg
            .get("version_skip")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        let skipped = skip_list
            .iter()
            .any(|s| s == &latest.id || s == &latest.name);

        if latest.id == current_id_str || auto_mode == "never" || skipped {
            let _ = sqlx::query("DELETE FROM modpack_versions WHERE server_id = ?")
                .bind(&id)
                .execute(&state.pool)
                .await;
            continue;
        }

        // modpack_versions.latest_id is INTEGER. CF ids parse cleanly; Modrinth
        // string ids fall back to 0 — the real id always lives in latest_name.
        let latest_id_int: i64 = latest.id.parse().unwrap_or(0);
        let now = chrono::Utc::now().timestamp();
        let _ = sqlx::query(
            "INSERT OR REPLACE INTO modpack_versions
             (server_id, latest_id, latest_name, latest_download_url, checked_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(latest_id_int)
        .bind(&latest.name)
        .bind(&latest.download_url)
        .bind(now)
        .execute(&state.pool)
        .await;

        if auto_mode == "apply" {
            event!(
                name: "anvil.modpack.poll.auto_apply",
                Level::INFO,
                server.id = %id,
                version.id = latest.id,
                "auto-applying detected update",
            );
            let Some(guard) = UpdateGuard::try_acquire(
                &id,
                state.update_locks.clone(),
                state.update_phase_buses.clone(),
            ) else {
                continue;
            };
            orchestrator::run(state.clone(), id.clone(), latest.id.clone(), guard).await;
        }
    }

    if let Err(err) = poll_individual_mods(state, &http).await {
        event!(
            name: "anvil.modpack.poll.individual_error",
            Level::WARN,
            err = %err,
            "per-mod update poll failed; continuing",
        );
    }
    Ok(())
}

/// Walks every modded / paper server and updates the `mod_updates` table.
///
/// One upstream call per installed mod. Failures (network, parse, missing
/// upstream) are logged and skipped — they never bubble up.
async fn poll_individual_mods(state: &AppState, http: &ModpackHttp<'_>) -> anyhow::Result<()> {
    let rows = sqlx::query(
        "SELECT id, mc_version, source_kind, source_config FROM servers
         WHERE source_kind IN ('modded', 'paper')",
    )
    .fetch_all(&state.pool)
    .await?;

    for row in rows {
        let id: String = row.try_get("id")?;
        let mc_version: String = row.try_get("mc_version")?;
        let source_kind: String = row.try_get("source_kind")?;
        let source_config: String = row.try_get("source_config")?;

        let cfg: Value = match serde_json::from_str(&source_config) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let (loader, mods_field): (&str, &str) = if source_kind == "paper" {
            ("paper", "plugins")
        } else {
            let runtime = cfg
                .get("runtime")
                .and_then(Value::as_str)
                .unwrap_or("fabric");
            (runtime, "mods")
        };
        let mods = cfg
            .get(mods_field)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        for m in &mods {
            let provider = m.get("provider").and_then(Value::as_str).unwrap_or("");
            let project_id = m.get("project_id").and_then(Value::as_str).unwrap_or("");
            let cur_ver = m.get("version_id").and_then(Value::as_str).unwrap_or("");
            if provider.is_empty() || project_id.is_empty() {
                continue;
            }

            if let Err(err) = check_one_mod(
                state,
                &id,
                provider,
                project_id,
                cur_ver,
                &mc_version,
                loader,
                http,
            )
            .await
            {
                event!(
                    name: "anvil.modpack.poll.mod_check_error",
                    Level::WARN,
                    server.id = %id,
                    provider,
                    project_id,
                    err = %err,
                    "per-mod update check failed",
                );
            }
            sleep(PER_MOD_RATE_LIMIT_GAP).await;
        }
    }
    Ok(())
}

#[derive(Debug)]
struct LatestPick {
    version_id: String,
    version_name: String,
    published_at: Option<String>,
}

#[allow(
    clippy::too_many_arguments,
    reason = "internal helper, params are flat values"
)]
async fn check_one_mod(
    state: &AppState,
    server_id: &str,
    provider: &str,
    project_id: &str,
    current_version_id: &str,
    mc_version: &str,
    loader: &str,
    http: &ModpackHttp<'_>,
) -> anyhow::Result<()> {
    let Some(latest) =
        fetch_latest_compatible(http, provider, project_id, mc_version, loader).await?
    else {
        delete_mod_update(state, server_id, provider, project_id).await;
        return Ok(());
    };
    if latest.version_id == current_version_id {
        delete_mod_update(state, server_id, provider, project_id).await;
        return Ok(());
    }
    upsert_mod_update(
        state,
        server_id,
        provider,
        project_id,
        current_version_id,
        &latest,
    )
    .await;
    Ok(())
}

async fn fetch_latest_compatible(
    http: &ModpackHttp<'_>,
    provider: &str,
    project_id: &str,
    mc_version: &str,
    loader: &str,
) -> anyhow::Result<Option<LatestPick>> {
    match provider {
        "modrinth" => {
            let versions = http.mr.list_versions(project_id).await?;
            let pick = versions
                .iter()
                .filter(|v| v.loaders.iter().any(|l| l == loader))
                .filter(|v| v.game_versions.iter().any(|g| g == mc_version))
                .filter(|v| v.files.iter().any(|f| f.primary))
                .max_by(|a, b| a.date_published.cmp(&b.date_published))
                .cloned();
            Ok(pick.map(|v| LatestPick {
                version_id: v.id,
                version_name: v.version_number,
                published_at: Some(v.date_published),
            }))
        }
        "curseforge" => {
            let Some(cf) = http.cf else { return Ok(None) };
            let project_id_u32: u32 = project_id.parse()?;
            let files = cf.list_files(project_id_u32).await?;
            let pick = files
                .iter()
                .filter(|f| {
                    f.game_versions
                        .iter()
                        .any(|v| v.eq_ignore_ascii_case(mc_version))
                })
                .filter(|f| {
                    f.game_versions
                        .iter()
                        .any(|v| v.eq_ignore_ascii_case(loader))
                })
                .max_by(|a, b| a.file_date.cmp(&b.file_date))
                .cloned();
            Ok(pick.map(|f| LatestPick {
                version_id: f.id.to_string(),
                version_name: f.display_name,
                published_at: Some(f.file_date),
            }))
        }
        _ => Ok(None),
    }
}

async fn delete_mod_update(state: &AppState, server_id: &str, provider: &str, project_id: &str) {
    let _ = sqlx::query(
        "DELETE FROM mod_updates WHERE server_id = ? AND provider = ? AND project_id = ?",
    )
    .bind(server_id)
    .bind(provider)
    .bind(project_id)
    .execute(&state.pool)
    .await;
}

async fn upsert_mod_update(
    state: &AppState,
    server_id: &str,
    provider: &str,
    project_id: &str,
    current_version_id: &str,
    latest: &LatestPick,
) {
    let now = chrono::Utc::now().timestamp();
    let _ = sqlx::query(
        "INSERT OR REPLACE INTO mod_updates
             (server_id, provider, project_id, current_version_id, latest_version_id,
              latest_version_name, latest_published_at, checked_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(server_id)
    .bind(provider)
    .bind(project_id)
    .bind(current_version_id)
    .bind(&latest.version_id)
    .bind(&latest.version_name)
    .bind(latest.published_at.as_deref())
    .bind(now)
    .execute(&state.pool)
    .await;
}

#[cfg(test)]
mod tests {
    use crate::db;

    #[tokio::test]
    async fn upsert_then_delete_mod_update_round_trips() {
        let pool = db::init("sqlite::memory:").await.unwrap();
        sqlx::query(
            "INSERT INTO servers (
                id, name, mc_version, memory_mi, source_kind, exposure_mode,
                storage_size_gi, source_config, created_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("srv-1")
        .bind("smp")
        .bind("1.21.4")
        .bind(4096_i64)
        .bind("modded")
        .bind("clusterip")
        .bind(10_i64)
        .bind("{}")
        .bind(0_i64)
        .execute(&pool)
        .await
        .unwrap();

        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "INSERT INTO mod_updates
                 (server_id, provider, project_id, current_version_id,
                  latest_version_id, latest_version_name, latest_published_at, checked_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("srv-1")
        .bind("modrinth")
        .bind("fabric-api")
        .bind("ver-old")
        .bind("ver-new")
        .bind("Fabric API 0.99.0")
        .bind("2026-04-01T00:00:00Z")
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mod_updates WHERE server_id = ?")
            .bind("srv-1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);

        sqlx::query(
            "DELETE FROM mod_updates WHERE server_id = ? AND provider = ? AND project_id = ?",
        )
        .bind("srv-1")
        .bind("modrinth")
        .bind("fabric-api")
        .execute(&pool)
        .await
        .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mod_updates WHERE server_id = ?")
            .bind("srv-1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn mod_updates_cascade_delete_on_server_drop() {
        let pool = db::init("sqlite::memory:").await.unwrap();
        sqlx::query(
            "INSERT INTO servers (
                id, name, mc_version, memory_mi, source_kind, exposure_mode,
                storage_size_gi, source_config, created_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("srv-2")
        .bind("paper-srv")
        .bind("1.21.1")
        .bind(2048_i64)
        .bind("paper")
        .bind("clusterip")
        .bind(10_i64)
        .bind("{}")
        .bind(0_i64)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query(
            "INSERT INTO mod_updates
                 (server_id, provider, project_id, current_version_id,
                  latest_version_id, latest_version_name, latest_published_at, checked_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("srv-2")
        .bind("modrinth")
        .bind("luckperms")
        .bind("v1")
        .bind("v2")
        .bind("LP 5.5")
        .bind(Option::<String>::None)
        .bind(0_i64)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query("DELETE FROM servers WHERE id = ?")
            .bind("srv-2")
            .execute(&pool)
            .await
            .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mod_updates")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}
