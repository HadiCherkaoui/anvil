//! Sync FSM for `modded` mods and Paper plugins.
//!
//! Both flows share an identical scale → Job → scale → verify dance —
//! only the `source_config` field that is read/written and the
//! data-relative `target_dir` differ. [`SyncTarget::Mods`] reads
//! `modded.pending` and commits to `modded.mods`;
//! [`SyncTarget::Plugins`] reads `paper.pending_plugins` and commits to
//! `paper.plugins`. Both reuse [`UpdateGuard`] + `snapshot_pvc_lock` +
//! the WS-bus pattern. No backup; the synced dir is recoverable by
//! clicking apply again.

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use chrono::Utc;
use k8s_openapi::api::batch::v1::Job;
use kube::Api;
use kube::api::{DeleteParams, PostParams};
use serde_json::json;
use sqlx::SqlitePool;
use tokio::time::sleep;

use crate::AppState;
use crate::modpack::guard::{UpdateGuard, record_terminal, set_update_error};
use crate::modpack::jobs::build_mod_sync_job;
use crate::modpack::modded::{Config as ModdedConfig, ModEntry, ModdedRuntime};
use crate::modpack::orchestrator::{
    UpdatePhase, current_replicas, scale_to, wait_job, wait_pod_gone,
};
use crate::modpack::paper::Config as PaperConfig;
use crate::routes::servers::create::insert_audit;

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
    // serializing all panel-spawned Jobs keeps the cluster gentle and
    // matches the M5 update FSM's pattern.
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

    // Sync. UpdatePhase::Swapping is reused as the sync phase so the
    // existing UpdateSheet phase list keeps working unchanged.
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
    );
    let job_name = sync_job
        .metadata
        .name
        .clone()
        .ok_or_else(|| anyhow!("sync Job missing name"))?;
    let jobs: Api<Job> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    if jobs.get_opt(&job_name).await?.is_some() {
        let _ = jobs.delete(&job_name, &DeleteParams::default()).await;
        sleep(Duration::from_secs(1)).await;
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

    // Restore prior replica count. Sync has no rollback (re-applying
    // re-syncs), no backup, and no boot-marker to verify against —
    // mirroring `run_backup`, we only restart if the server was running
    // at the start. A manual-create + initial-mod-install path leaves
    // the server stopped so the user's first Start click is the explicit
    // boot trigger.
    if was_running {
        guard.emit(UpdatePhase::Starting);
        scale_to(&state.kube, &state.mc_namespace, server_id, 1).await?;
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

/// Persists the post-sync state: the desired list becomes the committed
/// list, and the pending field is cleared.
async fn commit(
    pool: &SqlitePool,
    server_id: &str,
    raw: &str,
    target: SyncTarget,
    desired: Vec<ModEntry>,
) -> Result<usize> {
    let new_count = desired.len();
    let new_raw = match target {
        SyncTarget::Mods => {
            let mut cfg: ModdedConfig =
                serde_json::from_str(raw).context("source_config not modded JSON")?;
            cfg.mods = desired;
            cfg.pending = Vec::new();
            serde_json::to_string(&cfg)?
        }
        SyncTarget::Plugins => {
            let mut cfg: PaperConfig =
                serde_json::from_str(raw).context("source_config not paper JSON")?;
            cfg.plugins = desired;
            cfg.pending_plugins = Vec::new();
            serde_json::to_string(&cfg)?
        }
    };
    sqlx::query("UPDATE servers SET source_config = ? WHERE id = ?")
        .bind(&new_raw)
        .bind(server_id)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE servers SET last_started_at = ? WHERE id = ?")
        .bind(Utc::now().timestamp())
        .bind(server_id)
        .execute(pool)
        .await?;
    Ok(new_count)
}
