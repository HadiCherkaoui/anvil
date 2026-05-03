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
use crate::modpack::{from_db, orchestrator};

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
    let cf = state
        .cf_client
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("CF disabled, poller should not be running"))?;

    let rows = sqlx::query(
        "SELECT id, source_kind, source_config FROM servers WHERE source_kind != 'vanilla'",
    )
    .fetch_all(&state.pool)
    .await?;

    for row in rows {
        let id: String = row.try_get("id")?;
        let source_kind: String = row.try_get("source_kind")?;
        let source_config: String = row.try_get("source_config")?;

        // One slow CF call per server, every poll_interval; serial is fine
        // at homelab scale.
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

        let latest = match provider.latest(cf).await {
            Ok(Some(v)) => v,
            Ok(None) => continue, // vanilla returns None
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
        let current_id = cfg
            .get("current_version_id")
            .and_then(Value::as_u64)
            .and_then(|u| u32::try_from(u).ok())
            .unwrap_or(0);
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

        let id_str = latest.id.to_string();
        let skipped = skip_list.iter().any(|s| s == &id_str || s == &latest.name);

        if latest.id == current_id || auto_mode == "never" || skipped {
            // Clear any stale modpack_versions row — current is up to date.
            let _ = sqlx::query("DELETE FROM modpack_versions WHERE server_id = ?")
                .bind(&id)
                .execute(&state.pool)
                .await;
            continue;
        }

        let now = chrono::Utc::now().timestamp();
        let _ = sqlx::query(
            "INSERT OR REPLACE INTO modpack_versions
             (server_id, latest_id, latest_name, latest_download_url, checked_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(i64::from(latest.id))
        .bind(&latest.name)
        .bind(&latest.download_url)
        .bind(now)
        .execute(&state.pool)
        .await;

        if auto_mode == "apply" {
            // Fire the orchestrator inline — same task handles one auto-update
            // at a time so we don't pile up. Other servers in the same tick
            // wait for this one to finish.
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
            // Run synchronously inside the poll tick — keeps things simple
            // and avoids an unbounded task pile-up.
            orchestrator::run(state.clone(), id.clone(), latest.id, guard).await;
        }
    }
    Ok(())
}
