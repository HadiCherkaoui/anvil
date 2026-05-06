//! User-facing manual backup + restore tasks (Spec 5).
//!
//! Mirrors the orchestrator's phasing — announce → stop → backup/restore →
//! [swap] → start → verify — but writes archives under the `manual/` subdir
//! of the snapshots PVC, opts out of GC, and snapshots the server's full
//! restore-time config in `SQLite` so a restore can revert `mc_version`,
//! memory, source kind/config, and `StatefulSet` env in one shot.

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
use crate::modpack::guard::UpdateGuard;
use crate::modpack::jobs::{build_backup_job, build_restore_job};
use crate::modpack::orchestrator::{
    BACKUP_JOB_TIMEOUT, POD_RUNNING_TIMEOUT, POD_TERMINATE_TIMEOUT, RESTORE_JOB_TIMEOUT,
    UpdatePhase, announce_and_save, scale_to, spawn_job, wait_for_done_marker, wait_job,
    wait_pod_gone, wait_pod_running,
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
pub async fn run_backup(
    state: AppState,
    server_id: String,
    backup_id: String,
    name: Option<String>,
    guard: UpdateGuard,
) {
    let outcome = run_backup_inner(&state, &server_id, &backup_id, name.as_deref(), &guard).await;
    let now = Utc::now().timestamp();
    match outcome {
        Ok(()) => {
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
            // Best-effort scale to 1 so the server isn't left stopped.
            let _ = scale_to(&state.kube, &state.mc_namespace, &server_id, 1).await;
        }
    }
}

async fn run_backup_inner(
    state: &AppState,
    server_id: &str,
    backup_id: &str,
    name: Option<&str>,
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

    guard.emit(UpdatePhase::Starting);
    scale_to(&state.kube, &state.mc_namespace, server_id, 1).await?;

    guard.emit(UpdatePhase::Verifying);
    wait_pod_running(&state.kube, &state.mc_namespace, &pod, POD_RUNNING_TIMEOUT).await?;
    let timeout = boot_timeout_for_kind(&snap.source_kind);
    wait_for_done_marker(&state.kube, &state.mc_namespace, server_id, timeout).await?;
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
    wait_job(
        &state.kube,
        &state.mc_namespace,
        &job_name,
        RESTORE_JOB_TIMEOUT,
    )
    .await?;

    // Swap: revert SQLite + env. Service / SC / size are NOT touched per spec §4.5.
    guard.emit(UpdatePhase::Swapping);
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
    patch_statefulset_env(&state.kube, &state.mc_namespace, server_id, &env).await?;

    guard.emit(UpdatePhase::Starting);
    scale_to(&state.kube, &state.mc_namespace, server_id, 1).await?;

    guard.emit(UpdatePhase::Verifying);
    wait_pod_running(&state.kube, &state.mc_namespace, &pod, POD_RUNNING_TIMEOUT).await?;
    let timeout = boot_timeout_for_kind(&snap.source_kind);
    wait_for_done_marker(&state.kube, &state.mc_namespace, server_id, timeout).await?;
    Ok(())
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
