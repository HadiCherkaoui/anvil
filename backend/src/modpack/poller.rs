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
use tracing::{Level, event};

use crate::AppState;
use crate::modpack::guard::UpdateGuard;
use crate::modpack::{ModpackHttp, from_db, orchestrator};

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
                state.update_errors.clone(),
            ) else {
                continue;
            };
            // Spawn detached: the orchestrator can run for many minutes
            // (PVC tar Job, mod sync Job, STS rollout). Awaiting here would
            // stall the entire poll tick — and we hold no per-server lock
            // beyond `guard`, which moves into the task.
            let task_state = state.clone();
            let task_id = id.clone();
            let task_version = latest.id.clone();
            tokio::spawn(async move {
                orchestrator::run(task_state, task_id, task_version, guard).await;
            });
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
    if let Err(err) = poll_loader_versions(state).await {
        event!(
            name: "anvil.modpack.poll.loader_error",
            Level::WARN,
            err = %err,
            "loader-version poll failed; continuing",
        );
    }
    Ok(())
}

/// Walks every modded forge/neoforge server, pulls the latest published
/// loader for the server's MC version from maven, and upserts/deletes
/// the `loader_updates` row. Fabric is skipped — itzg pulls LATEST every
/// boot. Paper is out of scope (`paper_build` is rarely pinned).
#[allow(
    clippy::too_many_lines,
    reason = "linear per-server fan-out; splitting it loses sequence context"
)]
async fn poll_loader_versions(state: &AppState) -> anyhow::Result<()> {
    let rows = sqlx::query("SELECT id, source_config FROM servers WHERE source_kind = 'modded'")
        .fetch_all(&state.pool)
        .await?;

    for row in rows {
        let id: String = row.try_get("id")?;
        let cfg_raw: String = row.try_get("source_config")?;
        let cfg: Value = match serde_json::from_str(&cfg_raw) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let runtime = cfg
            .get("runtime")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let runtime: &'static str = match runtime {
            "forge" => "forge",
            "neoforge" => "neoforge",
            // Fabric / unknown — itzg LATEST or unsupported. Drop any
            // stale row left from a previous runtime.
            _ => {
                let _ = sqlx::query("DELETE FROM loader_updates WHERE server_id = ?")
                    .bind(&id)
                    .execute(&state.pool)
                    .await;
                continue;
            }
        };
        let auto_mode = cfg
            .get("auto_update_mode")
            .and_then(Value::as_str)
            .unwrap_or("notify");
        if auto_mode == "never" {
            let _ = sqlx::query("DELETE FROM loader_updates WHERE server_id = ?")
                .bind(&id)
                .execute(&state.pool)
                .await;
            continue;
        }
        let mc_version = cfg
            .get("mc_version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let current_loader = cfg
            .get("loader_version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if mc_version.is_empty() || current_loader.is_empty() {
            continue;
        }

        let listing =
            match crate::routes::runtimes::cached_or_fetch(&state.loader_version_cache, runtime)
                .await
            {
                Ok(v) => v,
                Err(err) => {
                    event!(
                        name: "anvil.modpack.poll.loader_upstream_error",
                        Level::WARN,
                        server.id = %id,
                        runtime,
                        err = %err,
                        "skipping server",
                    );
                    continue;
                }
            };
        // `by_mc[mc_version]` is sorted newest-first by the parsers.
        let Some(latest) = listing
            .by_mc
            .get(&mc_version)
            .and_then(|v| v.first())
            .cloned()
        else {
            // No published loader for this MC; clear any stale row.
            let _ = sqlx::query("DELETE FROM loader_updates WHERE server_id = ?")
                .bind(&id)
                .execute(&state.pool)
                .await;
            continue;
        };
        if latest == current_loader {
            let _ = sqlx::query("DELETE FROM loader_updates WHERE server_id = ?")
                .bind(&id)
                .execute(&state.pool)
                .await;
            continue;
        }

        let now = chrono::Utc::now().timestamp();
        let _ = sqlx::query(
            "INSERT OR REPLACE INTO loader_updates
                 (server_id, current_loader, latest_loader, checked_at)
                 VALUES (?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(&current_loader)
        .bind(&latest)
        .bind(now)
        .execute(&state.pool)
        .await;
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
        // Paper plugins ship as Bukkit/Spigot jars too — Modrinth tags
        // `paper`-only entries on a minority of plugins, so we widen the
        // accept-set rather than missing valid updates.
        let (loaders, mods_field): (&[&str], &str) = if source_kind == "paper" {
            (&["paper", "bukkit", "spigot"], "plugins")
        } else {
            let runtime = cfg
                .get("runtime")
                .and_then(Value::as_str)
                .unwrap_or("fabric");
            // Tied to the lifetime of cfg; map the owned String to a static
            // accept-list via the runtime label captured below.
            match runtime {
                "forge" => (&["forge"][..], "mods"),
                "neoforge" => (&["neoforge"][..], "mods"),
                "quilt" => (&["quilt"][..], "mods"),
                // Default to fabric — it's the most common and safe to
                // probe; an actual mismatch returns no upstream hits and
                // the row is left alone.
                _ => (&["fabric"][..], "mods"),
            }
        };
        let auto_mode = cfg
            .get("auto_update_mode")
            .and_then(Value::as_str)
            .unwrap_or("notify");

        // Never: skip the per-mod fan-out and drop any leftover rows the
        // last poll wrote so the UI stops surfacing stale updates.
        if auto_mode == "never" {
            let _ = sqlx::query("DELETE FROM mod_updates WHERE server_id = ?")
                .bind(&id)
                .execute(&state.pool)
                .await;
            continue;
        }

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
                loaders,
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

        // Apply: enqueue Bump pending ops for everything the per-mod
        // pass just flagged, then run the sync FSM. Done after the
        // notify pass so notify+apply share the same detection path.
        if auto_mode == "apply"
            && let Err(err) = auto_apply_pending(state, &id, &source_kind, http).await
        {
            event!(
                name: "anvil.modpack.poll.auto_apply_error",
                Level::WARN,
                server.id = %id,
                err = %err,
                "auto-apply pass failed",
            );
        }
    }
    Ok(())
}

/// Reads every `mod_updates` row for `server_id`, fetches the new
/// version's primary file metadata (filename/url/sha), appends a `Bump`
/// pending op for each, and — if any were queued — runs the sync FSM.
/// Errors per-mod are logged-and-skipped; the function only returns Err
/// for unrecoverable failures (DB / lock acquisition).
#[allow(
    clippy::too_many_lines,
    reason = "linear per-mod fan-out; splitting loses sequence context"
)]
async fn auto_apply_pending(
    state: &AppState,
    server_id: &str,
    source_kind: &str,
    http: &ModpackHttp<'_>,
) -> anyhow::Result<()> {
    use crate::modpack::mods_apply::{self, SyncTarget};

    let updates: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT provider, project_id, latest_version_id
         FROM mod_updates WHERE server_id = ?",
    )
    .bind(server_id)
    .fetch_all(&state.pool)
    .await?;
    if updates.is_empty() {
        return Ok(());
    }

    // Load the live source_config and append Bump ops in one shot. We
    // mutate the JSON Value to avoid round-tripping through the
    // strongly-typed Config (which differs between modded and paper).
    let raw: String = sqlx::query_scalar("SELECT source_config FROM servers WHERE id = ?")
        .bind(server_id)
        .fetch_one(&state.pool)
        .await?;
    let mut cfg: Value = serde_json::from_str(&raw)?;
    let installed_field = if source_kind == "paper" {
        "plugins"
    } else {
        "mods"
    };
    let pending_field = if source_kind == "paper" {
        "pending_plugins"
    } else {
        "pending"
    };
    let installed: Vec<Value> = cfg
        .get(installed_field)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut bumps_added = 0_usize;

    for (provider, project_id, latest_version_id) in updates {
        let Some(installed_entry) = installed.iter().find(|m| {
            m.get("provider").and_then(Value::as_str) == Some(provider.as_str())
                && m.get("project_id").and_then(Value::as_str) == Some(project_id.as_str())
        }) else {
            continue;
        };
        let cur_filename = installed_entry
            .get("filename")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if cur_filename.is_empty() {
            continue;
        }

        let pick = match fetch_version_with_file(http, &provider, &latest_version_id).await {
            Ok(p) => p,
            Err(err) => {
                event!(
                    name: "anvil.modpack.poll.auto_apply_lookup_error",
                    Level::WARN,
                    server.id = %server_id,
                    provider = provider.as_str(),
                    project_id = project_id.as_str(),
                    err = %err,
                    "auto-apply: latest version lookup failed; skipping",
                );
                continue;
            }
        };

        let bump = serde_json::json!({
            "op": "bump",
            "filename": cur_filename,
            "to_version_id": pick.version_id,
            "to_version_name": pick.version_name,
            "to_filename": pick.filename,
            "to_download_url": pick.download_url,
            "to_sha512": pick.sha512,
        });
        let pending_arr = cfg
            .get_mut(pending_field)
            .and_then(Value::as_array_mut)
            .ok_or_else(|| anyhow::anyhow!("source_config missing {pending_field} array"))?;
        pending_arr.push(bump);
        bumps_added += 1;
    }

    if bumps_added == 0 {
        return Ok(());
    }

    let new_raw = serde_json::to_string(&cfg)?;
    sqlx::query("UPDATE servers SET source_config = ? WHERE id = ?")
        .bind(&new_raw)
        .bind(server_id)
        .execute(&state.pool)
        .await?;

    let target = if source_kind == "paper" {
        SyncTarget::Plugins
    } else {
        SyncTarget::Mods
    };
    let Some(guard) = UpdateGuard::try_acquire(
        server_id,
        state.update_locks.clone(),
        state.update_phase_buses.clone(),
        state.update_errors.clone(),
    ) else {
        // Another apply / update holds the lock — leave the bumps in
        // pending; the next poll tick (or a manual click) picks them up.
        return Ok(());
    };
    // Fire-and-forget: AppState has no TaskTracker, so the JoinHandle is
    // intentionally dropped. If the panel restarts mid-apply, the spawned
    // task is killed with the runtime; the per-server lock (released by
    // the dropped UpdateGuard) and pending bumps in source_config let the
    // next poll tick or manual click resume. Log so operators notice.
    let task_state = state.clone();
    let task_id = server_id.to_owned();
    let task_id_log = task_id.clone();
    tokio::spawn(async move {
        mods_apply::run(task_state, task_id, guard, target).await;
    });
    event!(
        name: "anvil.modpack.poll.auto_apply_unmanaged",
        Level::WARN,
        server.id = %task_id_log,
        "auto-apply task spawned without lifecycle management; panel restart will abort it",
    );
    event!(
        name: "anvil.modpack.poll.auto_apply",
        Level::INFO,
        server.id = %server_id,
        bumps = bumps_added,
        "queued auto-apply for per-mod updates",
    );
    Ok(())
}

/// Like [`fetch_latest_compatible`] but returns the file metadata
/// (filename / download URL / sha) the sync Job needs. Used by the
/// `auto_update_mode = "apply"` path. Modrinth-only — individual mods
/// never come from `CurseForge` in the current catalog flow.
async fn fetch_version_with_file(
    http: &ModpackHttp<'_>,
    provider: &str,
    version_id: &str,
) -> anyhow::Result<FilePick> {
    if provider != "modrinth" {
        anyhow::bail!("auto-apply only supports modrinth (got {provider:?})");
    }
    let v = http.mr.version(version_id).await?;
    let f = v
        .files
        .iter()
        .find(|f| f.primary)
        .or_else(|| v.files.first())
        .ok_or_else(|| anyhow::anyhow!("modrinth version has no files"))?;
    Ok(FilePick {
        version_id: v.id.clone(),
        version_name: v.version_number.clone(),
        filename: f.filename.clone(),
        download_url: f.url.clone(),
        sha512: f.hashes.sha512.clone(),
    })
}

/// Subset of [`super::modded::ModEntry`] needed to construct a Bump op.
struct FilePick {
    version_id: String,
    version_name: String,
    filename: String,
    download_url: String,
    sha512: Option<String>,
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
    loaders: &[&str],
    http: &ModpackHttp<'_>,
) -> anyhow::Result<()> {
    let Some(latest) =
        fetch_latest_compatible(http, provider, project_id, mc_version, loaders).await?
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

/// Picks the newest Modrinth version compatible with `(mc_version, loader)`.
/// Returns `None` when nothing matches or `provider` isn't `"modrinth"`
/// (every individual mod in our catalog flow is Modrinth-sourced — see
/// `routes::catalog::search`).
async fn fetch_latest_compatible(
    http: &ModpackHttp<'_>,
    provider: &str,
    project_id: &str,
    mc_version: &str,
    loaders: &[&str],
) -> anyhow::Result<Option<LatestPick>> {
    if provider != "modrinth" {
        return Ok(None);
    }
    let versions = http.mr.list_versions(project_id).await?;
    // Tier the filter so a Paper plugin with both `paper`-only and
    // `bukkit`/`spigot` builds prefers Paper-tagged ones, falling back
    // only when none exist. The first non-empty tier wins.
    let pick = loaders.iter().find_map(|accept| {
        versions
            .iter()
            .filter(|v| v.loaders.iter().any(|l| l == accept))
            .filter(|v| v.game_versions.iter().any(|g| g == mc_version))
            .filter(|v| v.files.iter().any(|f| f.primary))
            .max_by(|a, b| a.date_published.cmp(&b.date_published))
            .cloned()
    });
    Ok(pick.map(|v| LatestPick {
        version_id: v.id,
        version_name: v.version_number,
        published_at: Some(v.date_published),
    }))
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
