<!--
SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

# Anvil — MC version change for non-modpack servers (Spec 3)

**Date:** 2026-05-06
**Author:** Hadi (with Claude as scribe)
**Status:** Brainstormed — awaiting user signoff
**Spec series:** 3 of 4. Companions: Spec 1 (bugs & small UX, signed off), Spec 2 (PVC/files/status), Spec 4 (mod deps · per-mod updates · paper plugin pre-select).

---

## 1. Context

Item D#4 from the 2026-05-06 triage: vanilla, paper, and modded servers can't change MC version after creation. The Settings tab currently shows the placeholder "version changes ship with sub-project B (modpack runtime registry)." (`SettingsBody.tsx:112-115`) — Spec 1's loader-versions endpoint provides the runtime registry, this spec wires it through.

Modpack servers are excluded — their version comes from the pack and updates via the existing modpack orchestrator.

The existing modpack orchestrator (`backend/src/modpack/orchestrator.rs`) already implements a robust FSM for this kind of change:

```
announce → stop → backup → swap → start → verify → (rollback on failure)
```

with the snapshots PVC, the per-server `UpdateGuard`, the WS phase bus, and the `/api/servers/{id}/update/stream` consumer (the existing `UpdateSheet` component on the frontend). **Reuse this.** The naive "patch env + delete pod" approach loses the snapshot safety the user asked about and re-implements a worse version of what's already shipping.

---

## 2. Scope

| Item | Action |
|---|---|
| D#4 | Version change for vanilla / paper / modded — orchestrated, with snapshot + rollback |

**Out of scope:**

- **Modpack version change.** Already handled by the existing flow.
- **Mod / plugin compatibility hints in the picker.** Deferred to Spec 4 (per-mod compatibility lookup is its own design — needs Modrinth/CF queries and UI).
- **Auto-update mods to compatible versions.** Spec 4.
- **Paper build picker.** itzg's `PAPER_BUILD` defaults to `LATEST` for the chosen MC version, which has been fine. Add a picker only if the user reports it as missing.
- **MC version downgrade safety net beyond the snapshot.** The snapshot covers the data PVC; world-format incompatibility on a downgrade still fails at boot, the orchestrator detects via `wait_for_done_marker` timeout, and rolls back. No additional downgrade-specific logic.

---

## 3. Anti-overengineering guardrails

- **No new shared FSM abstraction.** Two callers (modpack update, version change) is not three. The new `version_change::run` is a sibling of `orchestrator::run` — copy-paste of the FSM shape with a different swap and persist step. Premature de-duplication makes both harder to read.
- **No Paper build picker** (out of scope above).
- **No new RBAC.** Verified: PVC, Pod, StatefulSet, Job verbs already cover what's needed.
- **No new SQLite migration.** `mc_version` already exists; `loader_version` rides in JSON `source_config` per Spec 1 §5.2.2.
- **No new frontend tab.** Uses the existing Settings card layout and the existing `UpdateSheet` component for progress.
- **Snapshot is mandatory.** Cluster has `snapshots_pvc` configured (it's required for modpack updates today). If somehow not configured, version change fails the same way modpack updates fail — with a clear error. No "best effort" fallback.

---

## 4. Design

### 4.1 Backend FSM — `modpack/version_change.rs` (new file)

Mirrors the modpack orchestrator's structure. Public entry point:

```rust
pub async fn run(
    state: AppState,
    server_id: String,
    new_mc: String,
    new_loader: Option<String>,
    guard: UpdateGuard,
)
```

Phases (reusing existing `UpdatePhase` enum so the FE's `UpdateSheet` works unchanged):

| Phase | Reuses | Notes |
|---|---|---|
| 1 — Announce | `announce_and_save` | Best-effort RCON broadcast + `save-all`. |
| 2 — Stop | `scale_to(0)` + `wait_pod_gone` | Acquires `state.snapshot_pvc_lock` first. |
| 3 — Backup | `build_backup_job` + `spawn_job` + `wait_job` | Tar of the data PVC into the snapshots PVC. Same Job builder, same timeout (`BACKUP_JOB_TIMEOUT = 10min`). |
| 4 — Swap | new `version_change_swap` | Update SQLite (`mc_version`, `loader_version` for modded) → rebuild env via the runtime's existing builder → `k8s_patches::patch_statefulset_env(...)`. |
| 5 — Start | `scale_to(1)` | Releases snapshot lock first. |
| 6 — Verify | `wait_pod_running` + `wait_for_done_marker` | Same boot-readiness wait the orchestrator uses. Per-runtime `boot_timeout()` — same ones the existing providers expose. |
| 7 — Persist | new `persist_new_mc_version` | Update `last_started_at`, audit log entries (`version_change_started`, `version_change_succeeded`). |

**Failure & rollback** — on any phase 2–6 error, run rollback exactly like `orchestrator::run`:

```rust
guard.emit(UpdatePhase::RollingBack);
match version_change_rollback(&state, &server_id, &old_env, &guard).await { ... }
```

`version_change_rollback` does:

1. Re-acquire `snapshot_pvc_lock`.
2. Spawn restore Job from the just-taken snapshot — reuse `build_restore_job`.
3. Wait for the Job to succeed (`RESTORE_JOB_TIMEOUT = 10min`).
4. Patch the StatefulSet env back to `old_env` via `patch_statefulset_env`.
5. Revert SQLite `mc_version` (and `loader_version`).
6. `scale_to(1)` to restart on the old version.
7. Emit `RolledBack`.

**Capturing `old_env`:** read the StatefulSet's current `mc` container env before phase 4. The phase-2 stop only cleared replicas; the StatefulSet (and its env) is intact.

**Helper visibility:** all `pub(crate) fn` helpers in `orchestrator.rs` (`scale_to`, `wait_pod_gone`, `wait_pod_running`, `spawn_job`, `wait_job`, `wait_for_done_marker`, `announce_and_save`, `fetch_memory_mi`, `fetch_source`) are already crate-public — `version_change.rs` imports them as a sibling module. The `rollback` function in `orchestrator.rs` stays modpack-specific (it touches `modpack_versions` table and `current_version_id`); `version_change_rollback` is a separate fn that omits those bits.

**Endpoint:** `PATCH /api/servers/:id/version` — body `{ "mc_version": "1.21.5", "loader_version": "21.5.10"? }`.

| Validation | Response |
|---|---|
| Server is `source_kind ∈ {curseforge, modrinth}` | 409 `version_change_unsupported` |
| Server is `source_kind == "modded"` and `runtime ∈ {forge, neoforge}` and `loader_version` missing | 400 `loader_version_required` |
| `mc_version` empty or fails `validation::validate_mc_version` | 400 `invalid_mc_version` |
| `mc_version == current_mc_version` and `loader_version == current_loader_version` | 400 `nothing_to_change` |
| Server has an in-flight update or apply | 409 `update_in_progress` |
| `snapshots_pvc` not configured | 503 `snapshots_unavailable` |

On success: acquire `UpdateGuard`, spawn `version_change::run`, return 202 `{ "status": "started", "server_id": ... }`. Same response shape as the existing modpack update route so the FE polling/streaming code is uniform.

**WS stream:** the existing `/api/servers/:id/update/stream` consumer (used by modpack updates and mod-apply) handles version-change phases too. No new endpoint.

### 4.2 Per-runtime swap detail

`version_change_swap` rebuilds the env for the chosen runtime with the new MC + loader. The per-runtime `extra_env()` and `build_env()` already take the relevant config; we construct a new config with the new values and call them.

**Vanilla:**

- Update SQLite `mc_version`.
- Build `VanillaConfig { mc_version: new_mc, ... }`.
- New env from `extra_env()` + memory env.

**Paper:**

- Same as vanilla. Paper uses `VERSION=<mc>` and itzg picks `PAPER_BUILD=LATEST` automatically.

**Modded:**

- Update SQLite `mc_version`. Update `loader_version` inside `source_config` JSON.
- Build `ModdedConfig { mc_version: new_mc, loader_version: new_loader, ... }`.
- Per Spec 1 §5.2.2, `extra_env()` emits `FORGE_VERSION` / `NEOFORGE_VERSION` based on `loader_version`. Fabric ignores the loader value.

The shared building block is the small fn:

```rust
fn build_runtime_env(
    source_kind: &str,
    source_config: &str,
    new_mc: &str,
    new_loader: Option<&str>,
    memory_mi: u32,
) -> Result<Vec<EnvVar>>
```

which dispatches on `source_kind` and calls the runtime's existing builder. Lives in `version_change.rs` since it has only one caller.

### 4.3 Frontend

#### 4.3.1 Settings tab — version card

**File:** `frontend/app/servers/tabs/SettingsBody.tsx`

Add a "version" card after the existing memory card. Hide when `detail.source_kind ∈ {curseforge, modrinth}`.

```
version
  mc           1.21.4    [edit]
  neoforge     21.4.81
```

(loader row only renders for modded with forge/neoforge.)

`[edit]` button opens a `<Sheet>`:

```
─── change mc version ──────────────────────────
  mc version       [picker]
  loader version   [picker]   (modded only)

  ⚠ this stops the server, snapshots data, swaps
    in the new version, and restarts. world data
    may not migrate cleanly across major versions.
    on failure, the server auto-restores to the
    snapshot.

  [cancel]              [change version]
─────────────────────────────────────────────────
```

The two pickers reuse the cascading-pickers component built in Spec 1 (modded create form). For vanilla / paper, only the MC picker shows — pulled from the existing `useMcVersions` hook.

On submit:

1. Close the picker sheet, open the existing `UpdateSheet` (the modpack-update progress component).
2. POST `PATCH /api/servers/:id/version`.
3. `UpdateSheet` connects to `/api/servers/:id/update/stream` and renders phase progress.
4. On `Succeeded`: success toast, `refresh()`.
5. On `RolledBack`: warning toast "version change failed · rolled back to <old>", `refresh()`.

#### 4.3.2 API client

`frontend/app/lib/api.ts` adds:

```ts
export const changeServerVersion = (
  id: string,
  body: { mc_version: string; loader_version?: string },
): Promise<{ status: string; server_id: string }>;
```

calling `PATCH /api/servers/{id}/version`. Wire response into Zod (`changeVersionResponseSchema`).

---

## 5. Data flow / deployment

- New endpoint, new backend file, no Helm-template changes, no SQLite migration.
- Reuses the existing snapshots PVC dependency. No additional infra.
- WS reuses `/api/servers/{id}/update/stream` — no new WS route.

---

## 6. Error handling

| Path | Failure | Behaviour |
|---|---|---|
| Phase 2 stop | `wait_pod_gone` timeout | Same as modpack: bail, no rollback needed (no swap yet), pod-stuck error in audit. |
| Phase 3 backup | Job fails or times out | Bail, scale back to 1. Phase 5 wouldn't have run; no env swap done; just restore replicas. |
| Phase 4 swap | `patch_statefulset_env` returns 4xx | Bail. SQLite update may have already run — `version_change_rollback` reverts both DB and env. |
| Phase 5 start | `scale_to(1)` errors | Run `version_change_rollback`. |
| Phase 6 verify | `wait_pod_running` or `wait_for_done_marker` timeout (e.g., world-format mismatch on downgrade) | Run `version_change_rollback` — restores PVC + reverts env + restarts on old version. |
| Endpoint validation failures | (table in §4.1) | Synchronous 4xx, no FSM started. |

---

## 7. Testing

| Area | Test |
|---|---|
| Endpoint validation | Backend unit: each row of the validation table → expected status code |
| Happy path | Backend integration: vanilla 1.21.3 → 1.21.4 — assert SQLite mc_version, StatefulSet env, audit log entries `version_change_started` + `version_change_succeeded` |
| Modded happy path | Backend integration: modded fabric 1.21.4 → modded fabric 1.21.5 (no loader change since fabric) |
| Modded with loader | Backend integration: modded neoforge (mc=1.21.4, loader=21.4.81) → (mc=1.21.5, loader=21.5.10). Assert `NEOFORGE_VERSION` env reflects new loader. |
| Rollback | Backend integration with mocked `wait_for_done_marker` timeout → assert PVC restored, env reverted, replicas back to 1 |
| FE | Manual repro |

---

## 8. Open questions

None. Locked:

1. **Auto-restart with snapshot orchestration** — yes (per user response 2026-05-06).
2. **Modal/sheet for the picker** — yes.
3. **Snapshot reuse** — yes, full orchestrator FSM (per user response 2026-05-06).
4. **No mod compat hints** — deferred to Spec 4.
5. **No Paper build picker** — deferred until requested.

---

## 9. Future work

- **Mod/plugin compatibility hints** in the picker sheet — Spec 4 will add a `POST /api/runtimes/compat-check` endpoint and overlay the picker with green/yellow/red dots. Scope kept tight here so version change ships fast.
- **Paper build picker** — symmetric to NeoForge/Forge pickers from Spec 1.
- **Auto-update mods to compatible versions on version change** — non-trivial UX (which version of each mod to pick? what if no compat exists?). Layer once the basic compat hints land.

---

## 10. Implementation prompt

Generated by writing-plans skill in the next workflow step (after all four specs are signed off).
