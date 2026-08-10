<!--
SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

# Anvil — PVC resize · file-helper kill · status nuance Implementation Plan (Spec 2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Three small features — PVC grow-only resize, manual file-helper kill, restart-loop visible as Error.

**Architecture:** Three independent items with no inter-task dependencies. Order them by risk (status fix first since it's pure-function + tests, then file-helper, then PVC since the latter touches capabilities + a new endpoint + UI). Reuses Spec 1's refreshable `ServerDetailContext` (assumed landed).

**Tech Stack:** Rust 1.83+ · axum 0.8 · kube-rs · sqlx · Next.js 16 · TypeScript · Zod v4. No new deps.

**Source spec:** `docs/superpowers/specs/2026-05-06-anvil-pvc-files-status-design.md` (signed off 2026-05-06).
**Depends on:** Spec 1 plan landed (refreshable context in `frontend/app/lib/server-detail-context.ts`).

---

## Hard constraints

- No new RBAC needed (verified in spec §3 against `deploy/templates/role.yaml`).
- No new SQLite migration.
- No new top-level deps.
- Standard build/test gates per task:
  ```
  cd backend && cargo fmt --all && cargo clippy --all-targets --features serve-dir -- -D warnings && cargo test --all
  cd frontend && pnpm typecheck && pnpm lint
  ```

---

## Decisions locked from spec §8

1. PVC grow-only (no shrink).
2. Aggressive restart-error rule: `restart_count > 0 && !ready → Error`.
3. Per-SC expansion flag (`expandable_storage_classes`) on capabilities, not a single global bool.
4. File-helper kill returns 409 when server is running (defensive guard).
5. No PVC resize progress streaming — fire-and-forget.

---

## File structure

### Backend (`backend/`)

| File | Change |
|---|---|
| `src/k8s_status.rs` | EDIT — add restart-count clause to `pod_in_error_state` (`:62-76`); extend test module |
| `src/routes/cluster.rs` | EDIT — add `expandable_storage_classes: Vec<String>` to `ClusterCapabilities`; populate in handle |
| `src/routes/servers/storage.rs` | NEW — `PATCH /api/servers/:id/storage` |
| `src/routes/servers/files.rs` | NEW or EDIT — `DELETE /api/servers/:id/files/helper` |
| `src/routes/servers/get.rs` | EDIT — extend `ServerDetail` with `files_helper_running: bool` |
| `src/routes/servers/mod.rs` | EDIT — register the new routes |
| `tests/storage_resize.rs` | NEW — integration tests for PVC PATCH |
| `tests/files_helper_kill.rs` | NEW — integration tests for file-helper DELETE |

### Frontend (`frontend/app/`)

| File | Change |
|---|---|
| `lib/api.ts` | EDIT — add `expandable_storage_classes` to `clusterCapabilitiesSchema`; add `files_helper_running: boolean` to `serverDetailSchema`; add `resizeServerStorage`, `killFilesHelper` API functions |
| `servers/tabs/SettingsBody.tsx` | EDIT — new "storage" card (hidden when SC not in expandable list) |
| `servers/tabs/FilesBody.tsx` | EDIT — "stop file viewer" button when `status === "stopped" && files_helper_running` |

---

## Tasks

### Task 1: Status restart-loop nuance (D#14)

**Files:**
- Modify: `backend/src/k8s_status.rs`

- [ ] **Step 1: Add new tests in the existing `tests` module**

Append to the `mod tests` block at the bottom of `k8s_status.rs` (around `:133`):

```rust
fn make_pod_with_restart_count(restart_count: i32, ready: bool) -> Pod {
    Pod {
        status: Some(PodStatus {
            container_statuses: Some(vec![ContainerStatus {
                name: "mc".to_owned(),
                restart_count,
                ready,
                state: Some(ContainerState::default()),
                ..ContainerStatus::default()
            }]),
            ..PodStatus::default()
        }),
        ..Pod::default()
    }
}

#[test]
fn replicas_one_restart_count_unready_is_error() {
    let pod = make_pod_with_restart_count(1, false);
    assert_eq!(derive_status(1, 0, Some(&pod)), ServerStatus::Error);
}
#[test]
fn replicas_one_restart_count_ready_is_running() {
    // ready_replicas short-circuits to Running before pod_in_error_state runs
    let pod = make_pod_with_restart_count(2, true);
    assert_eq!(derive_status(1, 1, Some(&pod)), ServerStatus::Running);
}
#[test]
fn replicas_one_zero_restarts_no_error_reason_is_starting() {
    // regression: existing behaviour
    let pod = make_pod_with_restart_count(0, false);
    assert_eq!(derive_status(1, 0, Some(&pod)), ServerStatus::Starting);
}
```

- [ ] **Step 2: Run, verify failure**

```
cargo test --lib k8s_status -- --nocapture
```

Expected: `replicas_one_restart_count_unready_is_error` fails (current behaviour returns Starting).

- [ ] **Step 3: Implement**

Replace the body of `pod_in_error_state` (`:62-76`) with:

```rust
fn pod_in_error_state(pod: &Pod) -> bool {
    let Some(status) = pod.status.as_ref() else { return false; };
    let Some(statuses) = status.container_statuses.as_ref() else { return false; };
    statuses.iter().any(|cs| {
        let waiting_error = cs
            .state.as_ref()
            .and_then(|st| st.waiting.as_ref())
            .and_then(|w| w.reason.as_deref())
            .is_some_and(|r| ERROR_REASONS.contains(&r));
        let restarted_unready = cs.restart_count > 0 && !cs.ready;
        waiting_error || restarted_unready
    })
}
```

- [ ] **Step 4: Run all status tests, verify pass**

```
cargo test --lib k8s_status
```

- [ ] **Step 5: Commit**

```bash
git add backend/src/k8s_status.rs
git commit -m "fix(status): treat restart_count > 0 + unready as Error"
```

---

### Task 2: Capability — `expandable_storage_classes`

**Files:**
- Modify: `backend/src/routes/cluster.rs`
- Modify: `frontend/app/lib/api.ts`

- [ ] **Step 1: Backend test for the field**

Add a unit test in `cluster.rs` (or its existing test module if any) using a hand-built `Vec<StorageClass>`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::storage::v1::StorageClass;
    use kube::core::ObjectMeta;
    use std::collections::BTreeMap;

    fn sc(name: &str, allow: bool, default: bool) -> StorageClass {
        let mut annotations = BTreeMap::new();
        if default {
            annotations.insert(
                "storageclass.kubernetes.io/is-default-class".to_owned(),
                "true".to_owned(),
            );
        }
        StorageClass {
            metadata: ObjectMeta { name: Some(name.to_owned()),
                annotations: if default { Some(annotations) } else { None },
                ..ObjectMeta::default() },
            allow_volume_expansion: Some(allow),
            ..StorageClass::default()
        }
    }

    #[test]
    fn capabilities_compute_expandable_set() {
        let scs = vec![
            sc("tank", true, true),
            sc("openebs-hostpath", false, false),
            sc("fast", true, false),
        ];
        let caps = compute_caps_from_scs(&scs, true, true);
        assert!(caps.expandable_storage_classes.contains(&"tank".to_owned()));
        assert!(caps.expandable_storage_classes.contains(&"fast".to_owned()));
        assert!(!caps.expandable_storage_classes.contains(&"openebs-hostpath".to_owned()));
    }
}
```

This requires extracting a helper `compute_caps_from_scs(scs, lb_supported, cf_present) -> ClusterCapabilities` from the current `handle()` body so it's testable.

- [ ] **Step 2: Run, verify failure**

```
cargo test --lib routes::cluster
```

- [ ] **Step 3: Implement**

In `cluster.rs`:

```rust
pub struct ClusterCapabilities {
    pub loadbalancer: bool,
    pub nodeport: bool,
    pub clusterip: bool,
    pub available_storage_classes: Vec<String>,
    pub expandable_storage_classes: Vec<String>,        // NEW
    pub default_storage_class: Option<String>,
    pub cf_api_key_present: bool,
}

fn compute_caps_from_scs(
    scs: &[StorageClass],
    loadbalancer: bool,
    cf_api_key_present: bool,
) -> ClusterCapabilities {
    let mut classes = Vec::new();
    let mut expandable = Vec::new();
    let mut default: Option<String> = None;
    for sc in scs {
        let Some(name) = sc.metadata.name.clone() else { continue; };
        let is_default = sc.metadata.annotations.as_ref()
            .and_then(|a| a.get("storageclass.kubernetes.io/is-default-class"))
            .map(String::as_str) == Some("true");
        if is_default { default = Some(name.clone()); }
        if sc.allow_volume_expansion.unwrap_or(false) {
            expandable.push(name.clone());
        }
        classes.push(name);
    }
    classes.sort();
    expandable.sort();
    ClusterCapabilities {
        loadbalancer, nodeport: true, clusterip: true,
        available_storage_classes: classes,
        expandable_storage_classes: expandable,
        default_storage_class: default,
        cf_api_key_present,
    }
}
```

In the existing `handle()`, replace the inline build with `compute_caps_from_scs(&list.items, state.loadbalancer_supported, state.cf_client.is_some())`.

- [ ] **Step 4: Frontend zod**

In `frontend/app/lib/api.ts`, extend `clusterCapabilitiesSchema`:

```ts
expandable_storage_classes: z.array(z.string()),
```

- [ ] **Step 5: Tests + types**

```
cd backend && cargo test --lib
cd frontend && pnpm typecheck
```

- [ ] **Step 6: Commit**

```bash
git add backend/src/routes/cluster.rs frontend/app/lib/api.ts
git commit -m "feat(capabilities): expose expandable_storage_classes"
```

---

### Task 3: PVC resize endpoint + integration test

**Files:**
- Create: `backend/src/routes/servers/storage.rs`
- Modify: `backend/src/routes/servers/mod.rs`
- Test: `backend/tests/storage_resize.rs`

- [ ] **Step 1: Failing integration test**

```rust
// backend/tests/storage_resize.rs
mod common;

#[tokio::test]
async fn patch_storage_grows_pvc_and_db() {
    let (state, _) = common::test_state().await;
    let id = common::seed_vanilla_server(&state, "ts-grow", 4096).await;
    common::set_storage_size_gi(&state, &id, 20).await;

    let resp = common::patch_storage(&state, &id, 50).await.unwrap();
    assert_eq!(resp.size_gi, 50);

    let pvc = common::fetch_pvc(&state, &format!("data-mc-{id}-0")).await;
    let storage = pvc.spec.unwrap().resources.unwrap().requests.unwrap()
        .get("storage").unwrap().0.clone();
    assert!(storage.starts_with("50"), "got {storage}");

    let row = common::fetch_server_row(&state, &id).await;
    assert_eq!(row.storage_size_gi, 50);
}

#[tokio::test]
async fn patch_storage_shrink_rejected() {
    let (state, _) = common::test_state().await;
    let id = common::seed_vanilla_server(&state, "ts-shrink", 4096).await;
    common::set_storage_size_gi(&state, &id, 50).await;
    let err = common::patch_storage(&state, &id, 20).await.unwrap_err();
    assert_eq!(err.code, "shrink_unsupported");
}

#[tokio::test]
async fn patch_storage_unsupported_sc_rejected() {
    let (state, _) = common::test_state().await;
    let id = common::seed_with_sc(&state, "ts-nosc", "no-expand").await;
    let err = common::patch_storage(&state, &id, 100).await.unwrap_err();
    assert_eq!(err.code, "expansion_unsupported");
}
```

- [ ] **Step 2: Run, verify failures**

```
cargo test --test storage_resize
```

- [ ] **Step 3: Implement handler**

```rust
// backend/src/routes/servers/storage.rs
use axum::{extract::{Path, State}, Json, http::StatusCode};
use k8s_openapi::api::core::v1::PersistentVolumeClaim;
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use kube::{Api, api::{Patch, PatchParams}};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::AppError;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ResizeRequest { pub size_gi: u32 }

#[derive(Debug, Serialize)]
pub struct ResizeResponse { pub size_gi: u32 }

pub async fn handle(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(req): Json<ResizeRequest>,
) -> Result<(StatusCode, Json<ResizeResponse>), AppError> {
    let row: (i64, Option<String>) = sqlx::query_as(
        "SELECT storage_size_gi, storage_class FROM servers WHERE id = ?")
        .bind(&id).fetch_optional(&state.pool).await?
        .ok_or(AppError::NotFound { code: "server_not_found" })?;
    let current = row.0 as u32;
    let sc = row.1.unwrap_or_else(|| "tank".to_string());

    if req.size_gi <= current {
        return Err(AppError::BadRequest {
            code: "shrink_unsupported",
            message: "storage size can only grow".to_owned(),
        });
    }

    let caps = crate::routes::cluster::current_caps(&state).await?;
    if !caps.expandable_storage_classes.iter().any(|n| n == &sc) {
        return Err(AppError::Conflict {
            code: "expansion_unsupported",
            message: format!("storage class {sc} does not support volume expansion"),
        });
    }

    let pvc_api: Api<PersistentVolumeClaim> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let patch = json!({ "spec": { "resources": { "requests": {
        "storage": format!("{}Gi", req.size_gi)
    } } } });
    pvc_api.patch(&format!("data-mc-{id}-0"), &PatchParams::default(),
                  &Patch::Strategic(&patch))
        .await
        .map_err(|e| AppError::Internal {
            code: "pvc_patch_failed",
            message: format!("PVC PATCH failed: {e}"),
        })?;

    sqlx::query("UPDATE servers SET storage_size_gi = ? WHERE id = ?")
        .bind(req.size_gi as i64).bind(&id).execute(&state.pool).await?;

    Ok((StatusCode::OK, Json(ResizeResponse { size_gi: req.size_gi })))
}
```

`current_caps` is a small helper extracted in `cluster.rs` that returns a `Result<ClusterCapabilities, AppError>` reading the cache or fetching fresh.

- [ ] **Step 4: Wire route**

In `routes/servers/mod.rs`:

```rust
.route("/api/servers/{id}/storage", patch(servers::storage::handle))
```

- [ ] **Step 5: Tests + clippy**

```
cargo test --test storage_resize
cargo clippy --all-targets --features serve-dir -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add backend/src/routes/servers/storage.rs backend/src/routes/servers/mod.rs backend/tests/storage_resize.rs
git commit -m "feat(api): PATCH /api/servers/{id}/storage (grow only)"
```

---

### Task 4: PVC resize — frontend storage card

**Files:**
- Modify: `frontend/app/lib/api.ts`
- Modify: `frontend/app/servers/tabs/SettingsBody.tsx`

- [ ] **Step 1: API client + Zod**

```ts
// frontend/app/lib/api.ts
const resizeResponseSchema = z.object({ size_gi: z.number() });

export async function resizeServerStorage(
  id: string,
  size_gi: number,
): Promise<{ size_gi: number }> {
  const res = await fetch(`/api/servers/${id}/storage`, {
    method: "PATCH",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ size_gi }),
  });
  if (!res.ok) throw await ApiError.fromResponse(res);
  return resizeResponseSchema.parse(await res.json());
}
```

- [ ] **Step 2: Storage card in SettingsBody**

After the memory card, before the version placeholder:

```tsx
const { detail, refresh } = useServerDetail();
const [pendingSize, setPendingSize] = useState<number>(detail.storage_size_gi);
const expandable = caps?.expandable_storage_classes ?? [];
const sc = detail.storage_class ?? caps?.default_storage_class ?? "";
const canExpand = expandable.includes(sc);

const onExpand = (): void => {
  if (pendingSize <= detail.storage_size_gi) return;
  resizeServerStorage(detail.id, pendingSize)
    .then(() => { refresh(); toast.push("resize requested", "success"); })
    .catch((err: unknown) => {
      const msg = err instanceof ApiError ? `${err.code}: ${err.message}` : "unknown error";
      toast.push(`resize failed · ${msg}`, "error");
    });
};

{canExpand && (
  <Card header="storage">
    <p className="font-mono text-[12px] text-text-body">current: {detail.storage_size_gi} Gi</p>
    <RangeSlider
      min={detail.storage_size_gi}
      max={detail.storage_size_gi * 4}
      value={pendingSize}
      onChange={setPendingSize}
    />
    <Button onClick={onExpand} disabled={pendingSize <= detail.storage_size_gi}>
      expand to {pendingSize} Gi
    </Button>
    <p className="mt-2 font-mono text-[11px] text-text-faint">grow only · shrink not supported</p>
  </Card>
)}
```

- [ ] **Step 3: Manual repro**

Build, run, navigate to a server's settings. Verify the card renders for `tank`-backed servers, doesn't render for non-expandable SC.

- [ ] **Step 4: Commit**

```bash
git add frontend/app/lib/api.ts frontend/app/servers/tabs/SettingsBody.tsx
git commit -m "feat(settings): storage expansion control"
```

---

### Task 5: File-helper kill — backend endpoint + status field

**Files:**
- Modify: `backend/src/routes/servers/get.rs`
- Modify: `backend/src/routes/servers/files.rs` (or new) — add helper kill handler
- Modify: `backend/src/routes/servers/mod.rs`
- Test: `backend/tests/files_helper_kill.rs`

- [ ] **Step 1: Failing integration test**

```rust
// backend/tests/files_helper_kill.rs
mod common;

#[tokio::test]
async fn delete_helper_kills_pod_when_present() {
    let (state, _) = common::test_state().await;
    let id = common::seed_vanilla_server(&state, "ts-fh", 4096).await;
    common::ensure_helper(&state, &id).await;

    let resp = common::delete_helper(&state, &id).await.unwrap();
    assert_eq!(resp.status, 204);

    assert!(common::helper_pod_gone(&state, &id).await);
}

#[tokio::test]
async fn delete_helper_no_op_when_absent() {
    let (state, _) = common::test_state().await;
    let id = common::seed_vanilla_server(&state, "ts-noh", 4096).await;
    let resp = common::delete_helper(&state, &id).await.unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body["already_gone"], serde_json::json!(true));
}

#[tokio::test]
async fn delete_helper_blocked_when_running() {
    let (state, _) = common::test_state().await;
    let id = common::seed_vanilla_server(&state, "ts-rh", 4096).await;
    common::ensure_helper(&state, &id).await;
    common::set_replicas(&state, &id, 1).await;
    let err = common::delete_helper(&state, &id).await.unwrap_err();
    assert_eq!(err.code, "helper_unsafe_to_kill");
}
```

- [ ] **Step 2: Implement**

```rust
// backend/src/routes/servers/files.rs (add to existing or create)
use axum::{extract::{Path, State}, Json, http::StatusCode, response::IntoResponse};
use k8s_openapi::api::core::v1::Pod;
use kube::Api;
use serde_json::json;

use crate::error::AppError;
use crate::state::AppState;
use crate::files_helper;

pub async fn kill_helper(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    // 409 when the MC server is running.
    let ss_api: Api<k8s_openapi::api::apps::v1::StatefulSet> =
        Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let ss = ss_api.get(&format!("mc-{id}")).await
        .map_err(|_| AppError::NotFound { code: "server_not_found" })?;
    let replicas = ss.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0);
    if replicas > 0 {
        return Err(AppError::Conflict {
            code: "helper_unsafe_to_kill",
            message: "stop the server first".to_owned(),
        });
    }

    let pod_api: Api<Pod> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let exists = pod_api.get_opt(&format!("mc-{id}-files")).await?.is_some();
    if !exists {
        return Ok((StatusCode::OK, Json(json!({ "already_gone": true }))).into_response());
    }
    files_helper::tear_down_helper(&state, &id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
```

- [ ] **Step 3: Add `files_helper_running` to ServerDetail**

In `routes/servers/get.rs`, extend `ServerDetail` and compute alongside the existing concurrent fetches:

```rust
let pod_api: Api<Pod> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
let helper = pod_api.get_opt(&format!("mc-{id}-files")).await
    .ok().flatten()
    .map(|p| p.metadata.deletion_timestamp.is_none())
    .unwrap_or(false);
// ... include in ServerDetail under `files_helper_running: helper`
```

(Place this in the existing `tokio::join!` block to keep parallelism.)

- [ ] **Step 4: Wire route**

```rust
.route("/api/servers/{id}/files/helper", delete(servers::files::kill_helper))
```

- [ ] **Step 5: Test + clippy**

```
cargo test --test files_helper_kill
cargo clippy --all-targets --features serve-dir -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add backend/src/routes/servers/{files,get,mod}.rs backend/tests/files_helper_kill.rs
git commit -m "feat(files): manual file-helper kill endpoint + status field"
```

---

### Task 6: File-helper kill — frontend button

**Files:**
- Modify: `frontend/app/lib/api.ts`
- Modify: `frontend/app/servers/tabs/FilesBody.tsx`

- [ ] **Step 1: API client**

```ts
export async function killFilesHelper(id: string): Promise<void> {
  const res = await fetch(`/api/servers/${id}/files/helper`, { method: "DELETE" });
  if (!res.ok) throw await ApiError.fromResponse(res);
}
```

Extend `serverDetailSchema` with `files_helper_running: z.boolean()`.

- [ ] **Step 2: Wire button**

In `FilesBody.tsx`, at the top of the body (above the file list):

```tsx
const { detail, refresh } = useServerDetail();
const onKill = (): void => {
  killFilesHelper(detail.id)
    .then(() => { refresh(); toast.push("file viewer stopped", "success"); })
    .catch((err: unknown) => {
      const msg = err instanceof ApiError ? `${err.code}: ${err.message}` : "unknown error";
      toast.push(`stop failed · ${msg}`, "error");
    });
};

{detail.status === "stopped" && detail.files_helper_running && (
  <div className="flex items-center justify-between border-b border-border px-3 py-2">
    <span className="font-mono text-[12px] text-text-faint">file viewer is running · idle</span>
    <Button onClick={onKill} variant="danger">stop file viewer</Button>
  </div>
)}
```

- [ ] **Step 3: Manual repro**

Stop a server, browse files (helper auto-spawns), navigate back; the bar appears; click → helper goes away.

- [ ] **Step 4: Commit**

```bash
git add frontend/app/lib/api.ts frontend/app/servers/tabs/FilesBody.tsx
git commit -m "feat(files): stop file viewer button"
```

---

## Verification

- [ ] `cd backend && cargo fmt --all && cargo clippy --all-targets --features serve-dir -- -D warnings && cargo clippy --all-targets --features embed -- -D warnings && cargo test --all`
- [ ] `cd frontend && pnpm typecheck && pnpm lint && pnpm build`
- [ ] Single-binary smoke run.
- [ ] Manual repro of all three FE flows.

---

## Implementation prompt

```
Implement the plan at docs/superpowers/plans/2026-05-06-anvil-pvc-files-status-impl.md.

Use the superpowers:executing-plans skill (or superpowers:subagent-driven-development).
Tasks 1 → 6 in order. The spec at
docs/superpowers/specs/2026-05-06-anvil-pvc-files-status-design.md is the design authority.

This plan depends on Spec 1's refreshable ServerDetailContext — confirm it's landed before
starting (`useServerDetail` hook in lib/server-detail-context.ts).

Run the verification commands at the end. Commit per task in conventional commits style.
Read frontend/AGENTS.md before frontend code.
```
