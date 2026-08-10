// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Sync FSM for `modded` mods and Paper plugins.
//!
//! [`SyncTarget::Mods`] reads `modded.pending` and commits to `modded.mods`;
//! [`SyncTarget::Plugins`] reads `paper.pending_plugins` and commits to
//! `paper.plugins`. No backup — the synced dir is recoverable by re-applying.

use std::time::Duration;

use crate::AppState;
use crate::modpack::guard::{UpdateGuard, record_terminal, set_update_error};
use crate::modpack::jobs::build_mod_sync_job;
use crate::modpack::modded::{Config as ModdedConfig, ModEntry, ModdedRuntime};
use crate::modpack::orchestrator::{
    UpdatePhase, current_replicas, delete_job_and_wait, scale_to, wait_job, wait_pod_gone,
};
use crate::modpack::paper::Config as PaperConfig;
use crate::routes::servers::create::insert_audit;
use anyhow::{Context as _, Result, anyhow, bail};
use chrono::Utc;
use k8s_openapi::api::batch::v1::Job;
use kube::Api;
use kube::api::PostParams;
use serde_json::json;
use sqlx::SqlitePool;

const POD_TERMINATE_TIMEOUT: Duration = Duration::from_secs(90);
const SYNC_JOB_TIMEOUT: Duration = Duration::from_mins(15);

/// Selects which `source_config` field + on-disk subdir the sync FSM operates on.
#[derive(Debug, Clone, Copy)]
pub enum SyncTarget {
    /// Operates on `modded.pending` / `modded.mods`, syncs `/data/mods/`.
    Mods,
    /// Operates on `paper.pending_plugins` / `paper.plugins`, syncs `/data/plugins/`.
    Plugins,
}

impl SyncTarget {
    /// Data-relative subdir the sync Job manages.
    fn target_dir(self) -> &'static str {
        match self {
            Self::Mods => "mods",
            Self::Plugins => "plugins",
        }
    }
    /// `servers.source_kind` discriminator this target is valid for.
    fn expected_source_kind(self) -> &'static str {
        match self {
            Self::Mods => "modded",
            Self::Plugins => "paper",
        }
    }
    /// Audit-action prefix (`mods_apply_*` / `plugins_apply_*`).
    fn audit_prefix(self) -> &'static str {
        match self {
            Self::Mods => "mods",
            Self::Plugins => "plugins",
        }
    }
}

/// Kicks off the sync FSM for `server_id`. Long-running task; spawned by
/// the route handler; drops `guard` on completion.
pub async fn run(state: AppState, server_id: String, guard: UpdateGuard, target: SyncTarget) {
    let outcome = run_inner(&state, &server_id, &guard, target).await;
    match outcome {
        Ok(()) => {
            record_terminal(&state, &server_id, UpdatePhase::Succeeded);
            guard.emit(UpdatePhase::Succeeded);
            tracing::info!(
                server.id = %server_id,
                target = target.audit_prefix(),
                "sync succeeded",
            );
        }
        Err(err) => {
            set_update_error(&state, &server_id, err.to_string());
            record_terminal(&state, &server_id, UpdatePhase::Failed);
            guard.emit(UpdatePhase::Failed);
            tracing::error!(
                server.id = %server_id,
                target = target.audit_prefix(),
                err = %err,
                "sync failed",
            );
            let now = Utc::now().timestamp();
            let action = format!("{}_apply_failed", target.audit_prefix());
            let _ = insert_audit(
                &state.pool,
                &server_id,
                &action,
                Some(json!({"err": err.to_string()})),
                now,
            )
            .await;
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "linear FSM reads top-to-bottom; splitting it loses context"
)]
async fn run_inner(
    state: &AppState,
    server_id: &str,
    guard: &UpdateGuard,
    target: SyncTarget,
) -> Result<()> {
    let now = Utc::now().timestamp();
    let started_action = format!("{}_apply_started", target.audit_prefix());
    insert_audit(&state.pool, server_id, &started_action, None, now).await?;

    // Load + validate the config.
    let row: (String, String) =
        sqlx::query_as("SELECT source_kind, source_config FROM servers WHERE id = ?")
            .bind(server_id)
            .fetch_one(&state.pool)
            .await
            .with_context(|| format!("loading source for {server_id}"))?;
    if row.0 != target.expected_source_kind() {
        bail!(
            "{}_apply only valid for {} servers (got {})",
            target.audit_prefix(),
            target.expected_source_kind(),
            row.0
        );
    }
    let desired = compute_desired(&row.1, target)?;

    // Capture pre-sync replica count so we don't auto-start a server the
    // user had stopped. Mirrors the manual-backup `was_running` pattern;
    // sync has no rollback (re-apply recovers the dir), so leaving the
    // server at the user's prior state is the right default.
    let was_running = current_replicas(&state.kube, &state.mc_namespace, server_id).await? >= 1;

    // Acquire the global Job lock. Sync only mounts the data PVC, but
    // serializing all panel-spawned Jobs keeps the cluster gentle.
    let permit = state.snapshot_pvc_lock.lock().await;

    // Stop. No-op when already stopped — the wait_pod_gone returns
    // immediately because the pod is already absent.
    guard.emit(UpdatePhase::Stopping);
    scale_to(&state.kube, &state.mc_namespace, server_id, 0).await?;
    let mc_pod = format!("mc-{server_id}-0");
    wait_pod_gone(
        &state.kube,
        &state.mc_namespace,
        &mc_pod,
        POD_TERMINATE_TIMEOUT,
    )
    .await?;

    // Swapping doubles as the sync phase.
    guard.emit(UpdatePhase::Swapping);
    let keep: Vec<&str> = desired.iter().map(|m| m.filename.as_str()).collect();
    let urls: Vec<(&str, &str, Option<&str>)> = desired
        .iter()
        .map(|m| {
            (
                m.filename.as_str(),
                m.download_url.as_str(),
                m.sha512.as_deref(),
            )
        })
        .collect();
    let ts = Utc::now().timestamp();
    let sync_job = build_mod_sync_job(
        server_id,
        ts,
        &state.mc_namespace,
        target.target_dir(),
        &keep,
        &urls,
        &state.mc_alpine_image,
    );
    let job_name = sync_job
        .metadata
        .name
        .clone()
        .ok_or_else(|| anyhow!("sync Job missing name"))?;
    let jobs: Api<Job> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    if jobs.get_opt(&job_name).await?.is_some() {
        delete_job_and_wait(&jobs, &job_name, Duration::from_secs(30)).await?;
    }
    jobs.create(&PostParams::default(), &sync_job)
        .await
        .with_context(|| format!("creating Job {job_name}"))?;
    wait_job(
        &state.kube,
        &state.mc_namespace,
        &job_name,
        SYNC_JOB_TIMEOUT,
    )
    .await?;

    drop(permit);

    // No rollback: re-applying re-syncs. Only restart if the server was
    // running at the start — a stopped server stays stopped so the user's
    // first Start click is the explicit boot trigger.
    if was_running {
        guard.emit(UpdatePhase::Starting);
        scale_to(&state.kube, &state.mc_namespace, server_id, 1).await?;
        sqlx::query("UPDATE servers SET last_started_at = ? WHERE id = ?")
            .bind(Utc::now().timestamp())
            .bind(server_id)
            .execute(&state.pool)
            .await?;
    }

    // Persist: replace committed list, clear pending.
    let new_count = commit(&state.pool, server_id, &row.1, target, desired).await?;

    let now = Utc::now().timestamp();
    let succeeded_action = format!("{}_apply_succeeded", target.audit_prefix());
    insert_audit(
        &state.pool,
        server_id,
        &succeeded_action,
        Some(json!({"count": new_count})),
        now,
    )
    .await?;
    Ok(())
}

/// Resolves the desired file list from the persisted `source_config`. Errors
/// when the config has no pending changes — the caller surfaces this as
/// `nothing_pending` via the route handler.
fn compute_desired(raw: &str, target: SyncTarget) -> Result<Vec<ModEntry>> {
    match target {
        SyncTarget::Mods => {
            let cfg: ModdedConfig =
                serde_json::from_str(raw).context("source_config not modded JSON")?;
            if cfg.pending.is_empty() {
                bail!("no pending changes to apply");
            }
            Ok(ModdedRuntime::new(cfg).desired_mods())
        }
        SyncTarget::Plugins => {
            let cfg: PaperConfig =
                serde_json::from_str(raw).context("source_config not paper JSON")?;
            if cfg.pending_plugins.is_empty() {
                bail!("no pending changes to apply");
            }
            Ok(cfg.pending_plugins)
        }
    }
}

/// Persists the post-sync state: the applied `desired` list becomes the
/// committed list, and the pending entries we applied are cleared.
///
/// The sync Job runs for minutes, during which the user can stage more
/// pending edits. Those must survive — so this re-reads the *current*
/// `source_config` instead of trusting the pre-Job snapshot, writes under a
/// compare-and-swap, and only clears pending when nothing changed meanwhile.
/// `start_raw` is the pre-Job snapshot, used to identify which pending
/// entries were the ones we just applied.
async fn commit(
    pool: &SqlitePool,
    server_id: &str,
    start_raw: &str,
    target: SyncTarget,
    desired: Vec<ModEntry>,
) -> Result<usize> {
    let new_count = desired.len();
    for attempt in 0..2 {
        let current_raw = fetch_source_config(pool, server_id).await?;
        let new_raw = build_committed_config(&current_raw, start_raw, target, &desired)?;
        let res =
            sqlx::query("UPDATE servers SET source_config = ? WHERE id = ? AND source_config = ?")
                .bind(&new_raw)
                .bind(server_id)
                .bind(&current_raw)
                .execute(pool)
                .await?;
        if res.rows_affected() > 0 {
            break;
        }
        if attempt == 1 {
            // Two writers in the millisecond window between this read and the
            // CAS is astronomically unlikely on a single-user homelab. Fail
            // loudly (pending is preserved in the DB) rather than clobber it;
            // a re-apply re-syncs idempotently.
            bail!("source_config changed concurrently while committing apply");
        }
    }
    Ok(new_count)
}

/// Reads the live `source_config` JSON for `server_id`.
async fn fetch_source_config(pool: &SqlitePool, server_id: &str) -> Result<String> {
    let row: (String,) = sqlx::query_as("SELECT source_config FROM servers WHERE id = ?")
        .bind(server_id)
        .fetch_one(pool)
        .await
        .with_context(|| format!("re-reading source_config for {server_id}"))?;
    Ok(row.0)
}

/// Builds the committed `source_config`: the applied `desired` list becomes
/// the installed list, and pending is cleared *only* of what we applied.
///
/// When the config is byte-identical to the pre-Job snapshot
/// (`current_raw == start_raw`), no edit landed during the Job, so pending is
/// emptied wholesale. When it changed, the user staged more during the Job —
/// keep those entries so the next apply installs them.
fn build_committed_config(
    current_raw: &str,
    start_raw: &str,
    target: SyncTarget,
    desired: &[ModEntry],
) -> Result<String> {
    match target {
        SyncTarget::Mods => {
            let mut cfg: ModdedConfig =
                serde_json::from_str(current_raw).context("source_config not modded JSON")?;
            cfg.mods = desired.to_vec();
            if current_raw == start_raw {
                cfg.pending = Vec::new();
            } else {
                // Drop only the ops we applied; keep ones staged mid-Job.
                let start_cfg: ModdedConfig = serde_json::from_str(start_raw)
                    .context("pre-Job source_config not modded JSON")?;
                cfg.pending.retain(|op| !start_cfg.pending.contains(op));
            }
            Ok(serde_json::to_string(&cfg)?)
        }
        SyncTarget::Plugins => {
            let mut cfg: PaperConfig =
                serde_json::from_str(current_raw).context("source_config not paper JSON")?;
            cfg.plugins = desired.to_vec();
            // pending_plugins is the full staged list; clear it only when
            // nothing changed during the Job. If it did, the current
            // pending_plugins is the newer desired list — keep it as pending.
            if current_raw == start_raw {
                cfg.pending_plugins = Vec::new();
            }
            Ok(serde_json::to_string(&cfg)?)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> ModEntry {
        ModEntry {
            provider: "modrinth".to_owned(),
            project_id: name.to_owned(),
            project_slug: name.to_owned(),
            project_name: name.to_owned(),
            version_id: "v1".to_owned(),
            version_name: "1.0".to_owned(),
            filename: format!("{name}.jar"),
            download_url: format!("https://cdn.modrinth.com/{name}.jar"),
            sha512: None,
        }
    }

    fn paper_raw(plugins: &[&str], pending: &[&str]) -> String {
        let cfg = PaperConfig {
            mc_version: "1.21.4".to_owned(),
            paper_build: None,
            plugins: plugins.iter().map(|n| entry(n)).collect(),
            pending_plugins: pending.iter().map(|n| entry(n)).collect(),
            auto_update_mode: crate::modpack::modded::AutoUpdateMode::default(),
        };
        serde_json::to_string(&cfg).unwrap()
    }

    async fn seed(pool: &SqlitePool, id: &str, source_config: &str) {
        sqlx::query(
            "INSERT INTO servers
                (id, name, mc_version, memory_mi, source_kind, exposure_mode,
                 storage_size_gi, source_config, created_at)
             VALUES (?, ?, '1.21.4', 4096, 'paper', 'clusterip', 10, ?, 0)",
        )
        .bind(id)
        .bind(id)
        .bind(source_config)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn commit_preserves_plugin_edits_staged_during_apply() {
        let pool = crate::db::init("sqlite::memory:").await.unwrap();
        // Pre-Job snapshot: "sodium" staged.
        let start_raw = paper_raw(&[], &["sodium"]);
        seed(&pool, "s1", &start_raw).await;
        // During the Job the user stages "lithium" too.
        let mid_raw = paper_raw(&[], &["sodium", "lithium"]);
        sqlx::query("UPDATE servers SET source_config = ? WHERE id = ?")
            .bind(&mid_raw)
            .bind("s1")
            .execute(&pool)
            .await
            .unwrap();

        // commit() runs with the STALE snapshot + the desired computed from it.
        commit(
            &pool,
            "s1",
            &start_raw,
            SyncTarget::Plugins,
            vec![entry("sodium")],
        )
        .await
        .unwrap();

        let (raw,): (String,) = sqlx::query_as("SELECT source_config FROM servers WHERE id = ?")
            .bind("s1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let cfg: PaperConfig = serde_json::from_str(&raw).unwrap();
        assert_eq!(cfg.plugins.len(), 1, "sodium committed");
        assert_eq!(cfg.plugins[0].filename, "sodium.jar");
        assert!(
            cfg.pending_plugins
                .iter()
                .any(|p| p.filename == "lithium.jar"),
            "plugin staged during the apply must survive commit",
        );
    }

    #[tokio::test]
    async fn commit_clears_pending_on_clean_apply() {
        let pool = crate::db::init("sqlite::memory:").await.unwrap();
        let start_raw = paper_raw(&[], &["sodium"]);
        seed(&pool, "s1", &start_raw).await;

        commit(
            &pool,
            "s1",
            &start_raw,
            SyncTarget::Plugins,
            vec![entry("sodium")],
        )
        .await
        .unwrap();

        let (raw,): (String,) = sqlx::query_as("SELECT source_config FROM servers WHERE id = ?")
            .bind("s1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let cfg: PaperConfig = serde_json::from_str(&raw).unwrap();
        assert_eq!(cfg.plugins.len(), 1);
        assert!(
            cfg.pending_plugins.is_empty(),
            "pending cleared when nothing changed during the apply",
        );
    }
}
