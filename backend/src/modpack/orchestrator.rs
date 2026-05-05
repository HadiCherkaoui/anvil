//! Update FSM for modded servers.
//!
//! `run` owns one in-flight update from announce → stop → backup → swap →
//! start → verify, then either commits (`Succeeded`) or rolls back via the
//! restore Job (`RolledBack` / `Failed`). All phase transitions are emitted
//! through the [`UpdateGuard`]'s watch sender so the WS at
//! `/api/servers/:id/update/stream` can stream them to the frontend.
//!
//! M5 swap step: CF + Modrinth both run on `itzg/minecraft-server`, which
//! redownloads its pack when `CF_FILE_ID` / `MODRINTH_VERSION` changes.
//! The Swapping phase is therefore a `StatefulSet` env patch — no separate
//! Job, no manual unzip, no `WIPE_LIST` / `PRESERVE_LIST` script. Backup
//! still runs as a tar Job because rollback needs a snapshot.

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use chrono::Utc;
use futures_util::AsyncBufReadExt as _;
use futures_util::TryStreamExt as _;
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{Pod, Secret};
use kube::Api;
use kube::api::{LogParams, Patch, PatchParams, PostParams};
use serde::Serialize;
use serde_json::json;
use tokio::net::TcpStream;
use tokio::time::{Instant, sleep, timeout};
use tracing::{Level, event};

use crate::AppState;
use crate::k8s_status::RCON_PORT;
use crate::modpack::guard::UpdateGuard;
use crate::modpack::jobs::{build_backup_job, build_restore_job};
use crate::modpack::{ModpackHttp, ModpackProvider, ProviderContext, VersionInfo, from_db};
use crate::routes::servers::create::insert_audit;

/// Phase of the running update.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdatePhase {
    Queued,
    Announcing,
    Stopping,
    BackingUp,
    Swapping,
    Starting,
    Verifying,
    Succeeded,
    RollingBack,
    RolledBack,
    Failed,
}

/// Pod terminate timeout (M2 uses 90 s for the same wait).
const POD_TERMINATE_TIMEOUT: Duration = Duration::from_secs(90);
/// Backup Job ceiling — ATM-11 ~5–10 GB on ZFS finishes in well under a
/// minute; 10 absorbs the cold-pod / pull-image cases.
const BACKUP_JOB_TIMEOUT: Duration = Duration::from_mins(10);
/// Restore Job ceiling — symmetric with backup.
const RESTORE_JOB_TIMEOUT: Duration = Duration::from_mins(10);
/// Pod-status poll interval, mirrors restart.rs / delete.rs.
const POD_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Job-status poll interval — Job objects update infrequently.
const JOB_POLL_INTERVAL: Duration = Duration::from_secs(3);
/// Pod-Running wait used before tailing logs for boot verification.
const POD_RUNNING_TIMEOUT: Duration = Duration::from_mins(2);
/// RCON announce + save-all timeout. Best-effort; orchestrator carries on
/// if RCON is unavailable.
const ANNOUNCE_TIMEOUT: Duration = Duration::from_secs(10);

/// Kicks off the update FSM for `server_id`, targeting `target_version_id`.
///
/// Long-running task: spawned by the route handler, runs until completion,
/// drops the [`UpdateGuard`] which releases the per-server lock + WS bus.
pub async fn run(
    state: AppState,
    server_id: String,
    target_version_id: String,
    guard: UpdateGuard,
) {
    let outcome = run_inner(&state, &server_id, &target_version_id, &guard).await;
    match outcome {
        Ok(()) => {
            guard.emit(UpdatePhase::Succeeded);
            event!(
                name: "anvil.update.succeeded",
                Level::INFO,
                server.id = %server_id,
                "update succeeded",
            );
        }
        Err(err) => {
            event!(
                name: "anvil.update.failed",
                Level::ERROR,
                server.id = %server_id,
                err = %err,
                "update failed; attempting rollback",
            );
            guard.emit(UpdatePhase::RollingBack);
            match rollback(&state, &server_id, &guard).await {
                Ok(()) => {
                    guard.emit(UpdatePhase::RolledBack);
                    let now = Utc::now().timestamp();
                    let _ = insert_audit(
                        &state.pool,
                        &server_id,
                        "update_failed_rolled_back",
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
                        "update_failed",
                        Some(json!({"err": err.to_string(), "rollback_err": rb.to_string()})),
                        now,
                    )
                    .await;
                    event!(
                        name: "anvil.update.rollback_failed",
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

/// Happy-path orchestration; rollback handled by the caller.
#[allow(
    clippy::too_many_lines,
    reason = "the FSM reads top-to-bottom with no obvious split that doesn't lose context"
)]
async fn run_inner(
    state: &AppState,
    server_id: &str,
    target_version_id: &str,
    guard: &UpdateGuard,
) -> Result<()> {
    let now_start = Utc::now().timestamp();
    insert_audit(
        &state.pool,
        server_id,
        "update_started",
        Some(json!({"target_version_id": target_version_id})),
        now_start,
    )
    .await?;

    // Resolve provider + target version.
    let (source_kind, source_config) = fetch_source(&state.pool, server_id).await?;
    let mut provider = from_db(&source_kind, &source_config)
        .with_context(|| format!("provider for {server_id}"))?;
    if matches!(provider.kind(), "vanilla" | "modded" | "paper") {
        bail!(
            "source_kind {} cannot be updated via the modpack orchestrator",
            provider.kind()
        );
    }

    let http = ModpackHttp {
        cf: state.cf_client.as_deref(),
        mr: state.mr_client.as_ref(),
    };
    let snapshots_pvc = state.snapshots_pvc.as_ref();

    let version = pick_target_version(&*provider, &http, target_version_id).await?;

    // ─── Phase 1: announce ───────────────────────────────────────────────
    guard.emit(UpdatePhase::Announcing);
    let _ = announce_and_save(state, server_id).await; // best-effort

    // Acquire the global Job lock so backup/swap/restore Jobs don't race
    // concurrent updates of other servers on the shared snapshots PVC.
    let job_permit = state.snapshot_pvc_lock.lock().await;

    // ─── Phase 2: stop ───────────────────────────────────────────────────
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

    // ─── Phase 3: backup ─────────────────────────────────────────────────
    guard.emit(UpdatePhase::BackingUp);
    let backup_ts = Utc::now().timestamp();
    let backup_job = build_backup_job(
        server_id,
        backup_ts,
        &state.mc_namespace,
        snapshots_pvc.as_str(),
    );
    let backup_name = backup_job
        .metadata
        .name
        .clone()
        .ok_or_else(|| anyhow!("backup Job missing name"))?;
    spawn_job(&state.kube, &state.mc_namespace, &backup_job).await?;
    wait_job(
        &state.kube,
        &state.mc_namespace,
        &backup_name,
        BACKUP_JOB_TIMEOUT,
    )
    .await?;
    insert_audit(
        &state.pool,
        server_id,
        "update_backup_done",
        Some(json!({"ts": backup_ts})),
        Utc::now().timestamp(),
    )
    .await?;

    // ─── Phase 4: swap ───────────────────────────────────────────────────
    // Patch the StatefulSet's container env so itzg picks up the new
    // CF_FILE_ID / MODRINTH_VERSION on next boot and reinstalls. No Job
    // needed — itzg handles the download + install on the MC pod itself.
    guard.emit(UpdatePhase::Swapping);
    let memory_mi = fetch_memory_mi(&state.pool, server_id).await?;
    let new_provider = build_provider_for_version(&*provider, &version)?;
    let new_env = new_provider.extra_env(&ProviderContext {
        server_id,
        memory_mi,
    });
    patch_statefulset_env(&state.kube, &state.mc_namespace, server_id, &new_env).await?;
    insert_audit(
        &state.pool,
        server_id,
        "update_swap_done",
        Some(json!({"version_id": version.id, "version_name": version.name})),
        Utc::now().timestamp(),
    )
    .await?;

    // Snapshot lock can be released — start + verify don't touch the PVC.
    drop(job_permit);

    // ─── Phase 5: start ──────────────────────────────────────────────────
    guard.emit(UpdatePhase::Starting);
    scale_to(&state.kube, &state.mc_namespace, server_id, 1).await?;

    // ─── Phase 6: verify boot ────────────────────────────────────────────
    guard.emit(UpdatePhase::Verifying);
    wait_pod_running(
        &state.kube,
        &state.mc_namespace,
        &mc_pod,
        POD_RUNNING_TIMEOUT,
    )
    .await?;
    wait_for_done_marker(
        &state.kube,
        &state.mc_namespace,
        server_id,
        provider.boot_timeout(),
    )
    .await?;

    // ─── Phase 7: persist + audit ────────────────────────────────────────
    let now_end = Utc::now().timestamp();
    persist_new_version(&state.pool, server_id, &mut provider, &version).await?;
    sqlx::query("UPDATE servers SET last_started_at = ? WHERE id = ?")
        .bind(now_end)
        .bind(server_id)
        .execute(&state.pool)
        .await?;
    insert_audit(
        &state.pool,
        server_id,
        "update_succeeded",
        Some(json!({"version_id": version.id, "version_name": version.name})),
        now_end,
    )
    .await?;
    // Drop the modpack_versions row — the next poll will repopulate it if a
    // newer upstream exists; until then, no banner.
    sqlx::query("DELETE FROM modpack_versions WHERE server_id = ?")
        .bind(server_id)
        .execute(&state.pool)
        .await?;
    Ok(())
}

/// Fetches `source_kind` + `source_config` for a server.
async fn fetch_source(pool: &sqlx::SqlitePool, server_id: &str) -> Result<(String, String)> {
    let row: (String, String) =
        sqlx::query_as("SELECT source_kind, source_config FROM servers WHERE id = ?")
            .bind(server_id)
            .fetch_one(pool)
            .await
            .with_context(|| format!("loading source for server {server_id}"))?;
    Ok(row)
}

/// Selects the [`VersionInfo`] for `target_version_id` from the provider's
/// upstream version list.
async fn pick_target_version(
    provider: &dyn ModpackProvider,
    http: &ModpackHttp<'_>,
    target_version_id: &str,
) -> Result<VersionInfo> {
    // Use the provider's `latest` to populate cache; if the target is the
    // latest, we can return immediately.
    if let Some(latest) = provider.latest(http).await?
        && latest.id == target_version_id
    {
        return Ok(latest);
    }
    let project_id = provider
        .project_id()
        .ok_or_else(|| anyhow!("provider {} has no project id", provider.kind()))?;
    match provider.kind() {
        "curseforge" => {
            let cf = http
                .cf
                .ok_or_else(|| anyhow!("CurseForge client unavailable"))?;
            let project_id_u32: u32 = project_id
                .parse()
                .with_context(|| format!("CF project_id {project_id:?} not numeric"))?;
            let target_id_u32: u32 = target_version_id.parse().with_context(|| {
                format!("CF target version id {target_version_id:?} not numeric")
            })?;
            // First try the project's regular file listing (legacy direct
            // server packs land here). When the pack uses the modern linked
            // shape — every "main" file is a client with `serverPackFileId`
            // pointing at a sibling — the sibling won't appear in the
            // listing, so fall back to a per-id GET.
            let files = cf.list_files(project_id_u32).await?;
            if let Some(f) = files.iter().find(|f| f.id == target_id_u32) {
                return Ok(VersionInfo {
                    id: f.id.to_string(),
                    name: f.display_name.clone(),
                    download_url: f.download_url.clone().unwrap_or_default(),
                });
            }
            let f = cf.file(project_id_u32, target_id_u32).await?;
            Ok(VersionInfo {
                id: f.id.to_string(),
                name: f.display_name,
                download_url: f.download_url.unwrap_or_default(),
            })
        }
        "modrinth" => {
            let v = http.mr.version(target_version_id).await?;
            let primary = v.files.iter().find(|f| f.primary).ok_or_else(|| {
                anyhow!("Modrinth version {target_version_id} has no primary file")
            })?;
            Ok(VersionInfo {
                id: v.id.clone(),
                name: v.name.clone(),
                download_url: primary.url.clone(),
            })
        }
        other => bail!("unsupported provider for target lookup: {other}"),
    }
}

/// Reads `memory_mi` from the `servers` row — needed to rebuild the
/// provider's env block at swap time.
async fn fetch_memory_mi(pool: &sqlx::SqlitePool, server_id: &str) -> Result<i64> {
    sqlx::query_scalar("SELECT memory_mi FROM servers WHERE id = ?")
        .bind(server_id)
        .fetch_one(pool)
        .await
        .with_context(|| format!("loading memory_mi for {server_id}"))
}

/// Builds a fresh provider snapshot whose `current_version_id` matches the
/// orchestrator's target. The created provider is only used to render the
/// new env block; its persisted form lands in the DB later via
/// [`persist_new_version`].
fn build_provider_for_version(
    current: &dyn ModpackProvider,
    version: &VersionInfo,
) -> Result<Box<dyn ModpackProvider>> {
    use crate::modpack::CurseForgeServerPack;
    use crate::modpack::curseforge::Config as CfCfg;
    use crate::modpack::modrinth::{Config as MrCfg, ModrinthServerPack};

    match current.kind() {
        "curseforge" => {
            let project_id_str = current
                .project_id()
                .ok_or_else(|| anyhow!("CF provider missing project id"))?;
            let project_id: u32 = project_id_str
                .parse()
                .with_context(|| format!("CF project_id {project_id_str:?} not numeric"))?;
            let file_id: u32 = version
                .id
                .parse()
                .with_context(|| format!("CF target id {:?} not numeric", version.id))?;
            // Channel + auto-update mode don't matter for the env build —
            // the provider only reads `current_version_id` in extra_env.
            // Fill the rest with the orchestrator-safe defaults and let
            // `persist_new_version` keep the user's actual values intact.
            let cfg = CfCfg {
                project_id,
                channel: super::curseforge::Channel::Release,
                version_skip: Vec::new(),
                force_version: None,
                current_version_id: file_id,
                current_version_name: version.name.clone(),
                auto_update_mode: super::curseforge::AutoUpdateMode::Notify,
            };
            Ok(Box::new(CurseForgeServerPack::new(cfg)))
        }
        "modrinth" => {
            let project_id = current
                .project_id()
                .ok_or_else(|| anyhow!("Modrinth provider missing project id"))?;
            let cfg = MrCfg {
                project_id,
                channel: super::curseforge::Channel::Release,
                version_skip: Vec::new(),
                force_version: None,
                current_version_id: version.id.clone(),
                current_version_name: version.name.clone(),
                auto_update_mode: super::curseforge::AutoUpdateMode::Notify,
            };
            Ok(Box::new(ModrinthServerPack::new(cfg)))
        }
        other => bail!("provider {other} cannot be swapped via env patch"),
    }
}

/// Patches the `StatefulSet`'s container env so the next pod start picks up
/// the new `CF_FILE_ID` / `MODRINTH_VERSION` and itzg redownloads.
///
/// Server-side strategic-merge-patches the single `mc` container's env to
/// the supplied list. K8s replaces the env array wholesale on this path,
/// so callers must pass the complete env they want, not a delta.
async fn patch_statefulset_env(
    client: &kube::Client,
    ns: &str,
    server_id: &str,
    env: &[k8s_openapi::api::core::v1::EnvVar],
) -> Result<()> {
    let stsets: Api<StatefulSet> = Api::namespaced(client.clone(), ns);
    let resource_name = format!("mc-{server_id}");
    let patch = json!({
        "spec": {
            "template": {
                "spec": {
                    "containers": [
                        {
                            "name": "mc",
                            "env": env,
                        }
                    ]
                }
            }
        }
    });
    stsets
        .patch(
            &resource_name,
            &PatchParams::default(),
            &Patch::Strategic(&patch),
        )
        .await
        .with_context(|| format!("patching env on StatefulSet {resource_name}"))?;
    Ok(())
}

/// Best-effort RCON announce + save-all so the world flushes before stop.
pub(crate) async fn announce_and_save(state: &AppState, server_id: &str) -> Result<()> {
    let resource_name = format!("mc-{server_id}");
    let pod_name = format!("{resource_name}-0");
    let secret_name = format!("{resource_name}-rcon");
    let headless_dns = format!(
        "{pod_name}.{resource_name}-headless.{ns}.svc:{port}",
        ns = state.mc_namespace,
        port = RCON_PORT,
    );
    let secrets: Api<Secret> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let secret = secrets
        .get(&secret_name)
        .await
        .with_context(|| format!("loading rcon secret {secret_name}"))?;
    let pwd_bytes = secret
        .data
        .as_ref()
        .and_then(|d| d.get("password"))
        .map(|bs| bs.0.clone())
        .ok_or_else(|| anyhow!("rcon secret missing password key"))?;
    let pwd = String::from_utf8(pwd_bytes).context("rcon password not utf8")?;

    timeout(ANNOUNCE_TIMEOUT, async {
        let mut conn = <rcon::Connection<TcpStream>>::connect(&headless_dns, &pwd).await?;
        conn.cmd("say [Anvil] Update starting in 5 seconds...")
            .await?;
        // Give players a beat to see it.
        tokio::time::sleep(Duration::from_secs(5)).await;
        conn.cmd("save-all flush").await?;
        Ok::<_, rcon::Error>(())
    })
    .await
    .map_err(|_| anyhow!("rcon announce timed out"))?
    .map_err(|e| anyhow!("rcon error: {e}"))?;
    Ok(())
}

/// `kubectl scale --replicas=N statefulset/mc-{id}`.
pub(crate) async fn scale_to(
    client: &kube::Client,
    ns: &str,
    server_id: &str,
    replicas: i32,
) -> Result<()> {
    let stsets: Api<StatefulSet> = Api::namespaced(client.clone(), ns);
    stsets
        .patch_scale(
            &format!("mc-{server_id}"),
            &PatchParams::default(),
            &Patch::Merge(&json!({"spec": {"replicas": replicas}})),
        )
        .await
        .with_context(|| format!("scaling mc-{server_id} to {replicas}"))?;
    Ok(())
}

/// Polls `pod_name` until it disappears or the deadline elapses.
pub(crate) async fn wait_pod_gone(
    client: &kube::Client,
    ns: &str,
    pod_name: &str,
    timeout_dur: Duration,
) -> Result<()> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), ns);
    let deadline = Instant::now() + timeout_dur;
    loop {
        if pods.get_opt(pod_name).await?.is_none() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("pod {pod_name} did not terminate within {timeout_dur:?}");
        }
        sleep(POD_POLL_INTERVAL).await;
    }
}

/// Polls `pod_name` until its phase reaches `Running`.
pub(crate) async fn wait_pod_running(
    client: &kube::Client,
    ns: &str,
    pod_name: &str,
    timeout_dur: Duration,
) -> Result<()> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), ns);
    let deadline = Instant::now() + timeout_dur;
    loop {
        if let Some(p) = pods.get_opt(pod_name).await? {
            let phase = p
                .status
                .as_ref()
                .and_then(|s| s.phase.clone())
                .unwrap_or_default();
            if phase == "Running" {
                return Ok(());
            }
            if phase == "Failed" || phase == "Unknown" {
                bail!("pod {pod_name} entered phase {phase}");
            }
        }
        if Instant::now() >= deadline {
            bail!("pod {pod_name} did not reach Running within {timeout_dur:?}");
        }
        sleep(POD_POLL_INTERVAL).await;
    }
}

/// Creates a Job (best-effort delete-and-recreate if a stale one with the same name lingers).
pub(crate) async fn spawn_job(client: &kube::Client, ns: &str, job: &Job) -> Result<()> {
    let jobs: Api<Job> = Api::namespaced(client.clone(), ns);
    let name = job
        .metadata
        .name
        .as_deref()
        .ok_or_else(|| anyhow!("Job missing name"))?;
    if jobs.get_opt(name).await?.is_some() {
        // Should not normally happen — names are timestamped — but be tidy.
        let _ = jobs.delete(name, &kube::api::DeleteParams::default()).await;
        sleep(Duration::from_secs(1)).await;
    }
    jobs.create(&PostParams::default(), job)
        .await
        .with_context(|| format!("creating Job {name}"))?;
    Ok(())
}

/// Polls a Job's status until succeeded > 0 (Ok), failed > 0 (Err), or deadline.
pub(crate) async fn wait_job(
    client: &kube::Client,
    ns: &str,
    name: &str,
    timeout_dur: Duration,
) -> Result<()> {
    let jobs: Api<Job> = Api::namespaced(client.clone(), ns);
    let deadline = Instant::now() + timeout_dur;
    loop {
        if let Some(j) = jobs.get_opt(name).await?
            && let Some(s) = j.status.as_ref()
        {
            let succeeded = s.succeeded.unwrap_or(0);
            let failed = s.failed.unwrap_or(0);
            if succeeded > 0 {
                return Ok(());
            }
            if failed > 0 {
                bail!("Job {name} failed (status.failed = {failed})");
            }
        }
        if Instant::now() >= deadline {
            bail!("Job {name} did not complete within {timeout_dur:?}");
        }
        sleep(JOB_POLL_INTERVAL).await;
    }
}

/// Tails the pod logs until the canonical `Done (` boot marker appears or
/// the deadline elapses.
pub(crate) async fn wait_for_done_marker(
    client: &kube::Client,
    ns: &str,
    server_id: &str,
    timeout_dur: Duration,
) -> Result<()> {
    let pods: Api<Pod> = Api::namespaced(client.clone(), ns);
    let pod_name = format!("mc-{server_id}-0");
    let lp = LogParams {
        follow: true,
        container: Some("mc".to_owned()),
        tail_lines: None,
        ..LogParams::default()
    };
    let stream = pods
        .log_stream(&pod_name, &lp)
        .await
        .with_context(|| format!("opening log stream for {pod_name}"))?;
    let mut lines = stream.lines();
    let scan = async {
        while let Some(l) = lines.try_next().await.context("reading pod log line")? {
            if l.contains("Done (") {
                return Ok::<(), anyhow::Error>(());
            }
        }
        bail!("log stream ended without Done marker");
    };
    timeout(timeout_dur, scan)
        .await
        .map_err(|_| anyhow!("boot verification timed out after {timeout_dur:?}"))?
}

/// Updates `source_config.current_version_*` so a subsequent restart uses
/// the new version.
async fn persist_new_version(
    pool: &sqlx::SqlitePool,
    server_id: &str,
    provider: &mut Box<dyn ModpackProvider>,
    version: &VersionInfo,
) -> Result<()> {
    // Re-read source_config so we don't trample fields the orchestrator
    // doesn't know about (version_skip, force_version, auto_update_mode).
    let raw: String = sqlx::query_scalar("SELECT source_config FROM servers WHERE id = ?")
        .bind(server_id)
        .fetch_one(pool)
        .await?;
    let mut cfg: serde_json::Value =
        serde_json::from_str(&raw).context("source_config is not JSON")?;
    if let Some(obj) = cfg.as_object_mut() {
        obj.insert("current_version_id".into(), json!(version.id));
        obj.insert("current_version_name".into(), json!(version.name));
        // Force_version is a one-shot — clear it once we've used it.
        obj.insert("force_version".into(), json!(null));
    }
    let new_raw = serde_json::to_string(&cfg)?;
    sqlx::query("UPDATE servers SET source_config = ?, mc_version = ? WHERE id = ?")
        .bind(&new_raw)
        .bind(&version.name)
        .bind(server_id)
        .execute(pool)
        .await?;
    let _ = provider; // updated via the SQL above; provider is a value snapshot.
    Ok(())
}

/// Rolls back to the most recent successful backup (named in `mc-{id}/`).
///
/// The orchestrator owns the snapshot lock again here for the restore Job.
async fn rollback(state: &AppState, server_id: &str, _guard: &UpdateGuard) -> Result<()> {
    let snapshots_pvc = state.snapshots_pvc.as_ref();
    let permit = state.snapshot_pvc_lock.lock().await;

    // Stop again (the failed run may have left replicas=1 if it failed
    // during `Starting`/`Verifying`).
    scale_to(&state.kube, &state.mc_namespace, server_id, 0).await?;
    let mc_pod = format!("mc-{server_id}-0");
    wait_pod_gone(
        &state.kube,
        &state.mc_namespace,
        &mc_pod,
        POD_TERMINATE_TIMEOUT,
    )
    .await?;

    // Revert the env patch applied during Swapping. `persist_new_version`
    // only runs on the success path, so the DB still describes the pre-
    // update version — rebuilding the env from the persisted source_config
    // gives us the right "old" state to push back into the StatefulSet.
    let (source_kind, source_config) = fetch_source(&state.pool, server_id).await?;
    let old_provider = from_db(&source_kind, &source_config)
        .with_context(|| format!("rebuilding pre-update provider for {server_id}"))?;
    let memory_mi = fetch_memory_mi(&state.pool, server_id).await?;
    let old_env = old_provider.extra_env(&ProviderContext {
        server_id,
        memory_mi,
    });
    patch_statefulset_env(&state.kube, &state.mc_namespace, server_id, &old_env).await?;

    // Find the newest archive on disk via a tiny ls Job. Avoid spawning a
    // separate ls Job: the restore Job's container script picks the newest
    // archive itself — see the `latest archive` block in `build_restore_job`.
    // Here we re-derive the path the orchestrator just wrote (matches the
    // ts of the failed update's backup).
    //
    // For now, restore to the latest archive's filename pattern via a
    // shell-glob-aware Job. Backup ts is unknown to the rollback path
    // because the failure may have occurred mid-step; instead we shell out
    // to `ls -t | head -n 1` inside the Job. The current builder takes a
    // ts argument — pass 0 and override via a preceding `latest=$(...)` step
    // baked into a custom command.
    let ts = Utc::now().timestamp();
    let jobs: Api<Job> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let mut restore = build_restore_job(server_id, ts, &state.mc_namespace, snapshots_pvc.as_str());
    // Override the command to find + restore the newest archive.
    if let Some(spec) = restore.spec.as_mut()
        && let Some(pod) = spec.template.spec.as_mut()
        && let Some(c) = pod.containers.first_mut()
    {
        let resource_name = format!("mc-{server_id}");
        let cmd = format!(
            "set -eu; latest=$(ls -t /snap/{resource_name}/ | head -n 1); \
                     if [ -z \"$latest\" ]; then echo no backup to restore; exit 1; fi; \
                     echo restoring $latest; find /data -mindepth 1 -delete; \
                     tar xzf /snap/{resource_name}/$latest -C /data"
        );
        c.command = Some(vec!["sh".to_owned(), "-c".to_owned(), cmd]);
    }
    let name = restore
        .metadata
        .name
        .clone()
        .ok_or_else(|| anyhow!("restore Job missing name"))?;
    if jobs.get_opt(&name).await?.is_some() {
        let _ = jobs
            .delete(&name, &kube::api::DeleteParams::default())
            .await;
        sleep(Duration::from_secs(1)).await;
    }
    jobs.create(&PostParams::default(), &restore).await?;
    wait_job(&state.kube, &state.mc_namespace, &name, RESTORE_JOB_TIMEOUT).await?;

    drop(permit);
    // Boot back on the prior version.
    scale_to(&state.kube, &state.mc_namespace, server_id, 1).await?;
    wait_pod_running(
        &state.kube,
        &state.mc_namespace,
        &mc_pod,
        POD_RUNNING_TIMEOUT,
    )
    .await?;
    // Best-effort boot verification — use 15min default; we don't know the
    // provider here without re-reading. Vanilla updates don't reach this code.
    let _ = timeout(
        Duration::from_mins(15),
        wait_for_done_marker(
            &state.kube,
            &state.mc_namespace,
            server_id,
            Duration::from_mins(15),
        ),
    )
    .await;
    Ok(())
}
