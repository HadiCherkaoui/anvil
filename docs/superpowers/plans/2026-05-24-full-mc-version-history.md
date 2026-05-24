# Full Minecraft Version History — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users select any official Minecraft release (1.0 → latest) when creating vanilla or Paper servers, by removing the 20/25-version recency cap on the backend version endpoints and extending the offline fallback.

**Architecture:** Three targeted edits in the backend. (1) `routes/mc_versions.rs` — drop `MAX_VERSIONS = 20` cap; (2) `routes/papermc.rs` — drop `MAX_VERSIONS = 25` cap; (3) `validation.rs` — extend `KNOWN_MC_VERSIONS` offline floor with popular legacy anchors so cold-cache + upstream-outage still accepts common legacy versions. Tests rewritten to assert the new "no cap" contract. Frontend is untouched — the native `<select>` renders whatever the backend returns.

**Tech Stack:** Rust 1.83+, axum 0.8, cargo for tests/clippy/fmt. No new dependencies.

---

## File Map

| File | Change | Responsibility |
|------|--------|----------------|
| `backend/src/routes/mc_versions.rs` | Modify | Drop cap from `parse_manifest`, update module docstring + `McVersionsResponse` doc, rewrite cap tests. |
| `backend/src/routes/papermc.rs` | Modify | Drop cap from `parse_project`, update doc comments, rewrite cap tests. |
| `backend/src/validation.rs` | Modify | Extend `KNOWN_MC_VERSIONS` offline floor with 6 legacy anchors; extend the offline-pass test. |

Specs: `docs/superpowers/specs/2026-05-24-full-mc-version-history-design.md`

All `cargo` commands run from `backend/`.

---

## Task 1: Lift cap on `/api/cluster/mc-versions`

**Files:**
- Modify: `backend/src/routes/mc_versions.rs`

Remove the `MAX_VERSIONS = 20` cap so the Mojang manifest endpoint returns every release.

- [ ] **Step 1: Replace the existing tests to assert the new contract**

Open `backend/src/routes/mc_versions.rs` and replace the entire `#[cfg(test)] mod tests { ... }` block (currently lines 136–182) with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_versions_filters_snapshots() {
        let json = r#"{
          "latest": {"release": "1.21.4"},
          "versions": [
            {"id": "1.21.4", "type": "release"},
            {"id": "1.21.3", "type": "release"},
            {"id": "1.21.4-rc1", "type": "snapshot"},
            {"id": "1.21.2", "type": "release"}
          ]
        }"#;
        let v = parse_manifest(json).expect("parses");
        assert_eq!(v, vec!["1.21.4", "1.21.3", "1.21.2"]);
    }

    #[test]
    fn empty_versions_is_ok() {
        let json = r#"{"latest": {"release": "1.21.4"}, "versions": []}"#;
        let v = parse_manifest(json).expect("parses");
        assert!(v.is_empty());
    }

    #[test]
    fn returns_all_releases_no_cap() {
        // Simulates a full Mojang manifest with way more than the old 20-version
        // cap. Every release must come through so legacy versions (1.8, etc.)
        // remain selectable.
        let mut versions = Vec::new();
        for i in 0..100_usize {
            versions.push(format!(r#"{{"id":"v{i}","type":"release"}}"#));
        }
        let json = format!(
            r#"{{"latest":{{"release":"v0"}},"versions":[{}]}}"#,
            versions.join(",")
        );
        let v = parse_manifest(&json).expect("parses");
        assert_eq!(v.len(), 100);
        assert_eq!(v[0], "v0");
        assert_eq!(v[99], "v99");
    }

    #[test]
    fn snapshots_filtered_at_scale() {
        // Mixed snapshots and releases across a large manifest — only the
        // releases must come through, no cap on the count.
        let mut entries = Vec::new();
        for i in 0..50_usize {
            entries.push(format!(r#"{{"id":"r{i}","type":"release"}}"#));
            entries.push(format!(r#"{{"id":"s{i}","type":"snapshot"}}"#));
        }
        let json = format!(
            r#"{{"latest":{{"release":"r0"}},"versions":[{}]}}"#,
            entries.join(",")
        );
        let v = parse_manifest(&json).expect("parses");
        assert_eq!(v.len(), 50);
        assert!(v.iter().all(|s| s.starts_with('r')));
    }

    #[test]
    fn invalid_json_errors() {
        assert!(parse_manifest("not json").is_err());
    }
}
```

- [ ] **Step 2: Run the tests to confirm two of them fail**

Run from `/home/hadi/gitlab/anvil/backend`:

```bash
cargo test --lib routes::mc_versions::tests
```

Expected: `returns_all_releases_no_cap` FAILS (`assertion `left == right` failed: left=20, right=100`). `snapshots_filtered_at_scale` FAILS for the same reason (cap clips to 20). The other three tests pass.

- [ ] **Step 3: Remove the cap from `parse_manifest`**

In `backend/src/routes/mc_versions.rs`:

Delete the `MAX_VERSIONS` constant (currently lines 22–23):

```rust
/// Maximum number of release versions returned to clients.
pub const MAX_VERSIONS: usize = 20;
```

Update `parse_manifest` (currently lines 65–76) by removing the `.take(MAX_VERSIONS)` call. The function becomes:

```rust
/// Parses the Mojang manifest JSON into a release-only version list.
///
/// Mojang lists releases newest-first; that ordering is preserved.
///
/// # Errors
///
/// Returns the underlying [`serde_json::Error`] if the body is not the
/// expected shape.
pub fn parse_manifest(body: &str) -> Result<Vec<String>, serde_json::Error> {
    let m: Manifest = serde_json::from_str(body)?;
    let mut out: Vec<String> = m
        .versions
        .into_iter()
        .filter(|v| v.kind == "release")
        .map(|v| v.id)
        .collect();
    out.shrink_to_fit();
    Ok(out)
}
```

- [ ] **Step 4: Update the module docstring and response field doc**

Replace the module-level docstring at the top of `backend/src/routes/mc_versions.rs` (lines 1–5):

```rust
//! `GET /api/cluster/mc-versions` — cached Mojang version manifest.
//!
//! Returns every official release (snapshots filtered out) so the create
//! form can offer legacy versions like 1.8.9 alongside the latest. 24-hour
//! TTL via the `AppState` cache slot. Offline fallback to a hardcoded
//! baseline (see [`crate::validation::KNOWN_MC_VERSIONS`]) keeps the panel
//! usable when the Mojang CDN is unreachable.
```

Update the doc on `McVersionsResponse::versions` (currently around line 52). Change:

```rust
    /// Release versions, most recent first, capped at [`MAX_VERSIONS`].
    pub versions: Vec<String>,
```

to:

```rust
    /// Release versions, most recent first (every release Mojang lists).
    pub versions: Vec<String>,
```

- [ ] **Step 5: Run tests + clippy + fmt**

```bash
cargo test --lib routes::mc_versions::tests
cargo fmt --all
cargo clippy --all-targets --features serve-dir -- -D warnings
cargo clippy --all-targets --features embed -- -D warnings
```

Expected: all five tests in the module PASS. fmt makes no changes (or only whitespace). clippy clean for both feature flavors. Confirm no other code referenced `mc_versions::MAX_VERSIONS` — if clippy/compile fails with `unresolved name`, the offender is shown.

- [ ] **Step 6: Commit**

```bash
git add backend/src/routes/mc_versions.rs
git commit -m "$(cat <<'EOF'
feat(mc-versions): drop 20-release cap on /api/cluster/mc-versions

The cap hid every release older than ~1.20 — users who wanted to spin
up a 1.8 / 1.12 / 1.16 server had no way to select the version. Lift it
so the dropdown reflects the full Mojang release history. Snapshots
remain filtered out.
EOF
)"
```

---

## Task 2: Lift cap on `/api/papermc/versions`

**Files:**
- Modify: `backend/src/routes/papermc.rs`

Remove the `MAX_VERSIONS = 25` cap so the Paper endpoint returns every Paper-supported MC version (Paper ships back to 1.8).

- [ ] **Step 1: Replace the existing tests to assert the new contract**

Open `backend/src/routes/papermc.rs` and replace the entire `#[cfg(test)] mod tests { ... }` block (currently lines 163–196) with:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_project_reverses_ordering() {
        let body = r#"{"project_id":"paper","versions":["1.18","1.19","1.20","1.21"]}"#;
        let v = parse_project(body).expect("parse");
        assert_eq!(v, vec!["1.21", "1.20", "1.19", "1.18"]);
    }

    #[test]
    fn returns_all_versions_no_cap() {
        // Simulates a full Paper version list with way more than the old 25-version
        // cap. Every entry must come through (reversed to newest-first).
        let mut versions = Vec::new();
        for i in 0..80_usize {
            versions.push(format!("\"v{i}\""));
        }
        let body = format!("{{\"versions\":[{}]}}", versions.join(","));
        let v = parse_project(&body).expect("parse");
        assert_eq!(v.len(), 80);
        // Reversed: newest (highest index) first.
        assert_eq!(v[0], "v79");
        assert_eq!(v[79], "v0");
    }

    #[test]
    fn invalid_json_errors() {
        assert!(parse_project("not json").is_err());
    }

    #[test]
    fn fallback_versions_non_empty() {
        assert!(!FALLBACK_VERSIONS.is_empty());
    }
}
```

- [ ] **Step 2: Run the tests to confirm the new test fails**

Run from `/home/hadi/gitlab/anvil/backend`:

```bash
cargo test --lib routes::papermc::tests
```

Expected: `returns_all_versions_no_cap` FAILS (`assertion `left == right` failed: left=25, right=80`). The other three pass.

- [ ] **Step 3: Remove the cap from `parse_project`**

In `backend/src/routes/papermc.rs`:

Delete the `MAX_VERSIONS` constant (currently lines 24–26):

```rust
/// Maximum number of versions surfaced to the frontend. Paper supports
/// every patch release back to 1.8 — keeping the dropdown short.
pub const MAX_VERSIONS: usize = 25;
```

Update `parse_project` (currently lines 70–76) by removing the `.truncate(MAX_VERSIONS)` call. The function becomes:

```rust
/// Parses the `PaperMC` project response into a newest-first version list.
///
/// `PaperMC` returns versions in ascending order; we reverse so the
/// dropdown shows newest first. Every Paper-supported MC version is
/// included — Paper ships builds back to 1.8.
///
/// # Errors
///
/// Returns the underlying [`serde_json::Error`] if the body shape is wrong.
pub fn parse_project(body: &str) -> Result<Vec<String>, serde_json::Error> {
    let p: ProjectResponse = serde_json::from_str(body)?;
    let mut out = p.versions;
    out.reverse();
    Ok(out)
}
```

- [ ] **Step 4: Update the response field doc**

In `backend/src/routes/papermc.rs`, find the doc on `PaperVersionsResponse::versions` (currently around line 51):

```rust
    /// Paper-supported MC versions, newest first, capped at [`MAX_VERSIONS`].
    pub versions: Vec<String>,
```

Replace with:

```rust
    /// Paper-supported MC versions, newest first (Paper ships back to 1.8).
    pub versions: Vec<String>,
```

- [ ] **Step 5: Run tests + clippy + fmt**

```bash
cargo test --lib routes::papermc::tests
cargo fmt --all
cargo clippy --all-targets --features serve-dir -- -D warnings
cargo clippy --all-targets --features embed -- -D warnings
```

Expected: all four tests in the module PASS. fmt clean. clippy clean for both feature flavors.

- [ ] **Step 6: Commit**

```bash
git add backend/src/routes/papermc.rs
git commit -m "$(cat <<'EOF'
feat(papermc): drop 25-version cap on /api/papermc/versions

Paper publishes builds back to 1.8. The cap hid most of that range,
forcing users into the recent slice only. Lift it so legacy Paper
servers (1.8.8, 1.12.2, …) are selectable.
EOF
)"
```

---

## Task 3: Extend offline fallback with legacy anchors

**Files:**
- Modify: `backend/src/validation.rs`

Add popular legacy MC versions to the offline floor so a cold cache + Mojang outage still accepts common legacy versions during server creation. The list is only consulted when the live Mojang manifest cache is empty.

- [ ] **Step 1: Update the offline-pass test to cover legacy anchors**

Open `backend/src/validation.rs` and find the existing test (around line 522):

```rust
    #[test]
    fn offline_versions_pass() {
        assert!(is_known_mc_version_offline("1.21.4"));
        assert!(is_known_mc_version_offline("1.20.4"));
    }
```

Replace with:

```rust
    #[test]
    fn offline_versions_pass() {
        // Recent anchors — what the floor already had.
        assert!(is_known_mc_version_offline("1.21.4"));
        assert!(is_known_mc_version_offline("1.20.4"));
        // Legacy anchors — the create form must still accept these when
        // the Mojang cache is cold (e.g. fresh pod + transient outage).
        assert!(is_known_mc_version_offline("1.8.9"));
        assert!(is_known_mc_version_offline("1.12.2"));
        assert!(is_known_mc_version_offline("1.16.5"));
        assert!(is_known_mc_version_offline("1.18.2"));
        assert!(is_known_mc_version_offline("1.19.2"));
        assert!(is_known_mc_version_offline("1.20.1"));
    }
```

- [ ] **Step 2: Run the test to confirm it fails**

Run from `/home/hadi/gitlab/anvil/backend`:

```bash
cargo test --lib validation::tests::offline_versions_pass
```

Expected: FAIL on the first legacy assertion (`assertion failed: is_known_mc_version_offline("1.8.9")`).

- [ ] **Step 3: Extend `KNOWN_MC_VERSIONS` with the legacy anchors**

In `backend/src/validation.rs`, find the constant (currently lines 15–16):

```rust
pub const KNOWN_MC_VERSIONS: &[&str] =
    &["1.20.4", "1.20.6", "1.21.0", "1.21.1", "1.21.3", "1.21.4"];
```

Replace with:

```rust
pub const KNOWN_MC_VERSIONS: &[&str] = &[
    // Legacy anchors — the create form keeps accepting these even when
    // the Mojang cache is cold and the upstream is unreachable.
    "1.8.9", "1.12.2", "1.16.5", "1.18.2", "1.19.2", "1.20.1",
    // Recent releases.
    "1.20.4", "1.20.6", "1.21.0", "1.21.1", "1.21.3", "1.21.4",
];
```

- [ ] **Step 4: Run validation tests + clippy + fmt**

```bash
cargo test --lib validation::tests
cargo fmt --all
cargo clippy --all-targets --features serve-dir -- -D warnings
cargo clippy --all-targets --features embed -- -D warnings
```

Expected: all validation tests PASS, including `offline_versions_pass` (now 8 assertions) and `offline_unknown_fails` (which still asserts `1.7.10` and `"garbage"` are NOT in the floor — neither was added). fmt clean. clippy clean for both feature flavors.

- [ ] **Step 5: Commit**

```bash
git add backend/src/validation.rs
git commit -m "$(cat <<'EOF'
feat(validation): extend KNOWN_MC_VERSIONS with legacy anchors

Add 1.8.9, 1.12.2, 1.16.5, 1.18.2, 1.19.2, 1.20.1 to the offline floor
so server creation for popular legacy versions still succeeds during a
cold-cache + Mojang outage. The live cache (now uncapped) is still the
primary source.
EOF
)"
```

---

## Task 4: Whole-suite verification

**Files:** none modified.

Catch any cross-module regressions before declaring the feature done.

- [ ] **Step 1: Run the full backend test suite**

```bash
cargo test --all
```

Expected: every test passes. No tests reference the removed `MAX_VERSIONS` constants (those references were inside the route-module test blocks that Tasks 1 and 2 rewrote). If anything fails, the failure points at a missed reference.

- [ ] **Step 2: Run clippy in both feature flavors**

```bash
cargo clippy --all-targets --features serve-dir -- -D warnings
cargo clippy --all-targets --features embed -- -D warnings
```

Expected: clean. The two features are mutually exclusive — must be checked separately per `CLAUDE.md`.

- [ ] **Step 3: Manual smoke (optional, requires running backend)**

If a backend is running locally:

```bash
curl -sS http://localhost:8080/api/cluster/mc-versions | jq '.versions | length, .versions[0:5]'
curl -sS http://localhost:8080/api/papermc/versions   | jq '.versions | length, .versions[0:5]'
```

Expected: lengths are much larger than 20 / 25 (typically 75+ for vanilla, 50+ for Paper). First entries are recent (1.21.x). Last entries (if you list them all) include 1.8.x.

- [ ] **Step 4: No commit needed** — Task 4 only verifies.

---

## Self-Review

- **Spec coverage:**
  - Vanilla cap lifted → Task 1.
  - Paper cap lifted → Task 2.
  - `KNOWN_MC_VERSIONS` extended → Task 3.
  - Module / response docs updated → Tasks 1 (steps 4) and 2 (step 4).
  - Tests rewritten per spec (rename cap test, add no-cap test, extend offline-pass test) → Tasks 1, 2, 3.
  - Frontend unchanged per spec → no task (correct).
  - Modded servers unchanged per spec → no task (correct).
- **Placeholder scan:** none — every step shows the exact code to write, exact commands, expected output.
- **Type consistency:** `parse_manifest` / `parse_project` signatures unchanged. `McVersionsResponse` / `PaperVersionsResponse` field types unchanged. `KNOWN_MC_VERSIONS` type unchanged (`&[&str]`).
