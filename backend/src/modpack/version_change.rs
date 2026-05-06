//! Orchestrated MC version change for non-modpack servers.
//!
//! Mirrors `orchestrator::run` shape: announce → stop → backup → swap → start
//! → verify, with auto-rollback on failure. Caller spawns this as a task and
//! it owns the [`UpdateGuard`] until completion. Only `vanilla`, `paper`, and
//! `modded` source kinds are accepted — modpack servers update via the
//! modpack orchestrator.

use std::time::Duration;

use anyhow::{anyhow, bail, Context as _, Result};
use chrono::Utc;
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::EnvVar;
use kube::Api;
use serde_json::json;
use tracing::{event, Level};

use crate::k8s_patches::patch_statefulset_env;
use crate::modpack::guard::UpdateGuard;
use crate::modpack::jobs::{build_backup_job, build_restore_job, BACKUP_KEEP_COUNT};
use crate::modpack::orchestrator::{
    announce_and_save, scale_to, spawn_job, wait_for_done_marker, wait_job, wait_pod_gone,
    wait_pod_running, UpdatePhase, BACKUP_JOB_TIMEOUT, POD_RUNNING_TIMEOUT, POD_TERMINATE_TIMEOUT,
    RESTORE_JOB_TIMEOUT,
};
use crate::routes::servers::create::insert_audit;
use crate::AppState;

/// Pre-swap state captured by [`run_inner`] so [`rollback`] can revert the
/// `StatefulSet` env, the `SQLite` row, and the data PVC if a phase 4–6
/// failure occurs.
#[derive(Debug)]
struct RollbackContext {
    old_env: Vec<EnvVar>,
    old_mc: String,
    old_source_config: String,
    backup_ts: i64,
}

/// Differentiates failure modes so [`run`] knows whether a rollback is
/// warranted. `Pre` failures happen before the swap step ran; the `SQLite`
/// row and `StatefulSet` env are still pristine, so no rollback is needed
/// (the server is just stopped, the user can re-start manually). `Post`
/// failures happen after the swap step started; the rollback Job + env
/// revert + DB revert restore the prior state.
#[derive(Debug)]
enum FsmError {
    Pre(anyhow::Error),
    Post(RollbackContext, anyhow::Error),
}

/// Kicks off the version-change FSM for `server_id`.
///
/// Long-running task: spawned by the route handler, runs until completion,
/// drops the [`UpdateGuard`] which releases the per-server lock + WS bus.
pub async fn run(
    state: AppState,
    server_id: String,
    new_mc: String,
    new_loader: Option<String>,
    guard: UpdateGuard,
) {
    let outcome = run_inner(&state, &server_id, &new_mc, new_loader.as_deref(), &guard).await;
    match outcome {
        Ok(()) => {
            guard.emit(UpdatePhase::Succeeded);
            event!(
                name: "anvil.version_change.succeeded",
                Level::INFO,
                server.id = %server_id,
                "version change succeeded",
            );
        }
        Err(FsmError::Pre(err)) => {
            event!(
                name: "anvil.version_change.failed",
                Level::ERROR,
                server.id = %server_id,
                err = %err,
                "version change failed before swap; no rollback required",
            );
            guard.emit(UpdatePhase::Failed);
            let now = Utc::now().timestamp();
            let _ = insert_audit(
                &state.pool,
                &server_id,
                "version_change_failed",
                Some(json!({"err": err.to_string()})),
                now,
            )
            .await;
        }
        Err(FsmError::Post(ctx, err)) => {
            event!(
                name: "anvil.version_change.failed",
                Level::ERROR,
                server.id = %server_id,
                err = %err,
                "version change failed after swap; attempting rollback",
            );
            guard.emit(UpdatePhase::RollingBack);
            match rollback(&state, &server_id, &ctx).await {
                Ok(()) => {
                    guard.emit(UpdatePhase::RolledBack);
                    let now = Utc::now().timestamp();
                    let _ = insert_audit(
                        &state.pool,
                        &server_id,
                        "version_change_failed_rolled_back",
                        Some(json!({"err": err.to_string()})),
                        now,
                    )
                    .await;
                }
                Err(rb) => {
                    guard.emit(UpdatePhase::Failed);
                    let now = Utc::now().timestamp();
                    let _ = insert_audit(
                        &state.pool,
                        &server_id,
                        "version_change_failed",
                        Some(json!({"err": err.to_string(), "rollback_err": rb.to_string()})),
                        now,
                    )
                    .await;
                    event!(
                        name: "anvil.version_change.rollback_failed",
                        Level::ERROR,
                        server.id = %server_id,
                        err = %rb,
                        "rollback failed; manual intervention required",
                    );
                }
            }
        }
    }
}

/// Drives every phase. Phase 1–3 failures wrap in [`FsmError::Pre`]; phase 4
/// onward wraps in [`FsmError::Post`] with the captured rollback context.
#[expect(
    clippy::too_many_lines,
    reason = "FSM reads top-to-bottom; splitting it up loses sequence context"
)]
async fn run_inner(
    state: &AppState,
    server_id: &str,
    new_mc: &str,
    new_loader: Option<&str>,
    guard: &UpdateGuard,
) -> Result<(), FsmError> {
    let now_start = Utc::now().timestamp();
    insert_audit(
        &state.pool,
        server_id,
        "version_change_started",
        Some(json!({"new_mc": new_mc, "new_loader": new_loader})),
        now_start,
    )
    .await
    .map_err(|e| FsmError::Pre(e.into()))?;

    let snapshots_pvc = state.snapshots_pvc.as_ref();

    let SourceRow {
        source_kind,
        source_config,
        memory_mi,
        old_mc,
    } = fetch_source(&state.pool, server_id)
        .await
        .map_err(FsmError::Pre)?;

    if matches!(source_kind.as_str(), "curseforge" | "modrinth") {
        return Err(FsmError::Pre(anyhow!(
            "source_kind {source_kind} cannot use version_change orchestrator",
        )));
    }

    // ─── Phase 1: announce ───────────────────────────────────────────────
    guard.emit(UpdatePhase::Announcing);
    let _ = announce_and_save(state, server_id).await; // best-effort

    let job_permit = state.snapshot_pvc_lock.lock().await;

    // ─── Phase 2: stop ───────────────────────────────────────────────────
    guard.emit(UpdatePhase::Stopping);
    scale_to(&state.kube, &state.mc_namespace, server_id, 0)
        .await
        .map_err(FsmError::Pre)?;
    let mc_pod = format!("mc-{server_id}-0");
    wait_pod_gone(
        &state.kube,
        &state.mc_namespace,
        &mc_pod,
        POD_TERMINATE_TIMEOUT,
    )
    .await
    .map_err(FsmError::Pre)?;

    // ─── Phase 3: backup ─────────────────────────────────────────────────
    guard.emit(UpdatePhase::BackingUp);
    let backup_ts = Utc::now().timestamp();
    let backup_job = build_backup_job(
        server_id,
        &backup_ts.to_string(),
        &state.mc_namespace,
        snapshots_pvc.as_str(),
        "auto",
        Some(BACKUP_KEEP_COUNT),
    );
    let backup_name = backup_job
        .metadata
        .name
        .clone()
        .ok_or_else(|| FsmError::Pre(anyhow!("backup Job missing name")))?;
    spawn_job(&state.kube, &state.mc_namespace, &backup_job)
        .await
        .map_err(FsmError::Pre)?;
    wait_job(
        &state.kube,
        &state.mc_namespace,
        &backup_name,
        BACKUP_JOB_TIMEOUT,
    )
    .await
    .map_err(FsmError::Pre)?;
    insert_audit(
        &state.pool,
        server_id,
        "version_change_backup_done",
        Some(json!({"ts": backup_ts})),
        Utc::now().timestamp(),
    )
    .await
    .map_err(|e| FsmError::Pre(e.into()))?;

    // Snapshot the live `mc` container env BEFORE swap so rollback has a
    // deterministic target to revert to.
    let old_env = fetch_current_env(state, server_id)
        .await
        .map_err(FsmError::Pre)?;
    let ctx = RollbackContext {
        old_env,
        old_mc,
        old_source_config: source_config.clone(),
        backup_ts,
    };

    // ─── Phase 4: swap ───────────────────────────────────────────────────
    guard.emit(UpdatePhase::Swapping);
    let boot_timeout = match apply_swap(
        state,
        server_id,
        &source_kind,
        &source_config,
        new_mc,
        new_loader,
        memory_mi,
    )
    .await
    {
        Ok(t) => t,
        Err(e) => return Err(FsmError::Post(ctx, e)),
    };
    drop(job_permit);

    // ─── Phase 5: start ──────────────────────────────────────────────────
    guard.emit(UpdatePhase::Starting);
    if let Err(e) = scale_to(&state.kube, &state.mc_namespace, server_id, 1).await {
        return Err(FsmError::Post(ctx, e));
    }

    // ─── Phase 6: verify boot ────────────────────────────────────────────
    guard.emit(UpdatePhase::Verifying);
    let verify = async {
        wait_pod_running(
            &state.kube,
            &state.mc_namespace,
            &mc_pod,
            POD_RUNNING_TIMEOUT,
        )
        .await?;
        wait_for_done_marker(&state.kube, &state.mc_namespace, server_id, boot_timeout).await
    }
    .await;
    if let Err(e) = verify {
        return Err(FsmError::Post(ctx, e));
    }

    // ─── Phase 7: persist + audit ────────────────────────────────────────
    let now_end = Utc::now().timestamp();
    sqlx::query("UPDATE servers SET last_started_at = ? WHERE id = ?")
        .bind(now_end)
        .bind(server_id)
        .execute(&state.pool)
        .await
        .map_err(|e| FsmError::Pre(e.into()))?;
    insert_audit(
        &state.pool,
        server_id,
        "version_change_succeeded",
        Some(json!({"new_mc": new_mc, "new_loader": new_loader})),
        now_end,
    )
    .await
    .map_err(|e| FsmError::Pre(e.into()))?;
    Ok(())
}

/// Subset of the `servers` row needed by the FSM.
#[derive(Debug)]
struct SourceRow {
    source_kind: String,
    source_config: String,
    memory_mi: i64,
    old_mc: String,
}

async fn fetch_source(pool: &sqlx::SqlitePool, server_id: &str) -> Result<SourceRow> {
    let row: (String, String, i64, String) = sqlx::query_as(
        "SELECT source_kind, source_config, memory_mi, mc_version FROM servers WHERE id = ?",
    )
    .bind(server_id)
    .fetch_one(pool)
    .await
    .with_context(|| format!("loading source row for server {server_id}"))?;
    Ok(SourceRow {
        source_kind: row.0,
        source_config: row.1,
        memory_mi: row.2,
        old_mc: row.3,
    })
}

/// Reads the live `mc` container env from the per-server `StatefulSet`.
async fn fetch_current_env(state: &AppState, server_id: &str) -> Result<Vec<EnvVar>> {
    let api: Api<StatefulSet> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let resource_name = format!("mc-{server_id}");
    let ss = api
        .get(&resource_name)
        .await
        .with_context(|| format!("fetching StatefulSet {resource_name}"))?;
    let containers = ss
        .spec
        .as_ref()
        .and_then(|s| s.template.spec.as_ref())
        .map(|s| s.containers.as_slice())
        .unwrap_or_default();
    let mc = containers
        .iter()
        .find(|c| c.name == "mc")
        .ok_or_else(|| anyhow!("mc container not found in {resource_name}"))?;
    Ok(mc.env.clone().unwrap_or_default())
}

/// Updates the persisted `source_config` (and `mc_version` column) for the
/// new target, rebuilds the runtime env, and patches the live `StatefulSet`.
///
/// Returns the runtime's `boot_timeout` so the verify phase can reuse the
/// same wait the per-runtime provider would normally pick.
async fn apply_swap(
    state: &AppState,
    server_id: &str,
    source_kind: &str,
    source_config: &str,
    new_mc: &str,
    new_loader: Option<&str>,
    memory_mi: i64,
) -> Result<Duration> {
    use crate::modpack::modded::{Config as ModdedCfg, ModdedRuntime};
    use crate::modpack::paper::{Config as PaperCfg, PaperServerProvider};
    use crate::modpack::vanilla::VanillaProvider;
    use crate::modpack::ModpackProvider as _;
    use crate::modpack::ProviderContext;

    let ctx = ProviderContext {
        server_id,
        memory_mi,
    };

    let (new_env, new_source_config, boot_timeout) = match source_kind {
        "vanilla" => {
            let env = VanillaProvider::build_env(server_id, new_mc, memory_mi);
            (
                env,
                source_config.to_owned(),
                VanillaProvider::new().boot_timeout(),
            )
        }
        "paper" => {
            let mut cfg: PaperCfg =
                serde_json::from_str(source_config).context("paper source_config not JSON")?;
            new_mc.clone_into(&mut cfg.mc_version);
            let p = PaperServerProvider::new(cfg);
            let env = p.extra_env(&ctx);
            let cfg_json = serde_json::to_string(p.config())?;
            let timeout = p.boot_timeout();
            (env, cfg_json, timeout)
        }
        "modded" => {
            let mut cfg: ModdedCfg =
                serde_json::from_str(source_config).context("modded source_config not JSON")?;
            new_mc.clone_into(&mut cfg.mc_version);
            cfg.loader_version = new_loader.map(str::to_owned);
            let r = ModdedRuntime::new(cfg);
            let env = r.extra_env(&ctx);
            let cfg_json = serde_json::to_string(r.config())?;
            let timeout = r.boot_timeout();
            (env, cfg_json, timeout)
        }
        other => bail!("unsupported source_kind {other:?} for version change"),
    };

    sqlx::query("UPDATE servers SET mc_version = ?, source_config = ? WHERE id = ?")
        .bind(new_mc)
        .bind(&new_source_config)
        .bind(server_id)
        .execute(&state.pool)
        .await?;

    patch_statefulset_env(&state.kube, &state.mc_namespace, server_id, &new_env).await?;
    Ok(boot_timeout)
}

/// Reverts the `StatefulSet` env, `SQLite` row, and data PVC to the pre-swap
/// state captured in `ctx`.
async fn rollback(state: &AppState, server_id: &str, ctx: &RollbackContext) -> Result<()> {
    let snapshots_pvc = state.snapshots_pvc.as_ref();
    let _permit = state.snapshot_pvc_lock.lock().await;

    // Phase 5/6 failures may leave the pod up; phase 4 leaves it down.
    // Either way force replicas=0 so the restore Job can mount the data PVC.
    scale_to(&state.kube, &state.mc_namespace, server_id, 0).await?;
    let mc_pod = format!("mc-{server_id}-0");
    wait_pod_gone(
        &state.kube,
        &state.mc_namespace,
        &mc_pod,
        POD_TERMINATE_TIMEOUT,
    )
    .await?;

    // Restore the data PVC from the just-taken snapshot.
    let restore = build_restore_job(
        server_id,
        &ctx.backup_ts.to_string(),
        &state.mc_namespace,
        snapshots_pvc.as_str(),
        "auto",
    );
    let restore_name = restore
        .metadata
        .name
        .clone()
        .ok_or_else(|| anyhow!("restore Job missing name"))?;
    spawn_job(&state.kube, &state.mc_namespace, &restore).await?;
    wait_job(
        &state.kube,
        &state.mc_namespace,
        &restore_name,
        RESTORE_JOB_TIMEOUT,
    )
    .await?;

    // Revert env on the StatefulSet to the snapshot we took before swap.
    patch_statefulset_env(&state.kube, &state.mc_namespace, server_id, &ctx.old_env).await?;

    // Revert SQLite row.
    sqlx::query("UPDATE servers SET mc_version = ?, source_config = ? WHERE id = ?")
        .bind(&ctx.old_mc)
        .bind(&ctx.old_source_config)
        .bind(server_id)
        .execute(&state.pool)
        .await?;

    // Boot back on the prior version. Verify is best-effort — the rollback
    // succeeds even if the boot marker takes too long, since the data is
    // already restored and the user can intervene from the panel.
    scale_to(&state.kube, &state.mc_namespace, server_id, 1).await?;
    let _ = wait_pod_running(
        &state.kube,
        &state.mc_namespace,
        &mc_pod,
        POD_RUNNING_TIMEOUT,
    )
    .await;
    Ok(())
}
