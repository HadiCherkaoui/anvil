# File Browser Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the v2.x `FilesBody` placeholder with a working Files tab — list, download, upload (≤ 100 MiB), mkdir, rename, delete (single + recursive folder) — for any anvil-managed Minecraft server, including stopped servers via a lazy-spawned helper Pod.

**Architecture:** Backend exposes four endpoints under `/api/servers/{id}/files` that proxy to `kube-rs pods/exec` inside the running MC pod, or — when the server is stopped — into a per-server "files-helper" Pod (alpine, `sleep infinity`, mounts the existing data PVC) that's lazy-spawned on first request and torn down before Start re-attaches the PVC. Single argv-based exec, no shell-meta interpolation; one strict path validator; one RBAC verb added (`pods/exec: create`). Frontend uses URL-state navigation with `?path=…`, an `XMLHttpRequest`-based upload with progress, and reuses every applicable v2 primitive.

**Tech Stack:** Rust 1.83 · axum 0.8 · kube-rs 3.1 (with `ws` + `runtime` features enabled in Phase 1) · sqlx (SQLite); Next.js 16 (`output: 'export'`) · TypeScript · Tailwind v4 · Zod.

**Spec:** [`docs/superpowers/specs/2026-05-03-anvil-v2-file-browser-design.md`](../specs/2026-05-03-anvil-v2-file-browser-design.md)

---

## File Structure

| Path | Status | Responsibility |
|---|---|---|
| `backend/Cargo.toml` | MODIFY | Enable kube `ws` + `runtime` features. |
| `backend/src/files.rs` | NEW | Pure parsers (`parse_list_output`, `parse_stat_size`, `is_enotdir_sentinel`) + the `LIST_SCRIPT` constant. No I/O. |
| `backend/src/files_helper.rs` | NEW | Helper-Pod lifecycle (`ensure_helper`, `tear_down_helper`, `target_pod_for_files`) and the generic exec primitives (`pod_exec_capture`, `pod_exec_stream_in`, `pod_exec_stream_out`). |
| `backend/src/k8s_builders.rs` | MODIFY | Add `build_files_helper_pod`. |
| `backend/src/validation.rs` | MODIFY | Add `validate_data_path`. |
| `backend/src/config.rs` | MODIFY | Add `Config::files_helper_image: String`. |
| `backend/src/lib.rs` | MODIFY | `pub mod files; pub mod files_helper;`. Add `AppState.files_helper_image`. |
| `backend/src/main.rs` | MODIFY | Wire `files_helper_image` into the AppState construction. |
| `backend/src/modpack/orchestrator.rs` | MODIFY | Generalise `wait_pod_running` / `wait_pod_gone` to take a pod name. |
| `backend/src/routes/servers/restart.rs` | MODIFY | Pass pod name to generalised `wait_pod_*`. |
| `backend/src/routes/servers/files.rs` | NEW | Four handlers (list, download, upload, action). |
| `backend/src/routes/servers/mod.rs` | MODIFY | `pub mod files;` + four router entries. |
| `backend/src/routes/servers/start.rs` | MODIFY | Call `tear_down_helper` before scaling MC up. |
| `backend/src/routes/servers/delete.rs` | MODIFY | Call `tear_down_helper` (best-effort) at the top of cleanup. |
| `frontend/app/lib/api.ts` | MODIFY | Add file schemas (`fileEntrySchema`, `fileListResponseSchema`, `fileActionSchema`) + helpers (`fetchFileList`, `downloadFileUrl`, `uploadFile`, `runFileAction`). |
| `frontend/app/lib/use-files.ts` | NEW | Fetcher hook with status-aware "warming" state. |
| `frontend/app/components/ConfirmDeleteDialog.tsx` | REFACTOR | Generalise: take `targetName` + `onConfirm` callback. |
| `frontend/app/servers/ServerDetailView.tsx` | MODIFY | Update server-delete callsite to the new prop shape. |
| `frontend/app/components/UploadFileDialog.tsx` | NEW | File picker + progress bar + send. |
| `frontend/app/components/NameInputDialog.tsx` | NEW | Shared by mkdir + rename. |
| `frontend/app/components/FileEntryRow.tsx` | NEW | One file/dir row. |
| `frontend/app/components/FileActionMenu.tsx` | NEW | Per-entry-type Dropdown. |
| `frontend/app/servers/tabs/FilesBody.tsx` | REWRITE | Toolbar + list + URL-state navigation + warming/empty/error states. |
| `deploy/values.yaml` | MODIFY | Add `mc.filesHelperImage`. |
| `deploy/templates/configmap.yaml` | MODIFY | Add `ANVIL_FILES_HELPER_IMAGE` env entry. |
| `deploy/templates/role.yaml` | MODIFY | Add `pods/exec: create` rule. |
| `docs/milestones.md` | MODIFY | Mark sub-project D complete. |

---

## Phase 1: Backend foundations

### Task 1: Enable kube `ws` + `runtime` features

**Files:**
- Modify: `backend/Cargo.toml`

Background: `Api::exec` and `AttachParams` require the `ws` feature; the `runtime` feature provides watcher utilities used in helper-Pod lifecycle. Both are non-default in `kube` 3.x.

- [ ] **Step 1: Inspect current kube line**

```bash
grep -n '^kube ' /home/hadi/gitlab/anvil/backend/Cargo.toml
```

Expected: a single line declaring `kube = { version = "3.1.0", default-features = false, features = ["client", "ring", "rustls-tls"] }`.

- [ ] **Step 2: Enable the new features**

Replace the existing `kube = ...` line in `backend/Cargo.toml` with:

```toml
kube = { version = "3.1.0", default-features = false, features = ["client", "ring", "rustls-tls", "ws", "runtime"] }
```

- [ ] **Step 3: Verify the build**

```bash
cd /home/hadi/gitlab/anvil/backend
cargo build --features serve-dir 2>&1 | tail -20
```

Expected: `Finished … target(s)` with no errors. The lockfile will be updated to pull in `kube-runtime`, `tokio-tungstenite`, and `async-broadcast`.

- [ ] **Step 4: Verify `AttachParams` is reachable**

```bash
cargo doc --features serve-dir --no-deps 2>&1 | tail -5
rg -n 'AttachParams' /home/hadi/gitlab/anvil/backend/Cargo.lock | head -3
```

Expected: `cargo doc` succeeds; the lockfile mentions `kube-runtime` and `tokio-tungstenite`.

- [ ] **Step 5: Commit**

```bash
cd /home/hadi/gitlab/anvil
git add backend/Cargo.toml backend/Cargo.lock
git commit -m "$(cat <<'EOF'
chore(deps): enable kube ws+runtime features for pods/exec

Sub-project D needs Api::exec + AttachParams (ws) for streaming
file ops over kube-rs into running pods, and the runtime feature
for helper-Pod lifecycle watchers. The kube dep itself is already
in Cargo.toml — this only flips additional features.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Path validator (`backend/src/validation.rs`)

**Files:**
- Modify: `backend/src/validation.rs`

- [ ] **Step 1: Read the existing file shape**

```bash
sed -n '1,40p' /home/hadi/gitlab/anvil/backend/src/validation.rs
```

Note the import block + the existing `validate_*` functions. We'll append the new validator above the `#[cfg(test)] mod tests` block, and append tests inside that module.

- [ ] **Step 2: Write failing tests**

Append these tests inside the existing `#[cfg(test)] mod tests { … }` block in `backend/src/validation.rs`:

```rust
    #[test]
    fn data_path_accepts_root_and_normal_paths() {
        for p in ["/", "/world", "/world/region", "/.fabric", "/.cache/seeds",
                  "/server.properties", "/World 2.zip", "/mods/sodium.jar"] {
            assert!(validate_data_path(p).is_ok(), "expected {p:?} to pass");
        }
    }

    #[test]
    fn data_path_treats_empty_as_root() {
        assert_eq!(validate_data_path("").unwrap(), "/");
    }

    #[test]
    fn data_path_rejects_relative_paths() {
        for p in ["world", "./world", "world/level.dat", "  "] {
            assert!(validate_data_path(p).is_err(), "expected {p:?} to fail");
        }
    }

    #[test]
    fn data_path_rejects_traversal_segments() {
        for p in ["/..", "/world/..", "/foo/../bar", "/../etc/passwd"] {
            assert!(validate_data_path(p).is_err(), "expected {p:?} to fail");
        }
    }

    #[test]
    fn data_path_rejects_dot_segments() {
        for p in ["/.", "/world/.", "/./bar"] {
            assert!(validate_data_path(p).is_err(), "expected {p:?} to fail");
        }
    }

    #[test]
    fn data_path_rejects_double_slash() {
        for p in ["//", "/foo//bar", "/foo/"] {
            assert!(validate_data_path(p).is_err(), "expected {p:?} to fail");
        }
    }

    #[test]
    fn data_path_rejects_leading_dash() {
        for p in ["/-rf", "/foo/-bar"] {
            assert!(validate_data_path(p).is_err(), "expected {p:?} to fail");
        }
    }

    #[test]
    fn data_path_rejects_control_chars() {
        for bad in ["/foo\nbar", "/foo\tbar", "/foo\0bar", "/foo\x7fbar"] {
            assert!(validate_data_path(bad).is_err(), "expected {bad:?} to fail");
        }
    }

    #[test]
    fn data_path_rejects_quote_and_backslash() {
        for bad in ["/foo'bar", "/foo\\bar"] {
            assert!(validate_data_path(bad).is_err(), "expected {bad:?} to fail");
        }
    }

    #[test]
    fn data_path_rejects_oversize_segment() {
        let long = "a".repeat(256);
        let p = format!("/{long}");
        assert!(validate_data_path(&p).is_err());
    }

    #[test]
    fn data_path_rejects_oversize_total() {
        let big = format!("/{}", "a/".repeat(2050));  // > 4096 chars total
        assert!(validate_data_path(&big).is_err());
    }
```

Run:

```bash
cd /home/hadi/gitlab/anvil/backend
cargo test --features serve-dir --lib validation::tests::data_path 2>&1 | tail -10
```

Expected: failures with "cannot find function `validate_data_path`".

- [ ] **Step 3: Implement `validate_data_path`**

Insert above the `#[cfg(test)]` block in `backend/src/validation.rs`:

```rust
/// Maximum total length of a `/data`-relative path (bytes).
const DATA_PATH_MAX_LEN: usize = 4096;
/// Maximum length of a single path segment (bytes).
const DATA_PATH_SEGMENT_MAX_LEN: usize = 255;

/// Validates a path under the managed server's `/data` PVC. Empty input
/// is normalised to `"/"`. The validated string is the input as-is so
/// callers can interpolate it into argv as `/data{path}`.
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] with one of: `path_too_long`,
/// `segment_empty`, `segment_traversal`, `segment_dot`,
/// `segment_leading_dash`, `segment_too_long`, `segment_invalid_char`.
pub fn validate_data_path(s: &str) -> Result<&str, AppError> {
    if s.is_empty() {
        return Ok("/");
    }
    if !s.starts_with('/') {
        return Err(bad_path("path_invalid", "path must start with /"));
    }
    if s.len() > DATA_PATH_MAX_LEN {
        return Err(bad_path(
            "path_too_long",
            &format!("path must be ≤ {DATA_PATH_MAX_LEN} bytes"),
        ));
    }
    if s == "/" {
        return Ok(s);
    }
    // Strip the leading slash and split on '/'. Any empty segment means
    // a "//" or trailing "/" — both rejected.
    for seg in s[1..].split('/') {
        validate_segment(seg)?;
    }
    Ok(s)
}

fn validate_segment(seg: &str) -> Result<(), AppError> {
    if seg.is_empty() {
        return Err(bad_path("segment_empty", "empty path segment"));
    }
    if seg == "." {
        return Err(bad_path("segment_dot", "'.' segment not allowed"));
    }
    if seg == ".." {
        return Err(bad_path("segment_traversal", "'..' segment not allowed"));
    }
    if seg.len() > DATA_PATH_SEGMENT_MAX_LEN {
        return Err(bad_path(
            "segment_too_long",
            &format!("segment exceeds {DATA_PATH_SEGMENT_MAX_LEN} bytes"),
        ));
    }
    if seg.as_bytes()[0] == b'-' {
        return Err(bad_path(
            "segment_leading_dash",
            "segment may not start with '-'",
        ));
    }
    for &b in seg.as_bytes() {
        let valid = (0x20..=0x7E).contains(&b) && b != b'\'' && b != b'\\';
        if !valid {
            return Err(bad_path(
                "segment_invalid_char",
                "segment contains a disallowed byte (control char, single-quote, or backslash)",
            ));
        }
    }
    Ok(())
}

fn bad_path(code: &'static str, message: &str) -> AppError {
    AppError::BadRequest {
        code,
        message: message.to_owned(),
    }
}
```

- [ ] **Step 4: Run the new tests**

```bash
cargo test --features serve-dir --lib validation::tests::data_path 2>&1 | tail -20
```

Expected: 11 passed.

- [ ] **Step 5: Format + clippy**

```bash
cargo fmt --all
cargo clippy --features serve-dir --all-targets -- -D warnings 2>&1 | tail -10
cargo clippy --features embed --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
cd /home/hadi/gitlab/anvil
git add backend/src/validation.rs
git commit -m "$(cat <<'EOF'
feat(api): validate_data_path — strict /data path validator

Rejects traversal (..), '.' segments, leading-dash segments, control
chars, single-quotes, backslashes, and oversize input. Returns the
input as-is so callers can interpolate via /data{path} in argv.
Eleven test cases cover every rejection branch plus valid hidden-file
and spaces-in-names paths.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Files parser module (`backend/src/files.rs`)

**Files:**
- Create: `backend/src/files.rs`
- Modify: `backend/src/lib.rs`

- [ ] **Step 1: Create `backend/src/files.rs` with the type scaffold + LIST_SCRIPT**

Create `backend/src/files.rs` with:

```rust
//! Pure parsers for the file-browser exec output.
//!
//! Functions are I/O-free and `kube`-free. Tests cover real busybox
//! outputs the helper script emits.

use serde::Serialize;

/// Discriminator for [`FileEntry`].
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FileEntryType {
    F,
    D,
    L,
    O,
}

/// One row of a directory listing.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub entry_type: FileEntryType,
    pub size: u64,
    pub mtime: i64,
}

/// Bulk-read response shape for `GET /api/servers/{id}/files`.
#[derive(Debug, Serialize)]
pub struct FileListResponse {
    pub path: String,
    pub entries: Vec<FileEntry>,
}

/// Sentinel emitted by [`LIST_SCRIPT`] when the target is missing or not
/// a directory. The handler treats this as a 404.
const ENOTDIR_LINE: &str = "ENOTDIR";

/// Returns true iff the captured stdout is the ENOTDIR sentinel.
#[must_use]
pub fn is_enotdir_sentinel(s: &str) -> bool {
    s.lines().next().map(str::trim) == Some(ENOTDIR_LINE)
}

/// Parses the size field emitted by `stat -c '%s'`. Returns None if the
/// stdout is non-numeric (file does not exist).
#[must_use]
pub fn parse_stat_size(s: &str) -> Option<u64> {
    s.trim().parse::<u64>().ok()
}

/// Shell script the file-browser execs to enumerate a directory.
///
/// Runs as `sh -c $LIST_SCRIPT _ /data/<safe-path>`. Emits one line per
/// entry: `type<TAB>size<TAB>mtime<TAB>name`. Skips `.` / `..`. On a
/// missing or non-directory target, prints the ENOTDIR sentinel and
/// exits non-zero.
pub const LIST_SCRIPT: &str = r#"cd "$1" 2>/dev/null || { printf 'ENOTDIR\n'; exit 1; }
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
    sz=$(stat -c '%s' "$entry" 2>/dev/null || printf '0')
  else
    sz=0
  fi
  mt=$(stat -c '%Y' "$entry" 2>/dev/null || printf '0')
  printf '%s\t%s\t%s\t%s\n' "$ty" "$sz" "$mt" "$entry"
done
"#;
```

- [ ] **Step 2: Add `pub mod files;` to `backend/src/lib.rs`**

Find the existing `pub mod` block near the top of `backend/src/lib.rs` (look for `pub mod players;`). Add adjacent:

```rust
pub mod files;
```

Verify it compiles:

```bash
cd /home/hadi/gitlab/anvil/backend
cargo build --features serve-dir 2>&1 | tail -5
```

Expected: `Finished … target(s)` with no errors (and a warning that `LIST_SCRIPT` etc. are unused — that's fine pre-handler).

- [ ] **Step 3: Write tests for `parse_list_output` (failing)**

Append to `backend/src/files.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_parses_empty_dir() {
        let out = parse_list_output("");
        assert_eq!(out, Vec::<FileEntry>::new());
    }

    #[test]
    fn list_parses_single_file() {
        let out = parse_list_output("f\t1234\t1714000000\tlevel.dat\n");
        assert_eq!(out, vec![FileEntry {
            name: "level.dat".into(),
            entry_type: FileEntryType::F,
            size: 1234,
            mtime: 1_714_000_000,
        }]);
    }

    #[test]
    fn list_parses_dir_with_zero_size() {
        let out = parse_list_output("d\t0\t1714000000\tregion\n");
        assert_eq!(out[0].entry_type, FileEntryType::D);
        assert_eq!(out[0].size, 0);
    }

    #[test]
    fn list_parses_hidden() {
        let out = parse_list_output("d\t0\t1714000000\t.cache\n");
        assert_eq!(out[0].name, ".cache");
    }

    #[test]
    fn list_parses_symlink() {
        let out = parse_list_output("l\t32\t1714000000\told.jar.disabled\n");
        assert_eq!(out[0].entry_type, FileEntryType::L);
        assert_eq!(out[0].size, 32);
    }

    #[test]
    fn list_parses_other_type() {
        let out = parse_list_output("o\t0\t1714000000\tsome_socket\n");
        assert_eq!(out[0].entry_type, FileEntryType::O);
    }

    #[test]
    fn list_parses_name_with_spaces() {
        let out = parse_list_output("f\t100\t1714000000\tWorld 2.zip\n");
        assert_eq!(out[0].name, "World 2.zip");
    }

    #[test]
    fn list_skips_malformed_lines() {
        let out = parse_list_output("garbage no tabs\nf\t1\t0\ta\nalsomalformed\nf\t2\t0\tb\n");
        let names: Vec<&str> = out.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn list_handles_unknown_type_byte_as_other() {
        // If for some reason the script emits an unrecognized type, we
        // map it to FileEntryType::O rather than dropping the row.
        let out = parse_list_output("z\t0\t0\tweird\n");
        assert_eq!(out[0].entry_type, FileEntryType::O);
    }

    #[test]
    fn stat_size_parses_numeric() {
        assert_eq!(parse_stat_size("1234\n"), Some(1234));
        assert_eq!(parse_stat_size("0"), Some(0));
    }

    #[test]
    fn stat_size_returns_none_on_garbage() {
        assert_eq!(parse_stat_size(""), None);
        assert_eq!(parse_stat_size("not a number"), None);
    }

    #[test]
    fn enotdir_sentinel_detected() {
        assert!(is_enotdir_sentinel("ENOTDIR"));
        assert!(is_enotdir_sentinel("ENOTDIR\n"));
        assert!(is_enotdir_sentinel("ENOTDIR\nextra\n"));
        assert!(!is_enotdir_sentinel(""));
        assert!(!is_enotdir_sentinel("not-a-sentinel\n"));
    }
}
```

Run:

```bash
cargo test --features serve-dir --lib files::tests 2>&1 | tail -20
```

Expected: failures with "cannot find function `parse_list_output`".

- [ ] **Step 4: Implement `parse_list_output`**

Insert before the `#[cfg(test)]` block in `backend/src/files.rs`:

```rust
/// Parses the tab-delimited output of [`LIST_SCRIPT`] into entries.
/// Malformed lines are silently skipped — defensive for busybox quirks.
#[must_use]
pub fn parse_list_output(s: &str) -> Vec<FileEntry> {
    s.lines()
        .filter_map(parse_list_line)
        .collect()
}

fn parse_list_line(line: &str) -> Option<FileEntry> {
    let mut parts = line.splitn(4, '\t');
    let ty = parts.next()?;
    let size = parts.next()?.parse::<u64>().ok()?;
    let mtime = parts.next()?.parse::<i64>().ok()?;
    let name = parts.next()?;
    if name.is_empty() {
        return None;
    }
    let entry_type = match ty {
        "f" => FileEntryType::F,
        "d" => FileEntryType::D,
        "l" => FileEntryType::L,
        _ => FileEntryType::O,
    };
    Some(FileEntry {
        name: name.to_owned(),
        entry_type,
        size,
        mtime,
    })
}
```

Run:

```bash
cargo test --features serve-dir --lib files::tests 2>&1 | tail -20
```

Expected: 12 passed.

- [ ] **Step 5: Format + clippy**

```bash
cargo fmt --all
cargo clippy --features serve-dir --all-targets -- -D warnings 2>&1 | tail -10
cargo clippy --features embed --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
cd /home/hadi/gitlab/anvil
git add backend/src/files.rs backend/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(api): files parsing module — list output + LIST_SCRIPT

Pure parsers for the upcoming file-browser handlers. parse_list_output
reads the tab-delimited output of LIST_SCRIPT, parse_stat_size reads
'stat -c %s', is_enotdir_sentinel detects the ENOTDIR marker. Twelve
test cases cover all six entry shapes and graceful handling of
malformed lines.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Helper-Pod builder (`backend/src/k8s_builders.rs`)

**Files:**
- Modify: `backend/src/k8s_builders.rs`

- [ ] **Step 1: Read existing imports and labels**

```bash
sed -n '1,80p' /home/hadi/gitlab/anvil/backend/src/k8s_builders.rs
```

Confirm the existing label constants (`MANAGED_BY_LABEL`, `LABEL_SERVER`, etc.) and the `BTreeMap` / kube-openapi imports are in scope.

- [ ] **Step 2: Write tests for `build_files_helper_pod` (failing)**

Append to the `#[cfg(test)] mod tests` block at the bottom of `k8s_builders.rs`:

```rust
    #[test]
    fn helper_pod_name_and_namespace() {
        let pod = build_files_helper_pod("abcd1234", "mc", "alpine@sha256:beef");
        assert_eq!(pod.metadata.name.as_deref(), Some("mc-abcd1234-files"));
        assert_eq!(pod.metadata.namespace.as_deref(), Some("mc"));
    }

    #[test]
    fn helper_pod_carries_managed_labels_plus_role() {
        let pod = build_files_helper_pod("abcd1234", "mc", "alpine@sha256:beef");
        let labels = pod.metadata.labels.as_ref().unwrap();
        assert_eq!(
            labels.get(MANAGED_BY_LABEL).map(String::as_str),
            Some(MANAGED_BY_VALUE),
        );
        assert_eq!(
            labels.get(LABEL_SERVER).map(String::as_str),
            Some("abcd1234"),
        );
        assert_eq!(
            labels.get("app.anvil.io/role").map(String::as_str),
            Some("files-helper"),
        );
    }

    #[test]
    fn helper_pod_runs_sleep_infinity() {
        let pod = build_files_helper_pod("abcd1234", "mc", "alpine@sha256:beef");
        let container = &pod.spec.as_ref().unwrap().containers[0];
        assert_eq!(container.image.as_deref(), Some("alpine@sha256:beef"));
        assert_eq!(
            container.command.as_ref().unwrap(),
            &vec!["sleep".to_owned(), "infinity".to_owned()],
        );
        assert_eq!(container.working_dir.as_deref(), Some("/data"));
    }

    #[test]
    fn helper_pod_mounts_data_pvc_by_claim_name() {
        let pod = build_files_helper_pod("abcd1234", "mc", "alpine@sha256:beef");
        let spec = pod.spec.as_ref().unwrap();
        let volume = spec.volumes.as_ref().unwrap().iter().find(|v| v.name == "data").unwrap();
        let pvc = volume.persistent_volume_claim.as_ref().unwrap();
        assert_eq!(pvc.claim_name, "data-mc-abcd1234-0");
        let mount = &spec.containers[0].volume_mounts.as_ref().unwrap()[0];
        assert_eq!(mount.name, "data");
        assert_eq!(mount.mount_path, "/data");
    }

    #[test]
    fn helper_pod_resource_limits_are_modest() {
        let pod = build_files_helper_pod("abcd1234", "mc", "alpine@sha256:beef");
        let res = pod.spec.as_ref().unwrap().containers[0].resources.as_ref().unwrap();
        assert!(res.requests.is_none());
        let limits = res.limits.as_ref().unwrap();
        assert_eq!(limits.get("cpu").unwrap().0, "100m");
        assert_eq!(limits.get("memory").unwrap().0, "32Mi");
    }
```

Run:

```bash
cd /home/hadi/gitlab/anvil/backend
cargo test --features serve-dir --lib k8s_builders::tests::helper 2>&1 | tail -10
```

Expected: failures with "cannot find function `build_files_helper_pod`".

- [ ] **Step 3: Implement `build_files_helper_pod`**

Insert into `backend/src/k8s_builders.rs` (placement: after `build_rcon_secret`, before the `pod_resources` private helper). Add the new imports at the top of the file as needed:

```rust
use k8s_openapi::api::core::v1::{
    Pod, PodSpec, PersistentVolumeClaimVolumeSource, Volume,
};
```

Then the function:

```rust
/// Builds the files-helper Pod for sub-project D. Mounts the existing
/// data PVC (`data-mc-{id}-0`) so anvil can run `pods/exec` against
/// `/data` while the MC server is stopped. Owned and torn down by
/// anvil; no controller wraps it.
#[must_use]
pub fn build_files_helper_pod(id: &str, namespace: &str, image: &str) -> Pod {
    let pod_name = format!("mc-{id}-files");
    let pvc_name = format!("data-mc-{id}-0");

    let mut labels = server_labels(id);
    labels.insert("app.anvil.io/role".to_owned(), "files-helper".to_owned());

    let mut limits: BTreeMap<String, Quantity> = BTreeMap::new();
    limits.insert("cpu".to_owned(), Quantity("100m".to_owned()));
    limits.insert("memory".to_owned(), Quantity("32Mi".to_owned()));
    let resources = ResourceRequirements {
        requests: None,
        limits: Some(limits),
        claims: None,
    };

    let container = Container {
        name: "files-helper".to_owned(),
        image: Some(image.to_owned()),
        command: Some(vec!["sleep".to_owned(), "infinity".to_owned()]),
        working_dir: Some("/data".to_owned()),
        resources: Some(resources),
        volume_mounts: Some(vec![VolumeMount {
            name: "data".to_owned(),
            mount_path: "/data".to_owned(),
            ..VolumeMount::default()
        }]),
        ..Container::default()
    };

    let volume = Volume {
        name: "data".to_owned(),
        persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
            claim_name: pvc_name,
            read_only: Some(false),
        }),
        ..Volume::default()
    };

    Pod {
        metadata: ObjectMeta {
            name: Some(pod_name),
            namespace: Some(namespace.to_owned()),
            labels: Some(labels),
            ..ObjectMeta::default()
        },
        spec: Some(PodSpec {
            containers: vec![container],
            volumes: Some(vec![volume]),
            restart_policy: Some("Always".to_owned()),
            ..PodSpec::default()
        }),
        status: None,
    }
}
```

- [ ] **Step 4: Run tests**

```bash
cargo test --features serve-dir --lib k8s_builders::tests::helper 2>&1 | tail -10
```

Expected: 5 passed.

- [ ] **Step 5: Format + clippy**

```bash
cargo fmt --all
cargo clippy --features serve-dir --all-targets -- -D warnings 2>&1 | tail -10
cargo clippy --features embed --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
cd /home/hadi/gitlab/anvil
git add backend/src/k8s_builders.rs
git commit -m "$(cat <<'EOF'
feat(api): build_files_helper_pod — sub-project D helper Pod

Bare Pod (alpine + sleep infinity) that mounts the existing data PVC
by claimName so anvil can pods/exec into /data while MC is stopped.
Lifecycle is owned by anvil; no controller wrapper. Resources capped
at 100m/32Mi limits.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: `Config::files_helper_image` + `AppState`

**Files:**
- Modify: `backend/src/config.rs`
- Modify: `backend/src/lib.rs`
- Modify: `backend/src/main.rs`

- [ ] **Step 1: Read current Config shape**

```bash
sed -n '1,60p' /home/hadi/gitlab/anvil/backend/src/config.rs
grep -n 'files_helper\|cf_api_key\|mc_namespace' /home/hadi/gitlab/anvil/backend/src/config.rs | head
```

Note where required env vars (e.g. `mc_namespace`) get read via `env::var(...).context(...)`.

- [ ] **Step 2: Add the field to `Config`**

In `backend/src/config.rs`, add `pub files_helper_image: String,` to the `Config` struct definition (the same block where `mc_namespace`, `mc_storage_class`, etc. live).

- [ ] **Step 3: Read the env var in `Config::from_env`**

In the same file, inside `from_env`, add the line that reads the env var. Place it adjacent to the other required reads (e.g. just below the `mc_namespace` read):

```rust
let files_helper_image = env::var("ANVIL_FILES_HELPER_IMAGE")
    .context("ANVIL_FILES_HELPER_IMAGE must be set")?;
```

Then add `files_helper_image,` to the `Config { … }` constructor at the bottom of `from_env`.

- [ ] **Step 4: Add the field to `AppState`**

In `backend/src/lib.rs`, locate `pub struct AppState { … }`. Add:

```rust
pub files_helper_image: String,
```

- [ ] **Step 5: Wire it through in `main.rs`**

In `backend/src/main.rs`, find the `AppState { … }` literal where it's constructed. Add `files_helper_image: cfg.files_helper_image.clone(),` (or move ownership; whichever matches the surrounding pattern for similar `String` fields like `mc_namespace`).

- [ ] **Step 6: Run any tests that touch Config**

```bash
cd /home/hadi/gitlab/anvil/backend
cargo test --features serve-dir --lib config 2>&1 | tail -10
```

If `Config::from_env` has tests that build a Config from env vars, they may fail until the new env var is provided. Update those test fixtures to set `ANVIL_FILES_HELPER_IMAGE` to a placeholder value (e.g. `"alpine@sha256:test"`).

- [ ] **Step 7: Run the full backend test suite**

```bash
cargo test --features serve-dir 2>&1 | tail -20
```

Adjust any test that constructs a literal `AppState` inline (search with `rg 'AppState \{'`) to include `files_helper_image: "alpine@sha256:test".to_owned()`.

- [ ] **Step 8: Format + clippy**

```bash
cargo fmt --all
cargo clippy --features serve-dir --all-targets -- -D warnings 2>&1 | tail -10
cargo clippy --features embed --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: no warnings.

- [ ] **Step 9: Commit**

```bash
cd /home/hadi/gitlab/anvil
git add backend/src/config.rs backend/src/lib.rs backend/src/main.rs
git commit -m "$(cat <<'EOF'
feat(api): Config + AppState carry files_helper_image

Required env var ANVIL_FILES_HELPER_IMAGE; backend fails to start
without it. AppState exposes the digest-pinned alpine image string
to the upcoming files-helper Pod builder caller.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2: Pod-exec primitives + helper lifecycle

### Task 6: Generalise `wait_pod_running` / `wait_pod_gone`

**Files:**
- Modify: `backend/src/modpack/orchestrator.rs`
- Modify: `backend/src/routes/servers/restart.rs` (and any other callers)

- [ ] **Step 1: Find all callers**

```bash
cd /home/hadi/gitlab/anvil/backend
rg -n 'wait_pod_(gone|running)' src/ tests/
```

Expected: definitions in `modpack/orchestrator.rs` plus call sites in the orchestrator FSM and `routes/servers/restart.rs`. Note each caller's surrounding context.

- [ ] **Step 2: Update the function signatures**

In `backend/src/modpack/orchestrator.rs`, change:

```rust
pub(crate) async fn wait_pod_gone(
    client: &kube::Client,
    ns: &str,
    server_id: &str,
    timeout_dur: Duration,
) -> Result<()> {
    // Old body uses `format!("mc-{server_id}-0")` internally.
}
```

to:

```rust
pub(crate) async fn wait_pod_gone(
    client: &kube::Client,
    ns: &str,
    pod_name: &str,
    timeout_dur: Duration,
) -> Result<()> {
    // Body uses `pod_name` directly.
}
```

Apply the same change to `wait_pod_running`. Replace any internal `format!("mc-{server_id}-0")` with `pod_name`.

- [ ] **Step 3: Update every caller to pass an explicit pod name**

For each caller surfaced in Step 1, change:

```rust
wait_pod_gone(&client, ns, &server_id, Duration::from_secs(60)).await?;
```

to:

```rust
let pod_name = format!("mc-{server_id}-0");
wait_pod_gone(&client, ns, &pod_name, Duration::from_secs(60)).await?;
```

(And the same for `wait_pod_running`.) Reuse the same `pod_name` variable across consecutive calls within a function to avoid re-allocating.

- [ ] **Step 4: Run the existing tests**

```bash
cargo test --features serve-dir 2>&1 | tail -20
cargo test --features embed 2>&1 | tail -20
```

Expected: same passing count as before, no regressions.

- [ ] **Step 5: Format + clippy**

```bash
cargo fmt --all
cargo clippy --features serve-dir --all-targets -- -D warnings 2>&1 | tail -10
cargo clippy --features embed --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
cd /home/hadi/gitlab/anvil
git add backend/src/modpack/orchestrator.rs backend/src/routes/servers/restart.rs
git commit -m "$(cat <<'EOF'
refactor(api): wait_pod_* take an explicit pod_name

Sub-project D needs the same wait helpers for the files-helper Pod
(mc-{id}-files) — generalise rather than duplicate. Existing callers
pass &format!('mc-{server_id}-0'). No behaviour change.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: `pod_exec_capture` (`backend/src/files_helper.rs`)

**Files:**
- Create: `backend/src/files_helper.rs`
- Modify: `backend/src/lib.rs`

- [ ] **Step 1: Create the module skeleton**

Create `backend/src/files_helper.rs`:

```rust
//! Helper-Pod lifecycle (`mc-{id}-files`) and the generic `pods/exec`
//! primitives sub-project D uses for file ops. Pure plumbing — handler
//! logic lives in `routes/servers/files.rs`.

use std::time::Duration;

use anyhow::Context as _;
use bytes::Bytes;
use futures_util::{Stream, StreamExt as _};
use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::{Api, AttachParams, DeleteParams, PostParams},
    Client,
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::error::AppError;
use crate::AppState;

/// Result of a non-streaming exec invocation.
#[derive(Debug)]
pub struct PodExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

/// 5-second cap for capture-shape execs (list, stat, mkdir, rename,
/// delete). Streaming variants use a longer idle-read timeout instead.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
```

- [ ] **Step 2: Add `pub mod files_helper;` to `backend/src/lib.rs`**

Append to the `pub mod` block:

```rust
pub mod files_helper;
```

Verify it compiles:

```bash
cd /home/hadi/gitlab/anvil/backend
cargo build --features serve-dir 2>&1 | tail -5
```

Expected: warnings about unused imports are OK at this stage; no errors.

- [ ] **Step 3: Implement `pod_exec_capture`**

Append to `backend/src/files_helper.rs`:

```rust
/// Runs `cmd` in `pod_name`, capturing stdout / stderr / exit code.
/// 5-second end-to-end timeout. Used for: list (LIST_SCRIPT), stat
/// pre-flights, mkdir, rename, single-file delete, recursive delete.
///
/// # Errors
///
/// `AppError::KubeUnavailable` on transport failure;
/// `AppError::Internal` on timeout or stream read failure.
pub async fn pod_exec_capture(
    state: &AppState,
    namespace: &str,
    pod_name: &str,
    cmd: &[&str],
) -> Result<PodExecResult, AppError> {
    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), namespace);

    let attach = AttachParams::default()
        .stdin(false)
        .stdout(true)
        .stderr(true);

    let fut = async {
        let mut process = pods.exec(pod_name, cmd, &attach).await?;
        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();

        if let Some(mut s) = process.stdout() {
            tokio::io::copy(&mut s, &mut stdout_buf).await?;
        }
        if let Some(mut e) = process.stderr() {
            tokio::io::copy(&mut e, &mut stderr_buf).await?;
        }

        let status = process.take_status();
        let exit_code = match status {
            Some(fut) => match fut.await {
                Some(s) => parse_exit_code(s.status.as_deref(), s.message.as_deref()),
                None => None,
            },
            None => None,
        };

        anyhow::Ok(PodExecResult {
            stdout: String::from_utf8_lossy(&stdout_buf).into_owned(),
            stderr: String::from_utf8_lossy(&stderr_buf).into_owned(),
            exit_code,
        })
    };

    match tokio::time::timeout(CAPTURE_TIMEOUT, fut).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => {
            // Propagate kube transport failures with the Kube variant so
            // the wire shape stays consistent with the rest of anvil.
            if let Some(kube_err) = err.downcast_ref::<kube::Error>() {
                return Err(AppError::KubeUnavailable(kube_err.clone()));
            }
            Err(AppError::Internal(err))
        }
        Err(_) => Err(AppError::Internal(anyhow::anyhow!(
            "pod_exec_capture timed out after {} seconds",
            CAPTURE_TIMEOUT.as_secs()
        ))),
    }
}

/// Maps a k8s exec termination status into a numeric exit code. Status
/// `"Success"` is exit 0; otherwise we look at the status `message` for
/// `command terminated with exit code N`.
fn parse_exit_code(status: Option<&str>, message: Option<&str>) -> Option<i32> {
    match status {
        Some("Success") => Some(0),
        _ => {
            // The k8s exec status message looks like:
            // "command terminated with exit code 1"
            let msg = message?;
            let idx = msg.rfind(' ')?;
            msg[idx + 1..].parse::<i32>().ok()
        }
    }
}
```

(`kube::Error` does not derive `Clone`; if `clone()` doesn't compile, change the propagation to `AppError::Internal(err)` for the kube-error branch as well — the behaviour is identical from the wire side.)

- [ ] **Step 4: Format + build**

```bash
cd /home/hadi/gitlab/anvil/backend
cargo fmt --all
cargo build --features serve-dir 2>&1 | tail -10
cargo build --features embed 2>&1 | tail -10
```

If the `kube::Error.clone()` line fails to compile, replace the matching branch in `pod_exec_capture` with `Err(AppError::Internal(err))` unconditionally and rebuild. Expected after fix: clean build.

- [ ] **Step 5: Clippy**

```bash
cargo clippy --features serve-dir --all-targets -- -D warnings 2>&1 | tail -10
cargo clippy --features embed --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
cd /home/hadi/gitlab/anvil
git add backend/src/files_helper.rs backend/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(api): pod_exec_capture primitive

Runs a command inside a named pod over kube-rs Api::exec; captures
stdout / stderr / exit code with a 5s timeout. Foundation for the
upcoming files-helper lifecycle and file-op handlers in sub-project
D. No live exec test (covered by smoke).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: `pod_exec_stream_in` and `pod_exec_stream_out`

**Files:**
- Modify: `backend/src/files_helper.rs`

- [ ] **Step 1: Append `pod_exec_stream_in`**

Append to `backend/src/files_helper.rs`:

```rust
/// 60-second idle-read timeout for streaming variants. The total
/// duration is unbounded — anvil keeps the connection open as long as
/// data flows.
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Streams a request body into the named pod's exec stdin. Aborts
/// mid-stream and returns `payload_too_large` if the cap is exceeded.
/// Returns the byte count written on success.
///
/// # Errors
///
/// `BadRequest("payload_too_large")` on cap breach; `KubeUnavailable`
/// or `Internal` on transport / IO errors.
pub async fn pod_exec_stream_in<S>(
    state: &AppState,
    namespace: &str,
    pod_name: &str,
    cmd: &[&str],
    mut body: S,
    cap_bytes: u64,
) -> Result<u64, AppError>
where
    S: Stream<Item = Result<Bytes, axum::Error>> + Send + Unpin + 'static,
{
    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), namespace);
    let attach = AttachParams::default()
        .stdin(true)
        .stdout(false)
        .stderr(true);

    let mut process = pods
        .exec(pod_name, cmd, &attach)
        .await
        .map_err(AppError::KubeUnavailable)?;

    let mut stdin = process
        .stdin()
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("exec stdin unavailable")))?;

    let mut total: u64 = 0;
    while let Some(chunk) = body.next().await {
        let bytes = chunk.map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        let new_total = total.saturating_add(bytes.len() as u64);
        if new_total > cap_bytes {
            // Best-effort: close stdin to abort the remote command,
            // then drop the process handle.
            drop(stdin);
            return Err(AppError::BadRequest {
                code: "payload_too_large",
                message: format!("upload exceeded {cap_bytes} bytes"),
            });
        }
        stdin
            .write_all(&bytes)
            .await
            .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;
        total = new_total;
    }

    drop(stdin); // signal EOF to the remote command

    // Drain stderr and check exit code under the idle-read timeout.
    let mut stderr_buf = Vec::new();
    if let Some(mut e) = process.stderr() {
        let _ = tokio::time::timeout(
            STREAM_IDLE_TIMEOUT,
            tokio::io::copy(&mut e, &mut stderr_buf),
        )
        .await;
    }

    let status = process.take_status();
    let exit_code = match status {
        Some(fut) => match tokio::time::timeout(STREAM_IDLE_TIMEOUT, fut).await {
            Ok(Some(s)) => parse_exit_code(s.status.as_deref(), s.message.as_deref()),
            _ => None,
        },
        None => None,
    };

    if exit_code != Some(0) {
        let stderr_str = String::from_utf8_lossy(&stderr_buf);
        return Err(AppError::Internal(anyhow::anyhow!(
            "remote command failed: exit={:?}, stderr={}",
            exit_code,
            stderr_str.trim(),
        )));
    }

    Ok(total)
}
```

- [ ] **Step 2: Append `pod_exec_stream_out`**

```rust
/// Returns an owned async stream of stdout bytes from the named pod's
/// exec. Caller pipes it into `axum::body::Body::from_stream`. Idle-read
/// timeout per chunk: 60 seconds (the connection terminates if the
/// remote `cat` blocks for that long).
///
/// # Errors
///
/// `KubeUnavailable` on attach failure; the returned stream surfaces
/// per-chunk errors as `std::io::Error`.
pub async fn pod_exec_stream_out(
    state: &AppState,
    namespace: &str,
    pod_name: &str,
    cmd: &[&str],
) -> Result<
    impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static,
    AppError,
> {
    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), namespace);
    let attach = AttachParams::default()
        .stdin(false)
        .stdout(true)
        .stderr(false);

    let mut process = pods
        .exec(pod_name, cmd, &attach)
        .await
        .map_err(AppError::KubeUnavailable)?;

    let stdout = process
        .stdout()
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("exec stdout unavailable")))?;

    // Hold the process alive for the duration of the stream. We use a
    // try_stream that owns `process` so it isn't dropped (which would
    // close the channel) until the consumer finishes.
    Ok(async_stream::try_stream! {
        let _process_guard = process; // keep process alive
        let mut reader = stdout;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = tokio::time::timeout(STREAM_IDLE_TIMEOUT, reader.read(&mut buf))
                .await
                .map_err(|_| std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "exec stdout idle timeout",
                ))??;
            if n == 0 { break; }
            yield Bytes::copy_from_slice(&buf[..n]);
        }
    })
}
```

- [ ] **Step 3: Add the `async-stream` dependency**

The `async_stream::try_stream!` macro lives in the `async-stream` crate. Check if it's already in `backend/Cargo.toml`:

```bash
grep -n 'async-stream' /home/hadi/gitlab/anvil/backend/Cargo.toml
```

If absent (likely), add via:

```bash
cd /home/hadi/gitlab/anvil/backend
cargo add async-stream@0.3
```

Expected: `Adding async-stream v0.3` in the output. Spec says "no new top-level deps" — `async-stream` is a tightly-scoped utility, not a new framework, but flag this in the impl notes if it surfaces in review. (Alternative: hand-roll a stream by passing an `mpsc::channel` from a spawned task. More code; less idiomatic.)

- [ ] **Step 4: Format + build**

```bash
cargo fmt --all
cargo build --features serve-dir 2>&1 | tail -10
cargo build --features embed 2>&1 | tail -10
```

Expected: clean build.

- [ ] **Step 5: Clippy**

```bash
cargo clippy --features serve-dir --all-targets -- -D warnings 2>&1 | tail -10
cargo clippy --features embed --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
cd /home/hadi/gitlab/anvil
git add backend/src/files_helper.rs backend/Cargo.toml backend/Cargo.lock
git commit -m "$(cat <<'EOF'
feat(api): pod_exec_stream_in + pod_exec_stream_out

Streaming variants of the kube-rs exec primitive. _stream_in writes a
body to stdin with a byte cap; _stream_out yields stdout chunks for
axum Body::from_stream. Adds async-stream@0.3 for the try_stream macro.
60s idle-read timeout per chunk.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: `ensure_helper`, `tear_down_helper`, `target_pod_for_files`

**Files:**
- Modify: `backend/src/files_helper.rs`

- [ ] **Step 1: Append `tear_down_helper`**

Append to `backend/src/files_helper.rs`:

```rust
/// Best-effort delete of the files-helper Pod. 404 is treated as
/// success. Waits up to 30 s for the Pod to be fully gone before
/// returning.
pub async fn tear_down_helper(state: &AppState, server_id: &str) -> Result<(), AppError> {
    let pod_name = format!("mc-{server_id}-files");
    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), &state.mc_namespace);

    match pods.delete(&pod_name, &DeleteParams::default()).await {
        Ok(_) | Err(kube::Error::Api(ref e)) if matches_404(state) => {}
        Err(kube::Error::Api(e)) if e.code == 404 => {}
        Err(e) => return Err(AppError::KubeUnavailable(e)),
        Ok(_) => {}
    }

    crate::modpack::orchestrator::wait_pod_gone(
        &state.kube,
        &state.mc_namespace,
        &pod_name,
        Duration::from_secs(30),
    )
    .await
    .map_err(AppError::Internal)?;

    Ok(())
}

/// Trivial helper kept to make the match in `tear_down_helper`
/// readable without flailing on borrow types — always returns false
/// and exists only to satisfy the unused-pattern arm above. Replace
/// with a direct match arm if clippy complains.
fn matches_404(_state: &AppState) -> bool { false }
```

(If the `match` shape above produces "unreachable pattern" or borrow-checker noise during impl, simplify to:

```rust
match pods.delete(&pod_name, &DeleteParams::default()).await {
    Ok(_) => {}
    Err(kube::Error::Api(e)) if e.code == 404 => {}
    Err(e) => return Err(AppError::KubeUnavailable(e)),
}
```

That shape is what the M5 orchestrator's tar-job teardown uses; mirror it.)

- [ ] **Step 2: Append `ensure_helper`**

```rust
/// Lazy-creates the files-helper Pod and waits for it to be Running.
/// On `409 AlreadyExists` we treat the pre-existing Pod as ours and
/// proceed to wait. On a "pvc not bound / not found" error from the
/// create call we surface `Conflict("pvc_not_initialized")` so the
/// frontend can show the "start the server once" gate copy.
pub async fn ensure_helper(state: &AppState, server_id: &str) -> Result<(), AppError> {
    let pod_name = format!("mc-{server_id}-files");
    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), &state.mc_namespace);

    // Fast path: helper already exists. Wait for Running and return.
    if let Ok(Some(_)) = pods.get_opt(&pod_name).await {
        return crate::modpack::orchestrator::wait_pod_running(
            &state.kube,
            &state.mc_namespace,
            &pod_name,
            Duration::from_secs(30),
        )
        .await
        .map_err(AppError::Internal);
    }

    let pod = crate::k8s_builders::build_files_helper_pod(
        server_id,
        &state.mc_namespace,
        &state.files_helper_image,
    );

    match pods.create(&PostParams::default(), &pod).await {
        Ok(_) => {}
        Err(kube::Error::Api(e)) if e.code == 409 => {
            // Race: another request created it. Treat as success.
        }
        Err(kube::Error::Api(e)) if pvc_not_found(&e.message) => {
            return Err(AppError::Conflict {
                code: "pvc_not_initialized",
                message: format!(
                    "data PVC for server {server_id} does not exist — start the server once to initialize storage"
                ),
            });
        }
        Err(e) => return Err(AppError::KubeUnavailable(e)),
    }

    crate::modpack::orchestrator::wait_pod_running(
        &state.kube,
        &state.mc_namespace,
        &pod_name,
        Duration::from_secs(30),
    )
    .await
    .map_err(AppError::Internal)
}

fn pvc_not_found(message: &str) -> bool {
    let lc = message.to_ascii_lowercase();
    lc.contains("persistentvolumeclaim") && (lc.contains("not found") || lc.contains("does not exist"))
}
```

- [ ] **Step 3: Append `target_pod_for_files`**

```rust
use crate::k8s_status::{derive_status, ServerStatus};

/// Returns the pod name to exec into based on the server's current
/// status. Lazy-creates the helper Pod when the server is stopped.
///
/// # Errors
///
/// `Conflict("pvc_not_initialized")` if the server has never started
/// and the data PVC therefore doesn't exist; `KubeUnavailable` on
/// transport failure; `Internal` on lifecycle wait timeout.
pub async fn target_pod_for_files(
    state: &AppState,
    server_id: &str,
) -> Result<String, AppError> {
    use k8s_openapi::api::apps::v1::StatefulSet;

    let stsets: Api<StatefulSet> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), &state.mc_namespace);

    let sts_name = format!("mc-{server_id}");
    let mc_pod = format!("mc-{server_id}-0");

    let sts = stsets
        .get_opt(&sts_name)
        .await
        .map_err(AppError::KubeUnavailable)?
        .ok_or(AppError::NotFound)?;

    let pod_opt = pods
        .get_opt(&mc_pod)
        .await
        .map_err(AppError::KubeUnavailable)?;

    let replicas = sts.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0);
    let ready = sts
        .status
        .as_ref()
        .and_then(|s| s.ready_replicas)
        .unwrap_or(0);
    let status = derive_status(replicas, ready, pod_opt.as_ref());

    match status {
        ServerStatus::Running => Ok(mc_pod),
        ServerStatus::Stopped => {
            ensure_helper(state, server_id).await?;
            Ok(format!("mc-{server_id}-files"))
        }
        ServerStatus::Starting | ServerStatus::Stopping => Err(AppError::Conflict {
            code: "server_transitioning",
            message: "server is starting or stopping; retry shortly".to_owned(),
        }),
        ServerStatus::Error => Err(AppError::Conflict {
            code: "server_error",
            message: "server is in error state; resolve before browsing files".to_owned(),
        }),
    }
}
```

- [ ] **Step 4: Format + build**

```bash
cd /home/hadi/gitlab/anvil/backend
cargo fmt --all
cargo build --features serve-dir 2>&1 | tail -15
cargo build --features embed 2>&1 | tail -10
```

If the `wait_pod_*` callsites complain about `anyhow::Error` vs `AppError`, ensure the Phase 2 generalisation completed: `wait_pod_*` returns `anyhow::Result`, so we map via `.map_err(AppError::Internal)`.

Expected: clean build.

- [ ] **Step 5: Clippy**

```bash
cargo clippy --features serve-dir --all-targets -- -D warnings 2>&1 | tail -15
cargo clippy --features embed --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
cd /home/hadi/gitlab/anvil
git add backend/src/files_helper.rs
git commit -m "$(cat <<'EOF'
feat(api): files-helper Pod lifecycle + target_pod_for_files

ensure_helper lazy-creates mc-{id}-files; tear_down_helper deletes
it (404-tolerant) and waits pod-gone. target_pod_for_files routes
file ops to mc-{id}-0 when running and to the helper when stopped.
Never-started servers surface as Conflict('pvc_not_initialized')
so the frontend can render the "start once" gate.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3: Files route module

### Task 10: List handler

**Files:**
- Create: `backend/src/routes/servers/files.rs`

- [ ] **Step 1: Create the module skeleton + list handler**

Create `backend/src/routes/servers/files.rs`:

```rust
//! File-browser handlers for sub-project D.
//!
//! All four endpoints share the same shape: route → fetch server row →
//! pick target pod via files_helper::target_pod_for_files → validate
//! path(s) → exec the appropriate command → audit (mutating only) →
//! respond.

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::AppError;
use crate::files::{
    is_enotdir_sentinel, parse_list_output, parse_stat_size, FileEntry, FileListResponse,
    LIST_SCRIPT,
};
use crate::files_helper::{
    pod_exec_capture, pod_exec_stream_in, pod_exec_stream_out, target_pod_for_files,
};
use crate::routes::servers::create::insert_audit;
use crate::validation::validate_data_path;
use crate::AppState;

const UPLOAD_CAP_BYTES: u64 = 100 * 1024 * 1024; // 100 MiB

#[derive(Debug, Deserialize)]
pub struct PathQuery {
    #[serde(default)]
    pub path: Option<String>,
}

fn data_path(path: &str) -> String {
    if path == "/" {
        "/data".to_owned()
    } else {
        format!("/data{path}")
    }
}

pub async fn list(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
    Query(q): Query<PathQuery>,
) -> Result<Json<FileListResponse>, AppError> {
    let raw_path = q.path.unwrap_or_else(|| "/".to_owned());
    let path = validate_data_path(&raw_path)?.to_owned();
    let target = data_path(&path);

    let pod_name = target_pod_for_files(&state, &server_id).await?;

    let result = pod_exec_capture(
        &state,
        &state.mc_namespace,
        &pod_name,
        &["sh", "-c", LIST_SCRIPT, "_", &target],
    )
    .await?;

    if is_enotdir_sentinel(&result.stdout) {
        return Err(AppError::NotFound);
    }
    if result.exit_code != Some(0) {
        return Err(AppError::Internal(anyhow::anyhow!(
            "list exec failed: stderr={}",
            result.stderr.trim()
        )));
    }

    let entries = parse_list_output(&result.stdout);
    Ok(Json(FileListResponse { path, entries }))
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cd /home/hadi/gitlab/anvil/backend
cargo build --features serve-dir 2>&1 | tail -10
```

Expected: warnings (unused things — they get used in the next tasks); no errors.

- [ ] **Step 3: Commit (handler-only commit deferred — proceed to next handler)**

This task contributes to a single combined commit at the end of Task 13.

---

### Task 11: Download handler

**Files:**
- Modify: `backend/src/routes/servers/files.rs`

- [ ] **Step 1: Append the download handler**

Append to `backend/src/routes/servers/files.rs`:

```rust
pub async fn download(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
    Query(q): Query<PathQuery>,
) -> Result<Response, AppError> {
    let raw_path = q.path.ok_or_else(|| AppError::BadRequest {
        code: "path_required",
        message: "path query parameter required".to_owned(),
    })?;
    let path = validate_data_path(&raw_path)?.to_owned();
    if path == "/" {
        return Err(AppError::BadRequest {
            code: "path_is_root",
            message: "cannot download the root directory".to_owned(),
        });
    }
    let target = data_path(&path);
    let pod_name = target_pod_for_files(&state, &server_id).await?;

    // Pre-flight: stat for size + existence. Missing file → 404.
    let stat = pod_exec_capture(
        &state,
        &state.mc_namespace,
        &pod_name,
        &["stat", "-c", "%s", &target],
    )
    .await?;
    if stat.exit_code != Some(0) {
        return Err(AppError::NotFound);
    }
    let size = parse_stat_size(&stat.stdout).ok_or_else(|| AppError::Internal(
        anyhow::anyhow!("stat returned non-numeric output: {}", stat.stdout)
    ))?;

    let basename = path.rsplit('/').next().unwrap_or("file");
    let stream = pod_exec_stream_out(
        &state,
        &state.mc_namespace,
        &pod_name,
        &["cat", &target],
    )
    .await?;

    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, "application/octet-stream".parse().unwrap());
    headers.insert(header::CONTENT_LENGTH, size.into());
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{basename}\"").parse().unwrap(),
    );

    Ok((headers, Body::from_stream(stream)).into_response())
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo build --features serve-dir 2>&1 | tail -10
```

Expected: warnings, no errors.

- [ ] **Step 3: (deferred commit — see Task 13)**

---

### Task 12: Upload handler

**Files:**
- Modify: `backend/src/routes/servers/files.rs`

- [ ] **Step 1: Append the upload handler**

Append to `backend/src/routes/servers/files.rs`:

```rust
pub async fn upload(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
    Query(q): Query<PathQuery>,
    request: Request,
) -> Result<StatusCode, AppError> {
    let raw_path = q.path.ok_or_else(|| AppError::BadRequest {
        code: "path_required",
        message: "path query parameter required".to_owned(),
    })?;
    let path = validate_data_path(&raw_path)?.to_owned();
    if path == "/" {
        return Err(AppError::BadRequest {
            code: "path_is_root",
            message: "cannot upload to the root directory".to_owned(),
        });
    }

    let target = data_path(&path);
    let pod_name = target_pod_for_files(&state, &server_id).await?;

    // Pre-flight: parent must exist and be a directory.
    let parent = match path.rsplit_once('/').map(|(p, _)| p) {
        Some("") | None => "/".to_owned(),
        Some(p) => p.to_owned(),
    };
    let parent_target = data_path(&parent);
    let parent_check = pod_exec_capture(
        &state,
        &state.mc_namespace,
        &pod_name,
        &["sh", "-c", "test -d \"$1\"", "_", &parent_target],
    )
    .await?;
    if parent_check.exit_code != Some(0) {
        return Err(AppError::Conflict {
            code: "parent_not_directory",
            message: format!("parent {parent} is not a directory"),
        });
    }

    let body_stream = request.into_body().into_data_stream();
    let upload_script = "cat > \"$1.tmp\" && mv \"$1.tmp\" \"$1\"";

    let bytes = pod_exec_stream_in(
        &state,
        &state.mc_namespace,
        &pod_name,
        &["sh", "-c", upload_script, "_", &target],
        body_stream,
        UPLOAD_CAP_BYTES,
    )
    .await?;

    insert_audit(
        &state.pool,
        &server_id,
        "files.upload",
        Some(json!({ "path": path, "bytes": bytes })),
        Utc::now().timestamp(),
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo build --features serve-dir 2>&1 | tail -10
```

Expected: clean build (or only handler-not-yet-routed warnings).

- [ ] **Step 3: (deferred commit — see Task 13)**

---

### Task 13: Action handler + router wiring

**Files:**
- Modify: `backend/src/routes/servers/files.rs`
- Modify: `backend/src/routes/servers/mod.rs`

- [ ] **Step 1: Append the action handler**

Append to `backend/src/routes/servers/files.rs`:

```rust
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum FileAction {
    Mkdir { path: String },
    Rename { from: String, to: String },
    Delete { path: String, recursive: bool },
}

pub async fn action(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
    Json(body): Json<FileAction>,
) -> Result<StatusCode, AppError> {
    let pod_name = target_pod_for_files(&state, &server_id).await?;
    let now = Utc::now().timestamp();

    match body {
        FileAction::Mkdir { path } => {
            let p = validate_data_path(&path)?.to_owned();
            if p == "/" {
                return Err(AppError::BadRequest {
                    code: "path_is_root",
                    message: "cannot mkdir the root directory".to_owned(),
                });
            }
            let target = data_path(&p);
            let r = pod_exec_capture(
                &state,
                &state.mc_namespace,
                &pod_name,
                &["mkdir", "-p", &target],
            )
            .await?;
            if r.exit_code != Some(0) {
                return Err(AppError::Internal(anyhow::anyhow!(
                    "mkdir failed: {}",
                    r.stderr.trim()
                )));
            }
            insert_audit(
                &state.pool,
                &server_id,
                "files.mkdir",
                Some(json!({ "path": p })),
                now,
            )
            .await?;
        }
        FileAction::Rename { from, to } => {
            let from_p = validate_data_path(&from)?.to_owned();
            let to_p = validate_data_path(&to)?.to_owned();
            if from_p == "/" || to_p == "/" {
                return Err(AppError::BadRequest {
                    code: "path_is_root",
                    message: "cannot rename involving the root directory".to_owned(),
                });
            }
            let from_t = data_path(&from_p);
            let to_t = data_path(&to_p);
            let r = pod_exec_capture(
                &state,
                &state.mc_namespace,
                &pod_name,
                &["mv", &from_t, &to_t],
            )
            .await?;
            if r.exit_code != Some(0) {
                return Err(AppError::Internal(anyhow::anyhow!(
                    "rename failed: {}",
                    r.stderr.trim()
                )));
            }
            insert_audit(
                &state.pool,
                &server_id,
                "files.rename",
                Some(json!({ "from": from_p, "to": to_p })),
                now,
            )
            .await?;
        }
        FileAction::Delete { path, recursive } => {
            let p = validate_data_path(&path)?.to_owned();
            if p == "/" {
                return Err(AppError::BadRequest {
                    code: "path_is_root",
                    message: "cannot delete the root directory".to_owned(),
                });
            }
            let target = data_path(&p);
            let cmd: Vec<&str> = if recursive {
                vec!["rm", "-rf", &target]
            } else {
                vec!["rm", &target]
            };
            let r = pod_exec_capture(&state, &state.mc_namespace, &pod_name, &cmd).await?;
            if r.exit_code != Some(0) {
                // rm without -r on a directory exits non-zero. Surface
                // this as recursive_required when stderr suggests it.
                let stderr = r.stderr.to_ascii_lowercase();
                if !recursive && stderr.contains("is a directory") {
                    return Err(AppError::BadRequest {
                        code: "recursive_required",
                        message: "target is a directory; pass recursive=true".to_owned(),
                    });
                }
                return Err(AppError::Internal(anyhow::anyhow!(
                    "delete failed: {}",
                    r.stderr.trim()
                )));
            }
            insert_audit(
                &state.pool,
                &server_id,
                "files.delete",
                Some(json!({ "path": p, "recursive": recursive })),
                now,
            )
            .await?;
        }
    }

    Ok(StatusCode::NO_CONTENT)
}
```

- [ ] **Step 2: Wire the routes**

At the bottom of `backend/src/routes/servers/files.rs`, append:

```rust
/// Builds the four file-browser routes for inclusion in the per-server
/// router.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/files", get(list).put(upload).layer(DefaultBodyLimit::max(UPLOAD_CAP_BYTES as usize)))
        .route("/files/raw", get(download))
        .route("/files/action", post(action))
}
```

(The `DefaultBodyLimit` layer applies to the whole `/files` route, which means PUT inherits it. GET requests don't have bodies so the limit is irrelevant for `list`. If axum 0.8 complains about layer placement on a multi-method route, split into two `.route(…)` calls — one for GET, one for PUT — and apply the layer only to the PUT path.)

- [ ] **Step 3: Mount in the per-server router**

In `backend/src/routes/servers/mod.rs`, add:

```rust
pub mod files;
```

Find the function that builds the per-server router (search for where `players::routes()` or similar is merged). Merge:

```rust
let router = router.merge(files::routes());
```

— matching the existing merge style for `players` / `mods` etc.

- [ ] **Step 4: Build and lint**

```bash
cd /home/hadi/gitlab/anvil/backend
cargo fmt --all
cargo build --features serve-dir 2>&1 | tail -15
cargo build --features embed 2>&1 | tail -10
cargo clippy --features serve-dir --all-targets -- -D warnings 2>&1 | tail -15
cargo clippy --features embed --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: clean build, no warnings.

- [ ] **Step 5: Run tests**

```bash
cargo test --features serve-dir 2>&1 | tail -10
```

Expected: existing tests still green.

- [ ] **Step 6: Commit**

```bash
cd /home/hadi/gitlab/anvil
git add backend/src/routes/servers/files.rs backend/src/routes/servers/mod.rs
git commit -m "$(cat <<'EOF'
feat(api): files route module — list, download, upload, action

Four endpoints under /api/servers/{id}/files: GET list, GET /raw,
PUT upload (100 MiB cap, streamed via pod_exec_stream_in), POST
/action (mkdir / rename / delete). Routes through target_pod_for_files
so stopped servers transparently use the helper Pod. Mutating actions
write one audit_log row each.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 4: Backend lifecycle hooks

### Task 14: Tear-down on Start and Delete

**Files:**
- Modify: `backend/src/routes/servers/start.rs`
- Modify: `backend/src/routes/servers/delete.rs`

- [ ] **Step 1: Read existing start.rs**

```bash
sed -n '1,80p' /home/hadi/gitlab/anvil/backend/src/routes/servers/start.rs
```

Locate the line that fetches the server row, and the line that constructs `Api::<StatefulSet>` or calls `patch_scale(...)`. The `tear_down_helper` call goes between them.

- [ ] **Step 2: Insert tear-down in start.rs**

Add the import at the top:

```rust
use crate::files_helper::tear_down_helper;
```

After the `fetch_server_row(&state.pool, &id)` call (which produces `let _server = ...`), and before the StatefulSet API construction, insert:

```rust
tear_down_helper(&state, &id).await?;
```

Note: the function is on `AppError`, so this propagates with `?` directly.

- [ ] **Step 3: Read existing delete.rs**

```bash
sed -n '1,80p' /home/hadi/gitlab/anvil/backend/src/routes/servers/delete.rs
```

Locate the "must_be_stopped" 409 guard and the start of the cleanup sequence (`stsets.delete(...)`).

- [ ] **Step 4: Insert tear-down in delete.rs**

Add the import:

```rust
use crate::files_helper::tear_down_helper;
```

Just before the first `stsets.delete(...)` line (after the must-be-stopped guard returns), insert:

```rust
let _ = tear_down_helper(&state, &id).await; // best-effort
```

- [ ] **Step 5: Build and test**

```bash
cd /home/hadi/gitlab/anvil/backend
cargo fmt --all
cargo build --features serve-dir 2>&1 | tail -10
cargo build --features embed 2>&1 | tail -10
cargo test --features serve-dir 2>&1 | tail -10
cargo clippy --features serve-dir --all-targets -- -D warnings 2>&1 | tail -10
cargo clippy --features embed --all-targets -- -D warnings 2>&1 | tail -10
```

Expected: clean build, all existing tests green, no clippy warnings.

- [ ] **Step 6: Commit**

```bash
cd /home/hadi/gitlab/anvil
git add backend/src/routes/servers/start.rs backend/src/routes/servers/delete.rs
git commit -m "$(cat <<'EOF'
feat(api): tear down files-helper on Start and Delete

Start: fail-fast tear-down before scaling MC up, so the helper Pod
releases the RWO PVC. Delete: best-effort tear-down at the top of
the cleanup sequence so we don't leak a helper if the server is
deleted while stopped.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 5: Helm chart deltas

### Task 15: Helm value, ConfigMap entry, RBAC verb

**Files:**
- Modify: `deploy/values.yaml`
- Modify: `deploy/templates/configmap.yaml`
- Modify: `deploy/templates/role.yaml`

- [ ] **Step 1: Add `mc.filesHelperImage` to values.yaml**

Open `deploy/values.yaml`. Locate the `mc:` (or equivalent) section. Add inside it (or, if no `mc:` block exists, alongside the other `mc*` keys):

```yaml
# Pinned image for the files-helper Pod (sub-project D). Mounted into
# alpine + sleep infinity so anvil can pods/exec into the data PVC
# while the MC server is stopped. Pin by digest, not tag.
filesHelperImage: "alpine@sha256:21dc6063fd678b478f57c0e13f47560d0ea4b4e6c19720b88d02a8c01cf91d04"
```

(The digest is `alpine:3.20` from public.ecr.aws/docker/library/alpine. Confirm at impl time and update if a newer 3.20 patch has been published.)

If the existing `mcDefaults` block is the right home (matching the convention for `storageClassName`, `serviceType`, etc.), nest it under `mcDefaults:` instead:

```yaml
mcDefaults:
  # ... existing keys ...
  filesHelperImage: "alpine@sha256:21dc6063fd678b478f57c0e13f47560d0ea4b4e6c19720b88d02a8c01cf91d04"
```

Pick whichever matches the existing layout — verify by running `grep -n 'mc' deploy/values.yaml` and following the established structure.

- [ ] **Step 2: Add the ConfigMap entry**

Open `deploy/templates/configmap.yaml`. Add a new line under the `data:` section (alongside the other `ANVIL_*` env entries):

```yaml
  ANVIL_FILES_HELPER_IMAGE: {{ .Values.mcDefaults.filesHelperImage | quote }}
```

(If you placed the value under `mc.filesHelperImage` instead of `mcDefaults.filesHelperImage` in Step 1, adjust the template accordingly.)

- [ ] **Step 3: Add the `pods/exec` RBAC verb**

Open `deploy/templates/role.yaml`. Find the existing rule block for `pods/log`. Append a new rule after it:

```yaml
  # File browser (sub-project D) — exec into pods for streamed file ops.
  - apiGroups: [""]
    resources: ["pods/exec"]
    verbs: ["create"]
```

- [ ] **Step 4: Render and inspect**

```bash
cd /home/hadi/gitlab/anvil
helm template deploy/ \
  --set mcDefaults.storageClassName=tank \
  --set oidc.enabled=false \
  > /tmp/anvil-render.yaml
grep -nE '(pods/exec|FILES_HELPER)' /tmp/anvil-render.yaml
```

Expected: both lines render. The `pods/exec` rule appears in the Role; `ANVIL_FILES_HELPER_IMAGE: "alpine@sha256:..."` appears in the ConfigMap.

- [ ] **Step 5: Lint the chart**

```bash
helm lint deploy/ --set mcDefaults.storageClassName=tank
```

Expected: `1 chart(s) linted, 0 chart(s) failed`.

- [ ] **Step 6: Commit**

```bash
git add deploy/values.yaml deploy/templates/configmap.yaml deploy/templates/role.yaml
git commit -m "$(cat <<'EOF'
chore(deploy): pods/exec RBAC + filesHelperImage value

Wires sub-project D's helper-Pod image (digest-pinned alpine) and
the one new RBAC verb (pods/exec: create) the file-browser handlers
need. ConfigMap injects ANVIL_FILES_HELPER_IMAGE; the existing
envFrom in the deployment picks it up automatically.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 6: Frontend foundations

### Task 16: API schemas + fetch helpers

**Files:**
- Modify: `frontend/app/lib/api.ts`

- [ ] **Step 1: Read existing api.ts shape**

```bash
sed -n '1,50p' /home/hadi/gitlab/anvil/frontend/app/lib/api.ts
grep -n 'noContentOrThrow\|ApiError\|export const.*Schema\|export async function fetch' /home/hadi/gitlab/anvil/frontend/app/lib/api.ts | head -20
```

Note the import block, the existing schema export pattern (likely `z.object({ ... })`), and the fetch-wrapper pattern used by `fetchPlayers` / `fetchServerByName` / `runPlayerAction`.

- [ ] **Step 2: Append the schemas**

Append to `frontend/app/lib/api.ts` (place near the other server-related schemas):

```ts
export const fileEntryTypeSchema = z.enum(["f", "d", "l", "o"]);
export type FileEntryType = z.infer<typeof fileEntryTypeSchema>;

export const fileEntrySchema = z.object({
  name: z.string().min(1),
  type: fileEntryTypeSchema,
  size: z.number().nonnegative(),
  mtime: z.number(),
});
export type FileEntry = z.infer<typeof fileEntrySchema>;

export const fileListResponseSchema = z.object({
  path: z.string().startsWith("/"),
  entries: z.array(fileEntrySchema),
});
export type FileListResponse = z.infer<typeof fileListResponseSchema>;

export const fileActionSchema = z.discriminatedUnion("action", [
  z.object({ action: z.literal("mkdir"), path: z.string().min(1) }),
  z.object({
    action: z.literal("rename"),
    from: z.string().min(1),
    to: z.string().min(1),
  }),
  z.object({
    action: z.literal("delete"),
    path: z.string().min(1),
    recursive: z.boolean(),
  }),
]);
export type FileAction = z.infer<typeof fileActionSchema>;
```

- [ ] **Step 3: Append `fetchFileList`**

```ts
export async function fetchFileList(
  serverId: string,
  path: string,
  signal: AbortSignal,
): Promise<FileListResponse> {
  const url = `/api/servers/${encodeURIComponent(serverId)}/files?path=${encodeURIComponent(path)}`;
  const res = await fetch(url, { signal, credentials: "same-origin" });
  if (res.status === 401) {
    redirectToLogin();
    throw new ApiError(401, "unauthorized", "redirecting to login");
  }
  if (!res.ok) {
    const body = await res.json().catch(() => ({ error: res.statusText, code: "unknown" }));
    throw new ApiError(res.status, body.code ?? "unknown", body.error ?? res.statusText);
  }
  const json: unknown = await res.json();
  return fileListResponseSchema.parse(json);
}
```

(If your existing api.ts has a private `fetchJson<T>` helper that handles the 401 redirect + error parsing, refactor `fetchFileList` to use it instead — match the surrounding pattern.)

- [ ] **Step 4: Append `downloadFileUrl`**

```ts
export function downloadFileUrl(serverId: string, path: string): string {
  return `/api/servers/${encodeURIComponent(serverId)}/files/raw?path=${encodeURIComponent(path)}`;
}
```

- [ ] **Step 5: Append `uploadFile`**

```ts
export async function uploadFile(
  serverId: string,
  path: string,
  blob: Blob,
  opts: { onProgress?: (frac: number) => void; signal?: AbortSignal } = {},
): Promise<void> {
  return new Promise<void>((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    const url = `/api/servers/${encodeURIComponent(serverId)}/files?path=${encodeURIComponent(path)}`;
    xhr.open("PUT", url);
    xhr.responseType = "json";
    xhr.upload.onprogress = (e) => {
      if (opts.onProgress && e.lengthComputable) {
        opts.onProgress(e.loaded / e.total);
      }
    };
    xhr.onload = () => {
      if (xhr.status === 401) {
        redirectToLogin();
        reject(new ApiError(401, "unauthorized", "redirecting to login"));
        return;
      }
      if (xhr.status === 204) {
        resolve();
        return;
      }
      const body = xhr.response as { error?: string; code?: string } | null;
      reject(
        new ApiError(
          xhr.status,
          body?.code ?? "unknown",
          body?.error ?? xhr.statusText,
        ),
      );
    };
    xhr.onerror = () => {
      reject(new ApiError(0, "network", "network error during upload"));
    };
    xhr.onabort = () => {
      reject(new ApiError(0, "aborted", "upload cancelled"));
    };
    if (opts.signal) {
      const onAbort = () => xhr.abort();
      opts.signal.addEventListener("abort", onAbort, { once: true });
    }
    xhr.send(blob);
  });
}
```

- [ ] **Step 6: Append `runFileAction`**

```ts
export async function runFileAction(
  serverId: string,
  action: FileAction,
): Promise<void> {
  const res = await fetch(
    `/api/servers/${encodeURIComponent(serverId)}/files/action`,
    {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(action),
      credentials: "same-origin",
    },
  );
  await noContentOrThrow(res);
}
```

(`noContentOrThrow` is the private helper at `frontend/app/lib/api.ts:275` confirmed during exploration.)

- [ ] **Step 7: Lint + typecheck**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm lint 2>&1 | tail -10
pnpm typecheck 2>&1 | tail -10
```

Expected: no errors. Common issue: `redirectToLogin` may be a private helper — replace with whatever the existing `fetchPlayers` uses for 401 handling.

- [ ] **Step 8: Commit**

```bash
cd /home/hadi/gitlab/anvil
git add frontend/app/lib/api.ts
git commit -m "$(cat <<'EOF'
feat(frontend): file API schemas + fetch helpers

Adds Zod schemas for the four file-browser endpoints and four typed
helpers (fetchFileList, downloadFileUrl, uploadFile via XHR for
progress events, runFileAction). All wire boundaries flow through Zod;
ApiError preserved for the existing error display path.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 17: `useFiles` hook

**Files:**
- Create: `frontend/app/lib/use-files.ts`

- [ ] **Step 1: Read use-players.ts as a reference**

```bash
sed -n '1,80p' /home/hadi/gitlab/anvil/frontend/app/lib/use-players.ts
```

Note the AbortController pattern, the `useRef`-based stale-closure break, and the `useEffect` cleanup shape.

- [ ] **Step 2: Create `use-files.ts`**

Create `frontend/app/lib/use-files.ts`:

```ts
"use client";

import { useCallback, useEffect, useRef, useState } from "react";

import {
  ApiError,
  fetchFileList,
  type FileListResponse,
  type ServerDetail,
} from "./api";

export type UseFilesStatus = "loading" | "warming" | "ready" | "error";

export interface UseFilesResult {
  data: FileListResponse | null;
  status: UseFilesStatus;
  lastError: string | null;
  refresh: () => void;
}

/**
 * Re-fetches on `(serverId, path)` change and on `refresh()`. No
 * polling — file lists only change as a result of the same anvil
 * endpoints, so the hook trusts post-action callbacks to nudge it.
 *
 * `status === "warming"` covers the helper-Pod boot path on the very
 * first request when the server is stopped (5–15 s). Subsequent fetches
 * use plain `loading`.
 */
export function useFiles(
  serverId: string,
  path: string,
  opts: { enabled: boolean; serverStatus: ServerDetail["status"] },
): UseFilesResult {
  const [data, setData] = useState<FileListResponse | null>(null);
  const [status, setStatus] = useState<UseFilesStatus>("loading");
  const [lastError, setLastError] = useState<string | null>(null);
  const abortRef = useRef<AbortController | null>(null);
  const firstFetchRef = useRef(true);
  const tickRef = useRef(0);

  const doFetch = useCallback(
    async (warming: boolean) => {
      abortRef.current?.abort();
      const ctrl = new AbortController();
      abortRef.current = ctrl;
      setStatus(warming ? "warming" : "loading");
      const myTick = ++tickRef.current;
      try {
        const result = await fetchFileList(serverId, path, ctrl.signal);
        if (tickRef.current !== myTick) return;
        setData(result);
        setStatus("ready");
        setLastError(null);
        firstFetchRef.current = false;
      } catch (err: unknown) {
        if (err instanceof DOMException && err.name === "AbortError") return;
        if (tickRef.current !== myTick) return;
        const message =
          err instanceof ApiError
            ? `${err.code}: ${err.message}`
            : err instanceof Error
              ? err.message
              : "unknown error";
        setStatus("error");
        setLastError(message);
      }
    },
    [serverId, path],
  );

  useEffect(() => {
    if (!opts.enabled) {
      setStatus("loading");
      setData(null);
      return undefined;
    }
    const warming = firstFetchRef.current && opts.serverStatus === "stopped";
    void doFetch(warming);
    return () => {
      abortRef.current?.abort();
    };
  }, [doFetch, opts.enabled, opts.serverStatus]);

  const refresh = useCallback(() => {
    void doFetch(false);
  }, [doFetch]);

  return { data, status, lastError, refresh };
}
```

- [ ] **Step 3: Lint + typecheck**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm lint 2>&1 | tail -10
pnpm typecheck 2>&1 | tail -10
```

Expected: no errors.

- [ ] **Step 4: Commit**

```bash
cd /home/hadi/gitlab/anvil
git add frontend/app/lib/use-files.ts
git commit -m "$(cat <<'EOF'
feat(frontend): useFiles hook — path-driven fetch + warming state

Re-fetches on (serverId, path) change and on refresh(); no polling.
Surfaces a 'warming' status on the first stopped-server fetch so the
UI can render 'starting offline file editor…' copy while the helper
Pod boots. AbortController per fetch, tick guard against late
responses.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 7: Generic dialogs

### Task 18: Generalise `ConfirmDeleteDialog`

**Files:**
- Modify: `frontend/app/components/ConfirmDeleteDialog.tsx`
- Modify: every existing callsite (verify with grep)

- [ ] **Step 1: Find every callsite**

```bash
cd /home/hadi/gitlab/anvil
rg -n 'ConfirmDeleteDialog' frontend/
```

Expected: definition + at least one call (likely the server-delete row in `ServerDetailView.tsx` or `ServerList.tsx`).

- [ ] **Step 2: Read the existing component**

```bash
cat /home/hadi/gitlab/anvil/frontend/app/components/ConfirmDeleteDialog.tsx
```

Note the current props (`{ open, onClose, serverId, serverName, onDeleted }`) and the embedded `deleteServer` API call.

- [ ] **Step 3: Replace the file with the generalised version**

Overwrite `frontend/app/components/ConfirmDeleteDialog.tsx`:

```tsx
"use client";

import { useEffect, useState, type ReactElement } from "react";

import { Button } from "./Button";
import { Modal } from "./Modal";

export interface ConfirmDeleteDialogProps {
  open: boolean;
  onClose: () => void;
  /** The string the user must type to enable confirm. */
  targetName: string;
  /** Optional override for the verb shown on the busy button. Default: "deleting…". */
  busyLabel?: string;
  /** Called when the user clicks confirm. The dialog closes on resolve. */
  onConfirm: () => Promise<void>;
}

/**
 * Generic "type the name to confirm" destructive dialog. Used for both
 * server delete (sub-project A) and recursive folder delete (sub-project
 * D). Caller owns the API call; this component owns the typed-name
 * pattern, the busy state, and the Modal lifecycle.
 */
export function ConfirmDeleteDialog({
  open,
  onClose,
  targetName,
  busyLabel = "deleting…",
  onConfirm,
}: ConfirmDeleteDialogProps): ReactElement | null {
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) {
      setTyped("");
      setBusy(false);
      setError(null);
    }
  }, [open]);

  const matches = typed === targetName;

  const handleConfirm = async (): Promise<void> => {
    if (!matches || busy) return;
    setBusy(true);
    setError(null);
    try {
      await onConfirm();
      onClose();
    } catch (err: unknown) {
      const message = err instanceof Error ? err.message : "delete failed";
      setError(message);
      setBusy(false);
    }
  };

  return (
    <Modal open={open} onClose={busy ? () => {} : onClose} title="confirm delete">
      <div className="space-y-3 font-mono text-[12px]">
        <p className="text-text-body">
          type <span className="text-accent">{targetName}</span> to confirm.
        </p>
        <input
          type="text"
          value={typed}
          onChange={(e) => {
            setTyped(e.target.value);
          }}
          className="w-full rounded-sm border border-border bg-bg px-2 py-1 text-text-primary"
          autoFocus
          disabled={busy}
        />
        {error !== null && (
          <p className="text-state-error">{error}</p>
        )}
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            cancel
          </Button>
          <Button
            variant="danger"
            onClick={() => {
              void handleConfirm();
            }}
            disabled={!matches || busy}
          >
            {busy ? busyLabel : "delete"}
          </Button>
        </div>
      </div>
    </Modal>
  );
}
```

- [ ] **Step 4: Update the existing server-delete callsite**

For each callsite found in Step 1, change:

```tsx
<ConfirmDeleteDialog
  open={confirmOpen}
  onClose={() => setConfirmOpen(false)}
  serverId={detail.id}
  serverName={detail.name}
  onDeleted={() => router.push("/")}
/>
```

to:

```tsx
<ConfirmDeleteDialog
  open={confirmOpen}
  onClose={() => setConfirmOpen(false)}
  targetName={detail.name}
  onConfirm={async () => {
    await deleteServer(detail.id);
    router.push("/");
  }}
/>
```

(Adjust the success path — `onDeleted` likely did the route push; move it into the `onConfirm`'s post-delete code.)

- [ ] **Step 5: Lint + typecheck + build**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm lint 2>&1 | tail -10
pnpm typecheck 2>&1 | tail -10
pnpm build 2>&1 | tail -20
```

Expected: no errors. The build produces a static export under `out/`.

- [ ] **Step 6: Commit**

```bash
cd /home/hadi/gitlab/anvil
git add frontend/app/components/ConfirmDeleteDialog.tsx frontend/app/servers/ServerDetailView.tsx
# Add any other touched callsite files surfaced by the rg in Step 1.
git commit -m "$(cat <<'EOF'
refactor(frontend): generalise ConfirmDeleteDialog

Now takes targetName + onConfirm callback. Server-delete callsite
passes deleteServer + router.push directly. Sub-project D will
reuse this for recursive folder delete (typing the folder name to
confirm). No behaviour change for server delete.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 19: `UploadFileDialog`

**Files:**
- Create: `frontend/app/components/UploadFileDialog.tsx`

- [ ] **Step 1: Create the component**

Create `frontend/app/components/UploadFileDialog.tsx`:

```tsx
"use client";

import { useEffect, useRef, useState, type ReactElement } from "react";

import { uploadFile, ApiError } from "../lib/api";
import { Button } from "./Button";
import { Modal } from "./Modal";
import { useToast } from "./Toast";

const UPLOAD_CAP_BYTES = 100 * 1024 * 1024;

export interface UploadFileDialogProps {
  open: boolean;
  onClose: () => void;
  serverId: string;
  /** Directory path to upload into, e.g. "/mods" or "/". */
  parentPath: string;
  onUploaded: () => void;
}

export function UploadFileDialog({
  open,
  onClose,
  serverId,
  parentPath,
  onUploaded,
}: UploadFileDialogProps): ReactElement | null {
  const [file, setFile] = useState<File | null>(null);
  const [progress, setProgress] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const abortRef = useRef<AbortController | null>(null);
  const toast = useToast();

  useEffect(() => {
    if (!open) {
      setFile(null);
      setProgress(0);
      setBusy(false);
      setError(null);
      abortRef.current?.abort();
      abortRef.current = null;
    }
  }, [open]);

  const targetPath = (): string => {
    if (file === null) return parentPath;
    const base = parentPath.endsWith("/") ? parentPath : `${parentPath}/`;
    return `${base}${file.name}`;
  };

  const send = async (): Promise<void> => {
    if (file === null) return;
    if (file.size > UPLOAD_CAP_BYTES) {
      setError(`file too large (max ${UPLOAD_CAP_BYTES / 1024 / 1024} MiB)`);
      return;
    }
    setBusy(true);
    setError(null);
    setProgress(0);
    const ctrl = new AbortController();
    abortRef.current = ctrl;
    try {
      await uploadFile(serverId, targetPath(), file, {
        onProgress: setProgress,
        signal: ctrl.signal,
      });
      toast.push(`uploaded ${file.name}`, "success");
      onUploaded();
      onClose();
    } catch (err: unknown) {
      const message =
        err instanceof ApiError
          ? `${err.code}: ${err.message}`
          : err instanceof Error
            ? err.message
            : "upload failed";
      setError(message);
      setBusy(false);
    }
  };

  const cancel = (): void => {
    abortRef.current?.abort();
    onClose();
  };

  return (
    <Modal open={open} onClose={busy ? () => {} : onClose} title="upload file">
      <div className="space-y-3 font-mono text-[12px]">
        <p className="text-text-muted">
          uploading into <span className="text-text-body">{parentPath}</span>
        </p>
        <input
          type="file"
          onChange={(e) => {
            setFile(e.target.files?.[0] ?? null);
            setError(null);
          }}
          disabled={busy}
          className="block w-full"
        />
        {file !== null && (
          <p className="text-text-muted">
            {file.name} · {Math.round(file.size / 1024)} KiB
          </p>
        )}
        {busy && (
          <div className="h-1 w-full bg-border-soft">
            <div
              className="h-full bg-accent transition-[width] duration-150"
              style={{ width: `${Math.round(progress * 100)}%` }}
            />
          </div>
        )}
        {error !== null && <p className="text-state-error">{error}</p>}
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={cancel}>
            {busy ? "cancel" : "close"}
          </Button>
          <Button
            variant="primary"
            onClick={() => {
              void send();
            }}
            disabled={file === null || busy}
          >
            {busy ? "uploading…" : "send"}
          </Button>
        </div>
      </div>
    </Modal>
  );
}
```

- [ ] **Step 2: Lint + typecheck**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm lint 2>&1 | tail -10
pnpm typecheck 2>&1 | tail -10
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
cd /home/hadi/gitlab/anvil
git add frontend/app/components/UploadFileDialog.tsx
git commit -m "$(cat <<'EOF'
feat(frontend): UploadFileDialog — file picker + progress + toast

Modal wraps the XMLHttpRequest-based uploadFile helper. Live progress
bar driven by upload.onprogress events. 100 MiB enforced client-side
before XHR fires. Toast on success; error rendered inline on failure.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 20: `NameInputDialog`

**Files:**
- Create: `frontend/app/components/NameInputDialog.tsx`

- [ ] **Step 1: Create the component**

Create `frontend/app/components/NameInputDialog.tsx`:

```tsx
"use client";

import { useEffect, useState, type ReactElement } from "react";

import { Button } from "./Button";
import { Modal } from "./Modal";

export interface NameInputDialogProps {
  open: boolean;
  onClose: () => void;
  mode: "create" | "rename";
  /** Empty string for create mode; the existing name for rename. */
  initialValue: string;
  /** Called with the typed name when the user submits. */
  onSubmit: (name: string) => Promise<void>;
}

const SEGMENT_RE = /^[\x20-\x7E]+$/;

function validateSegment(name: string): string | null {
  if (name.length === 0) return "name cannot be empty";
  if (name === "." || name === "..") return "'.' and '..' are reserved";
  if (name.startsWith("-")) return "name may not start with '-'";
  if (name.length > 255) return "name too long (max 255 bytes)";
  if (name.includes("/")) return "name may not contain '/'";
  if (!SEGMENT_RE.test(name)) return "only printable ASCII allowed";
  if (name.includes("'") || name.includes("\\")) return "single-quotes and backslashes are not allowed";
  return null;
}

export function NameInputDialog({
  open,
  onClose,
  mode,
  initialValue,
  onSubmit,
}: NameInputDialogProps): ReactElement | null {
  const [value, setValue] = useState(initialValue);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      setValue(initialValue);
      setBusy(false);
      setError(null);
    }
  }, [open, initialValue]);

  const validation = validateSegment(value);
  const canSubmit = validation === null && !busy;

  const submit = async (): Promise<void> => {
    if (!canSubmit) return;
    setBusy(true);
    setError(null);
    try {
      await onSubmit(value);
      onClose();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "operation failed");
      setBusy(false);
    }
  };

  const title = mode === "create" ? "new folder" : "rename";
  const submitLabel = mode === "create" ? "create" : "rename";

  return (
    <Modal open={open} onClose={busy ? () => {} : onClose} title={title}>
      <div className="space-y-3 font-mono text-[12px]">
        <input
          type="text"
          value={value}
          onChange={(e) => {
            setValue(e.target.value);
            setError(null);
          }}
          autoFocus
          disabled={busy}
          className="w-full rounded-sm border border-border bg-bg px-2 py-1 text-text-primary"
          placeholder={mode === "create" ? "folder-name" : ""}
        />
        {validation !== null && value.length > 0 && (
          <p className="text-state-warning">{validation}</p>
        )}
        {error !== null && <p className="text-state-error">{error}</p>}
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            cancel
          </Button>
          <Button
            variant="primary"
            onClick={() => {
              void submit();
            }}
            disabled={!canSubmit}
          >
            {busy ? `${submitLabel}…` : submitLabel}
          </Button>
        </div>
      </div>
    </Modal>
  );
}
```

- [ ] **Step 2: Lint + typecheck**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm lint 2>&1 | tail -10
pnpm typecheck 2>&1 | tail -10
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
cd /home/hadi/gitlab/anvil
git add frontend/app/components/NameInputDialog.tsx
git commit -m "$(cat <<'EOF'
feat(frontend): NameInputDialog — shared by mkdir + rename

One Modal with one input + a client-side validator that mirrors
backend validate_data_path's segment rules (printable ASCII minus
single-quote and backslash; no '..', '.', leading dash, or '/').
Caller owns the API call; the dialog owns the input + busy state.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 8: Files tab body

### Task 21: `FileEntryRow` + `FileActionMenu`

**Files:**
- Create: `frontend/app/components/FileEntryRow.tsx`
- Create: `frontend/app/components/FileActionMenu.tsx`

- [ ] **Step 1: Create `FileActionMenu`**

Create `frontend/app/components/FileActionMenu.tsx`:

```tsx
"use client";

import { type ReactElement } from "react";

import { type FileEntryType } from "../lib/api";
import { Dropdown } from "./Dropdown";

export interface FileActionMenuProps {
  entryType: FileEntryType;
  onDownload: () => void;
  onRename: () => void;
  onDelete: () => void;
}

export function FileActionMenu({
  entryType,
  onDownload,
  onRename,
  onDelete,
}: FileActionMenuProps): ReactElement {
  const items = [
    ...(entryType === "f"
      ? [{ id: "download", label: "download", onSelect: onDownload }]
      : []),
    { id: "rename", label: "rename", onSelect: onRename },
    {
      id: "delete",
      label: entryType === "d" ? "delete (recursive)" : "delete",
      onSelect: onDelete,
    },
  ];

  return (
    <Dropdown
      ariaLabel="file actions"
      trigger={<span aria-hidden>⋯</span>}
      items={items}
    />
  );
}
```

- [ ] **Step 2: Create `FileEntryRow`**

Create `frontend/app/components/FileEntryRow.tsx`:

```tsx
"use client";

import { type ReactElement } from "react";

import { type FileEntry, type FileEntryType } from "../lib/api";
import { FileActionMenu } from "./FileActionMenu";

export interface FileEntryRowProps {
  entry: FileEntry;
  onNavigate: (toPath: string) => void;
  onDownload: () => void;
  onRename: () => void;
  onDelete: () => void;
  /** Current directory path, used to construct the navigated-to path. */
  parentPath: string;
}

function humanSize(bytes: number, type: FileEntryType): string {
  if (type === "d") return "─";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MiB`;
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GiB`;
}

function relativeTime(unixSeconds: number): string {
  const diff = Math.round(Date.now() / 1000 - unixSeconds);
  if (diff < 60) return `${diff}s ago`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  return `${Math.floor(diff / 86400)}d ago`;
}

function glyph(type: FileEntryType): string {
  switch (type) {
    case "d": return "/";
    case "l": return "→";
    case "f":
    case "o":
    default:
      return " ";
  }
}

export function FileEntryRow({
  entry,
  onNavigate,
  onDownload,
  onRename,
  onDelete,
  parentPath,
}: FileEntryRowProps): ReactElement {
  const onNameClick = (): void => {
    if (entry.entry_type === "d") {
      const next = parentPath === "/" ? `/${entry.name}` : `${parentPath}/${entry.name}`;
      onNavigate(next);
    } else if (entry.entry_type === "f") {
      onDownload();
    }
  };

  return (
    <div className="grid grid-cols-[auto_1fr_auto_auto_auto] items-center gap-3 px-3 py-1 hover:bg-elevated">
      <span className="w-3 text-center font-mono text-[12px] text-text-faint">
        {glyph(entry.entry_type)}
      </span>
      <button
        type="button"
        onClick={onNameClick}
        className="text-left font-mono text-[12px] text-text-primary hover:text-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
      >
        {entry.name}
      </button>
      <span className="font-mono text-[12px] text-text-muted">
        {humanSize(entry.size, entry.entry_type)}
      </span>
      <span className="font-mono text-[12px] text-text-muted">
        {relativeTime(entry.mtime)}
      </span>
      <FileActionMenu
        entryType={entry.entry_type}
        onDownload={onDownload}
        onRename={onRename}
        onDelete={onDelete}
      />
    </div>
  );
}
```

- [ ] **Step 3: Lint + typecheck**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm lint 2>&1 | tail -10
pnpm typecheck 2>&1 | tail -10
```

Expected: no errors. Common issue: the Dropdown `items` shape may differ slightly — match `frontend/app/components/PlayerActionMenu.tsx` for the exact prop names.

- [ ] **Step 4: Commit**

```bash
cd /home/hadi/gitlab/anvil
git add frontend/app/components/FileEntryRow.tsx frontend/app/components/FileActionMenu.tsx
git commit -m "$(cat <<'EOF'
feat(frontend): FileEntryRow + FileActionMenu

Per-row composition for the upcoming FilesBody. Type-glyph,
clickable name (navigate dir / download file), size + relative
mtime, per-type action menu (file → download/rename/delete; dir →
rename/delete-recursive; symlink → rename/delete).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 22: `FilesBody` rewrite

**Files:**
- Modify: `frontend/app/servers/tabs/FilesBody.tsx`

- [ ] **Step 1: Read the existing stub**

```bash
cat /home/hadi/gitlab/anvil/frontend/app/servers/tabs/FilesBody.tsx
```

It's the placeholder Card from sub-project A. Replace entirely.

- [ ] **Step 2: Replace with the full surface**

Overwrite `frontend/app/servers/tabs/FilesBody.tsx`:

```tsx
"use client";

import { useRouter, useSearchParams } from "next/navigation";
import { useContext, useState, type ReactElement } from "react";

import {
  ApiError,
  downloadFileUrl,
  runFileAction,
  startServer,
  type FileEntry,
} from "../../lib/api";
import { ServerDetailContext } from "../../lib/server-detail-context";
import { useFiles } from "../../lib/use-files";

import { Button } from "../../components/Button";
import { Card } from "../../components/Card";
import { ConfirmDeleteDialog } from "../../components/ConfirmDeleteDialog";
import { FileEntryRow } from "../../components/FileEntryRow";
import { NameInputDialog } from "../../components/NameInputDialog";
import { PathBreadcrumb } from "../../components/PathBreadcrumb";
import { Skeleton } from "../../components/Skeleton";
import { useToast } from "../../components/Toast";
import { UploadFileDialog } from "../../components/UploadFileDialog";

export function FilesBody(): ReactElement {
  const detail = useContext(ServerDetailContext);
  const router = useRouter();
  const search = useSearchParams();
  const toast = useToast();

  if (detail === null) {
    return (
      <Card>
        <p className="font-mono text-[12px] text-state-error">
          server context missing
        </p>
      </Card>
    );
  }

  const path = search.get("path") ?? "/";
  const enabled =
    detail.status === "running" ||
    detail.status === "stopped";

  const { data, status, lastError, refresh } = useFiles(detail.id, path, {
    enabled,
    serverStatus: detail.status,
  });

  const [uploadOpen, setUploadOpen] = useState(false);
  const [folderOpen, setFolderOpen] = useState(false);
  const [renameOpen, setRenameOpen] = useState<FileEntry | null>(null);
  const [confirmFile, setConfirmFile] = useState<FileEntry | null>(null);
  const [confirmDir, setConfirmDir] = useState<FileEntry | null>(null);

  const navigate = (toPath: string): void => {
    const params = new URLSearchParams(Array.from(search.entries()));
    params.set("path", toPath);
    router.push(`?${params.toString()}`);
  };

  const childPath = (name: string): string =>
    path === "/" ? `/${name}` : `${path}/${name}`;

  const triggerDownload = (entry: FileEntry): void => {
    const url = downloadFileUrl(detail.id, childPath(entry.name));
    const a = document.createElement("a");
    a.href = url;
    a.download = entry.name;
    document.body.append(a);
    a.click();
    a.remove();
  };

  // ---------- gates ----------

  if (!enabled) {
    return (
      <Card>
        <p className="font-mono text-[12px] text-text-muted">
          server is in transition · refresh in a moment
        </p>
      </Card>
    );
  }

  if (status === "warming") {
    return (
      <Card>
        <p className="mb-3 font-mono text-[12px] text-text-muted">
          starting offline file editor…
        </p>
        <Skeleton variant="row" />
        <Skeleton variant="row" />
        <Skeleton variant="row" />
      </Card>
    );
  }

  if (status === "error" && lastError !== null) {
    if (lastError.includes("pvc_not_initialized")) {
      return (
        <Card>
          <p className="mb-3 font-mono text-[12px] text-text-muted">
            start the server once to initialize storage.
          </p>
          <Button
            variant="primary"
            onClick={() => {
              startServer(detail.id)
                .then(() => {
                  toast.push(`${detail.name} · start ok`, "success");
                })
                .catch((err: unknown) => {
                  const msg =
                    err instanceof ApiError
                      ? `${err.code}: ${err.message}`
                      : err instanceof Error
                        ? err.message
                        : "start failed";
                  toast.push(msg, "error");
                });
            }}
          >
            start server
          </Button>
        </Card>
      );
    }
    return (
      <Card>
        <p className="font-mono text-[12px] text-state-error">
          failed to load · {lastError}
        </p>
      </Card>
    );
  }

  if (status === "loading" || data === null) {
    return (
      <Card>
        <Skeleton variant="row" />
        <Skeleton variant="row" />
        <Skeleton variant="row" />
      </Card>
    );
  }

  // ---------- main surface ----------

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between gap-3">
        <PathBreadcrumb segments={["data", ...path.split("/").filter(Boolean)]} />
        <div className="flex gap-2">
          <Button variant="ghost" onClick={() => setFolderOpen(true)}>
            + folder
          </Button>
          <Button variant="primary" onClick={() => setUploadOpen(true)}>
            upload
          </Button>
        </div>
      </div>

      <Card>
        {data.entries.length === 0 ? (
          <p className="px-3 py-2 font-mono text-[12px] text-text-muted">
            empty directory
          </p>
        ) : (
          data.entries.map((entry) => (
            <FileEntryRow
              key={entry.name}
              entry={entry}
              parentPath={path}
              onNavigate={navigate}
              onDownload={() => triggerDownload(entry)}
              onRename={() => setRenameOpen(entry)}
              onDelete={() =>
                entry.entry_type === "d"
                  ? setConfirmDir(entry)
                  : setConfirmFile(entry)
              }
            />
          ))
        )}
      </Card>

      <UploadFileDialog
        open={uploadOpen}
        onClose={() => setUploadOpen(false)}
        serverId={detail.id}
        parentPath={path}
        onUploaded={refresh}
      />

      <NameInputDialog
        open={folderOpen}
        onClose={() => setFolderOpen(false)}
        mode="create"
        initialValue=""
        onSubmit={async (name) => {
          await runFileAction(detail.id, {
            action: "mkdir",
            path: childPath(name),
          });
          toast.push(`created ${name}/`, "success");
          refresh();
        }}
      />

      {renameOpen !== null && (
        <NameInputDialog
          open={true}
          onClose={() => setRenameOpen(null)}
          mode="rename"
          initialValue={renameOpen.name}
          onSubmit={async (name) => {
            await runFileAction(detail.id, {
              action: "rename",
              from: childPath(renameOpen.name),
              to: childPath(name),
            });
            toast.push(`renamed ${renameOpen.name} → ${name}`, "success");
            refresh();
          }}
        />
      )}

      {confirmFile !== null && (
        <ConfirmDeleteDialog
          open={true}
          onClose={() => setConfirmFile(null)}
          targetName={confirmFile.name}
          onConfirm={async () => {
            await runFileAction(detail.id, {
              action: "delete",
              path: childPath(confirmFile.name),
              recursive: false,
            });
            toast.push(`deleted ${confirmFile.name}`, "success");
            refresh();
          }}
        />
      )}

      {confirmDir !== null && (
        <ConfirmDeleteDialog
          open={true}
          onClose={() => setConfirmDir(null)}
          targetName={confirmDir.name}
          busyLabel="deleting recursively…"
          onConfirm={async () => {
            await runFileAction(detail.id, {
              action: "delete",
              path: childPath(confirmDir.name),
              recursive: true,
            });
            toast.push(`deleted ${confirmDir.name}/`, "success");
            refresh();
          }}
        />
      )}
    </div>
  );
}
```

- [ ] **Step 3: Lint + typecheck + build**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm lint 2>&1 | tail -15
pnpm typecheck 2>&1 | tail -15
pnpm build 2>&1 | tail -20
```

Expected: no errors. Common issues:
- `PathBreadcrumb` may take a different prop shape (e.g. `path: string` not `segments: string[]`). Match the existing usage in `CommandBar.tsx`.
- `Skeleton` variant names may differ; check `frontend/app/components/Skeleton.tsx`.
- `Dropdown` items prop shape — verify against `PlayerActionMenu.tsx`.

If any prop mismatch surfaces, fix locally and re-lint.

- [ ] **Step 4: Commit**

```bash
cd /home/hadi/gitlab/anvil
git add frontend/app/servers/tabs/FilesBody.tsx
git commit -m "$(cat <<'EOF'
feat(frontend): Files tab body — full surface

Replaces the v2.x placeholder. Toolbar (PathBreadcrumb + [+ folder]
+ [upload]) above a Card listing entries via FileEntryRow.
URL-state navigation via ?path=... so back/forward work. Helper-Pod
warming state, never-started gate, single-file vs recursive folder
delete confirmations all wired through the existing primitives.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 9: Verification + ship

### Task 23: Quality gates + manual smoke + milestones + ship

**Files:**
- Modify: `docs/milestones.md`

- [ ] **Step 1: Run all backend gates**

```bash
cd /home/hadi/gitlab/anvil/backend
cargo fmt --check 2>&1 | tail -5
cargo test --all 2>&1 | tail -15
cargo clippy --all-targets --features serve-dir -- -D warnings 2>&1 | tail -10
cargo clippy --all-targets --features embed -- -D warnings 2>&1 | tail -10
```

Expected: all green.

- [ ] **Step 2: Run all frontend gates**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm lint 2>&1 | tail -10
pnpm typecheck 2>&1 | tail -10
pnpm build 2>&1 | tail -20
```

Expected: all green; `out/` directory rebuilt cleanly.

- [ ] **Step 3: Render Helm chart**

```bash
cd /home/hadi/gitlab/anvil
helm lint deploy/ --set mcDefaults.storageClassName=tank
helm template deploy/ \
  --set mcDefaults.storageClassName=tank \
  --set oidc.enabled=false \
  > /tmp/anvil-render.yaml
grep -nE '(pods/exec|FILES_HELPER)' /tmp/anvil-render.yaml
```

Expected: 1 chart linted, 0 failed; both grep matches present.

- [ ] **Step 4: Manual smoke (live cluster + existing anvil instance)**

This requires a running anvil instance with at least one managed server. Walk through every checkbox in spec §10:

```text
[ ] Files tab on a never-started server: 'pvc_not_initialized' gate + start button
[ ] Files tab on a stopped server: 'starting offline editor…' → entries appear
[ ] Stopped → upload mod jar → start: jar present in running pod's /data/mods
[ ] Files tab on running server: list /, /world, /mods, /.fabric
[ ] Upload: 5 MiB jar → toast → row appears
[ ] Upload cap: 200 MiB file → client-side error before XHR fires
[ ] Mkdir: new folder dialog → name → row appears, toast 'created test/'
[ ] Rename: foo.jar → foo.jar.disabled, toast
[ ] Download: server.properties matches `kubectl exec ... cat`
[ ] Single-file delete: light Modal → confirm → row leaves, toast
[ ] Recursive delete: ConfirmDeleteDialog (type name) → confirm → toast 'deleted X/'
[ ] Path traversal: ?path=../etc/passwd → 400 path_invalid
[ ] Argv injection: ?path=/-rf → 400 segment_leading_dash
[ ] Helper teardown blocks Start: 90 MiB upload mid-stream + Start → upload aborts, MC starts
```

- [ ] **Step 5: Verify audit log**

```bash
sqlite3 /var/lib/anvil/anvil.db \
  "SELECT action, details FROM audit_log WHERE action LIKE 'files.%' ORDER BY id DESC LIMIT 10;"
```

(Adjust path if running locally with a different `ANVIL_DATABASE_URL`.) Expected: rows for `files.upload`, `files.mkdir`, `files.rename`, `files.delete` with the §5.3 payloads.

- [ ] **Step 6: Update milestones**

Open `docs/milestones.md`. Locate the "v2 series" block (lines 191–208 currently). Replace the "**D — File browser sidecar** — pending." line with:

```markdown
- **D — File browser** ✅ (2026-05-03): in-anvil FS endpoints over `kube-rs`
  pods/exec — list / download / upload (≤ 100 MiB, streamed) / mkdir / rename /
  delete (single + recursive). Stopped servers handled by a lazy-spawned
  helper Pod (`mc-{id}-files`) torn down on Start. **Adds one RBAC verb
  (`pods/exec: create`), one Helm value (`mc.filesHelperImage`), no DB
  migration, no new top-level dependencies (kube `ws`+`runtime` features
  enabled, async-stream@0.3 added).** Spec:
  `docs/superpowers/specs/2026-05-03-anvil-v2-file-browser-design.md`.
```

- [ ] **Step 7: Self-review against spec**

Re-read `docs/superpowers/specs/2026-05-03-anvil-v2-file-browser-design.md`. Confirm every §10 checkbox passes. Note any §11 open questions resolved during impl (likely #2 helper-pod digest pinned to a specific value; #3 stat pre-flight retained for cleaner errors).

- [ ] **Step 8: Final commit**

```bash
git add docs/milestones.md
git commit -m "$(cat <<'EOF'
docs(milestones): mark sub-project D complete

D shipped: in-anvil file browser over kube-rs pods/exec, with a
helper Pod handling stopped servers. v2 series (A/B/C/D) is now
complete; next session is Phase 4 (FluxCD deploy).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 9: Verify the working tree is clean**

```bash
git status
git log --oneline | head -25
```

Expected: working tree clean; recent log shows the Phase-1-through-9 commits.

---

## Self-review (against spec)

| Spec section | Coverage |
|---|---|
| §1 Context | Architecture preserved: helper Pod for stopped servers, no sidecar, no deep-link, no external FB. |
| §2 Scope (in scope) | All 10 in-scope items have tasks. |
| §2 Scope (out of scope) | No tasks pulled in for tar/edit/multi-file/janitor/structured-editor/previews/search. |
| §3 Anti-OE guardrails | No background tasks (helper has no TTL/janitor). No new top-level deps (kube features + `async-stream` flagged). No new RBAC except `pods/exec: create`. No DB migration. Helper is bare Pod. Single validator. Argv-only execs. |
| §4 Design POV | FilesBody uses workshop tokens; copper accent only on `[upload]` / `[+ folder]` brackets and active dropdown chevron. Mono for filenames. |
| §5.1 Wire types | `FileEntry` / `FileEntryType` / `FileListResponse` defined in Task 3. |
| §5.2 Action body | `FileAction` discriminated enum defined in Task 13. |
| §5.3 Audit log | `files.upload`, `files.mkdir`, `files.rename`, `files.delete` written in Tasks 12-13 with the spec'd payloads. |
| §6.1 Parsing module | Task 3. |
| §6.2 `validate_data_path` | Task 2. |
| §6.3 Exec primitives | Tasks 7-8. |
| §6.4 Helper lifecycle | Task 9. |
| §6.5 Helper builder | Task 4. |
| §6.6 Route module | Tasks 10-13. |
| §6.7 Lifecycle hooks | Task 14. |
| §6.8 Wiring | Tasks 5, 13. |
| §6.9 Cargo / dep changes | Task 1 (kube features); Task 8 notes `async-stream@0.3` addition. |
| §7.1 Schemas + API | Task 16. |
| §7.2 `useFiles` | Task 17. |
| §7.3 FilesBody composition | Task 22. |
| §7.4 New components | Tasks 18-21. |
| §7.5 Detail-page wiring | URL state via `?path=` — Task 22 reads `useSearchParams`. ServerDetailView already routes the `files` tab to FilesBody (sub-project A). |
| §7.6 Tab visibility | FilesBody renders for every kind — no kind-specific gating. |
| §8 k8s | Task 15. |
| §9 Migration | None — confirmed in Task 5 + Task 15 (no DB schema change, no SS shape change). |
| §10 Verification | Task 23. |
| §11 Open questions | Resolved at impl time per Task 23 step 7. |
| §13 Critical files modified | All listed paths covered by tasks. |

**Placeholder scan:** No `TBD` / `TODO` / "implement later" in this plan. Each step has executable code or a precise instruction. Where a step depends on existing patterns ("match the surrounding pattern"), the surrounding file is named for the engineer to inspect.

**Type consistency:** `FileEntryType` enum values agree (`f` / `d` / `l` / `o`) across spec, parser, schema, and components. `FileAction` discriminator shape matches between backend `serde(tag="action")` and frontend `z.discriminatedUnion("action", …)`. Helper Pod name (`mc-{id}-files`) consistent across builder, lifecycle, target picker, and start/delete hooks.

**Spec deviation note:** §13 of the spec listed `deploy/templates/deployment.yaml` as the place to wire `ANVIL_FILES_HELPER_IMAGE`. The actual env injection pattern goes through `deploy/templates/configmap.yaml` (the deployment uses one `envFrom.configMapRef`). Task 15 corrects this; the deployment template is unchanged.
