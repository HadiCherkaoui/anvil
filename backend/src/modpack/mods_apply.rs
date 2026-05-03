//! Mod-sync FSM for `modded` servers.
//!
//! Runs from a click on the Mods tab `[apply now]` button. Re-uses
//! [`UpdateGuard`] + `snapshot_pvc_lock` + the WS-bus pattern. No backup;
//! `mods/` is recoverable by clicking apply again.

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use chrono::Utc;
use k8s_openapi::api::batch::v1::Job;
use kube::Api;
use kube::api::{DeleteParams, PostParams};
use serde_json::json;
use tokio::time::sleep;

use crate::AppState;
use crate::modpack::guard::UpdateGuard;
use crate::modpack::jobs::build_mod_sync_job;
use crate::modpack::modded::{Config as ModdedConfig, ModdedRuntime};
use crate::modpack::orchestrator::{
    UpdatePhase, scale_to, wait_for_done_marker, wait_job, wait_pod_gone, wait_pod_running,
};
use crate::routes::servers::create::insert_audit;

const POD_TERMINATE_TIMEOUT: Duration = Duration::from_secs(90);
const SYNC_JOB_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const POD_RUNNING_TIMEOUT: Duration = Duration::from_secs(120);
const VERIFY_BOOT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Kicks off the mod-sync FSM for `server_id`. Long-running task; spawned
/// by the route handler; drops `guard` on completion.
pub async fn run(state: AppState, server_id: String, guard: UpdateGuard) {
    let outcome = run_inner(&state, &server_id, &guard).await;
    match outcome {
        Ok(()) => {
            guard.emit(UpdatePhase::Succeeded);
            tracing::info!(server.id = %server_id, "mod-sync succeeded");
        }
        Err(err) => {
            guard.emit(UpdatePhase::Failed);
            tracing::error!(server.id = %server_id, err = %err, "mod-sync failed");
            let now = Utc::now().timestamp();
            let _ = insert_audit(
                &state.pool,
                &server_id,
                "mods_apply_failed",
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
async fn run_inner(state: &AppState, server_id: &str, guard: &UpdateGuard) -> Result<()> {
    let now = Utc::now().timestamp();
    insert_audit(&state.pool, server_id, "mods_apply_started", None, now).await?;

    // Load + validate the config.
    let row: (String, String) =
        sqlx::query_as("SELECT source_kind, source_config FROM servers WHERE id = ?")
            .bind(server_id)
            .fetch_one(&state.pool)
            .await
            .with_context(|| format!("loading source for {server_id}"))?;
    if row.0 != "modded" {
        bail!("mods_apply only valid for modded servers (got {})", row.0);
    }
    let cfg: ModdedConfig =
        serde_json::from_str(&row.1).context("source_config not modded JSON")?;
    if cfg.pending.is_empty() {
        bail!("no pending changes to apply");
    }
    let runtime = ModdedRuntime::new(cfg.clone());
    let desired = runtime.desired_mods();

    // Acquire the global Job lock. Mod-sync only mounts the data PVC, but
    // serializing all panel-spawned Jobs keeps the cluster gentle and
    // matches the M5 update FSM's pattern.
    let permit = state.snapshot_pvc_lock.lock().await;

    // Stop.
    guard.emit(UpdatePhase::Stopping);
    scale_to(&state.kube, &state.mc_namespace, server_id, 0).await?;
    wait_pod_gone(
        &state.kube,
        &state.mc_namespace,
        server_id,
        POD_TERMINATE_TIMEOUT,
    )
    .await?;

    // Sync mods. UpdatePhase::Swapping is reused as the sync phase so the
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
    let sync_job = build_mod_sync_job(server_id, ts, &state.mc_namespace, &keep, &urls);
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

    // Start + verify.
    guard.emit(UpdatePhase::Starting);
    scale_to(&state.kube, &state.mc_namespace, server_id, 1).await?;

    guard.emit(UpdatePhase::Verifying);
    wait_pod_running(
        &state.kube,
        &state.mc_namespace,
        server_id,
        POD_RUNNING_TIMEOUT,
    )
    .await?;
    wait_for_done_marker(
        &state.kube,
        &state.mc_namespace,
        server_id,
        VERIFY_BOOT_TIMEOUT,
    )
    .await?;

    // Persist: replace mods, clear pending.
    let mut new_cfg = cfg;
    new_cfg.mods = desired;
    new_cfg.pending = Vec::new();
    let new_raw = serde_json::to_string(&new_cfg)?;
    sqlx::query("UPDATE servers SET source_config = ? WHERE id = ?")
        .bind(&new_raw)
        .bind(server_id)
        .execute(&state.pool)
        .await?;
    sqlx::query("UPDATE servers SET last_started_at = ? WHERE id = ?")
        .bind(Utc::now().timestamp())
        .bind(server_id)
        .execute(&state.pool)
        .await?;

    let now = Utc::now().timestamp();
    insert_audit(
        &state.pool,
        server_id,
        "mods_apply_succeeded",
        Some(json!({"mods": new_cfg.mods.len()})),
        now,
    )
    .await?;
    Ok(())
}
