# Anvil — Backups tab Implementation Plan (Spec 5)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Manual backup/restore/delete tab. Backups capture data PVC tar + full SQLite config snapshot for full-state restore.

**Architecture:** New `backups` table; new `modpack/backups.rs` module with `run_backup` and `run_restore` task fns mirroring the orchestrator pattern; four endpoints; new "backups" tab. Reuses Spec 3's UpdateSheet for progress.

**Tech Stack:** Rust 1.83+ · axum 0.8 · kube-rs · sqlx · Next.js 16 · TypeScript · Zod v4. No new top-level deps.

**Source spec:** `docs/superpowers/specs/2026-05-06-anvil-backups-tab-design.md` (signed off 2026-05-06).
**Depends on:** Spec 1 (refreshable context), Spec 3 (`UpdateSheet` reuse pattern).

---

## Hard constraints

- No pre-restore safety snapshot (locked).
- No retention / auto-cleanup (locked).
- Per-server tab only (no global backups page).
- Restore reverts data + env + DB config; **NOT** Service / SC / size (locked).
- Synchronous delete (Job runs, wait, return).
- Manual subdir separated from auto subdir in the snapshots PVC.
- Standard build/test gates per task.

---

## File structure

### Backend (`backend/`)

| File | Change |
|---|---|
| `migrations/0008_backups.sql` | NEW — `backups` table per spec §4.2 |
| `src/modpack/jobs.rs` | EDIT — `build_backup_job(..., subdir, gc_keep)` and `build_restore_job(..., subdir)` parameterized; existing callers in `orchestrator.rs` pass `("auto", Some(BACKUP_KEEP_COUNT))` |
| `src/modpack/orchestrator.rs` | EDIT — call sites pass `("auto", Some(BACKUP_KEEP_COUNT))` |
| `src/modpack/backups.rs` | NEW — `run_backup`, `run_restore`, helper SQL fns |
| `src/modpack/mod.rs` | EDIT — `pub mod backups;` |
| `src/modpack/orchestrator.rs` | EDIT — add `Restoring` to `UpdatePhase` enum |
| `src/routes/servers/backups.rs` | NEW — 4 handlers |
| `src/routes/servers/mod.rs` | EDIT — register routes |
| `src/routes/servers/delete.rs` | EDIT — schedule cleanup Job for snapshots PVC subdir |
| `tests/backups_e2e.rs` | NEW — backup → list → restore → delete |

### Frontend (`frontend/app/`)

| File | Change |
|---|---|
| `lib/api.ts` | EDIT — `Backup`, `fetchBackups`, `createBackup`, `restoreBackup`, `deleteBackup` |
| `lib/update-stream.ts` | EDIT — append `"restoring"` to `phaseSchema` enum |
| `components/UpdateSheet.tsx` | EDIT — add `"restoring"` to `ORDER` array |
| `servers/ServerDetailView.tsx` | EDIT — add `"backups"` tab id and label between players and files |
| `servers/tabs/BackupsBody.tsx` | NEW — list, create, restore, delete UI |

---

## Tasks

### Task 1: Migration `0008_backups.sql`

**Files:**
- Create: `backend/migrations/0008_backups.sql`

- [ ] **Step 1: Migration SQL**

```sql
CREATE TABLE IF NOT EXISTS backups (
    id              TEXT PRIMARY KEY,
    server_id       TEXT NOT NULL,
    name            TEXT,
    created_at      INTEGER NOT NULL,
    snapshot_path   TEXT NOT NULL,
    mc_version      TEXT NOT NULL,
    memory_mi       INTEGER NOT NULL,
    storage_size_gi INTEGER NOT NULL,
    storage_class   TEXT,
    exposure_mode   TEXT NOT NULL,
    source_kind     TEXT NOT NULL,
    source_config   TEXT NOT NULL,
    size_bytes      INTEGER,
    FOREIGN KEY (server_id) REFERENCES servers(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_backups_server ON backups(server_id, created_at DESC);
```

- [ ] **Step 2: Run migration locally + sqlx prepare**

```
cd backend
sqlx migrate run --database-url sqlite:dev.db
cargo sqlx prepare --workspace
```

- [ ] **Step 3: Commit**

```bash
git add backend/migrations/0008_backups.sql backend/.sqlx
git commit -m "feat(db): backups table"
```

---

### Task 2: Parameterize backup/restore Job builders

**Files:**
- Modify: `backend/src/modpack/jobs.rs`
- Modify: `backend/src/modpack/orchestrator.rs`

- [ ] **Step 1: Failing test for the new signature**

In `jobs.rs` test module, add:

```rust
#[test]
fn manual_backup_does_not_emit_gc_command() {
    let job = build_backup_job("abc", "bk-uuid", "ns", "snaps-pvc", "manual", None);
    let cmd = extract_command(&job);
    assert!(!cmd.contains("xargs"), "manual backup must not GC: {cmd}");
    assert!(cmd.contains("/snap/mc-abc/manual/bk-uuid.tgz"));
}

#[test]
fn auto_backup_keeps_gc() {
    let job = build_backup_job("abc", "1700000000", "ns", "snaps-pvc", "auto", Some(3));
    let cmd = extract_command(&job);
    assert!(cmd.contains("xargs -r rm -f"));
    assert!(cmd.contains("/snap/mc-abc/auto/1700000000.tgz"));
}

fn extract_command(j: &Job) -> String {
    j.spec.as_ref().unwrap().template.spec.as_ref().unwrap()
        .containers[0].command.as_ref().unwrap()[2].clone()
}
```

- [ ] **Step 2: Modify `build_backup_job` signature**

```rust
pub fn build_backup_job(
    server_id: &str,
    archive_id: &str,
    namespace: &str,
    snapshots_pvc: &str,
    subdir: &str,
    gc_keep: Option<usize>,
) -> Job {
    let resource_name = format!("mc-{server_id}");
    let pvc_name = format!("data-{resource_name}-0");
    let job_name = format!("backup-{resource_name}-{archive_id}");
    let archive_path = format!("/snap/{resource_name}/{subdir}/{archive_id}.tgz");
    let gc_cmd = match gc_keep {
        Some(keep) => format!(
            " && cd /snap/{resource_name}/{subdir} && ls -t | tail -n +{} | xargs -r rm -f",
            keep + 1
        ),
        None => String::new(),
    };
    let cmd = format!(
        "set -eu; mkdir -p /snap/{resource_name}/{subdir}; \
         tar czf {archive_path} -C /data .; \
         echo backup wrote {archive_path}{gc_cmd}"
    );
    // ... container build unchanged
}
```

`build_restore_job` gets a `subdir: &str` argument added:

```rust
pub fn build_restore_job(
    server_id: &str, archive_id: &str, namespace: &str,
    snapshots_pvc: &str, subdir: &str,
) -> Job {
    let resource_name = format!("mc-{server_id}");
    let archive_path = format!("/snap/{resource_name}/{subdir}/{archive_id}.tgz");
    // ... rest unchanged
}
```

(`archive_id` was `ts: i64` previously — convert call sites that pass `backup_ts` to `&backup_ts.to_string()`.)

- [ ] **Step 3: Update orchestrator call sites**

In `orchestrator.rs:203-208` and any restore call, pass `("auto", Some(BACKUP_KEEP_COUNT))` and `&backup_ts.to_string()`.

- [ ] **Step 4: Run tests + clippy**

```
cargo test --lib modpack::jobs
cargo test --all
cargo clippy --all-targets --features serve-dir -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add backend/src/modpack/jobs.rs backend/src/modpack/orchestrator.rs
git commit -m "refactor(jobs): parameterize subdir + gc_keep on backup/restore Jobs"
```

---

### Task 3: Add `Restoring` to UpdatePhase enum

**Files:**
- Modify: `backend/src/modpack/orchestrator.rs` (UpdatePhase enum)
- Modify: `frontend/app/lib/update-stream.ts`
- Modify: `frontend/app/components/UpdateSheet.tsx`

- [ ] **Step 1: Backend enum**

In `orchestrator.rs`, find `pub enum UpdatePhase`. Add `Restoring`:

```rust
pub enum UpdatePhase {
    Queued,
    Announcing,
    Stopping,
    BackingUp,
    Swapping,
    Starting,
    Verifying,
    Succeeded,
    Restoring,            // NEW
    RollingBack,
    RolledBack,
    Failed,
}
```

Serialise as `"restoring"` (kebab-case via existing serde attr).

- [ ] **Step 2: Frontend zod**

In `update-stream.ts:10-22`:

```ts
const phaseSchema = z.enum([
    "queued", "announcing", "stopping", "backing-up", "swapping",
    "starting", "verifying", "succeeded",
    "restoring",
    "rolling-back", "rolled-back", "failed",
]);
```

- [ ] **Step 3: UpdateSheet ORDER**

In `UpdateSheet.tsx:10`, append `"restoring"` to the `ORDER` array (position appropriate for the visual progress; recommend after `"backing-up"` since restore is the inverse of backup).

- [ ] **Step 4: Commit**

```bash
git add backend/src/modpack/orchestrator.rs frontend/app/lib/update-stream.ts frontend/app/components/UpdateSheet.tsx
git commit -m "feat(update-stream): add restoring phase"
```

---

### Task 4: `backups.rs` — `run_backup`

**Files:**
- Create: `backend/src/modpack/backups.rs`
- Modify: `backend/src/modpack/mod.rs`

- [ ] **Step 1: Module skeleton**

```rust
// backend/src/modpack/backups.rs
//! User-facing backup + restore tasks. Mirrors orchestrator phasing.
use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use crate::audit::insert_audit;
use crate::error::AppError;
use crate::modpack::{UpdateGuard, UpdatePhase};
use crate::modpack::orchestrator::{
    announce_and_save, scale_to, spawn_job, wait_job, wait_pod_gone,
    wait_pod_running, wait_for_done_marker,
    BACKUP_JOB_TIMEOUT, POD_TERMINATE_TIMEOUT, POD_RUNNING_TIMEOUT,
    RESTORE_JOB_TIMEOUT,
};
use crate::modpack::jobs::{build_backup_job, build_restore_job};
use crate::state::AppState;
use crate::k8s_patches::patch_statefulset_env;

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

pub fn new_backup_id() -> String {
    format!("bk-{}", Uuid::new_v4().simple())
}
```

- [ ] **Step 2: Insert pre-Job DB row**

```rust
async fn insert_backup_row(
    state: &AppState,
    backup_id: &str,
    server_id: &str,
    name: Option<&str>,
    snap: &BackupSnapshot,
) -> Result<()> {
    let now = Utc::now().timestamp();
    let snapshot_path = format!("manual/{backup_id}.tgz");
    sqlx::query(r#"
        INSERT INTO backups
        (id, server_id, name, created_at, snapshot_path, mc_version, memory_mi,
         storage_size_gi, storage_class, exposure_mode, source_kind, source_config)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    "#)
    .bind(backup_id).bind(server_id).bind(name).bind(now).bind(&snapshot_path)
    .bind(&snap.mc_version).bind(snap.memory_mi).bind(snap.storage_size_gi)
    .bind(snap.storage_class.as_deref()).bind(&snap.exposure_mode)
    .bind(&snap.source_kind).bind(&snap.source_config)
    .execute(&state.pool).await?;
    Ok(())
}

async fn delete_backup_row(state: &AppState, backup_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM backups WHERE id = ?")
        .bind(backup_id).execute(&state.pool).await?;
    Ok(())
}

async fn snapshot_current(state: &AppState, server_id: &str) -> Result<BackupSnapshot> {
    let row: (String, i64, i64, Option<String>, String, String, String) = sqlx::query_as(r#"
        SELECT mc_version, memory_mi, storage_size_gi, storage_class,
               exposure_mode, source_kind, source_config
        FROM servers WHERE id = ?
    "#).bind(server_id).fetch_one(&state.pool).await?;
    Ok(BackupSnapshot {
        mc_version: row.0, memory_mi: row.1, storage_size_gi: row.2,
        storage_class: row.3, exposure_mode: row.4,
        source_kind: row.5, source_config: row.6,
    })
}
```

- [ ] **Step 3: `run_backup`**

```rust
pub async fn run_backup(
    state: AppState,
    server_id: String,
    backup_id: String,
    name: Option<String>,
    guard: UpdateGuard,
) {
    let outcome = run_backup_inner(&state, &server_id, &backup_id, name.as_deref(), &guard).await;
    match outcome {
        Ok(()) => {
            guard.emit(UpdatePhase::Succeeded);
            let _ = insert_audit(&state.pool, &server_id, "backup_succeeded",
                Some(json!({"backup_id": backup_id})), Utc::now().timestamp()).await;
        }
        Err(err) => {
            guard.emit(UpdatePhase::Failed);
            let _ = delete_backup_row(&state, &backup_id).await;
            let _ = insert_audit(&state.pool, &server_id, "backup_failed",
                Some(json!({"backup_id": backup_id, "err": err.to_string()})),
                Utc::now().timestamp()).await;
            // Recover replicas to 1 best-effort.
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

    insert_audit(&state.pool, server_id, "backup_started",
        Some(json!({"backup_id": backup_id})), Utc::now().timestamp()).await?;

    let _permit = state.snapshot_pvc_lock.lock().await;

    guard.emit(UpdatePhase::Announcing);
    let _ = announce_and_save(state, server_id).await;

    guard.emit(UpdatePhase::Stopping);
    scale_to(&state.kube, &state.mc_namespace, server_id, 0).await?;
    let pod = format!("mc-{server_id}-0");
    wait_pod_gone(&state.kube, &state.mc_namespace, &pod, POD_TERMINATE_TIMEOUT).await?;

    guard.emit(UpdatePhase::BackingUp);
    let job = build_backup_job(server_id, backup_id, &state.mc_namespace,
        snapshots_pvc.as_str(), "manual", None);
    let job_name = job.metadata.name.clone()
        .ok_or_else(|| anyhow!("backup Job missing name"))?;
    spawn_job(&state.kube, &state.mc_namespace, &job).await?;
    wait_job(&state.kube, &state.mc_namespace, &job_name, BACKUP_JOB_TIMEOUT).await?;

    guard.emit(UpdatePhase::Starting);
    scale_to(&state.kube, &state.mc_namespace, server_id, 1).await?;

    guard.emit(UpdatePhase::Verifying);
    wait_pod_running(&state.kube, &state.mc_namespace, &pod, POD_RUNNING_TIMEOUT).await?;
    let timeout = boot_timeout_for_kind(&snap.source_kind);
    wait_for_done_marker(&state.kube, &state.mc_namespace, server_id, timeout).await?;
    Ok(())
}

fn boot_timeout_for_kind(source_kind: &str) -> std::time::Duration {
    match source_kind {
        "modded" => std::time::Duration::from_secs(300),
        _        => std::time::Duration::from_secs(120),
    }
}
```

- [ ] **Step 4: Build + clippy**

```
cargo build
cargo clippy --all-targets --features serve-dir -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add backend/src/modpack/backups.rs backend/src/modpack/mod.rs
git commit -m "feat(backups): run_backup task"
```

---

### Task 5: `backups.rs` — `run_restore`

**Files:**
- Modify: `backend/src/modpack/backups.rs`

- [ ] **Step 1: Implement**

```rust
pub async fn run_restore(
    state: AppState,
    server_id: String,
    backup_id: String,
    guard: UpdateGuard,
) {
    let outcome = run_restore_inner(&state, &server_id, &backup_id, &guard).await;
    match outcome {
        Ok(()) => {
            guard.emit(UpdatePhase::Succeeded);
            let _ = insert_audit(&state.pool, &server_id, "restore_succeeded",
                Some(json!({"backup_id": backup_id})), Utc::now().timestamp()).await;
        }
        Err(err) => {
            guard.emit(UpdatePhase::Failed);
            let _ = insert_audit(&state.pool, &server_id, "restore_failed",
                Some(json!({"backup_id": backup_id, "err": err.to_string()})),
                Utc::now().timestamp()).await;
            // Best-effort scale to 1 so the server isn't left stopped indefinitely.
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

    // Load the backup row.
    let row: (String, String, i64, Option<String>, String, String, String) = sqlx::query_as(r#"
        SELECT mc_version, memory_mi, storage_size_gi, storage_class,
               exposure_mode, source_kind, source_config
        FROM backups WHERE id = ? AND server_id = ?
    "#).bind(backup_id).bind(server_id).fetch_optional(&state.pool).await?
       .ok_or_else(|| anyhow!("backup not found"))?;
    let snap = BackupSnapshot {
        mc_version: row.0, memory_mi: row.1, storage_size_gi: row.2,
        storage_class: row.3, exposure_mode: row.4,
        source_kind: row.5, source_config: row.6,
    };

    insert_audit(&state.pool, server_id, "restore_started",
        Some(json!({"backup_id": backup_id})), Utc::now().timestamp()).await?;

    let _permit = state.snapshot_pvc_lock.lock().await;

    guard.emit(UpdatePhase::Announcing);
    let _ = announce_and_save(state, server_id).await;

    guard.emit(UpdatePhase::Stopping);
    scale_to(&state.kube, &state.mc_namespace, server_id, 0).await?;
    let pod = format!("mc-{server_id}-0");
    wait_pod_gone(&state.kube, &state.mc_namespace, &pod, POD_TERMINATE_TIMEOUT).await?;

    guard.emit(UpdatePhase::Restoring);
    let job = build_restore_job(server_id, backup_id, &state.mc_namespace,
        snapshots_pvc.as_str(), "manual");
    let job_name = job.metadata.name.clone().ok_or_else(|| anyhow!("restore Job missing name"))?;
    spawn_job(&state.kube, &state.mc_namespace, &job).await?;
    wait_job(&state.kube, &state.mc_namespace, &job_name, RESTORE_JOB_TIMEOUT).await?;

    // Swap: revert SQLite + env (Service / SC / size NOT touched per spec §4.5).
    guard.emit(UpdatePhase::Swapping);
    sqlx::query(r#"
        UPDATE servers
        SET mc_version = ?, memory_mi = ?, source_kind = ?, source_config = ?
        WHERE id = ?
    "#)
    .bind(&snap.mc_version).bind(snap.memory_mi)
    .bind(&snap.source_kind).bind(&snap.source_config).bind(server_id)
    .execute(&state.pool).await?;

    let env = build_runtime_env_from_snapshot(&snap, server_id)?;
    patch_statefulset_env(&state.kube, &state.mc_namespace, server_id, &env).await?;

    guard.emit(UpdatePhase::Starting);
    scale_to(&state.kube, &state.mc_namespace, server_id, 1).await?;

    guard.emit(UpdatePhase::Verifying);
    wait_pod_running(&state.kube, &state.mc_namespace, &pod, POD_RUNNING_TIMEOUT).await?;
    wait_for_done_marker(&state.kube, &state.mc_namespace, server_id,
        boot_timeout_for_kind(&snap.source_kind)).await?;
    Ok(())
}

fn build_runtime_env_from_snapshot(
    snap: &BackupSnapshot,
    server_id: &str,
) -> Result<Vec<k8s_openapi::api::core::v1::EnvVar>> {
    use crate::modpack::ProviderContext;
    let memory_mi = snap.memory_mi as u32;
    let provider = match snap.source_kind.as_str() {
        "vanilla" => crate::modpack::vanilla::VanillaRuntime::from_db(&snap.source_config, &snap.mc_version)?,
        "paper"   => crate::modpack::paper::PaperRuntime::from_db(&snap.source_config, &snap.mc_version)?,
        "modded"  => crate::modpack::modded::ModdedRuntime::from_db(&snap.source_config, &snap.mc_version)?,
        other => anyhow::bail!("unsupported source_kind {other}"),
    };
    Ok(provider.extra_env(&ProviderContext { server_id, memory_mi }))
}
```

(Reuses Spec 3's `from_db` constructors. If Spec 3 hasn't shipped yet, add them in this task — same trivial fns.)

- [ ] **Step 2: Build + clippy**

```
cargo build
cargo clippy --all-targets --features serve-dir -- -D warnings
```

- [ ] **Step 3: Commit**

```bash
git add backend/src/modpack/backups.rs
git commit -m "feat(backups): run_restore task"
```

---

### Task 6: Endpoints

**Files:**
- Create: `backend/src/routes/servers/backups.rs`
- Modify: `backend/src/routes/servers/mod.rs`

- [ ] **Step 1: Handler module**

```rust
// backend/src/routes/servers/backups.rs
use axum::{extract::{Path, State}, Json, http::StatusCode};
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::modpack::{UpdateGuard, backups};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct CreateRequest { pub name: Option<String> }

#[derive(Debug, Serialize)]
pub struct CreateResponse { pub status: &'static str, pub backup_id: String }

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct BackupListItem {
    pub id: String,
    pub name: Option<String>,
    pub created_at: i64,
    pub mc_version: String,
    pub size_bytes: Option<i64>,
}

pub async fn create(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(req): Json<CreateRequest>,
) -> Result<(StatusCode, Json<CreateResponse>), AppError> {
    if state.snapshots_pvc.is_none() {
        return Err(AppError::ServiceUnavailable {
            code: "snapshots_unavailable",
            message: "snapshots PVC not configured".to_owned(),
        });
    }
    if let Some(n) = req.name.as_ref() {
        if n.len() > 64 || n.contains('\n') {
            return Err(AppError::BadRequest {
                code: "invalid_name", message: "name too long or contains newline".to_owned(),
            });
        }
    }
    let _ : (String,) = sqlx::query_as("SELECT id FROM servers WHERE id = ?")
        .bind(&id).fetch_optional(&state.pool).await?
        .ok_or(AppError::NotFound { code: "server_not_found" })?;

    let Some(guard) = UpdateGuard::try_acquire(
        &id, state.update_locks.clone(), state.update_phase_buses.clone()) else {
        return Err(AppError::Conflict { code: "update_in_progress",
            message: "another update or apply is running".to_owned() });
    };

    let backup_id = backups::new_backup_id();
    let task_state = state.clone();
    let task_id = id.clone();
    let task_backup = backup_id.clone();
    let task_name = req.name.clone();
    tokio::spawn(async move {
        backups::run_backup(task_state, task_id, task_backup, task_name, guard).await;
    });

    Ok((StatusCode::ACCEPTED, Json(CreateResponse { status: "started", backup_id })))
}

pub async fn list(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Vec<BackupListItem>>, AppError> {
    let rows: Vec<BackupListItem> = sqlx::query_as(r#"
        SELECT id, name, created_at, mc_version, size_bytes
        FROM backups WHERE server_id = ? ORDER BY created_at DESC
    "#).bind(&id).fetch_all(&state.pool).await?;
    Ok(Json(rows))
}

#[derive(Debug, Serialize)]
pub struct StartedResponse { pub status: &'static str }

pub async fn restore(
    Path((id, backup_id)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<StartedResponse>), AppError> {
    if state.snapshots_pvc.is_none() {
        return Err(AppError::ServiceUnavailable {
            code: "snapshots_unavailable",
            message: "snapshots PVC not configured".to_owned(),
        });
    }
    let _: (String,) = sqlx::query_as(
        "SELECT id FROM backups WHERE id = ? AND server_id = ?")
        .bind(&backup_id).bind(&id).fetch_optional(&state.pool).await?
        .ok_or(AppError::NotFound { code: "backup_not_found" })?;

    let Some(guard) = UpdateGuard::try_acquire(
        &id, state.update_locks.clone(), state.update_phase_buses.clone()) else {
        return Err(AppError::Conflict { code: "update_in_progress",
            message: "another update or apply is running".to_owned() });
    };
    let task_state = state.clone();
    let task_id = id.clone();
    let task_b = backup_id.clone();
    tokio::spawn(async move {
        backups::run_restore(task_state, task_id, task_b, guard).await;
    });

    Ok((StatusCode::ACCEPTED, Json(StartedResponse { status: "started" })))
}

pub async fn delete(
    Path((id, backup_id)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<StatusCode, AppError> {
    let _: (String,) = sqlx::query_as(
        "SELECT id FROM backups WHERE id = ? AND server_id = ?")
        .bind(&backup_id).bind(&id).fetch_optional(&state.pool).await?
        .ok_or(AppError::NotFound { code: "backup_not_found" })?;

    // Spawn rm Job + wait synchronously (small file, <1s).
    let job = backups::build_delete_job(&id, &backup_id,
        &state.mc_namespace, state.snapshots_pvc.as_ref().as_str())?;
    let job_name = job.metadata.name.clone().ok_or(AppError::Internal {
        code: "delete_job_no_name", message: "delete Job missing name".to_owned(),
    })?;
    crate::modpack::orchestrator::spawn_job(&state.kube, &state.mc_namespace, &job).await?;
    crate::modpack::orchestrator::wait_job(&state.kube, &state.mc_namespace, &job_name,
        std::time::Duration::from_secs(60)).await?;
    sqlx::query("DELETE FROM backups WHERE id = ?")
        .bind(&backup_id).execute(&state.pool).await?;
    Ok(StatusCode::NO_CONTENT)
}
```

- [ ] **Step 2: Add `build_delete_job` + `build_dir_cleanup_job` to backups.rs**

```rust
// In backups.rs:
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{Container, VolumeMount, Volume, PersistentVolumeClaimVolumeSource};
use anyhow::{Context, Result};

pub fn build_delete_job(
    server_id: &str, backup_id: &str, namespace: &str, snapshots_pvc: &str,
) -> Result<Job> {
    let cmd = format!("rm -f /snap/mc-{server_id}/manual/{backup_id}.tgz; \
                       echo deleted /snap/mc-{server_id}/manual/{backup_id}.tgz");
    Ok(small_pvc_job(
        &format!("backup-delete-{backup_id}"),
        namespace, snapshots_pvc, &cmd,
    ))
}

pub fn build_dir_cleanup_job(server_id: &str, namespace: &str, snapshots_pvc: &str) -> Job {
    let cmd = format!("rm -rf /snap/mc-{server_id}/manual; \
                       echo cleaned /snap/mc-{server_id}/manual");
    small_pvc_job(
        &format!("backup-cleanup-{server_id}"),
        namespace, snapshots_pvc, &cmd,
    )
}

fn small_pvc_job(name: &str, namespace: &str, snapshots_pvc: &str, cmd: &str) -> Job {
    use k8s_openapi::api::batch::v1::JobSpec;
    use k8s_openapi::api::core::v1::{PodSpec, PodTemplateSpec};
    use kube::core::ObjectMeta;
    use std::collections::BTreeMap;

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
    let mut labels = BTreeMap::new();
    labels.insert("app.kubernetes.io/name".to_owned(), "anvil".to_owned());
    Job {
        metadata: ObjectMeta {
            name: Some(name.to_owned()),
            namespace: Some(namespace.to_owned()),
            labels: Some(labels),
            ..ObjectMeta::default()
        },
        spec: Some(JobSpec {
            backoff_limit: Some(0),
            ttl_seconds_after_finished: Some(60),
            template: PodTemplateSpec {
                spec: Some(PodSpec {
                    restart_policy: Some("Never".to_owned()),
                    containers: vec![container],
                    volumes: Some(vec![Volume {
                        name: "snap".to_owned(),
                        persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
                            claim_name: snapshots_pvc.to_owned(),
                            read_only: Some(false),
                        }),
                        ..Volume::default()
                    }]),
                    ..PodSpec::default()
                }),
                ..PodTemplateSpec::default()
            },
            ..JobSpec::default()
        }),
        ..Job::default()
    }
}
```

(`small_pvc_job` should reuse `jobs::job`/`jobs::snapshots_volume` helpers — make those `pub(crate)` if not already.)

- [ ] **Step 3: Wire routes**

```rust
.route("/api/servers/{id}/backups", post(servers::backups::create).get(servers::backups::list))
.route("/api/servers/{id}/backups/{backup_id}/restore", post(servers::backups::restore))
.route("/api/servers/{id}/backups/{backup_id}", delete(servers::backups::delete))
```

- [ ] **Step 4: Build + clippy**

```
cargo build
cargo clippy --all-targets --features serve-dir -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add backend/src/routes/servers/backups.rs backend/src/routes/servers/mod.rs backend/src/modpack/backups.rs
git commit -m "feat(api): backups create/list/restore/delete endpoints"
```

---

### Task 7: Server delete cascade — manual subdir cleanup Job

**Files:**
- Modify: `backend/src/routes/servers/delete.rs`

- [ ] **Step 1: Schedule cleanup Job**

After existing per-resource cleanups in `delete.rs`, schedule the manual-subdir wipe:

```rust
if let Some(snapshots_pvc) = state.snapshots_pvc.as_ref() {
    let job = crate::modpack::backups::build_dir_cleanup_job(
        &id, &state.mc_namespace, snapshots_pvc.as_str());
    if let Err(e) = crate::modpack::orchestrator::spawn_job(
        &state.kube, &state.mc_namespace, &job).await {
        tracing::warn!(?e, server.id = %id, "backup dir cleanup Job failed to spawn");
    }
}
let _ = crate::audit::insert_audit(&state.pool, &id,
    "backup_dir_cleanup_scheduled", None, chrono::Utc::now().timestamp()).await;
```

- [ ] **Step 2: Build + clippy**

- [ ] **Step 3: Commit**

```bash
git add backend/src/routes/servers/delete.rs
git commit -m "feat(delete): schedule backup-dir cleanup on server delete"
```

---

### Task 8: Backend integration test — backup → list → restore → delete

**Files:**
- Create: `backend/tests/backups_e2e.rs`

- [ ] **Step 1: e2e test**

```rust
mod common;

#[tokio::test]
async fn backup_list_restore_delete_cycle() {
    let (state, _) = common::test_state().await;
    let id = common::seed_vanilla_server(&state, "ts-bk", 4096).await;

    // 1. Create.
    let create = common::post_create_backup(&state, &id, Some("pre-test")).await.unwrap();
    common::wait_for_backup_complete(&state, &id, &create.backup_id, std::time::Duration::from_mins(1)).await;

    // 2. List.
    let list = common::get_backups(&state, &id).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name.as_deref(), Some("pre-test"));

    // 3. Mutate the server (e.g. memory_mi).
    common::patch_settings_memory(&state, &id, 8192).await;

    // 4. Restore.
    common::post_restore_backup(&state, &id, &create.backup_id).await.unwrap();
    common::wait_for_restore_complete(&state, &id, std::time::Duration::from_mins(2)).await;

    let row = common::fetch_server_row(&state, &id).await;
    assert_eq!(row.memory_mi, 4096); // reverted

    // 5. Delete.
    common::delete_backup(&state, &id, &create.backup_id).await.unwrap();
    let list = common::get_backups(&state, &id).await.unwrap();
    assert!(list.is_empty());
}
```

- [ ] **Step 2: Run + green**

```
cargo test --test backups_e2e
```

- [ ] **Step 3: Commit**

```bash
git add backend/tests/backups_e2e.rs
git commit -m "test(backups): e2e create/list/restore/delete"
```

---

### Task 9: Frontend — API client + new tab

**Files:**
- Modify: `frontend/app/lib/api.ts`
- Modify: `frontend/app/servers/ServerDetailView.tsx`
- Create: `frontend/app/servers/tabs/BackupsBody.tsx`

- [ ] **Step 1: API client + Zod**

```ts
const backupSchema = z.object({
  id: z.string(),
  name: z.string().nullable(),
  created_at: z.number(),
  mc_version: z.string(),
  size_bytes: z.number().nullable(),
});
export type Backup = z.infer<typeof backupSchema>;

export async function fetchBackups(id: string, signal?: AbortSignal): Promise<Backup[]> {
  const res = await fetch(`/api/servers/${id}/backups`, { signal });
  if (!res.ok) throw await ApiError.fromResponse(res);
  return z.array(backupSchema).parse(await res.json());
}

export async function createBackup(id: string, name?: string): Promise<{ status: string; backup_id: string }> {
  const res = await fetch(`/api/servers/${id}/backups`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(name !== undefined ? { name } : {}),
  });
  if (!res.ok) throw await ApiError.fromResponse(res);
  return z.object({ status: z.string(), backup_id: z.string() }).parse(await res.json());
}

export async function restoreBackup(id: string, backupId: string): Promise<{ status: string }> {
  const res = await fetch(`/api/servers/${id}/backups/${backupId}/restore`, { method: "POST" });
  if (!res.ok) throw await ApiError.fromResponse(res);
  return z.object({ status: z.string() }).parse(await res.json());
}

export async function deleteBackup(id: string, backupId: string): Promise<void> {
  const res = await fetch(`/api/servers/${id}/backups/${backupId}`, { method: "DELETE" });
  if (!res.ok) throw await ApiError.fromResponse(res);
}
```

- [ ] **Step 2: Add tab**

In `ServerDetailView.tsx`:

```tsx
type TabId = "overview" | "console" | "mods" | "players" | "backups" | "files" | "settings";

const TAB_ORDER: TabId[] = ["overview", "console", "mods", "players", "backups", "files", "settings"];

// in tabs array:
{ id: "backups", label: "backups", href: tabHref("backups") },

// in body switch:
{tab === "backups" && <BackupsBody />}
```

- [ ] **Step 3: BackupsBody component**

```tsx
// frontend/app/servers/tabs/BackupsBody.tsx
"use client";
import { useEffect, useMemo, useState } from "react";
import {
  fetchBackups, createBackup, restoreBackup, deleteBackup,
  type Backup, ApiError,
} from "../../lib/api";
import { useServerDetail } from "../../lib/server-detail-context";
import { Button } from "../../components/Button";
import { Card } from "../../components/Card";
import { Modal } from "../../components/Modal";
import { useToast } from "../../components/Toast";
import { UpdateSheet } from "../../components/UpdateSheet";

function fmtBytes(n: number | null): string {
  if (n === null) return "—";
  const u = ["B","KB","MB","GB","TB"]; let i = 0; let x = n;
  while (x >= 1024 && i < u.length-1) { x /= 1024; i++; }
  return `${x.toFixed(1)} ${u[i]}`;
}
function fmtTs(s: number): string { return new Date(s * 1000).toLocaleString(); }

export function BackupsBody(): JSX.Element {
  const { detail, refresh } = useServerDetail();
  const toast = useToast();
  const [backups, setBackups] = useState<Backup[]>([]);
  const [createOpen, setCreateOpen] = useState(false);
  const [createName, setCreateName] = useState("");
  const [confirm, setConfirm] = useState<{ kind: "restore" | "delete"; b: Backup } | null>(null);
  const [progressOpen, setProgressOpen] = useState(false);

  const loadList = (): void => {
    fetchBackups(detail.id).then(setBackups).catch(() => { /* surface via toast if needed */ });
  };
  useEffect(loadList, [detail.id]);

  const totalBytes = useMemo(
    () => backups.reduce((acc, b) => acc + (b.size_bytes ?? 0), 0),
    [backups]
  );

  const onCreate = (): void => {
    createBackup(detail.id, createName.trim() === "" ? undefined : createName.trim())
      .then(() => { setCreateOpen(false); setCreateName(""); setProgressOpen(true); })
      .catch((err: unknown) => {
        const msg = err instanceof ApiError ? err.message : "unknown";
        toast.push(`backup failed · ${msg}`, "error");
      });
  };
  const onRestore = (b: Backup): void => {
    restoreBackup(detail.id, b.id)
      .then(() => { setConfirm(null); setProgressOpen(true); })
      .catch((err: unknown) => {
        const msg = err instanceof ApiError ? err.message : "unknown";
        toast.push(`restore failed · ${msg}`, "error");
      });
  };
  const onDelete = (b: Backup): void => {
    deleteBackup(detail.id, b.id)
      .then(() => { setConfirm(null); loadList(); toast.push("deleted", "success"); })
      .catch((err: unknown) => {
        const msg = err instanceof ApiError ? err.message : "unknown";
        toast.push(`delete failed · ${msg}`, "error");
      });
  };

  return (
    <>
      <Card header="backups">
        <div className="flex items-center justify-between border-b border-border px-3 py-2">
          <span className="font-mono text-[12px] text-text-faint">
            {backups.length} backup{backups.length === 1 ? "" : "s"} · {fmtBytes(totalBytes)}
          </span>
          <Button onClick={() => setCreateOpen(true)}>+ create backup</Button>
        </div>
        <ul className="divide-y divide-border">
          {backups.map((b) => (
            <li key={b.id} className="grid grid-cols-[1fr_auto_auto_auto_auto] items-center gap-3 px-3 py-2">
              <span className="font-mono text-[12px] text-text-body">{b.name ?? "(unnamed)"}</span>
              <span className="font-mono text-[11px] text-text-faint">{fmtTs(b.created_at)}</span>
              <span className="font-mono text-[11px] text-text-faint">{b.mc_version}</span>
              <span className="font-mono text-[11px] text-text-faint">{fmtBytes(b.size_bytes)}</span>
              <span className="flex gap-2">
                <Button variant="ghost" onClick={() => setConfirm({ kind: "restore", b })}>restore</Button>
                <Button variant="danger" onClick={() => setConfirm({ kind: "delete", b })}>delete</Button>
              </span>
            </li>
          ))}
        </ul>
      </Card>

      {/* Create modal */}
      <Modal open={createOpen} onClose={() => setCreateOpen(false)} title="create backup">
        <input
          value={createName}
          onChange={(e) => setCreateName(e.target.value)}
          maxLength={64}
          placeholder="optional name"
          className="w-full rounded border border-border bg-bg px-2 py-1 font-mono text-[12px]"
        />
        <div className="mt-2 flex gap-2">
          <Button variant="secondary" onClick={() => setCreateOpen(false)}>cancel</Button>
          <Button onClick={onCreate}>create backup</Button>
        </div>
      </Modal>

      {/* Restore / delete confirmations */}
      {confirm && (
        <Modal open onClose={() => setConfirm(null)} title={`${confirm.kind} ${confirm.b.name ?? "(unnamed)"}?`}>
          <p className="text-[12px] text-text-body">
            {confirm.kind === "restore"
              ? "this will stop the server, replace data and config with the snapshot, and restart. on failure, server may end in a mixed state."
              : "delete this backup permanently?"}
          </p>
          <div className="mt-2 flex gap-2">
            <Button variant="secondary" onClick={() => setConfirm(null)}>cancel</Button>
            <Button variant={confirm.kind === "delete" ? "danger" : "primary"}
              onClick={() => confirm.kind === "restore" ? onRestore(confirm.b) : onDelete(confirm.b)}>
              {confirm.kind}
            </Button>
          </div>
        </Modal>
      )}

      <UpdateSheet
        open={progressOpen}
        onClose={() => { setProgressOpen(false); refresh(); loadList(); }}
        serverId={detail.id}
      />
    </>
  );
}
```

- [ ] **Step 4: Manual repro**

Build, run, navigate to a server's backups tab. Create, list, restore (verify restart), delete.

- [ ] **Step 5: Commit**

```bash
git add frontend/app/lib/api.ts frontend/app/servers/ServerDetailView.tsx frontend/app/servers/tabs/BackupsBody.tsx
git commit -m "feat(ui): backups tab with create/restore/delete"
```

---

## Verification

- [ ] `cd backend && cargo fmt --all && cargo clippy --all-targets --features serve-dir -- -D warnings && cargo clippy --all-targets --features embed -- -D warnings && cargo test --all`
- [ ] `cd frontend && pnpm typecheck && pnpm lint && pnpm build`
- [ ] Manual e2e: backup → mutate server → restore → confirm reverts → delete → server delete → confirm tarballs gone.

---

## Implementation prompt

```
Implement the plan at docs/superpowers/plans/2026-05-06-anvil-backups-tab-impl.md.

Use superpowers:executing-plans (or subagent-driven-development). Tasks 1 → 9 in order.
The spec at docs/superpowers/specs/2026-05-06-anvil-backups-tab-design.md is the design
authority.

Depends on:
- Spec 1 plan landed (refreshable ServerDetailContext).
- Spec 3 plan ideally landed (UpdateSheet reuse pattern). If not, this plan stands alone
  but you'll be the first caller to use UpdateSheet from Settings/Backups tabs, so verify
  the component's prop shape matches the call site.

Run the verification commands. Commit per task in conventional commits style.
Read frontend/AGENTS.md before frontend code.
```
