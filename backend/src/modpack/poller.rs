//! Hourly modpack version poller.
//!
//! On each tick, iterates every non-vanilla server, asks its provider for
//! the latest upstream version, and either upserts `modpack_versions` (so
//! the frontend banner shows up) or — when `auto_update_mode=apply` — fires
//! the update orchestrator inline.

use std::time::Duration;

use serde_json::Value;
use sqlx::Row as _;
use tokio::time::sleep;
use tracing::{Level, event};

use crate::AppState;
use crate::modpack::guard::UpdateGuard;
use crate::modpack::{ModpackHttp, from_db, orchestrator};

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
        // current_version_id is stored as a number for legacy CF rows and as
        // a string for Modrinth — handle both.
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
    Ok(())
}
