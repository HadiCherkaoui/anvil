<!--
SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

# Anvil — Mod deps · per-mod updates · paper plugin pre-select Implementation Plan (Spec 4)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Three connected items — auto-pull required mod dependencies on add, per-mod / per-plugin update notifications, paper plugin pre-select on create — sharing the Modrinth/CurseForge plumbing.

**Architecture:** Three lanes. Lane A — upstream metadata: deserialize `dependencies` on Modrinth & CurseForge versions, normalize, build a transitive resolver. Lane B — per-mod update tracking: SQLite table + poller extension + ServerDetail surfacing + Mods/Plugins UI bumps. Lane C — paper plugin pre-select on create, symmetric to modded mods.

**Tech Stack:** Rust 1.83+ · axum 0.8 · kube-rs · sqlx · Next.js 16 · TypeScript · Zod v4. No new top-level deps.

**Source spec:** `docs/superpowers/specs/2026-05-06-anvil-mod-deps-and-updates-design.md` (signed off 2026-05-06).
**Depends on:** Spec 1 plan landed (refreshable context, auto-apply on create pattern, picked-mods list pattern).

---

## Hard constraints

- Optional deps **ignored** silently (locked spec §8).
- Incompatible deps silently skipped — server fails to boot if the user's pick is bad.
- Recursion depth cap 5; cycle detection via visited set.
- One new SQLite migration: `0007_mod_updates.sql`.
- No new RBAC; no new top-level deps.
- Standard build/test gates per task.

---

## Decisions locked from spec §8

1. Optional deps ignored.
2. Incompatible deps skipped (no gate).
3. Conflict resolution = first-write-wins.
4. `mod_updates` table covers mods AND plugins (provider column distinguishes).
5. Paper plugins reuse `ModEntry` shape (no new `PluginEntry` type).

---

## File structure

### Backend (`backend/`)

| File | Change |
|---|---|
| `migrations/0007_mod_updates.sql` | NEW — table per spec §4.3.1 |
| `src/modpack/mr_client.rs` | EDIT — `MrDependency` struct, `MrVersion.dependencies: Vec<MrDependency>` |
| `src/modpack/cf_client.rs` | EDIT — `CfDependency` struct, version-response field |
| `src/modpack/deps.rs` | NEW — `DependencySpec`, `from_modrinth`, `from_curseforge` normalisation |
| `src/modpack/dep_resolver.rs` | NEW — `resolve_required(seed, ctx, http)` BFS resolver |
| `src/routes/servers/mods.rs` | EDIT — Add op runs resolver, returns `added` array |
| `src/routes/servers/plugins.rs` | EDIT — same pattern for plugins |
| `src/routes/servers/create.rs` | EDIT — `paper.initial_plugins` field; both initial_mods and initial_plugins run resolver before persist |
| `src/routes/servers/get.rs` | EDIT — embed `mod_updates: Vec<ModUpdateInfo>` from new table |
| `src/modpack/poller.rs` | EDIT — extend to iterate `source_kind IN ('modded', 'paper')`, upsert `mod_updates` |
| `src/modpack/mod.rs` | EDIT — module decls |
| `tests/dep_resolver.rs` | NEW — resolver unit + integration |
| `tests/mod_updates_poller.rs` | NEW — poller integration |
| `tests/create_paper_plugins.rs` | NEW — create with initial_plugins integration |

### Frontend (`frontend/app/`)

| File | Change |
|---|---|
| `lib/api.ts` | EDIT — `mod_updates` + `ModUpdateInfo` + Zod; toast helper for "added X + N deps"; `paper.initial_plugins` in CreateServerRequest |
| `servers/tabs/ModsBody.tsx` | EDIT — ↑ chip + per-row update button + "update all" — shared between modded mods and paper plugins (PaperPluginsBody) |
| `servers/new/page.tsx` | EDIT — paper branch gets "+ pre-pick plugins"; picked-plugins list mirrors picked-mods (Spec 1 §5.8) |
| `servers/ServerDetailView.tsx` | EDIT — tab `mark` fires when modpack `update_available` OR `mod_updates.length > 0` |

---

## Tasks

### Lane A — Upstream metadata + resolver

#### Task A1: Modrinth dependencies deserialise

**Files:**
- Modify: `backend/src/modpack/mr_client.rs`

- [ ] **Step 1: Failing unit test**

```rust
#[cfg(test)]
mod dep_tests {
    use super::*;

    const FIXTURE: &str = r#"{
        "id": "ver1", "project_id": "proj1", "name": "X",
        "version_number": "1.0.0", "version_type": "release",
        "loaders": ["fabric"], "game_versions": ["1.21.4"],
        "date_published": "2026-01-01T00:00:00Z",
        "files": [],
        "dependencies": [
            {"version_id": null, "project_id": "fabric-api", "file_name": null, "dependency_type": "required"},
            {"version_id": "ver-x", "project_id": null, "file_name": null, "dependency_type": "optional"}
        ]
    }"#;

    #[test]
    fn parses_dependency_array() {
        let v: MrVersion = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(v.dependencies.len(), 2);
        assert_eq!(v.dependencies[0].project_id.as_deref(), Some("fabric-api"));
        assert_eq!(v.dependencies[0].dependency_type, "required");
    }
    #[test]
    fn missing_dependency_array_is_empty() {
        let no_deps = r#"{
            "id": "v", "project_id": "p", "name": "X", "version_number": "1",
            "version_type": "release", "loaders": [], "game_versions": [],
            "date_published": "2026-01-01T00:00:00Z", "files": []
        }"#;
        let v: MrVersion = serde_json::from_str(no_deps).unwrap();
        assert!(v.dependencies.is_empty());
    }
}
```

- [ ] **Step 2: Run, fail**

```
cargo test --lib modpack::mr_client::dep_tests
```

- [ ] **Step 3: Add struct + field**

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct MrDependency {
    pub version_id: Option<String>,
    pub project_id: Option<String>,
    pub file_name: Option<String>,
    pub dependency_type: String,
}

// In MrVersion:
#[serde(default)]
pub dependencies: Vec<MrDependency>,
```

- [ ] **Step 4: Run, pass**

```
cargo test --lib modpack::mr_client
```

- [ ] **Step 5: Commit**

```bash
git add backend/src/modpack/mr_client.rs
git commit -m "feat(mr): deserialize MrVersion.dependencies"
```

---

#### Task A2: CurseForge dependencies deserialise + relation mapping

**Files:**
- Modify: `backend/src/modpack/cf_client.rs`

- [ ] **Step 1: Failing test for relation enum mapping**

```rust
#[cfg(test)]
mod cf_dep_tests {
    use super::*;

    const FIX: &str = r#"{
        "id": 1, "modId": 100, "fileName": "x.jar", "displayName": "X",
        "fileLength": 0, "fileFingerprint": 0,
        "downloadUrl": "https://example.com/x.jar",
        "dependencies": [
            { "modId": 200, "relationType": 3 },
            { "modId": 201, "relationType": 2 },
            { "modId": 202, "relationType": 5 },
            { "modId": 203, "relationType": 6 }
        ],
        "gameVersions": [], "releaseType": 1
    }"#;

    #[test]
    fn parses_cf_deps() {
        let f: CfFile = serde_json::from_str(FIX).unwrap();
        assert_eq!(f.dependencies.len(), 4);
        assert_eq!(f.dependencies[0].relation_type, 3);
    }
}
```

- [ ] **Step 2: Add struct**

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct CfDependency {
    #[serde(rename = "modId")]
    pub mod_id: u32,
    #[serde(rename = "relationType")]
    pub relation_type: u8,
}

// In CfFile (or wherever the version-equivalent struct lives):
#[serde(default)]
pub dependencies: Vec<CfDependency>,
```

- [ ] **Step 3: Run, pass**

```
cargo test --lib modpack::cf_client
```

- [ ] **Step 4: Commit**

```bash
git add backend/src/modpack/cf_client.rs
git commit -m "feat(cf): deserialize file.dependencies + relation_type"
```

---

#### Task A3: Normalised `DependencySpec`

**Files:**
- Create: `backend/src/modpack/deps.rs`
- Modify: `backend/src/modpack/mod.rs`

- [ ] **Step 1: Module + tests**

```rust
// backend/src/modpack/deps.rs
use crate::modpack::{Provider, mr_client::MrDependency, cf_client::CfDependency};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepKind { Required, Optional }

#[derive(Debug, Clone)]
pub struct DependencySpec {
    pub provider: Provider,
    pub project_id: String,
    pub pinned_version_id: Option<String>,
    pub kind: DepKind,
}

pub fn from_modrinth(deps: &[MrDependency]) -> Vec<DependencySpec> {
    deps.iter().filter_map(|d| {
        let kind = match d.dependency_type.as_str() {
            "required" => DepKind::Required,
            "optional" => DepKind::Optional,
            _ => return None,
        };
        let project_id = d.project_id.clone()?;
        Some(DependencySpec {
            provider: Provider::Modrinth,
            project_id,
            pinned_version_id: d.version_id.clone(),
            kind,
        })
    }).collect()
}

pub fn from_curseforge(deps: &[CfDependency]) -> Vec<DependencySpec> {
    deps.iter().filter_map(|d| {
        let kind = match d.relation_type {
            3 => DepKind::Required,
            2 => DepKind::Optional,
            _ => return None,
        };
        Some(DependencySpec {
            provider: Provider::Curseforge,
            project_id: d.mod_id.to_string(),
            pinned_version_id: None,
            kind,
        })
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modrinth_filters_to_required_and_optional() {
        let raw = vec![
            MrDependency { version_id: None, project_id: Some("a".into()), file_name: None, dependency_type: "required".into() },
            MrDependency { version_id: None, project_id: Some("b".into()), file_name: None, dependency_type: "incompatible".into() },
            MrDependency { version_id: None, project_id: Some("c".into()), file_name: None, dependency_type: "optional".into() },
        ];
        let out = from_modrinth(&raw);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, DepKind::Required);
        assert_eq!(out[1].kind, DepKind::Optional);
    }
    #[test]
    fn cf_relation_3_required_2_optional() {
        let raw = vec![
            CfDependency { mod_id: 1, relation_type: 3 },
            CfDependency { mod_id: 2, relation_type: 2 },
            CfDependency { mod_id: 3, relation_type: 5 },
        ];
        let out = from_curseforge(&raw);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, DepKind::Required);
    }
}
```

Add `pub mod deps;` to `modpack/mod.rs`.

- [ ] **Step 2: Run, pass**

```
cargo test --lib modpack::deps
```

- [ ] **Step 3: Commit**

```bash
git add backend/src/modpack/deps.rs backend/src/modpack/mod.rs
git commit -m "feat(deps): normalize Modrinth + CF deps to DependencySpec"
```

---

#### Task A4: Resolver — BFS with depth cap + cycle detection

**Files:**
- Create: `backend/src/modpack/dep_resolver.rs`
- Modify: `backend/src/modpack/mod.rs`

- [ ] **Step 1: Failing test — depth cap and cycle**

Use mocked HTTP via the existing test infra (or a trait the resolver takes that we can implement in tests). Sketch:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::modpack::{Provider, ModEntry, deps::{DepKind, DependencySpec}};

    struct FakeHttp { /* maps project_id -> Vec<DependencySpec> + version metadata */ }
    impl ModpackHttp for FakeHttp { /* ... */ }

    #[tokio::test]
    async fn resolves_transitive_deps() {
        let http = FakeHttp::with_deps(/* A -> B -> C, all required */);
        let seed = mod_entry("A");
        let mut ctx = ResolveContext {
            mc_version: "1.21.4", loader: "fabric",
            installed: HashSet::new(), pending: HashSet::new(),
        };
        let out = resolve_required(&seed, &mut ctx, &http).await.unwrap();
        let names: Vec<&str> = out.iter().map(|m| m.project_name.as_str()).collect();
        assert_eq!(names, vec!["B", "C"]);
    }
    #[tokio::test]
    async fn cycle_short_circuits() { /* A -> B -> A */ }
    #[tokio::test]
    async fn skips_already_installed() { /* A -> B, B already installed */ }
    #[tokio::test]
    async fn depth_cap() { /* A -> B -> C -> D -> E -> F, expect D-onwards skipped */ }
}
```

- [ ] **Step 2: Implement**

```rust
// backend/src/modpack/dep_resolver.rs
use std::collections::{HashSet, VecDeque};
use anyhow::Result;
use tracing::warn;

use crate::modpack::{ModEntry, Provider, ModpackHttp};
use crate::modpack::deps::{DepKind, DependencySpec};

const MAX_DEPTH: usize = 5;

pub struct ResolveContext<'a> {
    pub mc_version: &'a str,
    pub loader: &'a str,
    pub installed: HashSet<(Provider, String)>,
    pub pending: HashSet<(Provider, String)>,
}

pub async fn resolve_required(
    seed: &ModEntry,
    ctx: &mut ResolveContext<'_>,
    http: &ModpackHttp<'_>,
) -> Result<Vec<ModEntry>> {
    let mut out: Vec<ModEntry> = Vec::new();
    let mut queue: VecDeque<(ModEntry, usize)> = VecDeque::new();
    let mut visited: HashSet<(Provider, String)> = HashSet::new();
    queue.push_back((seed.clone(), 0));

    while let Some((cur, depth)) = queue.pop_front() {
        let key = (cur.provider, cur.project_id.clone());
        if !visited.insert(key.clone()) { continue; }
        if depth > MAX_DEPTH {
            warn!(project_id = %cur.project_id, "dep resolver depth cap hit");
            continue;
        }

        // Fetch dependencies for this entry's version_id.
        let deps = fetch_deps(http, &cur).await?;
        for spec in deps.into_iter().filter(|d| d.kind == DepKind::Required) {
            let dep_key = (spec.provider, spec.project_id.clone());
            if ctx.installed.contains(&dep_key) || ctx.pending.contains(&dep_key)
               || visited.contains(&dep_key) { continue; }

            let entry = match resolve_one(http, &spec, ctx).await {
                Ok(e) => e,
                Err(e) => {
                    warn!(project_id = %spec.project_id, err = %e, "dep skipped");
                    continue;
                }
            };
            ctx.pending.insert(dep_key);
            out.push(entry.clone());
            queue.push_back((entry, depth + 1));
        }
    }
    Ok(out)
}

async fn fetch_deps(http: &ModpackHttp<'_>, entry: &ModEntry) -> Result<Vec<DependencySpec>> {
    match entry.provider {
        Provider::Modrinth => {
            let mr = http.mr.ok_or_else(|| anyhow::anyhow!("MR client unavailable"))?;
            let v = mr.fetch_version(&entry.version_id).await?;
            Ok(crate::modpack::deps::from_modrinth(&v.dependencies))
        }
        Provider::Curseforge => {
            let cf = http.cf.ok_or_else(|| anyhow::anyhow!("CF client unavailable"))?;
            let project_id_u32: u32 = entry.project_id.parse()?;
            let version_id_u32: u32 = entry.version_id.parse()?;
            let f = cf.fetch_file(project_id_u32, version_id_u32).await?;
            Ok(crate::modpack::deps::from_curseforge(&f.dependencies))
        }
    }
}

async fn resolve_one(
    http: &ModpackHttp<'_>,
    spec: &DependencySpec,
    ctx: &ResolveContext<'_>,
) -> Result<ModEntry> {
    match spec.provider {
        Provider::Modrinth => {
            let mr = http.mr.ok_or_else(|| anyhow::anyhow!("MR unavailable"))?;
            let version = if let Some(vid) = &spec.pinned_version_id {
                mr.fetch_version(vid).await?
            } else {
                mr.fetch_latest_compatible(&spec.project_id, ctx.mc_version, ctx.loader)
                  .await?
                  .ok_or_else(|| anyhow::anyhow!("no compatible MR version"))?
            };
            let project = mr.fetch_project(&spec.project_id).await?;
            let primary = version.files.first().ok_or_else(|| anyhow::anyhow!("no files"))?;
            Ok(ModEntry {
                provider: Provider::Modrinth,
                project_id: spec.project_id.clone(),
                project_slug: project.slug,
                project_name: project.title,
                version_id: version.id.clone(),
                version_name: version.version_number.clone(),
                filename: primary.filename.clone(),
                download_url: primary.url.clone(),
                sha512: primary.hashes.as_ref().and_then(|h| h.sha512.clone()),
            })
        }
        Provider::Curseforge => {
            let cf = http.cf.ok_or_else(|| anyhow::anyhow!("CF unavailable"))?;
            let project_id_u32: u32 = spec.project_id.parse()?;
            let file = cf.fetch_latest_compatible(project_id_u32, ctx.mc_version, ctx.loader)
                .await?
                .ok_or_else(|| anyhow::anyhow!("no compatible CF version"))?;
            let project = cf.fetch_project(project_id_u32).await?;
            Ok(ModEntry {
                provider: Provider::Curseforge,
                project_id: spec.project_id.clone(),
                project_slug: project.slug,
                project_name: project.name,
                version_id: file.id.to_string(),
                version_name: file.display_name.clone(),
                filename: file.file_name.clone(),
                download_url: file.download_url.clone(),
                sha512: None,
            })
        }
    }
}
```

The fetcher methods on `mr` / `cf` clients — `fetch_version`, `fetch_latest_compatible`, `fetch_project` — may not exist verbatim. Inspect the client code; either reuse existing equivalents or add small fns. Keep the surface minimal.

- [ ] **Step 3: Run, pass**

```
cargo test --lib modpack::dep_resolver
```

- [ ] **Step 4: Commit**

```bash
git add backend/src/modpack/dep_resolver.rs backend/src/modpack/mod.rs
git commit -m "feat(deps): transitive required-dep resolver with depth cap"
```

---

#### Task A5: Wire resolver into add routes (mods + plugins + create)

**Files:**
- Modify: `backend/src/routes/servers/mods.rs`
- Modify: `backend/src/routes/servers/plugins.rs`
- Modify: `backend/src/routes/servers/create.rs`

- [ ] **Step 1: Mods route — Add op resolves deps**

In `mods.rs`, find the `Add` op handling (around `:89-119` per Spec 1 survey). After building the user-picked `ModEntry` and BEFORE persisting:

```rust
let mut ctx = crate::modpack::dep_resolver::ResolveContext {
    mc_version: &cfg.mc_version,
    loader: cfg.runtime.as_str(),
    installed: cfg.mods.iter().map(|m| (m.provider, m.project_id.clone())).collect(),
    pending: cfg.pending.iter().filter_map(|p| match p {
        PendingOp::Add { mod_entry } => Some((mod_entry.provider, mod_entry.project_id.clone())),
        _ => None,
    }).collect(),
};
let http = ModpackHttp { cf: state.cf_client.as_deref(), mr: state.mr_client.as_ref() };
let extra = crate::modpack::dep_resolver::resolve_required(&entry, &mut ctx, &http).await
    .unwrap_or_default(); // skip on resolver failure rather than block the add

let mut new_pending: Vec<PendingOp> = vec![PendingOp::Add { mod_entry: entry.clone() }];
new_pending.extend(extra.iter().map(|m| PendingOp::Add { mod_entry: m.clone() }));
// merge into cfg.pending and persist
```

Return the count of added entries:

```rust
#[derive(Debug, Serialize)]
pub struct AddResponse {
    pub added: Vec<ModEntry>,
    pub added_count: usize,
}
// returns Ok((StatusCode::OK, Json(AddResponse { added: new_pending..., added_count: ... })))
```

- [ ] **Step 2: Plugins route — same shape**

Same pattern in `plugins.rs`. Loader is `"paper"`.

- [ ] **Step 3: Create route — initial_mods AND initial_plugins**

In `create.rs`, after computing `cfg`, run the resolver across `initial_mods` (modded) and `initial_plugins` (paper) and merge resolved deps into the pending list:

```rust
if !cfg.initial_mods.is_empty() {
    let mut ctx = ResolveContext { mc_version, loader, installed: HashSet::new(), pending: HashSet::new() };
    for m in cfg.initial_mods.iter() {
        let extra = resolve_required(m, &mut ctx, &http).await.unwrap_or_default();
        // merge extra into a deps_for_create vec; deduplicate
    }
    // pending = initial_mods + collected_deps
}
```

(Same for `initial_plugins`.)

- [ ] **Step 4: Frontend toast wording**

In `frontend/app/servers/tabs/ModsBody.tsx` (and PaperPluginsBody), after `addPendingMod` resolves, read `added_count` from the response and toast:

```tsx
const seedCount = 1;
const depCount = response.added_count - seedCount;
const msg = depCount === 0
  ? `added ${entry.project_name}`
  : `added ${entry.project_name} + ${depCount} ${depCount === 1 ? "dependency" : "dependencies"}`;
toast.push(msg, "success");
```

`addPendingMod` returns the API response — extend its return shape in `api.ts`.

- [ ] **Step 5: Tests + clippy**

```
cargo test --all
cargo clippy --all-targets --features serve-dir -- -D warnings
cd ../frontend && pnpm typecheck && pnpm lint
```

- [ ] **Step 6: Commit**

```bash
git add backend/src/routes/servers/{mods,plugins,create}.rs frontend/app/lib/api.ts frontend/app/servers/tabs/ModsBody.tsx
git commit -m "feat(mods): auto-pull required dependencies on add and create"
```

---

### Lane B — Per-mod / per-plugin updates

#### Task B1: SQLite migration `0007_mod_updates.sql`

**Files:**
- Create: `backend/migrations/0007_mod_updates.sql`

- [ ] **Step 1: Write migration**

```sql
CREATE TABLE IF NOT EXISTS mod_updates (
    server_id              TEXT NOT NULL,
    provider               TEXT NOT NULL,
    project_id             TEXT NOT NULL,
    current_version_id     TEXT NOT NULL,
    latest_version_id      TEXT NOT NULL,
    latest_version_name    TEXT NOT NULL,
    latest_published_at    TEXT,
    checked_at             INTEGER NOT NULL,
    PRIMARY KEY (server_id, provider, project_id),
    FOREIGN KEY (server_id) REFERENCES servers(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_mod_updates_server ON mod_updates(server_id);
```

- [ ] **Step 2: Run migration locally**

```
cd backend
sqlx migrate run --database-url sqlite:dev.db
```

(Or whatever the dev DB pattern is.)

- [ ] **Step 3: Refresh sqlx offline cache**

If the project uses `sqlx::query!` macros at compile time, regenerate:

```
cargo sqlx prepare --workspace
```

- [ ] **Step 4: Commit**

```bash
git add backend/migrations/0007_mod_updates.sql backend/.sqlx
git commit -m "feat(db): mod_updates table for per-mod / per-plugin updates"
```

---

#### Task B2: Poller extension

**Files:**
- Modify: `backend/src/modpack/poller.rs`
- Test: `backend/tests/mod_updates_poller.rs`

- [ ] **Step 1: Failing integration test**

```rust
// backend/tests/mod_updates_poller.rs
mod common;

#[tokio::test]
async fn poller_upserts_when_newer_version_available() {
    let (state, _) = common::test_state().await;
    let id = common::seed_modded_with_mod(&state, "ts-up", "fabric", "1.21.4",
        "fabric-api", "ver-old").await;
    common::stub_modrinth_latest(&state, "fabric-api", "1.21.4", "fabric", "ver-new", "0.99.0").await;

    common::run_poller_once(&state).await;

    let row = common::fetch_mod_update_row(&state, &id, "modrinth", "fabric-api").await.unwrap();
    assert_eq!(row.latest_version_id, "ver-new");
}

#[tokio::test]
async fn poller_deletes_row_when_now_current() {
    // seed mod_updates row, then run poller with upstream returning the same version → row gone
}
```

- [ ] **Step 2: Implement**

In `poller.rs`, alongside the existing modpack iteration:

```rust
async fn poll_individual_mods(state: &AppState) -> Result<()> {
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT id, mc_version, source_kind, source_config FROM servers
         WHERE source_kind IN ('modded', 'paper')")
        .fetch_all(&state.pool).await?;

    for (id, mc_version, source_kind, source_config) in rows {
        let cfg: serde_json::Value = match serde_json::from_str(&source_config) {
            Ok(v) => v, Err(_) => continue,
        };
        let loader = if source_kind == "paper" {
            "paper"
        } else {
            cfg.get("runtime").and_then(|v| v.as_str()).unwrap_or("fabric")
        };
        let mods = cfg.get("mods").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        for m in mods.iter() {
            let provider = m.get("provider").and_then(|v| v.as_str()).unwrap_or("");
            let project_id = m.get("project_id").and_then(|v| v.as_str()).unwrap_or("");
            let cur_ver = m.get("version_id").and_then(|v| v.as_str()).unwrap_or("");
            if provider.is_empty() || project_id.is_empty() { continue; }

            if let Err(e) = check_one_mod(state, &id, provider, project_id, cur_ver,
                                          &mc_version, loader).await {
                tracing::warn!(?e, server.id = %id, provider, project_id, "mod update check failed");
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await; // rate-limit defence
        }
    }
    Ok(())
}

async fn check_one_mod(
    state: &AppState,
    server_id: &str,
    provider: &str,
    project_id: &str,
    current_version_id: &str,
    mc_version: &str,
    loader: &str,
) -> Result<()> {
    let latest = match provider {
        "modrinth" => state.mr_client.as_ref().map(|c| c.fetch_latest_compatible(project_id, mc_version, loader)),
        "curseforge" => state.cf_client.as_ref().map(|c| c.fetch_latest_compatible(project_id.parse()?, mc_version, loader)),
        _ => return Ok(()),
    };
    // Pull the version_id from the response; if matches current, DELETE row; else UPSERT.
    // (Concrete fetch + comparison logic here; sketch elided to keep the plan tight.)
    Ok(())
}
```

In the poller's main tick (the existing periodic task), call `poll_individual_mods(&state).await`.

- [ ] **Step 3: Test + clippy**

```
cargo test --test mod_updates_poller
cargo clippy --all-targets --features serve-dir -- -D warnings
```

- [ ] **Step 4: Commit**

```bash
git add backend/src/modpack/poller.rs backend/tests/mod_updates_poller.rs
git commit -m "feat(poller): per-mod / per-plugin update tracking"
```

---

#### Task B3: ServerDetail surfaces `mod_updates`

**Files:**
- Modify: `backend/src/routes/servers/get.rs`
- Modify: `frontend/app/lib/api.ts`

- [ ] **Step 1: Backend struct + query**

```rust
#[derive(Debug, Serialize)]
pub struct ModUpdateInfo {
    pub provider: String,
    pub project_id: String,
    pub current_version_id: String,
    pub latest_version_id: String,
    pub latest_version_name: String,
}

// In ServerDetail:
pub mod_updates: Vec<ModUpdateInfo>,

// In handler:
let mod_updates: Vec<ModUpdateInfo> = sqlx::query_as!(
    ModUpdateInfo,
    "SELECT provider, project_id, current_version_id, latest_version_id, latest_version_name
     FROM mod_updates WHERE server_id = ?",
    id,
).fetch_all(&state.pool).await.unwrap_or_default();
```

- [ ] **Step 2: Zod schema**

```ts
const modUpdateInfoSchema = z.object({
  provider: z.string(),
  project_id: z.string(),
  current_version_id: z.string(),
  latest_version_id: z.string(),
  latest_version_name: z.string(),
});
// in serverDetailSchema:
mod_updates: z.array(modUpdateInfoSchema),
```

- [ ] **Step 3: Build + typecheck**

```
cargo build
cd ../frontend && pnpm typecheck
```

- [ ] **Step 4: Commit**

```bash
git add backend/src/routes/servers/get.rs frontend/app/lib/api.ts
git commit -m "feat(detail): surface mod_updates on ServerDetail"
```

---

#### Task B4: Frontend ↑ chip + per-row update + "update all"

**Files:**
- Modify: `frontend/app/servers/tabs/ModsBody.tsx`
- Modify: `frontend/app/servers/ServerDetailView.tsx`

- [ ] **Step 1: Map mod_updates to row indicators**

In `ModsBody.tsx` (and the inner `PaperPluginsBody` component), build a Map for fast lookup:

```tsx
const updatesByKey = useMemo(() => {
  const m = new Map<string, ModUpdateInfo>();
  for (const u of detail.mod_updates) m.set(`${u.provider}:${u.project_id}`, u);
  return m;
}, [detail.mod_updates]);

// for each installed mod row:
const u = updatesByKey.get(`${mod.provider}:${mod.project_id}`);
{u && <span className="ml-2 rounded bg-state-update/10 px-1 text-[11px] text-state-update">↑ {u.latest_version_name}</span>}
{u && (
  <Button onClick={() => bumpMod(detail.id, mod, u.latest_version_id)}>update</Button>
)}
```

`bumpMod` calls the existing pending-op API with `op: "bump", target_version_id: u.latest_version_id`. Backend route already handles this — see `mods.rs` for the `Bump` variant.

- [ ] **Step 2: "Update all" header**

```tsx
{detail.mod_updates.length > 0 && (
  <div className="flex items-center justify-between border-b border-border px-3 py-2">
    <span>{detail.mod_updates.length} updates available</span>
    <Button onClick={onUpdateAll}>update all</Button>
  </div>
)}
```

`onUpdateAll` iterates `detail.mod_updates` and POSTs Bump for each in sequence (or parallel; sequential is simpler).

- [ ] **Step 3: Tab badge**

In `ServerDetailView.tsx:177-182`:

```tsx
{
  id: "mods",
  label: detail.source_kind === "paper" ? "plugins" : "mods",
  href: tabHref("mods"),
  ...((detail.update_available || detail.mod_updates.length > 0) ? { mark: true } : {}),
},
```

- [ ] **Step 4: Manual repro**

Seed a server, manually insert a `mod_updates` row via sqlite CLI, verify the ↑ + button appear and a click produces a Bump pending op.

- [ ] **Step 5: Commit**

```bash
git add frontend/app/servers/tabs/ModsBody.tsx frontend/app/servers/ServerDetailView.tsx
git commit -m "feat(mods): show update-available indicators and bump UI"
```

---

### Lane C — Paper plugin pre-select on create

#### Task C1: Frontend create form + paper plugin pre-pick

**Files:**
- Modify: `frontend/app/servers/new/page.tsx`
- Modify: `frontend/app/lib/api.ts`

- [ ] **Step 1: Add `initial_plugins` to draft + request**

```tsx
// INITIAL:
initial_plugins: [],

// In paper branch (around the McVersionPicker for vanilla/paper at :341-349):
{draft.type === "paper" && (
  <div className="mt-3 flex items-center gap-2">
    <Button
      onClick={() => {
        if (draft.mc_version !== null) setBrowseOpen(true);
      }}
      disabled={draft.mc_version === null}
    >
      + pre-pick plugins
    </Button>
    <span className="font-mono text-[11px] text-text-faint">
      {draft.initial_plugins.length} picked
    </span>
  </div>
)}
```

In `onCatalogPick`, add a paper branch that pushes to `draft.initial_plugins`. The catalog needs to open with `mode="plugin"` and `loader="paper"` — adjust `browseMode` / `browseLoader` near `:281-290`:

```tsx
const browseMode: "modpack" | "mod" | "plugin" =
  draft.type === "modded" ? "mod"
  : draft.type === "paper" ? "plugin"
  : "modpack";
const browseLoader: Runtime | "paper" | undefined =
  draft.type === "modded" && draft.runtime !== null ? draft.runtime
  : draft.type === "paper" ? "paper"
  : undefined;
```

(`CatalogSheet` already accepts these per Spec 4 survey of `PaperPluginsBody`.)

- [ ] **Step 2: Render picked-plugins list**

Mirror Spec 1 §5.8 (picked-mods list) below the "+ pre-pick plugins" button.

- [ ] **Step 3: Submit `paper.initial_plugins`**

In the `request: CreateServerRequest = {...}` block (around `:229-263`), add:

```tsx
...(draft.type === "paper" && draft.initial_plugins.length > 0
  ? { paper: { initial_plugins: draft.initial_plugins } }
  : {}),
```

Update Zod / TS interface in `api.ts` to allow `paper?: { initial_plugins: ModEntry[] }`.

- [ ] **Step 4: Switch-type clears**

When type onChange goes away from paper, also `set("initial_plugins", [])`.

- [ ] **Step 5: Manual repro**

Build, create a paper server with 2 plugins picked, verify they land in `pending` then auto-apply via the Spec 1-pattern apply-on-create handler (which Lane A's resolver also touches).

- [ ] **Step 6: Commit**

```bash
git add frontend/app/servers/new/page.tsx frontend/app/lib/api.ts
git commit -m "feat(create): paper plugin pre-select symmetric to modded mods"
```

---

#### Task C2: Backend create — accept `paper.initial_plugins`

**Files:**
- Modify: `backend/src/routes/servers/create.rs`
- Test: `backend/tests/create_paper_plugins.rs`

- [ ] **Step 1: Failing integration test**

```rust
#[tokio::test]
async fn create_paper_with_initial_plugins_spawns_apply() {
    let (state, _) = common::test_state().await;
    let req = common::create_paper_request_with_plugins(&[
        common::mod_entry("modrinth", "luckperms"),
    ]);
    let resp = common::post_create(&state, req).await.unwrap();
    let id = resp.id;
    common::wait_for_apply_job(&state, &id, std::time::Duration::from_secs(5)).await
        .expect("plugin apply job spawned");
}
```

- [ ] **Step 2: Implement**

Extend `CreateRequest` with `paper: Option<PaperCreate>` (where `PaperCreate { initial_plugins: Vec<ModEntry> }`). For paper source kind, fold `initial_plugins` into pending plugin Adds the same way modded folds `initial_mods` (Spec 1 plan §C6 pattern), and spawn `mods_apply::run` with `SyncTarget::Plugins`.

```rust
if source_kind == "paper" {
    if let Some(paper) = req.paper.as_ref() {
        if !paper.initial_plugins.is_empty() {
            // 1. Run the dep resolver across initial_plugins (Lane A §A5).
            // 2. Persist pending Add ops in source_config.
            // 3. Spawn mods_apply::run with SyncTarget::Plugins.
        }
    }
}
```

- [ ] **Step 3: Tests + clippy**

```
cargo test --test create_paper_plugins
cargo clippy --all-targets --features serve-dir -- -D warnings
```

- [ ] **Step 4: Commit**

```bash
git add backend/src/routes/servers/create.rs backend/tests/create_paper_plugins.rs
git commit -m "feat(create): accept paper.initial_plugins"
```

---

## Verification

- [ ] `cd backend && cargo fmt --all && cargo clippy --all-targets --features serve-dir -- -D warnings && cargo clippy --all-targets --features embed -- -D warnings && cargo test --all`
- [ ] `cd frontend && pnpm typecheck && pnpm lint && pnpm build`
- [ ] Manual repro: install a fabric mod → deps appear in pending; install a plugin on paper → deps appear; ServerDetail surfaces `mod_updates` after a poller tick; "update all" works; create paper with picked plugins → apply Job spawns.

---

## Implementation prompt

```
Implement the plan at docs/superpowers/plans/2026-05-06-anvil-mod-deps-and-updates-impl.md.

Use superpowers:executing-plans (or subagent-driven-development). Lanes A → B → C in order.
The spec at docs/superpowers/specs/2026-05-06-anvil-mod-deps-and-updates-design.md is the
design authority.

Depends on Spec 1 plan having landed: refreshable ServerDetailContext, picked-mods list
pattern (mirrored for plugins), auto-apply on create.

Run the verification commands. Commit per task in conventional commits style.
Read frontend/AGENTS.md before frontend code.
```
