<!--
SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

# Anvil v2 — File Browser (Sub-project D)

**Date:** 2026-05-03
**Author:** Hadi (with Claude as scribe)
**Status:** Brainstormed — ready for an implementation plan
**Sub-project:** D of {A · Foundation, B · Mod ecosystem, C · Player management, D · File browser}

---

## 1. Context

A's foundation rehaul (M6) shipped on 2026-05-03 and intentionally
left the **Files** tab body as a one-line placeholder
(`frontend/app/servers/tabs/FilesBody.tsx:7`). B and C filled the
**Mods** and **Players** tabs the same day. D is the fourth and
final v2 leg: a working Files surface that lets you browse, upload,
download, mkdir, rename, and delete inside a managed server's `/data`
PVC — without leaving anvil.

CLAUDE.md flagged "file management" as something an external file
manager would handle. That deferral leaked through to
`docs/milestones.md` as the "(deferred) file-browser deep-link per
server" item from M3. The deferred plan is unworkable in practice:
managed PVCs are `ReadWriteOnce` (`zfs.csi.openebs.io`), so an
external file manager cannot mount the volume while the server pod is
running, and the homelab has no such tool deployed anyway. D
supersedes the deep-link plan with an in-anvil FS surface.

D also stays on a path that future work — a structured
`server.properties` editor, a live `mods/` inventory reader,
config-file linting — can plug into for free. The exec primitives
shipping in D are general-purpose; the structured surfaces sit on
top of them as thin handlers that are not in D's scope.

Driving constraint: ONE cluster, ~3 friends, internal use, NOT a
SaaS. Reuse the v2 design tokens, primitives, and patterns from A,
B, and C. Do not pull in new RBAC capabilities (beyond the single
`pods/exec: create` verb), top-level dependencies, or DB migrations.

---

## 2. Scope

**In scope:**

1. **Files tab body** — replaces the placeholder Card. Renders for
   every server kind.
2. **Read ops** — list directory entries, download a file (streamed
   octet-stream). Scope is `/data` and any subpath; symlinks are
   listed but never followed for ops.
3. **Write ops** — upload (raw octet-stream PUT, streamed, 100 MiB
   cap), mkdir, rename, delete (single-file or recursive folder).
4. **Stopped-server support** — a per-server "files-helper" Pod
   (alpine, `sleep infinity`, mounts the existing data PVC) is
   lazy-spawned the first time the Files tab is opened on a stopped
   server, and torn down before Start is allowed to re-attach the
   PVC.
5. **Path validator** — `validate_data_path` in
   `backend/src/validation.rs`. Strict alphabet, segment validation,
   traversal rejection.
6. **`pods/exec` primitives** — `pod_exec_capture`,
   `pod_exec_stream_in`, `pod_exec_stream_out` exposed from
   `backend/src/files_helper.rs`. Sole consumer is D; if a second
   consumer materialises (structured editor) we factor into
   `pod_exec.rs` then.
7. **Helm value addition** — `mc.filesHelperImage` (digest-pinned).
8. **RBAC delta** — one verb (`pods/exec: create`) on the existing
   Role.
9. **Audit log** — every mutating file op writes one row via
   `insert_audit`.
10. **Frontend** — 4 new components, 1 new hook, 1 rewrite of
    `FilesBody`. No new top-level deps.

**Out of scope (explicitly deferred or excluded):**

- **Folder upload/download as tar.** Scope is single-file. Folder
  backups via `kubectl cp` or M5 snapshots PVC.
- **In-browser editor.** Even for text files. The structured
  `server.properties` UI is the better follow-up and uses D's
  primitives.
- **Multi-file upload / selection.** YAGNI for five friends.
- **Helper-Pod idle TTL / janitor.** Stopped helpers cost ~15 MiB and
  are freed on Start. Build a janitor only if it ever bites.
- **Structured `server.properties` / mods-inventory editors.** Their
  own future features that reuse D's primitives.
- **Inline previews** (image thumbs, text peek), **file search**,
  **file sharing** — scope creep.
- **Sub-project A/B/C.** Already shipped.

---

## 3. Anti-overengineering guardrails

- **No background tasks.** Helper Pod has no idle TTL, no janitor,
  no liveness watcher. Lazy-create on Files endpoint hit; tear down
  on Start.
- **No new top-level deps.** `kube`, `axum`, `tokio`, `serde`,
  `bytes`, `futures-util` are already in `backend/Cargo.toml`. No
  new tree library, no file-manager library, no Monaco / CodeMirror.
- **No new RBAC except `pods/exec: create`.** No ClusterRole
  expansion, no new ServiceAccount.
- **No new DB migration.** No tables, no columns. File ops mutate
  the PVC; the audit_log records actions.
- **Helper Pod is a bare Pod, not a Deployment/StatefulSet.** Anvil
  owns the lifecycle; a wrapper controller would only fight us
  during teardown.
- **One validator, one place.** `validate_data_path` is the sole
  path validator. Not ten verb-specific ones.
- **Argv-only execs.** No shell interpolation of paths anywhere. The
  two `sh -c` callsites pass paths as `$1` (positional arg), never
  embedded in the script string.
- **Single-file operations only.** Tar streams, multi-file uploads,
  recursive copy — out of scope.
- **No live notifications.** The list view re-fetches on path change
  and after action success. No file-watch streaming.

---

## 4. Design POV

Reuse A's tokens 1:1. Copper accent only on:

- the `[upload]` and `[+ folder]` button brackets (primary CTAs in
  the toolbar)
- the active row's hover-revealed Dropdown chevron

Mono (`Fira Code`) for filenames, sizes, mtimes; sans (`Fira Sans`)
for empty-state copy and headings. State colours stay state-only —
deletes don't render in `--color-state-error`. The error banner uses
`--color-state-error` only when an action fails. No new colours, no
new fonts, no new radii.

The Files tab is a *workshop tool for an op*: scannable,
keyboard-friendly, no dashboard chrome. Toolbar tight on top —
breadcrumb + actions in one row. List below. Empty states are one
short line of copy.

---

## 5. Data model

**No DB changes.** Wire data is derived from `pods/exec` calls on
each request.

### 5.1 Wire types

```rust
// backend/src/files.rs (parsing types — not persisted)
#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FileEntryType { F, D, L, O }

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub entry_type: FileEntryType,
    pub size: u64,
    pub mtime: i64,
}

#[derive(Debug, Serialize)]
pub struct FileListResponse {
    pub path: String,
    pub entries: Vec<FileEntry>,
}
```

### 5.2 Action body (request side)

```rust
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum FileAction {
    Mkdir  { path: String },
    Rename { from: String, to: String },
    Delete { path: String, recursive: bool },
}
```

### 5.3 Audit log

| Endpoint              | Audit `action`    | `details` JSON                          |
|-----------------------|-------------------|------------------------------------------|
| `PUT  /files`         | `files.upload`    | `{"path":"…","bytes":N}`                 |
| `POST /files/action`  | `files.mkdir`     | `{"path":"…"}`                           |
| `POST /files/action`  | `files.rename`    | `{"from":"…","to":"…"}`                  |
| `POST /files/action`  | `files.delete`    | `{"path":"…","recursive":bool}`          |
| `GET  /files`         | (not audited)     |                                          |
| `GET  /files/raw`     | (not audited)     |                                          |

Read endpoints are not audited — same convention as `/logs` /
`/players` reads.

---

## 6. Backend

### 6.1 New parsing module — `backend/src/files.rs`

Pure functions over exec stdout. No I/O. No `kube`. Easy to
unit-test.

```rust
pub const LIST_SCRIPT: &str = r#"set -e
cd "$1" 2>/dev/null || { echo "ENOTDIR"; exit 1; }
for entry in * .*; do
  [ "$entry" = "." ] && continue
  [ "$entry" = ".." ] && continue
  [ -e "$entry" ] || [ -L "$entry" ] || continue
  if [ -L "$entry" ]; then ty=l
  elif [ -d "$entry" ]; then ty=d
  elif [ -f "$entry" ]; then ty=f
  else ty=o
  fi
  if [ "$ty" = "f" ] || [ "$ty" = "l" ]; then
    sz=$(stat -c '%s' "$entry" 2>/dev/null || echo 0)
  else
    sz=0
  fi
  mt=$(stat -c '%Y' "$entry" 2>/dev/null || echo 0)
  printf '%s\t%s\t%s\t%s\n' "$ty" "$sz" "$mt" "$entry"
done
"#;

pub fn parse_list_output(s: &str) -> Vec<FileEntry>;
pub fn parse_stat_size(s: &str) -> Option<u64>;
pub fn is_enotdir_sentinel(s: &str) -> bool;   // true iff first line == "ENOTDIR"
```

`parse_list_output` skips malformed lines silently (defensive for
busybox quirks). `parse_stat_size` returns `None` on non-numeric
output (file does not exist).

Test cases (in the same file):

| Case                  | Input                                          | Expected                                                  |
|-----------------------|------------------------------------------------|------------------------------------------------------------|
| Empty dir             | `""`                                           | `[]`                                                       |
| One file              | `f\t1234\t1714000000\tlevel.dat\n`             | one entry, name=`"level.dat"`, type=F, size=1234           |
| Hidden                | `d\t0\t1714000000\t.cache\n`                   | one entry, name=`".cache"`, type=D                         |
| Symlink               | `l\t32\t1714000000\told.jar.disabled\n`        | one entry, type=L                                          |
| Spaces in names       | `f\t100\t1714000000\tWorld 2.zip\n`            | one entry, name=`"World 2.zip"`                            |
| Malformed line        | `garbage no tabs\n`                            | `[]` (silently skipped)                                    |
| Mixed valid + garbage | `f\t1\t0\ta\ngarbage\nf\t2\t0\tb\n`            | two entries, names `"a"` and `"b"`                         |
| ENOTDIR sentinel      | `ENOTDIR\n`                                    | `is_enotdir_sentinel(s)` returns `true`                    |

### 6.2 Validation additions — `backend/src/validation.rs`

```rust
pub fn validate_data_path(s: &str) -> Result<&str, AppError>;
// Rules:
// - Empty → treat as "/"
// - Must start with "/"
// - Total length ≤ 4096
// - Split by "/", each segment:
//     - Non-empty (so no "//" allowed)
//     - Not "." or ".."
//     - First byte ≠ "-"
//     - Each byte in 0x20..=0x7E minus 0x27 (') and 0x5C (\)
//     - Length ≤ 255
```

Returns `AppError::BadRequest { code: "path_invalid", message }`
with one of: `path_too_long`, `segment_empty`,
`segment_traversal`, `segment_dot`, `segment_leading_dash`,
`segment_too_long`, `segment_invalid_char`.

Unit tests cover every rejection branch + valid hidden-file paths +
valid spaces-in-names paths.

### 6.3 `pod_exec` primitives — `backend/src/files_helper.rs`

```rust
pub struct PodExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

/// Run a command in the named pod, capture stdout/stderr/exit.
pub async fn pod_exec_capture(
    state: &AppState,
    namespace: &str,
    pod_name: &str,
    cmd: &[&str],
) -> Result<PodExecResult, AppError>;

/// Run a command, stream the request body into stdin, return bytes
/// uploaded. Aborts mid-stream and returns `payload_too_large` if
/// exceeded.
pub async fn pod_exec_stream_in<S>(
    state: &AppState,
    namespace: &str,
    pod_name: &str,
    cmd: &[&str],
    body: S,
    cap_bytes: u64,
) -> Result<u64, AppError>
where
    S: Stream<Item = Result<Bytes, axum::Error>> + Send + Unpin + 'static;

/// Run a command, return stdout as an owned async stream the caller
/// can pipe straight into `axum::body::Body::from_stream`.
pub async fn pod_exec_stream_out(
    state: &AppState,
    namespace: &str,
    pod_name: &str,
    cmd: &[&str],
) -> Result<
    impl Stream<Item = Result<Bytes, std::io::Error>> + Send,
    AppError
>;
```

All three: 5 s end-to-end timeout for `_capture`; 60 s read-idle
timeout for the streaming variants; no overall cap on streaming
duration (anvil keeps the connection open as long as the user is
downloading or uploading).

### 6.4 Helper-Pod lifecycle — `backend/src/files_helper.rs`

```rust
/// Returns the pod name to exec into.
/// - Running server → "mc-{id}-0"
/// - Stopped server → ensures helper Pod, returns "mc-{id}-files"
/// - Never-started → Err(AppError::Conflict("pvc_not_initialized"))
pub async fn target_pod_for_files(
    state: &AppState,
    server_id: &str,
) -> Result<String, AppError>;

pub async fn ensure_helper(state: &AppState, server_id: &str)
    -> Result<(), AppError>;

pub async fn tear_down_helper(state: &AppState, server_id: &str)
    -> Result<(), AppError>;
```

`ensure_helper`:

1. `get_pod("mc-{id}-files")` — if Running, return.
2. Otherwise `Api::create(build_files_helper_pod(...))`.
3. On `409 AlreadyExists`, treat as success, proceed.
4. `wait_pod_running("mc-{id}-files", timeout=30s)`.
5. On "PVC not bound" / "PVC not found" k8s errors, return
   `AppError::Conflict { code: "pvc_not_initialized" }`.

`tear_down_helper`:

1. `Api::delete("mc-{id}-files")` — best-effort; 404 is fine.
2. `wait_pod_gone("mc-{id}-files", timeout=30s)`.
3. Returns `Ok` even if no helper existed.

### 6.5 Helper-Pod builder — `backend/src/k8s_builders.rs`

```rust
pub fn build_files_helper_pod(
    id: &str,
    namespace: &str,
    image: &str,
) -> Pod;
```

Shape:

- Name: `mc-{id}-files`
- Labels: managed-by=anvil, server={id}, role=files-helper
- Container: `image`, command=`["sleep","infinity"]`, working_dir=`/data`
- VolumeMounts: `data` at `/data`
- Volumes: `data` referencing the existing PVC `data-mc-{id}-0` by
  `claimName` (does **not** create a new VolumeClaimTemplate)
- Resources: limits `cpu=100m`, `memory=32Mi`; no requests

Tests:

- Pod name = `mc-{id}-files`
- Volume references existing PVC `data-mc-{id}-0`
- Container command = `["sleep","infinity"]`
- Resources: cpu 100m / memory 32Mi limits
- Labels include the standard managed-by + server set, plus
  `role=files-helper`

### 6.6 New route module — `backend/src/routes/servers/files.rs`

Four handlers. Each:

1. Look up server by id (existing helper).
2. `target_pod_for_files(state, &id).await?`.
3. Validate path(s) via `validate_data_path`.
4. Run the exec via the appropriate primitive.
5. On mutation success, `insert_audit` per §5.3.
6. Return body or 204.

```text
GET    /api/servers/{id}/files                  list
GET    /api/servers/{id}/files/raw              download
PUT    /api/servers/{id}/files                  upload
POST   /api/servers/{id}/files/action           mkdir / rename / delete
```

The 100 MiB upload cap is set via
`axum::extract::DefaultBodyLimit::max(104_857_600)` on the upload
route only, plus a stream-level guard inside `pod_exec_stream_in`.

### 6.7 Lifecycle hooks — `start.rs` and `delete.rs`

`backend/src/routes/servers/start.rs` adds one new step before
scaling MC up:

```rust
crate::files_helper::tear_down_helper(&state, &id).await?;
// existing scale-to-1 logic
```

`backend/src/routes/servers/delete.rs` adds one new step at the
top of the cleanup sequence (best-effort):

```rust
let _ = crate::files_helper::tear_down_helper(&state, &id).await;
// existing StatefulSet → wait pod gone → PVC → ... cleanup
```

`stop.rs` is **untouched** — stopping the MC pod has no helper
interaction (helpers only exist when MC is already stopped).

### 6.8 Wiring

- `backend/src/lib.rs` — `pub mod files; pub mod files_helper;`.
- `backend/src/routes/servers/mod.rs` — `pub mod files;` plus four
  router entries inside the per-server router builder.
- `backend/src/config.rs` — `Config::files_helper_image: String`
  read from `ANVIL_FILES_HELPER_IMAGE` (required, no fallback).

### 6.9 Cargo / dep changes

**None.** `kube`, `tokio`, `axum`, `serde`, `bytes`, `futures-util`
are already in `backend/Cargo.toml`. The exec primitives use
`kube::api::AttachParams` + the existing `kube::Api<Pod>`.

---

## 7. Frontend

### 7.1 Schemas + API — `frontend/app/lib/api.ts`

```ts
export const fileEntryTypeSchema = z.enum(["f", "d", "l", "o"]);
export const fileEntrySchema = z.object({
  name: z.string().min(1),
  type: fileEntryTypeSchema,
  size: z.number().nonnegative(),
  mtime: z.number(),
});
export const fileListResponseSchema = z.object({
  path: z.string().startsWith("/"),
  entries: z.array(fileEntrySchema),
});

export const fileActionSchema = z.discriminatedUnion("action", [
  z.object({ action: z.literal("mkdir"),  path: z.string().min(1) }),
  z.object({ action: z.literal("rename"), from: z.string().min(1), to: z.string().min(1) }),
  z.object({ action: z.literal("delete"), path: z.string().min(1), recursive: z.boolean() }),
]);

export async function fetchFileList(
  id: string,
  path: string,
  signal: AbortSignal,
): Promise<FileListResponse>;

export function downloadFileUrl(id: string, path: string): string;

export async function uploadFile(
  id: string,
  path: string,
  blob: Blob,
  opts: { onProgress?: (frac: number) => void; signal?: AbortSignal },
): Promise<void>;

export async function runFileAction(
  id: string,
  action: FileAction,
): Promise<void>;
```

`uploadFile` uses `XMLHttpRequest` for `upload.onprogress` events.
`runFileAction` uses the existing `noContentOrThrow` helper.
`downloadFileUrl` returns the URL string; the frontend renders it
via a hidden `<a>` with `download={basename}` to trigger the
browser's native download UI.

### 7.2 Fetcher hook — `frontend/app/lib/use-files.ts`

```ts
export function useFiles(
  serverId: string,
  path: string,
  opts: { enabled: boolean; serverStatus: ServerDetail["status"] },
): {
  data: FileListResponse | null;
  status: "loading" | "warming" | "ready" | "error";
  lastError: string | null;
  refresh: () => void;
};
```

- No polling. Refetches on `(serverId, path)` change and on
  `refresh()`.
- AbortController per fetch; aborted on unmount or next fetch.
- `status === "warming"` when the *first* fetch is in flight AND
  `serverStatus === "stopped"` (helper Pod is booting).
- `status === "loading"` for subsequent fetches.
- `refresh()` triggers an out-of-band fetch (called by post-action
  callbacks).

### 7.3 FilesBody composition — `frontend/app/servers/tabs/FilesBody.tsx`

Layout, top-down:

```text
┌─────────────────────────────────────────────────┐
│ PathBreadcrumb            [+ folder]   [upload] │  ← 1-line toolbar
├─────────────────────────────────────────────────┤
│ Card                                             │
│   /  level.dat            1.2 KB    3m ago    ⋯  │
│   d  region                            ─      ⋯  │
│   d  .cache                            ─      ⋯  │
│   l  old.jar.disabled →   32 B     1h ago     ⋯  │
└─────────────────────────────────────────────────┘
```

Empty dir → Card body is one line: `empty directory`.

Path navigation: clicking a directory row navigates by setting a
URL query param `&path=/world` (URL state so back/forward work).
Clicking a file row triggers download. `PathBreadcrumb` (already
shipped from A) renders the path segments, each clickable to
navigate up the tree.

Never-started server (409 `pvc_not_initialized` from the first
fetch) → Card body shows `start the server once to initialize
storage` + a `[start server]` button that POSTs `/start`.

### 7.4 New components

| File | Purpose |
|---|---|
| `frontend/app/components/FileEntryRow.tsx` | One row. Type-glyph (`/` for dirs, `→` after name for symlinks, none for files) + name (mono, click → navigate or download) + size (right, human-readable, `─` for dirs) + mtime (right, relative) + `FileActionMenu`. |
| `frontend/app/components/FileActionMenu.tsx` | Wraps `Dropdown`. Items vary by entry type: file → `download` / `rename` / `delete`; dir → `rename` / `delete (recursive)`; symlink → `rename` / `delete`. |
| `frontend/app/components/UploadFileDialog.tsx` | Modal with `<input type="file">`, progress bar (driven by XHR `upload.onprogress`), cancel button, send button. 100 MiB enforced client-side with a clear `file too large (max 100 MiB)` error before XHR fires. |
| `frontend/app/components/NameInputDialog.tsx` | Shared by mkdir + rename. Props: `mode: "create" \| "rename"`, `initialValue: string`, `onSubmit(name): void`. One Modal with one input + a client-side path-segment validator (mirrors backend's alphabet) for instant feedback. |

Existing primitives reused: `Card`, `Button`, `Modal`,
`ConfirmDeleteDialog`, `Dropdown`, `Skeleton`, `Toast`, `Tooltip`,
`IconButton`, `PathBreadcrumb`. Nothing new beyond the four files
above.

Destructive confirms:

- Single-file `delete` → light yes/no `Modal` (no name-typing).
- Recursive folder `delete` → existing `ConfirmDeleteDialog` (type
  the folder name to confirm), reused as-is from M5.
- `rename` is non-destructive — no confirm.

### 7.5 Detail-page wiring

`frontend/app/servers/ServerDetailView.tsx` already routes the
`files` tab to `FilesBody`. No router change. The path query param
(`&path=...`) is the URL state for navigation within the Files tab;
defaults to `/` when absent.

### 7.6 Tab visibility

Files tab is rendered for **every server kind** (vanilla, paper,
modpack, modrinth, modded). Unlike B's Mods tab, which is hidden
for vanilla, file management is universally useful.

---

## 8. k8s

- **New verb** `pods/exec: create` added to the existing Role in
  `deploy/templates/role.yaml`.
- **Helper-Pod resource budget** — `cpu: 100m / memory: 32Mi`
  limits, no requests.
- **No StatefulSet shape change** for managed MC servers.
- **No new ServiceAccount.** Helper Pod inherits the existing
  namespace ServiceAccount.

---

## 9. Migration

**None.** No DB schema change, no k8s reconcile pass for existing
servers, no configuration migration. New endpoints are additive;
existing endpoints unchanged in behaviour.

The new Helm value (`mc.filesHelperImage`) needs to be set on next
chart upgrade — the deployment's env-var write fails closed if
unset, so the operator gets a clear error rather than a silent
default.

---

## 10. Verification (acceptance for D)

- [ ] `cargo test --all`,
      `cargo clippy --all-targets --features serve-dir -- -D warnings`,
      `cargo clippy --all-targets --features embed -- -D warnings`,
      `cargo fmt --check` — green.
- [ ] `pnpm lint`, `pnpm typecheck`, `pnpm build` — green.
- [ ] Unit tests cover every list-output shape (empty, file, dir,
      hidden, symlink, malformed-skip, ENOTDIR sentinel) and every
      `validate_data_path` rejection branch.
- [ ] Files tab on a **never-started server** shows the
      `pvc_not_initialized` gate copy + `[start server]` button. No
      further fetches fire until the user clicks Start.
- [ ] Files tab on a **stopped server**: first navigation shows
      `starting offline file editor…` Skeleton; entries appear
      within ~15 s.
- [ ] Stopped → upload → start: helper torn down on Start; the
      uploaded file appears in the running pod's `/data`.
- [ ] Files tab on a **running server**: list `/`, list `/world`,
      list `/mods`, list `/.fabric` (hidden dir).
- [ ] **Upload:** drop a 5 MiB jar via dialog → toast
      `uploaded foo.jar` → row appears.
- [ ] **Upload cap:** drag a 200 MiB file → client-side error
      `file too large (max 100 MiB)` before the request fires.
- [ ] **Mkdir:** new folder dialog → `test` → toast `created test/`
      → row appears.
- [ ] **Rename:** menu → rename → `foo.jar` → `foo.jar.disabled` →
      toast → name changes; the running MC stops loading the mod
      after restart (manual smoke).
- [ ] **Download:** menu → download → browser saves the file; bytes
      match `kubectl exec ... cat`.
- [ ] **Single-file delete:** menu → delete → light Modal → confirm
      → row leaves; toast `deleted foo.jar`.
- [ ] **Recursive delete:** menu on a dir → delete →
      `ConfirmDeleteDialog` (type the dir name) → confirm → toast
      `deleted world_nether/`.
- [ ] **Path traversal:** `GET /api/servers/<id>/files?path=../etc/passwd`
      → 400 `path_invalid`. Same for action endpoint with `..` in
      any path field.
- [ ] **Argv injection:** `?path=/-rf` → 400
      `segment_leading_dash`.
- [ ] **Audit log:** every mutating action writes one row with the
      §5.3 `details` payload.
- [ ] **Helper teardown blocks Start:** while helper is up,
      `POST /start` waits for helper's pod-gone before scaling MC;
      verify by sending Start while a 90 MiB upload is mid-stream
      → upload aborts with a clean error, MC starts.
- [ ] **No new top-level deps.** `cargo tree` and `pnpm list` diff
      vs `main` shows zero new direct dependencies.
- [ ] **Visual signature check:** workshop tokens — copper accent
      only on toolbar CTA brackets; mono for filenames; no
      purple/blue gradients, no glassmorphism.

---

## 11. Open questions

Genuinely unresolved; the rest are settled in the decisions above.

1. **Symlink target check.** §5 proposes a 400
   `symlink_target_outside_data` if a `realpath`-style stat shows
   the link points outside `/data`. Implementing this is one extra
   exec per `cat` / `mv` / `rm` on a symlink. If it adds latency we
   feel, fall back to "list as `type=l`, ops are no-ops on
   symlinks themselves, never follow." Decide at impl time.
2. **Helper-Pod image digest.** The value should land in
   `values.yaml` as a digest-pinned reference (e.g.
   `alpine@sha256:…`). Confirm the digest at impl time and pin it;
   it should not change during D's lifetime.
3. **Stat round-trip for upload pre-flight.** The
   `parent_not_directory` check on PUT requires one stat exec
   before opening the streaming exec. Alternative: try the upload
   optimistically and let `cat > /<missing>/file.tmp` fail at the
   shell. Lean: do the pre-flight (cleaner error code, ~100 ms cost
   on a warm helper). Decide at impl time.
4. **Upload progress fidelity.**
   `XMLHttpRequest.upload.onprogress` reports
   bytes-uploaded-by-the-browser, not bytes-received-by-the-server.
   For LAN this is fine; for WAN it can over-report. Acceptable for
   our homelab + 5-friend use case; flagged in case it ever feels
   off.

---

## 12. What ships at the end of D

A user opening any anvil-managed server's Files tab sees:

1. A workshop-aesthetic file browser with breadcrumb + entries
   list.
2. List, download, upload (≤ 100 MiB), mkdir, rename, delete
   (single + recursive folder).
3. Stopped servers transparently spawn a helper Pod on first
   request, work the same as running servers.
4. Never-started servers gate to *start the server once to
   initialize storage*.
5. Confirmations for destructive verbs via `Modal` (light) and
   `ConfirmDeleteDialog` (recursive).
6. Toasts on every successful action.
7. Path-traversal-safe, shell-meta-safe, alphabet-restricted
   validation.
8. **No new RBAC except `pods/exec: create`. No new DB migration.
   No new top-level dependencies.**

D is the last v2 leg before deploy. After D ships and
`docs/milestones.md` is updated to mark D ✅, the next session is
Phase 4: deploying anvil to the homelab k0s cluster via FluxCD per
the existing repo at `/home/hadi/Documents/GitHub/homelab-k8s-fluxcd/`.

---

## 13. Critical files modified

**Backend (Rust):**

- `backend/src/files.rs` — NEW. Parsing module + tests.
- `backend/src/files_helper.rs` — NEW. Helper-Pod lifecycle + the
  `pod_exec_capture` / `pod_exec_stream_in` / `pod_exec_stream_out`
  primitives.
- `backend/src/validation.rs` — Add `validate_data_path` with unit
  tests.
- `backend/src/k8s_builders.rs` — Add `build_files_helper_pod` with
  unit tests.
- `backend/src/routes/servers/files.rs` — NEW. Four handlers (list,
  download, upload, action).
- `backend/src/routes/servers/start.rs` — One delta: call
  `tear_down_helper` before scaling MC up.
- `backend/src/routes/servers/delete.rs` — One delta: call
  `tear_down_helper` (best-effort) at the top of cleanup.
- `backend/src/routes/servers/mod.rs` — `pub mod files;` plus four
  router entries.
- `backend/src/lib.rs` — `pub mod files; pub mod files_helper;`.
- `backend/src/config.rs` — `Config::files_helper_image: String`
  from `ANVIL_FILES_HELPER_IMAGE`.

**Frontend (TypeScript):**

- `frontend/app/lib/api.ts` — Add `fileEntrySchema`,
  `fileListResponseSchema`, `fileActionSchema`, four fetch
  wrappers.
- `frontend/app/lib/use-files.ts` — NEW. Fetcher hook with
  status-aware "warming" state.
- `frontend/app/servers/tabs/FilesBody.tsx` — Rewrite over the
  placeholder.
- `frontend/app/components/FileEntryRow.tsx` — NEW.
- `frontend/app/components/FileActionMenu.tsx` — NEW.
- `frontend/app/components/UploadFileDialog.tsx` — NEW.
- `frontend/app/components/NameInputDialog.tsx` — NEW.

**Helm:**

- `deploy/values.yaml` — Add `mc.filesHelperImage` (digest-pinned).
- `deploy/templates/role.yaml` — Add `pods/exec: create` verb.
- `deploy/templates/deployment.yaml` — Wire
  `ANVIL_FILES_HELPER_IMAGE` env from `mc.filesHelperImage`.

**Docs:**

- `docs/superpowers/specs/2026-05-03-anvil-v2-file-browser-design.md`
  — this document.
- `docs/superpowers/plans/2026-05-03-anvil-v2-file-browser-impl.md`
  — generated by `superpowers:writing-plans` after spec sign-off.
- `docs/milestones.md` — mark D complete after ship.
