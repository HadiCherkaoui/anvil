# Anvil — MC version change Implementation Plan (Spec 3)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow MC version change for vanilla / paper / modded servers via an orchestrated FSM (announce → stop → backup → swap → start → verify), with auto-rollback on failure. Modpacks excluded.

**Architecture:** New `backend/src/modpack/version_change.rs` mirrors `orchestrator.rs`'s FSM, reusing crate-public helpers. New endpoint `PATCH /api/servers/:id/version` spawns the task and returns 202. Frontend gets a "version" card in Settings with a sheet for cascading MC + loader pickers (reuses Spec 1's loader endpoint), and progress streams through the existing `UpdateSheet` over `/api/servers/:id/update/stream`.

**Tech Stack:** Rust 1.83+ · axum 0.8 · kube-rs · sqlx · Next.js 16 · TypeScript · Zod v4. No new deps.

**Source spec:** `docs/superpowers/specs/2026-05-06-anvil-mc-version-change-design.md` (signed off 2026-05-06).
**Depends on:**
- Spec 1 plan landed: `k8s_patches::patch_statefulset_env`, `ModdedConfig.loader_version`, loader endpoint, refreshable context, cascading-pickers component.
- Snapshots PVC configured (cluster contract).

---

## Hard constraints

- No new SQLite migration; `mc_version` exists, `loader_version` rides in `source_config` JSON per Spec 1.
- No new RBAC.
- No new top-level deps.
- Reuse existing `UpdatePhase` enum + `UpdateSheet` component — do **not** add a new WS or progress UI.
- Snapshot is mandatory — fail with 503 when `snapshots_pvc` is None.
- Standard build/test gates apply (see Spec 1 plan).

---

## Decisions locked from spec §8

1. Auto-restart-on-save (orchestrated FSM with snapshot + rollback).
2. Modal/sheet for picker.
3. Snapshot reuse — full orchestrator FSM.
4. No mod-compat hints (Spec 4).
5. No Paper build picker.
6. **No new shared FSM abstraction.** Two callers (modpack update, version change). Copy-paste of FSM body is the explicit design choice.

---

## File structure

### Backend (`backend/`)

| File | Change |
|---|---|
| `src/modpack/version_change.rs` | NEW — FSM mirroring `orchestrator.rs::run`/`run_inner`/`rollback` |
| `src/modpack/mod.rs` | EDIT — `pub mod version_change;` |
| `src/modpack/orchestrator.rs` | EDIT — `pub(crate)` exports remain (already done in Spec 1's refactor). If `rollback` is `pub(crate)` already, reuse. Otherwise the version-change rollback is its own copy. |
| `src/routes/servers/version.rs` | NEW — `PATCH /api/servers/:id/version` handler |
| `src/routes/servers/mod.rs` | EDIT — register the route |
| `src/validation.rs` | EDIT (if needed) — confirm `validate_mc_version` is callable from the new handler |
| `tests/version_change.rs` | NEW — integration tests (happy path vanilla, modded with loader, modpack rejected, rollback on verify timeout) |

### Frontend (`frontend/app/`)

| File | Change |
|---|---|
| `lib/api.ts` | EDIT — `changeServerVersion(id, body)` + Zod |
| `servers/tabs/SettingsBody.tsx` | EDIT — new "version" card hidden for modpack source kinds; opens version-change sheet |
| `components/VersionChangeSheet.tsx` | NEW — sheet with MC + loader cascading pickers (reuses `useLoaderVersions`) and warning copy |

---

## Tasks

### Task 1: `version_change.rs` skeleton — entry point + announce/stop/backup phases

**Files:**
- Create: `backend/src/modpack/version_change.rs`
- Modify: `backend/src/modpack/mod.rs`

- [ ] **Step 1: Module skeleton**

```rust
// backend/src/modpack/version_change.rs
//! Orchestrated MC version change for non-modpack servers.
//!
//! Mirrors `orchestrator::run` shape: announce → stop → backup → swap → start
//! → verify, with auto-rollback on failure. Caller spawns this as a task and
//! it owns the [`UpdateGuard`] until completion.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use k8s_openapi::api::core::v1::EnvVar;
use serde_json::json;
use tracing::{Level, event};

use crate::audit::insert_audit;
use crate::error::AppError;
use crate::k8s_patches::patch_statefulset_env;
use crate::modpack::orchestrator::{
    announce_and_save, scale_to, spawn_job, wait_job, wait_pod_gone, wait_pod_running,
    wait_for_done_marker, BACKUP_JOB_TIMEOUT, POD_TERMINATE_TIMEOUT, RESTORE_JOB_TIMEOUT,
};
use crate::modpack::jobs::{build_backup_job, build_restore_job};
use crate::modpack::{UpdateGuard, UpdatePhase};
use crate::state::AppState;

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
            event!(name: "anvil.version_change.succeeded", Level::INFO,
                   server.id = %server_id, "version change succeeded");
        }
        Err(err) => {
            event!(name: "anvil.version_change.failed", Level::ERROR,
                   server.id = %server_id, err = %err, "version change failed");
            // rollback (Task 3)
        }
    }
}

async fn run_inner(
    _state: &AppState,
    _server_id: &str,
    _new_mc: &str,
    _new_loader: Option<&str>,
    _guard: &UpdateGuard,
) -> Result<()> {
    todo!("implemented in subsequent tasks")
}
```

(The list of imports references `pub(crate)` items — verify each one is actually `pub(crate)` in `orchestrator.rs` after Spec 1 lands; if any aren't, promote them with the smallest possible visibility change.)

Add `pub mod version_change;` to `modpack/mod.rs`.

- [ ] **Step 2: Build**

```
cd backend && cargo build
```

Expected: clean build with `todo!()` warning suppressed by the function body (or use `#[allow(...)]`).

- [ ] **Step 3: Commit**

```bash
git add backend/src/modpack/version_change.rs backend/src/modpack/mod.rs
git commit -m "feat(version-change): scaffold module"
```

---

### Task 2: `run_inner` happy-path FSM

**Files:**
- Modify: `backend/src/modpack/version_change.rs`

- [ ] **Step 1: Add capture-old-state helpers**

```rust
async fn fetch_current_env(
    state: &AppState,
    server_id: &str,
) -> Result<Vec<EnvVar>> {
    use kube::Api;
    use k8s_openapi::api::apps::v1::StatefulSet;

    let api: Api<StatefulSet> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let ss = api.get(&format!("mc-{server_id}")).await
        .with_context(|| format!("fetching StatefulSet for {server_id}"))?;
    let containers = ss.spec.as_ref()
        .and_then(|s| s.template.spec.as_ref())
        .map(|s| s.containers.as_slice())
        .unwrap_or(&[]);
    let mc = containers.iter().find(|c| c.name == "mc")
        .ok_or_else(|| anyhow!("mc container not found"))?;
    Ok(mc.env.clone().unwrap_or_default())
}

async fn fetch_source(
    state: &AppState,
    server_id: &str,
) -> Result<(String, String, u32, String, Option<String>)> {
    let row: (String, String, i64, String, Option<String>) = sqlx::query_as(
        "SELECT source_kind, source_config, memory_mi, mc_version, NULL FROM servers WHERE id = ?",
    )
    .bind(server_id).fetch_one(&state.pool).await?;
    // loader_version is inside source_config JSON; parse if needed in Swap.
    Ok((row.0, row.1, row.2 as u32, row.3, row.4))
}
```

(Adjust the SELECT to actually pull the current loader version if the schema stores it separately. With the JSON-in-source_config approach from Spec 1 §5.2.2, parsing JSON in the Swap step is fine.)

- [ ] **Step 2: Implement `run_inner` body**

```rust
async fn run_inner(
    state: &AppState,
    server_id: &str,
    new_mc: &str,
    new_loader: Option<&str>,
    guard: &UpdateGuard,
) -> Result<()> {
    let now_start = Utc::now().timestamp();
    insert_audit(&state.pool, server_id, "version_change_started",
        Some(json!({"new_mc": new_mc, "new_loader": new_loader})), now_start).await?;

    let snapshots_pvc = state.snapshots_pvc.as_ref();

    // capture old state for rollback
    let old_env = fetch_current_env(state, server_id).await?;
    let (source_kind, source_config, memory_mi, old_mc, _) =
        fetch_source(state, server_id).await?;

    if matches!(source_kind.as_str(), "curseforge" | "modrinth") {
        bail!("source_kind {source_kind} cannot use version_change orchestrator");
    }

    // Phase 1: announce
    guard.emit(UpdatePhase::Announcing);
    let _ = announce_and_save(state, server_id).await;

    let job_permit = state.snapshot_pvc_lock.lock().await;

    // Phase 2: stop
    guard.emit(UpdatePhase::Stopping);
    scale_to(&state.kube, &state.mc_namespace, server_id, 0).await?;
    let mc_pod = format!("mc-{server_id}-0");
    wait_pod_gone(&state.kube, &state.mc_namespace, &mc_pod, POD_TERMINATE_TIMEOUT).await?;

    // Phase 3: backup
    guard.emit(UpdatePhase::BackingUp);
    let backup_ts = Utc::now().timestamp();
    let backup_job = build_backup_job(server_id, backup_ts, &state.mc_namespace, snapshots_pvc.as_str());
    let backup_name = backup_job.metadata.name.clone()
        .ok_or_else(|| anyhow!("backup Job missing name"))?;
    spawn_job(&state.kube, &state.mc_namespace, &backup_job).await?;
    wait_job(&state.kube, &state.mc_namespace, &backup_name, BACKUP_JOB_TIMEOUT).await?;
    insert_audit(&state.pool, server_id, "version_change_backup_done",
        Some(json!({"ts": backup_ts})), Utc::now().timestamp()).await?;

    // Phase 4: swap
    guard.emit(UpdatePhase::Swapping);
    apply_swap(state, server_id, &source_kind, &source_config, new_mc, new_loader, memory_mi).await?;
    drop(job_permit);

    // Phase 5: start
    guard.emit(UpdatePhase::Starting);
    scale_to(&state.kube, &state.mc_namespace, server_id, 1).await?;

    // Phase 6: verify
    guard.emit(UpdatePhase::Verifying);
    wait_pod_running(&state.kube, &state.mc_namespace, &mc_pod, POD_RUNNING_TIMEOUT).await?;
    wait_for_done_marker(&state.kube, &state.mc_namespace, server_id,
        boot_timeout_for_kind(&source_kind)).await?;

    // Phase 7: persist
    let now_end = Utc::now().timestamp();
    sqlx::query("UPDATE servers SET last_started_at = ? WHERE id = ?")
        .bind(now_end).bind(server_id).execute(&state.pool).await?;
    insert_audit(&state.pool, server_id, "version_change_succeeded",
        Some(json!({"new_mc": new_mc, "new_loader": new_loader, "old_mc": old_mc})),
        now_end).await?;
    Ok(())
}
```

`POD_RUNNING_TIMEOUT` and `POD_TERMINATE_TIMEOUT` exist in `orchestrator.rs` (verify visibility, promote if needed).

`boot_timeout_for_kind` is a small helper:

```rust
fn boot_timeout_for_kind(source_kind: &str) -> Duration {
    match source_kind {
        "modded" => Duration::from_mins(5),
        "paper"  => Duration::from_mins(2),
        _        => Duration::from_mins(2),  // vanilla
    }
}
```

- [ ] **Step 3: Implement `apply_swap`**

```rust
async fn apply_swap(
    state: &AppState,
    server_id: &str,
    source_kind: &str,
    source_config: &str,
    new_mc: &str,
    new_loader: Option<&str>,
    memory_mi: u32,
) -> Result<()> {
    // Update SQLite first.
    sqlx::query("UPDATE servers SET mc_version = ? WHERE id = ?")
        .bind(new_mc).bind(server_id).execute(&state.pool).await?;

    // For modded, also update loader_version inside source_config JSON.
    let new_source_config = if source_kind == "modded" {
        let mut cfg: serde_json::Value = serde_json::from_str(source_config)?;
        if let Some(obj) = cfg.as_object_mut() {
            obj.insert("loader_version".to_owned(),
                       new_loader.map(|s| serde_json::Value::String(s.to_owned()))
                                 .unwrap_or(serde_json::Value::Null));
        }
        let s = serde_json::to_string(&cfg)?;
        sqlx::query("UPDATE servers SET source_config = ? WHERE id = ?")
            .bind(&s).bind(server_id).execute(&state.pool).await?;
        s
    } else {
        source_config.to_owned()
    };

    // Rebuild env.
    let new_env = build_runtime_env(source_kind, &new_source_config, new_mc, memory_mi)?;

    // Patch StatefulSet.
    patch_statefulset_env(&state.kube, &state.mc_namespace, server_id, &new_env).await?;
    Ok(())
}

fn build_runtime_env(
    source_kind: &str,
    source_config: &str,
    new_mc: &str,
    memory_mi: u32,
) -> Result<Vec<EnvVar>> {
    use crate::modpack::ProviderContext;
    let provider = match source_kind {
        "vanilla" => crate::modpack::vanilla::VanillaRuntime::from_db(source_config, new_mc)?,
        "paper"   => crate::modpack::paper::PaperRuntime::from_db(source_config, new_mc)?,
        "modded"  => crate::modpack::modded::ModdedRuntime::from_db(source_config, new_mc)?,
        _ => bail!("unsupported source_kind {source_kind}"),
    };
    Ok(provider.extra_env(&ProviderContext { server_id: "", memory_mi }))
}
```

The `from_db(source_config, new_mc)` constructors don't necessarily exist today — add them per provider as small fns that parse the JSON (using existing `serde::Deserialize` impls), override `mc_version` to `new_mc`, and return the runtime struct. Each is one line of override + the existing parse.

- [ ] **Step 4: Build + clippy**

```
cargo build
cargo clippy --all-targets --features serve-dir -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add backend/src/modpack/version_change.rs backend/src/modpack/{vanilla,paper,modded}.rs backend/src/modpack/mod.rs
git commit -m "feat(version-change): happy-path FSM"
```

---

### Task 3: Rollback flow

**Files:**
- Modify: `backend/src/modpack/version_change.rs`

- [ ] **Step 1: Implement rollback**

```rust
async fn rollback(
    state: &AppState,
    server_id: &str,
    old_env: &[EnvVar],
    old_mc: &str,
    old_source_config: &str,
    backup_ts: i64,
    guard: &UpdateGuard,
) -> Result<()> {
    let snapshots_pvc = state.snapshots_pvc.as_ref();
    let _permit = state.snapshot_pvc_lock.lock().await;

    // Restore Job (only if a backup actually completed)
    let restore_job = build_restore_job(server_id, backup_ts, &state.mc_namespace, snapshots_pvc.as_str());
    let restore_name = restore_job.metadata.name.clone()
        .ok_or_else(|| anyhow!("restore Job missing name"))?;
    spawn_job(&state.kube, &state.mc_namespace, &restore_job).await?;
    wait_job(&state.kube, &state.mc_namespace, &restore_name, RESTORE_JOB_TIMEOUT).await?;

    // Revert env.
    patch_statefulset_env(&state.kube, &state.mc_namespace, server_id, old_env).await?;

    // Revert SQLite.
    sqlx::query("UPDATE servers SET mc_version = ?, source_config = ? WHERE id = ?")
        .bind(old_mc).bind(old_source_config).bind(server_id)
        .execute(&state.pool).await?;

    // Restart.
    scale_to(&state.kube, &state.mc_namespace, server_id, 1).await?;
    Ok(())
}
```

- [ ] **Step 2: Wire rollback into the failure path**

In `run`, replace the simple Err arm with the modpack-orchestrator-style branch:

```rust
Err(err) => {
    event!(name: "anvil.version_change.failed", Level::ERROR,
           server.id = %server_id, err = %err, "version change failed; attempting rollback");
    guard.emit(UpdatePhase::RollingBack);
    // The rollback function needs old_env, old_mc, old_source_config, backup_ts.
    // run_inner can return them as part of the error if we wrap the error with Context,
    // OR we can capture them in run via a try block. Cleanest: run_inner constructs a
    // RollbackContext and stores it in a tokio::sync::OnceCell that run reads on failure.
    // For simplicity in v1, restructure run/run_inner so run_inner returns a structured
    // result enum: Ok or Err(VersionChangeError { context, source }).
    ...
}
```

Refactor: replace `run_inner`'s `Result<()>` return with a custom error type that carries the rollback context.

```rust
struct RollbackContext {
    old_env: Vec<EnvVar>,
    old_mc: String,
    old_source_config: String,
    backup_ts: Option<i64>,  // None when failure happened before backup
}

enum FsmError {
    Pre(anyhow::Error),                            // failure before any swap (no rollback)
    Post(RollbackContext, anyhow::Error),          // failure after swap (rollback)
}
```

Adjust `run_inner` to return `Result<(), FsmError>`. After Phase 4 (swap) succeeds, wrap subsequent failures in `FsmError::Post(...)`. Phase 1-3 failures wrap in `FsmError::Pre(...)`.

```rust
match outcome {
    Ok(()) => guard.emit(UpdatePhase::Succeeded),
    Err(FsmError::Pre(e)) => {
        // No rollback needed — server is just stopped or partly stopped.
        guard.emit(UpdatePhase::Failed);
        let _ = insert_audit(&state.pool, &server_id, "version_change_failed",
            Some(json!({"err": e.to_string()})), Utc::now().timestamp()).await;
    }
    Err(FsmError::Post(ctx, e)) => {
        guard.emit(UpdatePhase::RollingBack);
        match rollback(&state, &server_id, &ctx.old_env, &ctx.old_mc,
                       &ctx.old_source_config, ctx.backup_ts.unwrap_or_default(), &guard).await {
            Ok(()) => {
                guard.emit(UpdatePhase::RolledBack);
                let _ = insert_audit(&state.pool, &server_id,
                    "version_change_failed_rolled_back",
                    Some(json!({"err": e.to_string()})), Utc::now().timestamp()).await;
            }
            Err(rb) => {
                guard.emit(UpdatePhase::Failed);
                let _ = insert_audit(&state.pool, &server_id, "version_change_failed",
                    Some(json!({"err": e.to_string(), "rollback_err": rb.to_string()})),
                    Utc::now().timestamp()).await;
            }
        }
    }
}
```

- [ ] **Step 3: Build + clippy**

```
cargo build
cargo clippy --all-targets --features serve-dir -- -D warnings
```

- [ ] **Step 4: Commit**

```bash
git add backend/src/modpack/version_change.rs
git commit -m "feat(version-change): rollback flow on post-swap failure"
```

---

### Task 4: Endpoint `PATCH /api/servers/:id/version`

**Files:**
- Create: `backend/src/routes/servers/version.rs`
- Modify: `backend/src/routes/servers/mod.rs`
- Test: `backend/tests/version_change.rs`

- [ ] **Step 1: Failing integration test**

```rust
// backend/tests/version_change.rs
mod common;

#[tokio::test]
async fn patch_version_modpack_rejected() {
    let (state, _) = common::test_state().await;
    let id = common::seed_modrinth_server(&state, "ts-mp").await;
    let err = common::patch_version(&state, &id, "1.21.5", None).await.unwrap_err();
    assert_eq!(err.code, "version_change_unsupported");
}

#[tokio::test]
async fn patch_version_neoforge_requires_loader() {
    let (state, _) = common::test_state().await;
    let id = common::seed_modded_server(&state, "ts-nf", "neoforge", "1.21.4", Some("21.4.81")).await;
    let err = common::patch_version(&state, &id, "1.21.5", None).await.unwrap_err();
    assert_eq!(err.code, "loader_version_required");
}

#[tokio::test]
async fn patch_version_vanilla_starts_fsm() {
    let (state, _) = common::test_state().await;
    let id = common::seed_vanilla_server(&state, "ts-v").await;
    let resp = common::patch_version(&state, &id, "1.21.5", None).await.unwrap();
    assert_eq!(resp.status, "started");
    // Optional: poll the WS or audit log to confirm phases progressed.
}
```

- [ ] **Step 2: Implement handler**

```rust
// backend/src/routes/servers/version.rs
use axum::{extract::{Path, State}, Json, http::StatusCode};
use serde::{Deserialize, Serialize};

use crate::error::AppError;
use crate::modpack::{UpdateGuard, version_change};
use crate::state::AppState;
use crate::validation::validate_mc_version;

#[derive(Debug, Deserialize)]
pub struct VersionRequest {
    pub mc_version: String,
    pub loader_version: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct VersionResponse {
    pub status: &'static str,
    pub server_id: String,
}

pub async fn handle(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(req): Json<VersionRequest>,
) -> Result<(StatusCode, Json<VersionResponse>), AppError> {
    // Snapshot dependency.
    if state.snapshots_pvc.is_none() {
        return Err(AppError::ServiceUnavailable {
            code: "snapshots_unavailable",
            message: "snapshots PVC not configured".to_owned(),
        });
    }

    // Server exists?
    let row: (String, String, String) = sqlx::query_as(
        "SELECT source_kind, source_config, mc_version FROM servers WHERE id = ?")
        .bind(&id).fetch_optional(&state.pool).await?
        .ok_or(AppError::NotFound { code: "server_not_found" })?;
    let (source_kind, source_config, current_mc) = row;

    // Modpack rejection.
    if matches!(source_kind.as_str(), "curseforge" | "modrinth") {
        return Err(AppError::Conflict {
            code: "version_change_unsupported",
            message: "modpack servers update via the modpack flow".to_owned(),
        });
    }

    // Validate mc_version.
    validate_mc_version(&state, &req.mc_version).await
        .map_err(|_| AppError::BadRequest {
            code: "invalid_mc_version",
            message: format!("{} is not a known mc version", req.mc_version),
        })?;

    // Modded with forge/neoforge requires loader_version.
    if source_kind == "modded" {
        let cfg: serde_json::Value = serde_json::from_str(&source_config)?;
        let runtime = cfg.get("runtime").and_then(|v| v.as_str()).unwrap_or("");
        if matches!(runtime, "forge" | "neoforge") && req.loader_version.is_none() {
            return Err(AppError::BadRequest {
                code: "loader_version_required",
                message: format!("{runtime} requires loader_version"),
            });
        }
    }

    // No-op detection.
    let current_loader = serde_json::from_str::<serde_json::Value>(&source_config).ok()
        .and_then(|v| v.get("loader_version").and_then(|x| x.as_str()).map(String::from));
    if req.mc_version == current_mc && req.loader_version == current_loader {
        return Err(AppError::BadRequest {
            code: "nothing_to_change",
            message: "mc_version and loader_version match current".to_owned(),
        });
    }

    // Acquire guard.
    let Some(guard) = UpdateGuard::try_acquire(
        &id, state.update_locks.clone(), state.update_phase_buses.clone()) else {
        return Err(AppError::Conflict {
            code: "update_in_progress",
            message: "another update or apply is running".to_owned(),
        });
    };

    let task_state = state.clone();
    let task_id = id.clone();
    let new_mc = req.mc_version.clone();
    let new_loader = req.loader_version.clone();
    tokio::spawn(async move {
        version_change::run(task_state, task_id, new_mc, new_loader, guard).await;
    });

    Ok((StatusCode::ACCEPTED, Json(VersionResponse { status: "started", server_id: id })))
}
```

- [ ] **Step 3: Wire route**

In `routes/servers/mod.rs`:

```rust
.route("/api/servers/{id}/version", patch(servers::version::handle))
```

- [ ] **Step 4: Run tests + clippy**

```
cargo test --test version_change
cargo clippy --all-targets --features serve-dir -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add backend/src/routes/servers/version.rs backend/src/routes/servers/mod.rs backend/tests/version_change.rs
git commit -m "feat(api): PATCH /api/servers/{id}/version"
```

---

### Task 5: Frontend — version card + change sheet

**Files:**
- Modify: `frontend/app/lib/api.ts`
- Modify: `frontend/app/servers/tabs/SettingsBody.tsx`
- Create: `frontend/app/components/VersionChangeSheet.tsx`

- [ ] **Step 1: API client**

```ts
const versionResponseSchema = z.object({
  status: z.string(),
  server_id: z.string(),
});

export async function changeServerVersion(
  id: string,
  body: { mc_version: string; loader_version?: string },
): Promise<{ status: string; server_id: string }> {
  const res = await fetch(`/api/servers/${id}/version`, {
    method: "PATCH",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  if (!res.ok) throw await ApiError.fromResponse(res);
  return versionResponseSchema.parse(await res.json());
}
```

- [ ] **Step 2: Version card in SettingsBody**

Replace the existing placeholder text (`SettingsBody.tsx:112-115`) with:

```tsx
const isModpack = detail.source_kind === "curseforge" || detail.source_kind === "modrinth";
const [versionSheetOpen, setVersionSheetOpen] = useState(false);

{!isModpack && (
  <Card header="version">
    <div className="grid grid-cols-[80px_1fr_auto] items-baseline gap-2">
      <span className="font-mono text-[11px] uppercase tracking-wider text-text-muted">mc</span>
      <span className="font-mono text-[12px] text-text-body">{detail.mc_version ?? "—"}</span>
      <Button onClick={() => setVersionSheetOpen(true)} variant="ghost">edit</Button>
    </div>
    {/* loader row only for modded forge/neoforge — derive from source_config */}
  </Card>
)}

<VersionChangeSheet
  open={versionSheetOpen}
  onClose={() => setVersionSheetOpen(false)}
  detail={detail}
/>
```

- [ ] **Step 3: VersionChangeSheet component**

```tsx
// frontend/app/components/VersionChangeSheet.tsx
"use client";
import { useEffect, useMemo, useState } from "react";
import { changeServerVersion, type ServerDetail } from "../lib/api";
import { useLoaderVersions } from "../lib/use-loader-versions";
import { useMcVersions } from "../lib/use-mc-versions";
import { useServerDetail } from "../lib/server-detail-context";
import { Sheet } from "./Sheet";
import { Button } from "./Button";
import { useToast } from "./Toast";
import { UpdateSheet } from "./UpdateSheet";

interface Props {
  open: boolean;
  onClose: () => void;
  detail: ServerDetail;
}

export function VersionChangeSheet({ open, onClose, detail }: Props): JSX.Element {
  const toast = useToast();
  const { refresh } = useServerDetail();
  const versions = useMcVersions();

  const moddedRuntime = useMemo(() => {
    if (detail.source_kind !== "modded") return null;
    try {
      const cfg = JSON.parse(detail.source_config);
      const r = cfg.runtime;
      return r === "forge" || r === "neoforge" ? r : null;
    } catch { return null; }
  }, [detail]);
  const loaderVs = useLoaderVersions(moddedRuntime);

  const [pickedMc, setPickedMc] = useState<string | null>(null);
  const [pickedLoader, setPickedLoader] = useState<string | null>(null);
  useEffect(() => {
    if (open) {
      setPickedMc(detail.mc_version ?? null);
      setPickedLoader(null);
    }
  }, [open, detail.mc_version]);

  const [progressOpen, setProgressOpen] = useState(false);

  const onSubmit = (): void => {
    if (pickedMc === null) return;
    changeServerVersion(detail.id, {
      mc_version: pickedMc,
      ...(pickedLoader !== null ? { loader_version: pickedLoader } : {}),
    })
      .then(() => {
        onClose();
        setProgressOpen(true);
      })
      .catch((err: unknown) => {
        const msg = err instanceof Error ? err.message : "unknown error";
        toast.push(`version change failed · ${msg}`, "error");
      });
  };

  const mcOptions = moddedRuntime !== null
    ? (loaderVs?.mc_versions ?? [])
    : (versions?.versions.map((v) => v.id) ?? []);

  return (
    <>
      <Sheet open={open} onClose={onClose} title="change mc version">
        <div className="flex flex-col gap-3 p-4">
          <select value={pickedMc ?? ""} onChange={(e) => setPickedMc(e.target.value || null)}>
            <option value="">— pick mc —</option>
            {mcOptions.map((m) => <option key={m} value={m}>{m}</option>)}
          </select>
          {moddedRuntime !== null && pickedMc !== null && (
            <select value={pickedLoader ?? ""} onChange={(e) => setPickedLoader(e.target.value || null)}>
              <option value="">— pick {moddedRuntime} version —</option>
              {(loaderVs?.by_mc[pickedMc] ?? []).map((v) => <option key={v} value={v}>{v}</option>)}
            </select>
          )}
          <p className="rounded border border-border bg-surface p-3 text-[12px] text-text-faint">
            this stops the server, snapshots data, swaps in the new version, and restarts.
            world data may not migrate cleanly across major versions. on failure the server
            auto-restores to the snapshot.
          </p>
          <div className="flex gap-2">
            <Button onClick={onClose} variant="secondary">cancel</Button>
            <Button onClick={onSubmit} disabled={pickedMc === null
              || (moddedRuntime !== null && pickedLoader === null)}>
              change version
            </Button>
          </div>
        </div>
      </Sheet>
      <UpdateSheet
        open={progressOpen}
        onClose={() => { setProgressOpen(false); refresh(); }}
        serverId={detail.id}
      />
    </>
  );
}
```

(`UpdateSheet` already accepts a `serverId` and connects to `/api/servers/{id}/update/stream`. Verify the prop name in the existing component; adjust if needed.)

- [ ] **Step 4: Manual repro**

Build, run, navigate to a vanilla server's settings, change MC version, watch the FSM progress in `UpdateSheet`.

- [ ] **Step 5: Commit**

```bash
git add frontend/app/lib/api.ts frontend/app/servers/tabs/SettingsBody.tsx frontend/app/components/VersionChangeSheet.tsx
git commit -m "feat(settings): mc version change sheet"
```

---

## Verification

- [ ] `cd backend && cargo fmt --all && cargo clippy --all-targets --features serve-dir -- -D warnings && cargo clippy --all-targets --features embed -- -D warnings && cargo test --all`
- [ ] `cd frontend && pnpm typecheck && pnpm lint && pnpm build`
- [ ] Manual repro: vanilla version change, modded forge change, modpack rejected, simulated bad version triggers rollback (downgrade to a major-incompatible MC).

---

## Implementation prompt

```
Implement the plan at docs/superpowers/plans/2026-05-06-anvil-mc-version-change-impl.md.

Use superpowers:executing-plans (or subagent-driven-development). Tasks 1 → 5 in order.
The spec at docs/superpowers/specs/2026-05-06-anvil-mc-version-change-design.md is the
design authority.

Depends on Spec 1 plan having landed: k8s_patches::patch_statefulset_env, ModdedConfig.loader_version,
GET /api/runtimes/:runtime/versions, refreshable ServerDetailContext.

Run the verification commands. Commit per task in conventional commits style.
Read frontend/AGENTS.md before frontend code.
```
