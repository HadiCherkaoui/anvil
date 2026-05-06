# Anvil — Backups tab (Spec 5)

**Date:** 2026-05-06
**Author:** Hadi (with Claude as scribe)
**Status:** Brainstormed — awaiting user signoff
**Spec series:** 5 of 5. Companions: Spec 1 (bugs & small UX, signed off), Spec 2 (PVC/files/status), Spec 3 (MC version change), Spec 4 (mod deps · per-mod updates · paper plugin pre-select).

---

## 1. Context

User-requested feature: a manual backup/restore tab. Today the panel takes implicit pre-update snapshots inside the modpack orchestrator (`orchestrator.rs:200-229`), but the user has no way to trigger backups on demand, list them, restore from a chosen one, or delete them.

User's framing: "very simple backup restore — backups are just snapshots, data gets saved in db and current server state like config and version, so I can fully restore not only data but also the server's config state, listing of backups, ability to delete them etc."

The orchestrator's existing `build_backup_job` / `build_restore_job` (`backend/src/modpack/jobs.rs:48,96`) do the heavy lifting via tar Jobs over the shared snapshots PVC. This spec wraps them in user-facing endpoints + a tab and adds a new SQLite table to store full config snapshots alongside the tar archives.

---

## 2. Scope

| Item | Action |
|---|---|
| New tab | `backups` between `players` and `files` in `ServerDetailView` |
| Backend | `POST/GET/DELETE` endpoints + restore endpoint, all per-server |
| Backup contents | Tar of data PVC + full SQLite-row config snapshot |
| Restore | Replaces data + DB config + StatefulSet env. Does **not** touch Service, PVC size, or SC. |

**Out of scope:**

- **Pre-restore safety snapshot.** Locked off per user signoff 2026-05-06. User takes a manual backup beforehand if they want a safety net.
- **Retention policies / auto-cleanup.** Explicit user delete only.
- **Global all-server backups page.** Per-server tab only.
- **Restore-to-different-server.**
- **Off-site / encrypted backups.**
- **Service exposure / PVC size / StorageClass revert on restore.** Out for safety reasons (§4.5).
- **Surfacing orchestrator's update-time backups in the user-facing tab.** Different lifecycle (auto-pruned), different intent. Stays separate.

---

## 3. Anti-overengineering guardrails

- **One new SQLite migration** (`backups` table). FK CASCADE handles cleanup on server delete.
- **No new abstractions.** The backup task is one fn in a new `backups.rs` orchestrator module. The restore task is another fn. The delete is a small sibling Job.
- **Two callers for `build_backup_job`** (orchestrator's auto path, this spec's manual path) → parameterize the existing builder, do not duplicate.
- **No new RBAC.** Existing Role grants Job create/delete + PVC mount; same shape as the orchestrator's backup Jobs.
- **`UpdatePhase` enum reused** for backup / restore progress, same as Spec 3. Frontend `UpdateSheet` works unchanged. Backup tasks are short — they use a subset of phases.
- **No new dependencies.**

---

## 4. Design

### 4.1 Storage layout split

The orchestrator's backup Job currently writes to `/snap/mc-{id}/mc-{id}-{ts}.tgz` and prunes to the newest `BACKUP_KEEP_COUNT = 3` (`jobs.rs:39,55-59`). This pruning is correct for ephemeral pre-update snapshots but would silently nuke user-facing backups.

**Action:** introduce subdirs.

| Path | Owner | GC |
|---|---|---|
| `/snap/mc-{id}/auto/{ts}.tgz` | orchestrator update flow | `tail -n +4 \| xargs rm` keeps newest 3 |
| `/snap/mc-{id}/manual/{backup_id}.tgz` | this spec's tab | none — explicit delete |

Modify `build_backup_job` signature to:

```rust
pub fn build_backup_job(
    server_id: &str,
    archive_id: &str,           // <ts> for auto, <backup_id> for manual
    namespace: &str,
    snapshots_pvc: &str,
    subdir: &str,                // "auto" | "manual"
    gc_keep: Option<usize>,      // None = no GC (manual), Some(n) = keep newest n
) -> Job
```

Existing orchestrator call in `orchestrator.rs:203-208` becomes:

```rust
let backup_job = build_backup_job(
    server_id,
    &backup_ts.to_string(),
    &state.mc_namespace,
    snapshots_pvc.as_str(),
    "auto",
    Some(BACKUP_KEEP_COUNT),
);
```

Manual path uses `("manual", None)`.

`build_restore_job` gets a parallel `subdir` parameter to point at the right subdirectory.

### 4.2 Schema — new `backups` table

Migration: `backend/migrations/0008_backups.sql` (after Spec 4's `0007_mod_updates.sql`).

```sql
CREATE TABLE IF NOT EXISTS backups (
    id              TEXT PRIMARY KEY,
    server_id       TEXT NOT NULL,
    name            TEXT,                                 -- optional user label
    created_at      INTEGER NOT NULL,                     -- unix seconds
    snapshot_path   TEXT NOT NULL,                        -- relative path inside snapshots PVC
    -- full config snapshot for restore:
    mc_version      TEXT NOT NULL,
    memory_mi       INTEGER NOT NULL,
    storage_size_gi INTEGER NOT NULL,
    storage_class   TEXT,                                 -- nullable to match servers schema
    exposure_mode   TEXT NOT NULL,
    source_kind     TEXT NOT NULL,
    source_config   TEXT NOT NULL,                        -- full JSON snapshot
    size_bytes      INTEGER,                              -- nullable; written post-Job
    FOREIGN KEY (server_id) REFERENCES servers(id) ON DELETE CASCADE
);
CREATE INDEX idx_backups_server ON backups(server_id, created_at DESC);
```

`id` is generated server-side via `uuid::Uuid::new_v4()` — `uuid = { version = "1.23.1", features = ["v4"] }` is already in `backend/Cargo.toml`. Time-ordering not needed because the index sorts by `created_at`.

### 4.3 Backup task — `backend/src/modpack/backups.rs` (new file)

```rust
pub async fn run_backup(
    state: AppState,
    server_id: String,
    backup_id: String,
    name: Option<String>,
    guard: UpdateGuard,
)
```

Flow:

1. Insert SQLite row with `snapshot_path = format!("manual/{backup_id}.tgz")`, fields read from current `servers` row + `source_config`. (We write the row first so that if the Job fails partway, we know to delete it on cleanup.)
2. Phases — reuse from `orchestrator.rs`:
   - `Announcing` — `announce_and_save` (best-effort RCON broadcast)
   - `Stopping` — acquire `state.snapshot_pvc_lock`, `scale_to(0)`, `wait_pod_gone`
   - `BackingUp` — spawn the parameterized backup Job, `wait_job` with `BACKUP_JOB_TIMEOUT`
   - `Starting` — `scale_to(1)`
   - `Verifying` — `wait_pod_running` + `wait_for_done_marker`
3. On success: emit `Succeeded`, audit `backup_succeeded`. Optionally: write `size_bytes` to the row by spawning a tiny `du -b` Job (deferred — `size_bytes` stays NULL in v1 if too fiddly; UI shows "—").
4. On any failure: emit `Failed`, **delete the SQLite row** (so no orphaned-metadata entry shows in the list), audit `backup_failed`. **No rollback needed** — the Job either wrote the tar or didn't, the StatefulSet wasn't mutated, just scale up to 1 to recover.

### 4.4 Restore task — same file

```rust
pub async fn run_restore(
    state: AppState,
    server_id: String,
    backup_id: String,
    guard: UpdateGuard,
)
```

Flow:

1. Load the `backups` row (404 if missing).
2. Capture **current** SQLite row values for the in-memory rollback path on phase failures (we don't take a fresh snapshot — see §3 — but we do hold the old DB row in memory so we can revert it if env-patch fails before the pod ever reboots).
3. Phases:
   - `Announcing` — best-effort
   - `Stopping` — acquire snapshot lock, scale-to-0, wait
   - `Restoring` — spawn restore Job pointing at `manual/{backup_id}.tgz`. **Add this variant to `UpdatePhase`** in both backend (`backend/src/modpack/orchestrator.rs` enum, serialised as `"restoring"`) and frontend (`frontend/app/lib/update-stream.ts:10-22` strict zod enum needs `"restoring"` appended). Also add it to the `ORDER` array in `UpdateSheet.tsx:10` so the visual progress shows it.
   - `Swapping` — UPDATE `servers` SET (`mc_version`, `memory_mi`, `source_kind`, `source_config`) = backup row values. Rebuild env via the runtime's existing builder (same shape as Spec 3 §4.2). `patch_statefulset_env(...)`.
   - `Starting` — scale-to-1
   - `Verifying` — wait pod, wait for done marker
4. On success: `Succeeded`, audit `restore_succeeded`.
5. On failure between phases 4 and 6: best-effort revert the SQLite row to the in-memory captured values + revert env. Audit `restore_failed`. **Data PVC is NOT reverted** — it's already partially or fully restored from the backup; reverting would require yet another backup. Document: "if restore fails, server may be in mixed state. take another backup beforehand or restore from a different one."
6. On failure during phase 3 (restore Job itself fails): no DB or env changes happened. Audit + scale to 1 to recover.

### 4.5 Restore scope — what reverts, what doesn't

| Field | Reverts on restore? | Why |
|---|---|---|
| Data PVC contents (`/data`) | Yes | Core purpose. |
| SQLite `mc_version` | Yes | Needed for env. |
| SQLite `memory_mi` | Yes | Needed for env. |
| SQLite `source_kind` + `source_config` | Yes | Needed for env (mods/plugins/loader/runtime). |
| StatefulSet env | Yes | Rebuilt from above. |
| `storage_size_gi` | **No** | PVC can't shrink. UI shows note in restore modal if backup size > current size. (Backup size <= current is fine — the data fits.) |
| `exposure_mode` / Service type | **No** | Could nuke a LoadBalancer IP people are connected to. User adjusts post-restore via Settings if intentional. |
| `storage_class` | **No** | Can't change SC of an existing PVC. |

The restore-confirmation modal lists what gets reverted and what doesn't, so there's no surprise.

### 4.6 Endpoints

#### 4.6.1 `POST /api/servers/:id/backups`

Body: `{ "name": "string?" }`. Validation: `name` ≤ 64 chars, no newlines.

Behaviour:

1. 404 if server doesn't exist.
2. 503 `snapshots_unavailable` if `snapshots_pvc` not configured.
3. 409 `update_in_progress` if `UpdateGuard` can't be acquired.
4. Generate `backup_id`. Spawn `run_backup`. Return 202 `{ "status": "started", "backup_id": "..." }`.

#### 4.6.2 `GET /api/servers/:id/backups`

Returns `Vec<BackupListItem>` sorted DESC by `created_at`:

```json
{ "id": "...", "name": "pre-1.21", "created_at": 1715000000, "mc_version": "1.21.4", "size_bytes": 412345678 }
```

#### 4.6.3 `POST /api/servers/:id/backups/:backup_id/restore`

Validation:

- 404 if backup or server missing.
- 503 `snapshots_unavailable`.
- 409 `update_in_progress`.

Returns 202 `{ "status": "started" }`. Spawns `run_restore`. The existing `/api/servers/:id/update/stream` WS surfaces phases.

#### 4.6.4 `DELETE /api/servers/:id/backups/:backup_id`

Spawns a tiny one-off Job (named `backup-delete-{backup_id}`) that mounts the snapshots PVC and runs:

```sh
rm -f /snap/mc-{id}/manual/{backup_id}.tgz
```

Returns 202 `{ "status": "started" }`. On Job success, delete the SQLite row. On Job failure, log + leave the row (idempotent: subsequent delete attempts work). Use `busybox:1.36` like the existing Jobs.

If the user is OK with synchronous delete: instead, run `rm` in a Job and `wait_job` — return 200 only when complete. Simpler UX (delete is instant for the user). I'll spec the synchronous variant since the file is small and `rm` over a mounted PVC takes <1s.

#### 4.6.5 Server delete cascade

Modify `DELETE /api/servers/:id` (in existing `routes/servers/delete.rs`):

After the existing per-resource cleanups, schedule one `backup-cleanup-{id}` Job that runs `rm -rf /snap/mc-{id}/manual/`. SQLite cascades the rows via FK. The Job is fire-and-forget — server delete returns immediately; if the Job fails, the orphan dir is small (just tarballs) and will be cleaned up on the next manual cleanup pass (future work) or noticed during PVC inspection.

Audit: `backup_dir_cleanup_scheduled` on server delete.

### 4.7 Frontend

#### 4.7.1 New tab in `ServerDetailView.tsx`

```tsx
{ id: "backups", label: "backups", href: tabHref("backups") },
```

Insert between `players` and `files`. Add `"backups"` to `TabId` union.

#### 4.7.2 `BackupsBody.tsx` (new)

```
─── backups ─────────────────────────────────
  3 backups · 1.2 GB                  [+ create backup]

  name           created             mc        size      
  pre-1.21       2026-05-06 14:30    1.21.4    412 MB    [restore] [delete]
  weekly         2026-05-04 09:00    1.21.4    389 MB    [restore] [delete]
  (unnamed)      2026-04-28 19:11    1.21.3    378 MB    [restore] [delete]
```

- Total bytes computed client-side from the list.
- `(unnamed)` rendered when `name === null`.
- Size column shows `—` when `size_bytes === null` (backup ran without size capture).

**Create modal** — single optional input `name`, primary action "create backup". On submit: POST → opens `UpdateSheet` (Spec 3's pattern).

**Restore confirmation modal:**

```
restore "pre-1.21"?

this will:
  · stop the server
  · replace world data and config with the snapshot
  · restart on mc 1.21.4 (current: 1.21.5)
  · keep current storage size (50 Gi) and address

if restore fails, server may end in a mixed state.
take another backup first if you want a safety net.

[cancel]                          [restore]
```

Renders mismatches between current and backup config (mc_version, memory, storage size). Submit → POST → `UpdateSheet`.

**Delete confirmation modal** — simple "delete forever?" prompt. Submit → DELETE → list refresh.

#### 4.7.3 API client + Zod

`frontend/app/lib/api.ts`:

```ts
export interface Backup {
  id: string;
  name: string | null;
  created_at: number;
  mc_version: string;
  size_bytes: number | null;
}

export const fetchBackups = (id: string, signal?: AbortSignal): Promise<Backup[]>;
export const createBackup = (id: string, name?: string): Promise<{ status: string; backup_id: string }>;
export const restoreBackup = (id: string, backupId: string): Promise<{ status: string }>;
export const deleteBackup = (id: string, backupId: string): Promise<void>;
```

with Zod schemas.

---

## 5. Data flow / deployment

- One new SQLite migration (`backups`).
- One new backend module (`modpack/backups.rs`).
- Two new frontend files (`BackupsBody.tsx`, modal additions).
- No Helm / RBAC changes (existing Role covers Job + PVC + Pod).
- New `UpdatePhase::Restoring` variant added to backend enum + frontend zod schema + `UpdateSheet`'s `ORDER` array (per §4.4).

---

## 6. Error handling

| Path | Failure | Behaviour |
|---|---|---|
| Backup row insert | SQLite error | 500, no Job spawned. |
| Backup Job | Job fails | Delete SQLite row. Scale to 1. Audit. Surface in `UpdateSheet` as Failed. |
| Restore — phase 3 (Job) | Job fails | No DB / env change yet. Scale to 1. Audit. |
| Restore — phase 4 (Swap) | env patch fails after DB write | Revert DB row from in-memory snapshot. Try env patch again with original env. Audit. |
| Restore — phase 5 (Start) | scale-to-1 fails | Try once more. If still fails, audit + leave for manual intervention. (Same as orchestrator's update flow.) |
| Restore — phase 6 (Verify) | wait timeout | Document the mixed state in audit; leave server stopped. User picks another backup or manually fixes. |
| Delete | Job fails | Log. Leave row. Subsequent delete attempts work (idempotent `rm -f`). |
| Server delete cascade | Cleanup Job fails | Log. Tarballs orphan in PVC. Cleanup left for future work. |

---

## 7. Testing

| Area | Test |
|---|---|
| Schema migration | Reuses `sqlx migrate` infra, no extra test |
| `build_backup_job` parameterization | Existing orchestrator test still passes; new test for `subdir = "manual"` + `gc_keep = None` (asserts no GC line in cmd) |
| Backup task | Backend integration: POST → Job runs → row written, snapshot_path matches |
| Restore task | Backend integration: take backup, mutate server (memory + mc_version), restore → assert SQLite reverted, env patched |
| Restore — preserved fields | Backend integration: assert `storage_size_gi` and `exposure_mode` are NOT reverted |
| Delete | Backend integration: backup → delete → row gone, file gone |
| Server delete cascade | Backend integration: server with 2 backups → DELETE server → backup rows + dir gone |
| FE | Manual repro |

---

## 8. Open questions

None. Locked:

1. No pre-restore safety snapshot.
2. No retention / auto-cleanup.
3. Per-server tab, no global view.
4. Restore reverts data + env + DB config; **not** Service / SC / size.
5. Synchronous delete (one Job, wait, return).
6. Manual subdir separated from auto.

---

## 9. Future work

- **Surfacing orchestrator auto-backups in the tab** — different lifecycle, different intent, but might be worth a "system snapshots" section if the user wants visibility.
- **`size_bytes` capture** — adding a `du -b` step to the backup Job that writes the result to a sidecar text file the backend reads and UPSERTs into the row.
- **Retention / auto-cleanup** if the snapshots PVC starts filling up.
- **Restore-to-different-server** — easy infra (just re-target the StatefulSet name in env restore + Job target), useful for "clone server from backup" workflows.
- **Off-site / encrypted backups** — periodic rclone-to-S3 Job. Big design.

---

## 10. Implementation prompt

Generated by writing-plans skill in the next workflow step (after all five specs are signed off).
