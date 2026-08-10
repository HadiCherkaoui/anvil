<!--
SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

# Anvil — PVC resize · file-helper kill · status nuance (Spec 2)

**Date:** 2026-05-06
**Author:** Hadi (with Claude as scribe)
**Status:** Brainstormed — awaiting user signoff
**Spec series:** 2 of 4. Companion specs: Spec 1 (bugs & small UX, signed off), Spec 3 (MC version change), Spec 4 (mod deps · per-mod updates · paper plugin pre-select).

---

## 1. Context

Three independent items from the 2026-05-06 triage that don't share much surface area beyond "settings tab needs more controls and the status state machine has a hole." Bundled because each is small.

- **D#8** — User wants to grow PVCs after creation. ZFS-CSI supports online expansion. No way to do it from the panel today.
- **D#15** — User wants a manual kill switch for the file-helper Pod when the server is stopped but the helper is still up. Today the helper auto-tears-down only on server start; no UI button to kill it independently.
- **D#14** — Restart-loop pods sometimes show "starting" instead of "error" in the UI because `derive_status` only fires Error on specific waiting reasons (`CrashLoopBackOff`, etc.). There's a window between a crash and `CrashLoopBackOff` kicking in where status flickers Starting.

---

## 2. Scope

| # | Item | Type |
|---|---|---|
| D#8 | PVC resize (grow only) | feature |
| D#15 | File-helper kill button | feature |
| D#14 | Restart-loop visible as Error | bug |

**Out of scope:**

- PVC shrink — rare, ZFS-CSI may not support, surprise factor. Defer until requested.
- Resize progress streaming (`FileSystemResizePending` condition surfacing) — fire-and-forget for v1; ZFS expands quickly, the next detail fetch picks up the new size.
- Distinct "Restarting" status. Reporting Error during a known-crashed restart is enough; user explicitly asked for "state for error", not a third state.
- File-helper auto-suspend by idle timeout. Manual kill is what the user asked for.

---

## 3. Anti-overengineering guardrails

- **No abstractions.** Three independent endpoints, three small frontend additions, one rule added to the existing `pod_in_error_state` function. No state machine refactor. No "expansion driver" trait.
- **No new SQLite migrations.** `storage_size_gi` already exists in the `servers` table.
- **No new top-level deps.** Everything reuses `kube-rs`, `axum`, the existing `ClusterCapabilities` cache.
- **No new RBAC.** Verified against `deploy/templates/role.yaml`:
  - PVC: `patch` already in the verb list (`role.yaml:20-21`).
  - Pod: `delete` already in the verb list (`role.yaml:26-27`).
  - StorageClass: `get` already in the cluster-role (`cluster-role.yaml:16-17`).
- **Aggressive restart-error rule** (`restart_count > 0 && !ready → Error`) over conservative (`>= 2`). Surface trouble fast. The user said "keeps restart looping" — single-restart blips are still informative noise that the user can dismiss visually once Ready returns.

---

## 4. Per-item design

### 4.1 D#8 — PVC resize (grow only)

#### 4.1.1 Capabilities — per-SC expansion flag

`ClusterCapabilities` (`backend/src/routes/cluster.rs:30`) currently has `available_storage_classes: Vec<String>`. Add a sibling field:

```rust
pub expandable_storage_classes: Vec<String>,
```

Names of `StorageClass`es where `.allow_volume_expansion == Some(true)`. Computed in the same loop that already iterates SCs (`cluster.rs:67-87`):

```rust
let allow_expand = sc.allow_volume_expansion.unwrap_or(false);
if allow_expand {
    expandable.push(name.clone());
}
```

The cluster-role already grants `get/list/watch` on `storageclasses` — no RBAC change.

The cluster's `tank` SC (per `docs/cluster-profile.md`, `zfs.csi.openebs.io`) supports expansion when `allowVolumeExpansion: true` is set on the SC manifest. If the user's `tank` SC doesn't have this set, expansion will not be offered until the SC is updated. That's a cluster-config concern, not a panel concern; the panel just reflects truth.

Frontend Zod schema (`frontend/app/lib/api.ts`) extends `clusterCapabilitiesSchema` with `expandable_storage_classes: z.array(z.string())`.

#### 4.1.2 Endpoint — `PATCH /api/servers/:id/storage`

**Route:** `backend/src/routes/servers/storage.rs` (new file). Wire in `routes/servers/mod.rs`.

**Request body:**

```rust
#[derive(Debug, Deserialize)]
pub struct ResizeRequest {
    pub size_gi: u32,
}
```

**Validation:**

| Condition | Response |
|---|---|
| `size_gi <= current_size_gi` | 400 `shrink_unsupported` |
| Server's SC not in `expandable_storage_classes` | 409 `expansion_unsupported` |
| Server doesn't exist | 404 `server_not_found` |

**Implementation:**

1. Load `servers` row → get `storage_class` (or fall back to default SC) and `storage_size_gi`.
2. Validate (above).
3. Build a Strategic Merge patch:
   ```json
   { "spec": { "resources": { "requests": { "storage": "<new>Gi" } } } }
   ```
4. PATCH the PVC `data-mc-{id}-0` via `Api::<PersistentVolumeClaim>::namespaced(namespace).patch(...)`.
5. On success, update SQLite `storage_size_gi`.
6. Return 200 with `{ "size_gi": <new> }`. The actual filesystem resize is async (kube + CSI); next detail fetch reflects the new requested size.

**Online vs offline:** ZFS-CSI does online expansion when the PVC is mounted, otherwise expands on next mount. Either way, the panel doesn't need to gate on server status. Allow resize whether running or stopped.

#### 4.1.3 Frontend — storage section in Settings

**File:** `frontend/app/servers/tabs/SettingsBody.tsx`

New "storage" section, hidden when:

```ts
caps.expandable_storage_classes.includes(detail.storage_class) === false
```

(or when `caps` is unavailable, since rendering nothing is safer than rendering a broken control).

Visible content:

```tsx
<Card header="storage">
  <p>current: {detail.storage_size_gi} Gi</p>
  <RangeSlider
    min={detail.storage_size_gi + 1}
    max={detail.storage_size_gi * 4}  // arbitrary cap; user can edit input directly
    value={pendingSize}
    onChange={setPendingSize}
  />
  <Button onClick={onExpand} disabled={pendingSize === detail.storage_size_gi}>
    expand to {pendingSize} Gi
  </Button>
  <p className="text-text-faint">
    grow only · shrink not supported
  </p>
</Card>
```

On submit: PATCH → `refresh()` from §4.2 of Spec 1's refreshable context → toast `"resize requested"`. The displayed size updates on next detail fetch.

#### 4.1.4 Tests

- Backend integration: `backend/tests/storage_resize.rs` — create test server, PATCH storage with larger size → fetch PVC, assert `spec.resources.requests.storage` updated; assert SQLite row updated.
- Backend unit: validation rejects shrink (400), rejects unsupported SC (409).
- Backend unit: `cluster.rs` — fixture SC list with mixed `allow_volume_expansion`, assert correct `expandable_storage_classes` output.

---

### 4.2 D#15 — file-helper kill button

#### 4.2.1 Backend — endpoint + status surfacing

**Route:** `DELETE /api/servers/:id/files/helper` (new in `backend/src/routes/servers/files.rs` if it exists, else new file).

**Implementation:** call existing `files_helper::tear_down_helper(state, server_id).await`. The helper's `tear_down_helper` (`files_helper.rs:260`) already deletes by name and tolerates absence — wrap the result:

| Outcome | Response |
|---|---|
| Pod existed and deleted | 204 No Content |
| Pod absent (already gone) | 200 `{ "already_gone": true }` |
| Pod present, server is running | 409 `helper_unsafe_to_kill` |

(The third row guards against the user accidentally killing the helper while files are mid-write from the running server. The helper auto-tears-down on server start, so this is a defensive check; in normal flows the user only sees this button when stopped.)

**Status field:** extend `ServerDetail` with `files_helper_running: bool`.

`backend/src/routes/servers/get.rs` is the detail handler. Add a single `Api::<Pod>::namespaced(...).get_opt("mc-{id}-files")` call. Computation:

```rust
let helper_running = pod_api.get_opt(&format!("mc-{id}-files")).await
    .ok().flatten()
    .map(|p| p.metadata.deletion_timestamp.is_none())
    .unwrap_or(false);
```

(No I/O cost beyond a single get; runs in parallel with the existing concurrent fetches per the recent `perf(logs-stream)` commit pattern.)

Zod schema in `api.ts` extends `serverDetailSchema` with `files_helper_running: z.boolean()`.

#### 4.2.2 Frontend — kill button

**File:** `frontend/app/servers/tabs/FilesBody.tsx`

When `detail.status === "stopped" && detail.files_helper_running`, render a control row above the file list:

```tsx
<div className="flex items-center justify-between border-b border-border px-3 py-2">
  <span className="font-mono text-[12px] text-text-faint">
    file viewer is running · idle
  </span>
  <Button onClick={onKillHelper} variant="danger">
    stop file viewer
  </Button>
</div>
```

`onKillHelper` calls `DELETE /api/servers/:id/files/helper`, then `refresh()`, then toast.

#### 4.2.3 Tests

- Backend integration: spawn a server, ensure helper, DELETE → assert pod gone.
- Backend integration: DELETE without helper → 200 `already_gone`.
- Backend integration: server running + helper present (mocked) → 409 `helper_unsafe_to_kill`.

---

### 4.3 D#14 — restart-loop visible as Error

**File:** `backend/src/k8s_status.rs:62-76`

Today's `pod_in_error_state` only checks waiting reason ∈ `ERROR_REASONS`. Replace with a two-clause OR:

```rust
fn pod_in_error_state(pod: &Pod) -> bool {
    let Some(status) = pod.status.as_ref() else {
        return false;
    };
    let Some(statuses) = status.container_statuses.as_ref() else {
        return false;
    };
    statuses.iter().any(|cs| {
        let waiting_error = cs
            .state
            .as_ref()
            .and_then(|st| st.waiting.as_ref())
            .and_then(|w| w.reason.as_deref())
            .is_some_and(|r| ERROR_REASONS.contains(&r));
        let restarted_unready = cs.restart_count > 0 && !cs.ready;
        waiting_error || restarted_unready
    })
}
```

#### 4.3.1 Tests

Extend the existing test module (`k8s_status.rs:133-297`):

| Test | Pod state | Expected |
|---|---|---|
| `replicas_one_unready_no_pod_is_starting` (regression) | `restart_count=0`, no waiting reason, `ready=false` | Starting |
| `replicas_one_pod_pending_is_starting` (regression) | `restart_count=0`, waiting `PodInitializing`, `ready=false` | Starting |
| `replicas_one_pod_crashloop_is_error` (regression) | waiting `CrashLoopBackOff`, `ready=false` | Error |
| `replicas_one_restart_count_unready_is_error` (NEW) | `restart_count=1`, no waiting reason, `ready=false` | Error |
| `replicas_one_restart_count_ready_is_running` (NEW) | `restart_count=2`, `ready=true` | Running (uses `ready_replicas=1` short-circuit) |
| `replicas_one_restart_count_pending_is_error` (NEW) | `restart_count=1`, waiting `PodInitializing`, `ready=false` | Error (restart-count rule fires) |

Add `make_pod_with_restart_count` helper alongside the existing `make_pod_with_waiting`.

---

## 5. Data flow / deployment

- New endpoints; no Helm-template changes; no SQLite migrations; no RBAC changes (verified above).
- New `ClusterCapabilities` field is additive — existing frontend code that ignores unknown fields keeps working until updated.

---

## 6. Error handling

| Path | Failure | Behaviour |
|---|---|---|
| PATCH storage | PVC patch returns 422 (immutable / invalid value) | Return 500 `pvc_patch_failed` with kube error in `details`. SQLite **not** updated. |
| PATCH storage | SQLite update fails after PVC patch succeeded | Log + audit entry "PVC resized but DB sync failed"; return 500. The PVC has the new size; next detail fetch reads the live PVC and updates in-memory. (We do not roll back the PVC since shrink is unsafe.) |
| DELETE helper | Server transitioned to running between detail fetch and kill | The 409 `helper_unsafe_to_kill` guard catches this. Frontend shows the error and refreshes. |
| Status | Pod has no `containerStatuses` yet | Both clauses default to false → fall through to Starting. Existing behaviour preserved. |

---

## 7. Testing

| Item | Test type |
|---|---|
| §4.1 PVC resize | Backend integration + unit (validation, capability) |
| §4.2 Helper kill | Backend integration |
| §4.2 ServerDetail field | Backend integration |
| §4.3 Status nuance | Backend unit (extended existing module) |
| All FE | Manual repro (no FE test runner) |

---

## 8. Open questions

None. Aggressive restart rule locked. Per-SC expansion flag locked. Helper kill 409-when-running guard locked.

---

## 9. Future work

- Surface PVC resize progress (`PersistentVolumeClaim.status.conditions[FileSystemResizePending]`) — only if the user reports it as missing.
- File-helper auto-suspend by idle timeout — if the user finds themselves frequently killing the helper manually.
- Distinct "Restarting" status with a different visual chip — only if Error feels too noisy in practice.

---

## 10. Implementation prompt

Generated by writing-plans skill in the next workflow step (after all four specs are signed off).
