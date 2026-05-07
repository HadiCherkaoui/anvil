//! User-facing manual backup + restore tasks (Spec 5).
//!
//! Backup is the lean path — `(maybe stop) → tar → (maybe start)` — no
//! announce, no done-marker verify; a backup is just a copy of `/data`,
//! not a state change. Restore mirrors the orchestrator's phasing
//! (announce → stop → restore → swap → start → verify) because untarring
//! into `/data` is destructive and the snapshot's runtime config has to
//! be reapplied.
//!
//! Both write archives under the `manual/` subdir of the snapshots PVC,
//! opt out of GC, and snapshot the server's full restore-time config in
//! `SQLite` so a restore can revert `mc_version`, memory, source
//! kind/config, and `StatefulSet` env in one shot.

use std::collections::BTreeMap;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use chrono::Utc;
use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    Container, EnvVar, PersistentVolumeClaimVolumeSource, PodSpec, PodTemplateSpec, Volume,
    VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use serde_json::json;
use uuid::Uuid;

use crate::AppState;
use crate::k8s_patches::patch_statefulset_env;
use crate::modpack::guard::{UpdateGuard, record_terminal, set_update_error};
use crate::modpack::jobs::{build_backup_job, build_restore_job};
use crate::modpack::orchestrator::{
    BACKUP_JOB_TIMEOUT, POD_RUNNING_TIMEOUT, POD_TERMINATE_TIMEOUT, RESTORE_JOB_TIMEOUT,
    UpdatePhase, announce_and_save, current_replicas, scale_to, spawn_job, wait_for_done_marker,
    wait_job, wait_pod_gone, wait_pod_running,
};
use crate::routes::servers::create::insert_audit;

/// Snapshot of the live `servers` row taken at backup time and re-applied
/// on restore.
#[derive(Debug, Clone)]
pub struct BackupSnapshot {
    pub mc_version: String,
    pub memory_mi: i64,
    pub storage_size_gi: i64,
    pub storage_class: Option<String>,
    pub exposure_mode: String,
    pub source_kind: String,
    pub source_config: String,
}

/// Generates a fresh backup id (`bk-<uuid-v4>`).
#[must_use]
pub fn new_backup_id() -> String {
    format!("bk-{}", Uuid::new_v4().simple())
}

/// Persists the pre-Job `backups` row. Written before spawning the Job so
/// a partial failure leaves a clear record we can clean up on the failure
/// path.
async fn insert_backup_row(
    state: &AppState,
    backup_id: &str,
    server_id: &str,
    name: Option<&str>,
    snap: &BackupSnapshot,
) -> Result<()> {
    let now = Utc::now().timestamp();
    let snapshot_path = format!("manual/{backup_id}.tgz");
    sqlx::query(
        "INSERT INTO backups
            (id, server_id, name, created_at, snapshot_path, mc_version, memory_mi,
             storage_size_gi, storage_class, exposure_mode, source_kind, source_config)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(backup_id)
    .bind(server_id)
    .bind(name)
    .bind(now)
    .bind(&snapshot_path)
    .bind(&snap.mc_version)
    .bind(snap.memory_mi)
    .bind(snap.storage_size_gi)
    .bind(snap.storage_class.as_deref())
    .bind(&snap.exposure_mode)
    .bind(&snap.source_kind)
    .bind(&snap.source_config)
    .execute(&state.pool)
    .await
    .context("inserting backup row")?;
    Ok(())
}

async fn delete_backup_row(state: &AppState, backup_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM backups WHERE id = ?")
        .bind(backup_id)
        .execute(&state.pool)
        .await
        .context("deleting backup row")?;
    Ok(())
}

/// SELECT shape shared by `snapshot_current` (`servers` row) and
/// `load_backup_snapshot` (`backups` row) — the columns line up.
type SnapRow = (String, i64, i64, Option<String>, String, String, String);

fn snap_from_row(row: SnapRow) -> BackupSnapshot {
    BackupSnapshot {
        mc_version: row.0,
        memory_mi: row.1,
        storage_size_gi: row.2,
        storage_class: row.3,
        exposure_mode: row.4,
        source_kind: row.5,
        source_config: row.6,
    }
}

/// Inserts an `auto`-kind backup row for an FSM-driven backup.
///
/// `ts` is the unix timestamp the orchestrator used as the archive id;
/// the DB row id is `bk-auto-{ts}` so rollback / list / restore can all
/// derive paths deterministically. `reason` is a free-form context
/// string surfaced in the Backups tab.
///
/// # Errors
///
/// Returns the underlying `sqlx` error if the `servers` row is missing
/// or the insert violates a constraint.
pub async fn insert_auto_backup_row(
    state: &AppState,
    server_id: &str,
    ts: i64,
    reason: &str,
) -> Result<String> {
    insert_auto_backup_row_with_status(state, server_id, ts, reason, "complete").await
}

/// Like [`insert_auto_backup_row`] but sets the row's `status` explicitly.
/// FSMs call this with `"pending"` before the tar Job runs, then flip to
/// `"complete"` / `"failed"` via [`mark_auto_backup_status`] once the Job
/// terminates so the UI never advertises a tarball that doesn't exist.
///
/// # Errors
///
/// Returns the underlying `sqlx` error if the `servers` row is missing or
/// the insert violates a constraint.
pub async fn insert_auto_backup_row_with_status(
    state: &AppState,
    server_id: &str,
    ts: i64,
    reason: &str,
    status: &str,
) -> Result<String> {
    let backup_id = format!("bk-auto-{ts}");
    let snap = snapshot_current(state, server_id).await?;
    let snapshot_path = format!("auto/{ts}.tgz");
    sqlx::query(
        "INSERT INTO backups
            (id, server_id, name, created_at, snapshot_path, mc_version, memory_mi,
             storage_size_gi, storage_class, exposure_mode, source_kind, source_config,
             kind, reason, status)
         VALUES (?, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'auto', ?, ?)",
    )
    .bind(&backup_id)
    .bind(server_id)
    .bind(ts)
    .bind(&snapshot_path)
    .bind(&snap.mc_version)
    .bind(snap.memory_mi)
    .bind(snap.storage_size_gi)
    .bind(snap.storage_class.as_deref())
    .bind(&snap.exposure_mode)
    .bind(&snap.source_kind)
    .bind(&snap.source_config)
    .bind(reason)
    .bind(status)
    .execute(&state.pool)
    .await
    .context("inserting auto backup row")?;
    Ok(backup_id)
}

/// Updates the `status` of an auto-backup row. Callers use this to flip
/// `pending` rows to `complete` after the tar Job lands, or `failed` if it
/// errored.
///
/// # Errors
///
/// Returns the underlying `sqlx` error.
pub async fn mark_auto_backup_status(
    state: &AppState,
    backup_id: &str,
    status: &str,
) -> Result<()> {
    sqlx::query("UPDATE backups SET status = ? WHERE id = ?")
        .bind(status)
        .bind(backup_id)
        .execute(&state.pool)
        .await
        .context("updating backup status")?;
    Ok(())
}

/// Marks any auto-backup rows whose `status = 'pending'` and `created_at`
/// is older than `older_than_secs` seconds ago as `'failed'`. Run from the
/// auto-backup GC pass so a crashed orchestrator does not leave rows
/// claiming "in progress" indefinitely.
///
/// # Errors
///
/// Returns the underlying `sqlx` error.
pub async fn fail_stale_pending_auto_backups(state: &AppState, older_than_secs: i64) -> Result<()> {
    let cutoff = Utc::now().timestamp() - older_than_secs;
    sqlx::query(
        "UPDATE backups SET status = 'failed'
         WHERE kind = 'auto' AND status = 'pending' AND created_at < ?",
    )
    .bind(cutoff)
    .execute(&state.pool)
    .await
    .context("failing stale pending auto backups")?;
    Ok(())
}

/// Trims `auto`-kind backup rows for `server_id` to the newest `keep`,
/// matching the inline `xargs -r rm -f` GC the backup Job's tar shell
/// runs on the snapshots PVC. Run after the backup Job reports success
/// so DB and disk stay in agreement.
///
/// # Errors
///
/// Returns the underlying `sqlx` error.
pub async fn gc_auto_backup_rows(state: &AppState, server_id: &str, keep: usize) -> Result<()> {
    sqlx::query(
        "DELETE FROM backups WHERE id IN (
            SELECT id FROM backups
            WHERE server_id = ? AND kind = 'auto'
            ORDER BY created_at DESC
            LIMIT -1 OFFSET ?
        )",
    )
    .bind(server_id)
    .bind(i64::try_from(keep).unwrap_or(i64::MAX))
    .execute(&state.pool)
    .await
    .context("gc'ing auto backup rows")?;
    Ok(())
}

async fn snapshot_current(state: &AppState, server_id: &str) -> Result<BackupSnapshot> {
    let row: SnapRow = sqlx::query_as(
        "SELECT mc_version, memory_mi, storage_size_gi, storage_class,
                exposure_mode, source_kind, source_config
         FROM servers WHERE id = ?",
    )
    .bind(server_id)
    .fetch_one(&state.pool)
    .await
    .with_context(|| format!("loading server row for {server_id}"))?;
    Ok(snap_from_row(row))
}

async fn load_backup_snapshot(
    state: &AppState,
    server_id: &str,
    backup_id: &str,
) -> Result<BackupSnapshot> {
    let row: Option<SnapRow> = sqlx::query_as(
        "SELECT mc_version, memory_mi, storage_size_gi, storage_class,
                exposure_mode, source_kind, source_config
         FROM backups WHERE id = ? AND server_id = ?",
    )
    .bind(backup_id)
    .bind(server_id)
    .fetch_optional(&state.pool)
    .await
    .context("loading backup row")?;
    Ok(snap_from_row(
        row.ok_or_else(|| anyhow!("backup not found"))?,
    ))
}

/// Picks a boot timeout based on `source_kind` so manual backup verify
/// waits the same amount as the per-runtime provider would.
fn boot_timeout_for_kind(source_kind: &str) -> Duration {
    match source_kind {
        // Modded boots include forge/fabric setup + mod loading; matches
        // ModdedRuntime::boot_timeout() in modded.rs.
        "modded" | "curseforge" | "modrinth" => Duration::from_mins(15),
        // Vanilla / paper / unknown — Done marker comes within ~minute.
        _ => Duration::from_mins(5),
    }
}

/// Drives a single manual backup. Owns the [`UpdateGuard`] for its lifetime
/// and emits phase transitions through it for the WS at
/// `/api/servers/:id/update/stream`.
///
/// A manual backup is just a tar of `/data` while the data PVC is
/// quiesced. If the server is running we stop it first (no announce —
/// this isn't an update) and bring it back to its prior replica count
/// when the tar finishes; if it was already stopped we never touch the
/// `StatefulSet`. There is no done-marker verify: the success criterion
/// is the tar Job, not a fresh boot.
pub async fn run_backup(
    state: AppState,
    server_id: String,
    backup_id: String,
    name: Option<String>,
    guard: UpdateGuard,
) {
    let was_running = match current_replicas(&state.kube, &state.mc_namespace, &server_id).await {
        Ok(r) => r >= 1,
        Err(err) => {
            set_update_error(&state, &server_id, err.to_string());
            record_terminal(&state, &server_id, UpdatePhase::Failed);
            guard.emit(UpdatePhase::Failed);
            let now = Utc::now().timestamp();
            let _ = insert_audit(
                &state.pool,
                &server_id,
                "backup_failed",
                Some(json!({"backup_id": backup_id, "err": err.to_string()})),
                now,
            )
            .await;
            return;
        }
    };
    let outcome = run_backup_inner(
        &state,
        &server_id,
        &backup_id,
        name.as_deref(),
        was_running,
        &guard,
    )
    .await;
    let now = Utc::now().timestamp();
    match outcome {
        Ok(()) => {
            record_terminal(&state, &server_id, UpdatePhase::Succeeded);
            guard.emit(UpdatePhase::Succeeded);
            let _ = insert_audit(
                &state.pool,
                &server_id,
                "backup_succeeded",
                Some(json!({"backup_id": backup_id})),
                now,
            )
            .await;
        }
        Err(err) => {
            set_update_error(&state, &server_id, err.to_string());
            record_terminal(&state, &server_id, UpdatePhase::Failed);
            guard.emit(UpdatePhase::Failed);
            let _ = delete_backup_row(&state, &backup_id).await;
            let _ = insert_audit(
                &state.pool,
                &server_id,
                "backup_failed",
                Some(json!({"backup_id": backup_id, "err": err.to_string()})),
                now,
            )
            .await;
            // Best-effort restore the server's prior replica count. If it
            // was stopped before the backup started, leave it stopped —
            // bringing it up would override the user's intent.
            if was_running {
                let _ = scale_to(&state.kube, &state.mc_namespace, &server_id, 1).await;
            }
        }
    }
}

async fn run_backup_inner(
    state: &AppState,
    server_id: &str,
    backup_id: &str,
    name: Option<&str>,
    was_running: bool,
    guard: &UpdateGuard,
) -> Result<()> {
    let snapshots_pvc = state.snapshots_pvc.as_ref();
    let snap = snapshot_current(state, server_id).await?;
    insert_backup_row(state, backup_id, server_id, name, &snap).await?;

    insert_audit(
        &state.pool,
        server_id,
        "backup_started",
        Some(json!({"backup_id": backup_id})),
        Utc::now().timestamp(),
    )
    .await?;

    let _permit = state.snapshot_pvc_lock.lock().await;

    let pod = format!("mc-{server_id}-0");
    if was_running {
        guard.emit(UpdatePhase::Stopping);
        scale_to(&state.kube, &state.mc_namespace, server_id, 0).await?;
        wait_pod_gone(
            &state.kube,
            &state.mc_namespace,
            &pod,
            POD_TERMINATE_TIMEOUT,
        )
        .await?;
    }

    guard.emit(UpdatePhase::BackingUp);
    let job = build_backup_job(
        server_id,
        backup_id,
        &state.mc_namespace,
        snapshots_pvc.as_str(),
        "manual",
        None,
    );
    let job_name = job
        .metadata
        .name
        .clone()
        .ok_or_else(|| anyhow!("backup Job missing name"))?;
    spawn_job(&state.kube, &state.mc_namespace, &job).await?;
    wait_job(
        &state.kube,
        &state.mc_namespace,
        &job_name,
        BACKUP_JOB_TIMEOUT,
    )
    .await?;

    if was_running {
        guard.emit(UpdatePhase::Starting);
        scale_to(&state.kube, &state.mc_namespace, server_id, 1).await?;
    }
    Ok(())
}

/// Drives a single manual restore: stop → tar back into /data → swap DB row
/// + env to the snapshot's values → start → verify.
///
/// Does NOT revert `storage_size_gi`, `exposure_mode`, or `storage_class`
/// (spec §4.5).
pub async fn run_restore(
    state: AppState,
    server_id: String,
    backup_id: String,
    guard: UpdateGuard,
) {
    let outcome = run_restore_inner(&state, &server_id, &backup_id, &guard).await;
    let now = Utc::now().timestamp();
    match outcome {
        Ok(()) => {
            record_terminal(&state, &server_id, UpdatePhase::Succeeded);
            guard.emit(UpdatePhase::Succeeded);
            let _ = insert_audit(
                &state.pool,
                &server_id,
                "restore_succeeded",
                Some(json!({"backup_id": backup_id})),
                now,
            )
            .await;
        }
        Err(err) => {
            set_update_error(&state, &server_id, err.to_string());
            record_terminal(&state, &server_id, UpdatePhase::Failed);
            guard.emit(UpdatePhase::Failed);
            let _ = insert_audit(
                &state.pool,
                &server_id,
                "restore_failed",
                Some(json!({"backup_id": backup_id, "err": err.to_string()})),
                now,
            )
            .await;
            // Best-effort scale to 1 so the server isn't left stopped.
            let _ = scale_to(&state.kube, &state.mc_namespace, &server_id, 1).await;
        }
    }
}

async fn run_restore_inner(
    state: &AppState,
    server_id: &str,
    backup_id: &str,
    guard: &UpdateGuard,
) -> Result<()> {
    let snapshots_pvc = state.snapshots_pvc.as_ref();

    let snap = load_backup_snapshot(state, server_id, backup_id).await?;

    // Capture the pre-restore row so a partial mid-swap failure can revert
    // SQL + env to match the (now-overwritten) PVC contents. Without this,
    // a failure between the tarball untar and env patch leaves the DB
    // claiming version A while /data is from version B.
    let pre = load_pre_restore_snapshot(state, server_id).await?;

    insert_audit(
        &state.pool,
        server_id,
        "restore_started",
        Some(json!({"backup_id": backup_id})),
        Utc::now().timestamp(),
    )
    .await?;

    let _permit = state.snapshot_pvc_lock.lock().await;

    guard.emit(UpdatePhase::Announcing);
    let _ = announce_and_save(state, server_id).await;

    guard.emit(UpdatePhase::Stopping);
    scale_to(&state.kube, &state.mc_namespace, server_id, 0).await?;
    let pod = format!("mc-{server_id}-0");
    wait_pod_gone(
        &state.kube,
        &state.mc_namespace,
        &pod,
        POD_TERMINATE_TIMEOUT,
    )
    .await?;

    guard.emit(UpdatePhase::Restoring);
    let job = build_restore_job(
        server_id,
        backup_id,
        &state.mc_namespace,
        snapshots_pvc.as_str(),
        "manual",
    );
    let job_name = job
        .metadata
        .name
        .clone()
        .ok_or_else(|| anyhow!("restore Job missing name"))?;
    spawn_job(&state.kube, &state.mc_namespace, &job).await?;
    // Restore Job failure leaves /data possibly inconsistent, but the DB
    // row still matches `pre` (no swap yet) so callers can retry safely.
    wait_job(
        &state.kube,
        &state.mc_namespace,
        &job_name,
        RESTORE_JOB_TIMEOUT,
    )
    .await?;

    // Swap: revert SQLite + env. Service / SC / size are NOT touched per spec §4.5.
    guard.emit(UpdatePhase::Swapping);
    // SQL update failure leaves env still pointing at `pre`, which is now
    // stale relative to /data. Bubble up — the caller surfaces the error
    // and best-effort scales back up.
    sqlx::query(
        "UPDATE servers
         SET mc_version = ?, memory_mi = ?, source_kind = ?, source_config = ?
         WHERE id = ?",
    )
    .bind(&snap.mc_version)
    .bind(snap.memory_mi)
    .bind(&snap.source_kind)
    .bind(&snap.source_config)
    .bind(server_id)
    .execute(&state.pool)
    .await
    .context("reverting servers row to backup snapshot")?;

    let env = build_runtime_env_from_snapshot(&snap, server_id)?;
    if let Err(e) = patch_statefulset_env(&state.kube, &state.mc_namespace, server_id, &env).await {
        // Env patch failed but SQL already swapped to `snap`. Roll back the
        // SQL UPDATE so DB matches the env (which is still `pre`). /data is
        // from the backup though, so the server may still misboot — log
        // loudly and return the original error.
        tracing::error!(
            server.id = %server_id,
            err = %e,
            "restore env patch failed after SQL swap; rolling back SQL to pre-restore"
        );
        let _ = sqlx::query(
            "UPDATE servers
             SET mc_version = ?, memory_mi = ?, source_kind = ?, source_config = ?
             WHERE id = ?",
        )
        .bind(&pre.mc_version)
        .bind(pre.memory_mi)
        .bind(&pre.source_kind)
        .bind(&pre.source_config)
        .bind(server_id)
        .execute(&state.pool)
        .await;
        return Err(e);
    }

    guard.emit(UpdatePhase::Starting);
    scale_to(&state.kube, &state.mc_namespace, server_id, 1).await?;

    guard.emit(UpdatePhase::Verifying);
    wait_pod_running(&state.kube, &state.mc_namespace, &pod, POD_RUNNING_TIMEOUT).await?;
    let timeout = boot_timeout_for_kind(&snap.source_kind);
    wait_for_done_marker(&state.kube, &state.mc_namespace, server_id, timeout).await?;
    Ok(())
}

/// Reads the live `servers` row into a [`BackupSnapshot`]-shaped value so a
/// failed restore can revert the SQL/env to match what /data was before.
async fn load_pre_restore_snapshot(state: &AppState, server_id: &str) -> Result<BackupSnapshot> {
    use sqlx::Row as _;
    let row = sqlx::query(
        "SELECT mc_version, memory_mi, storage_size_gi, storage_class,
                exposure_mode, source_kind, source_config
         FROM servers WHERE id = ?",
    )
    .bind(server_id)
    .fetch_one(&state.pool)
    .await
    .context("loading pre-restore servers row")?;
    Ok(BackupSnapshot {
        mc_version: row.try_get("mc_version")?,
        memory_mi: row.try_get("memory_mi")?,
        storage_size_gi: row.try_get("storage_size_gi")?,
        storage_class: row.try_get("storage_class")?,
        exposure_mode: row.try_get("exposure_mode")?,
        source_kind: row.try_get("source_kind")?,
        source_config: row.try_get("source_config")?,
    })
}

/// Rebuilds the `mc` container env from a [`BackupSnapshot`] so a restore
/// can patch the live `StatefulSet` to match the snapshot's runtime
/// configuration. Mirrors `version_change::apply_swap` for vanilla / paper
/// / modded; reuses [`crate::modpack::from_db`] for upstream modpacks.
fn build_runtime_env_from_snapshot(snap: &BackupSnapshot, server_id: &str) -> Result<Vec<EnvVar>> {
    use crate::modpack::ModpackProvider as _;
    use crate::modpack::ProviderContext;
    use crate::modpack::modded::{Config as ModdedCfg, ModdedRuntime};
    use crate::modpack::paper::{Config as PaperCfg, PaperServerProvider};
    use crate::modpack::vanilla::VanillaProvider;

    let ctx = ProviderContext {
        server_id,
        memory_mi: snap.memory_mi,
    };
    match snap.source_kind.as_str() {
        "vanilla" => Ok(VanillaProvider::build_env(
            server_id,
            &snap.mc_version,
            snap.memory_mi,
        )),
        "paper" => {
            let cfg: PaperCfg = serde_json::from_str(&snap.source_config)
                .context("paper source_config not JSON")?;
            Ok(PaperServerProvider::new(cfg).extra_env(&ctx))
        }
        "modded" => {
            let cfg: ModdedCfg = serde_json::from_str(&snap.source_config)
                .context("modded source_config not JSON")?;
            Ok(ModdedRuntime::new(cfg).extra_env(&ctx))
        }
        "curseforge" | "modrinth" => {
            let p = crate::modpack::from_db(&snap.source_kind, &snap.source_config)?;
            Ok(p.extra_env(&ctx))
        }
        other => Err(anyhow!("unsupported source_kind {other}")),
    }
}

/// Builds a one-shot busybox Job that removes the manual backup tarball
/// for `backup_id`. Mounts only the snapshots PVC; idempotent (`rm -f`).
#[must_use]
pub fn build_delete_job(
    server_id: &str,
    backup_id: &str,
    namespace: &str,
    snapshots_pvc: &str,
) -> Job {
    let cmd = format!(
        "rm -f /snap/mc-{server_id}/manual/{backup_id}.tgz; \
         echo deleted /snap/mc-{server_id}/manual/{backup_id}.tgz"
    );
    small_pvc_job(
        &format!("backup-delete-{backup_id}"),
        namespace,
        snapshots_pvc,
        &cmd,
    )
}

/// Builds a one-shot busybox Job that removes the per-server `manual/`
/// subdir on the snapshots PVC. Used by the server delete cascade.
#[must_use]
pub fn build_dir_cleanup_job(server_id: &str, namespace: &str, snapshots_pvc: &str) -> Job {
    let cmd = format!(
        "rm -rf /snap/mc-{server_id}/manual; \
         echo cleaned /snap/mc-{server_id}/manual"
    );
    small_pvc_job(
        &format!("backup-cleanup-{server_id}"),
        namespace,
        snapshots_pvc,
        &cmd,
    )
}

/// Constructs a busybox Job that mounts the snapshots PVC at `/snap` and
/// runs the given shell command. Backed by the same image / `RestartPolicy`
/// pattern the orchestrator's backup Jobs use.
fn small_pvc_job(name: &str, namespace: &str, snapshots_pvc: &str, cmd: &str) -> Job {
    let mut labels = BTreeMap::new();
    labels.insert("app.kubernetes.io/name".to_owned(), "anvil".to_owned());
    let container = Container {
        name: "rm".to_owned(),
        image: Some("busybox:1.36".to_owned()),
        command: Some(vec!["sh".to_owned(), "-c".to_owned(), cmd.to_owned()]),
        volume_mounts: Some(vec![VolumeMount {
            name: "snap".to_owned(),
            mount_path: "/snap".to_owned(),
            ..VolumeMount::default()
        }]),
        ..Container::default()
    };
    Job {
        metadata: ObjectMeta {
            name: Some(name.to_owned()),
            namespace: Some(namespace.to_owned()),
            labels: Some(labels.clone()),
            ..ObjectMeta::default()
        },
        spec: Some(JobSpec {
            backoff_limit: Some(0),
            ttl_seconds_after_finished: Some(60),
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels),
                    ..ObjectMeta::default()
                }),
                spec: Some(PodSpec {
                    restart_policy: Some("Never".to_owned()),
                    containers: vec![container],
                    volumes: Some(vec![Volume {
                        name: "snap".to_owned(),
                        persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
                            claim_name: snapshots_pvc.to_owned(),
                            ..PersistentVolumeClaimVolumeSource::default()
                        }),
                        ..Volume::default()
                    }]),
                    ..PodSpec::default()
                }),
            },
            ..JobSpec::default()
        }),
        status: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extract_command(j: &Job) -> String {
        j.spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers[0]
            .command
            .as_ref()
            .unwrap()[2]
            .clone()
    }

    #[test]
    fn delete_job_targets_manual_subdir() {
        let j = build_delete_job("abc", "bk-uuid", "mc", "mc-snapshots");
        let cmd = extract_command(&j);
        assert!(cmd.contains("/snap/mc-abc/manual/bk-uuid.tgz"));
        assert_eq!(j.metadata.name.as_deref(), Some("backup-delete-bk-uuid"));
    }

    #[test]
    fn cleanup_job_wipes_manual_dir() {
        let j = build_dir_cleanup_job("abc", "mc", "mc-snapshots");
        let cmd = extract_command(&j);
        assert!(cmd.contains("rm -rf /snap/mc-abc/manual"));
        assert_eq!(j.metadata.name.as_deref(), Some("backup-cleanup-abc"));
    }

    #[test]
    fn small_pvc_job_mounts_snapshots_pvc() {
        let j = build_delete_job("abc", "bk-uuid", "mc", "mc-snapshots");
        let v = j.spec.unwrap().template.spec.unwrap().volumes.unwrap();
        let snap = v.iter().find(|x| x.name == "snap").unwrap();
        let pvc = snap.persistent_volume_claim.as_ref().unwrap();
        assert_eq!(pvc.claim_name, "mc-snapshots");
    }
}
