# Anvil — Bugs & Small UX Implementation Plan (Spec 1)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the eight bug + small-UX items from the 2026-05-06 triage (memory env on settings save, NeoForge/Forge cascading version pickers, fabric runtime gate fix, discard-pending refresh, paper tab label, copy IP, auto-apply mods on create, picked-mods list during create).

**Architecture:** Three swim-lanes that can ship independently. Lane A — backend cross-cutting refactors that lane B and C depend on (memory env helper, `patch_statefulset_env` promotion, refreshable context). Lane B — NeoForge/Forge loader endpoint + per-runtime version env. Lane C — small frontend UX touching multiple tabs.

**Tech Stack:** Rust 1.83+ · axum 0.8 · kube-rs · sqlx (SQLite, offline) · Next.js 16 (App Router, static export) · TypeScript strict · Zod v4. New backend dep: `quick-xml`.

**Source spec:** `docs/superpowers/specs/2026-05-06-anvil-bugs-and-small-ux-design.md` (signed off 2026-05-06).

---

## Context

User filed a 16-item observation list 2026-05-06; triaged into A (no-ops), B (bugs), C (small UX), D (features deferred to specs 2–4). This plan implements groups B + C — eight items, single implementation session.

The non-trivial pieces:

- **Memory env on settings save** — today the PATCH writes only SQLite; running pod keeps old env (B#3).
- **NeoForge install fails** — itzg defaults `NEOFORGE_VERSION=LATEST` per MC version, but NeoForge skips MC versions; Anvil's picker offered MCs that don't have a NeoForge release. Fix is to let the user pick a real loader version. Same treatment for Forge while we're here (B#5 + #9).
- **Auto-apply mods on create** — `initial_mods` write to `pending` and require manual "apply now". Backend now spawns the apply Job automatically (C#10).

The rest are small.

---

## Hard constraints (carried over from CLAUDE.md)

- No new top-level deps without asking. Adding `quick-xml` is approved per spec §3.
- No traits with one impl, no plugin/extension architectures.
- Tailwind v4 utilities + project-local primitives.
- Frontend stays static export (`output: 'export'`); no API routes, SSR, middleware.
- Backend uses kube-rs typed APIs; axum 0.8 path syntax `{param}`.
- Conventional commits per logical change. Commit after each task or task pair.

Run before considering each task done:

```bash
# Backend
cd /home/hch/gitlab/anvil/backend
cargo fmt --all
cargo clippy --all-targets --features serve-dir -- -D warnings
cargo test --all

# Frontend
cd /home/hch/gitlab/anvil/frontend
pnpm typecheck
pnpm lint
```

---

## File structure

### Backend changes (`backend/`)

| File | Change |
|---|---|
| `Cargo.toml` | EDIT — `cargo add quick-xml --features serialize` |
| `src/k8s_patches.rs` | NEW — houses `pub async fn patch_statefulset_env(...)` (moved from `orchestrator.rs`) |
| `src/modpack/orchestrator.rs` | EDIT — remove the inlined `patch_statefulset_env` body, import from `k8s_patches` |
| `src/modpack/memory.rs` | NEW — `pub fn build_memory_env(memory_mi: u32) -> Vec<EnvVar>` + `init_memory_mi` |
| `src/modpack/vanilla.rs` | EDIT — call `memory::build_memory_env` instead of inlining |
| `src/modpack/modded.rs` | EDIT — same call swap; add `loader_version: Option<String>` to `ModdedConfig`; emit `FORGE_VERSION` / `NEOFORGE_VERSION` from it |
| `src/modpack/modrinth.rs` | EDIT — same call swap |
| `src/modpack/curseforge.rs` | EDIT — same call swap |
| `src/modpack/paper.rs` | EDIT — same call swap |
| `src/routes/servers/settings.rs` | EDIT — after the SQLite memory update, call `patch_statefulset_env` with the runtime's full new env |
| `src/routes/runtimes.rs` | NEW — `GET /api/runtimes/{runtime}/versions` (forge / neoforge), 1h cache |
| `src/state.rs` | EDIT — add `loader_version_cache: LoaderVersionCache` to `AppState` (mirror `capabilities_cache`) |
| `src/routes/mod.rs` | EDIT — register the new `runtimes` route |
| `src/routes/servers/create.rs` | EDIT — accept `modded.loader_version` in request; on `initial_mods` non-empty, spawn `mods_apply::run` |
| `src/lib.rs` (or wherever module tree is wired) | EDIT — `mod k8s_patches;` declaration |
| `tests/fixtures/neoforge_maven_metadata.xml` | NEW — fixture for parsing test |
| `tests/fixtures/forge_maven_metadata.xml` | NEW — fixture for parsing test |
| `tests/loader_versions.rs` | NEW — integration test for `GET /api/runtimes/:runtime/versions` |
| `tests/settings_memory.rs` | NEW — integration test for memory env patch on settings save |

### Frontend changes (`frontend/app/`)

| File | Change |
|---|---|
| `lib/server-detail-context.ts` | EDIT — widen value to `{ detail, refresh } \| null`; add `useServerDetail()` hook |
| `servers/ServerDetailView.tsx` | EDIT — extract fetch into `refresh` callback, pass through context provider; tab `label` becomes `detail.source_kind === "paper" ? "plugins" : "mods"` |
| `servers/tabs/ModsBody.tsx` | EDIT — call `refresh()` after `addPendingMod` / `removePendingMod` / `discardPending` (and the same in `PaperPluginsBody` inner component) |
| `servers/tabs/OverviewBody.tsx` | EDIT — add copy-IP icon button next to host:port |
| `servers/new/page.tsx` | EDIT — type-switch sets `runtime = "fabric"` when going to modded; render picked-mods list with remove buttons; cascading pickers when runtime ∈ {forge, neoforge}; submit `loader_version` |
| `lib/api.ts` | EDIT — `fetchLoaderVersions` + Zod; `CreateServerRequest.modded.loader_version`; `LoaderVersions` type |
| `lib/use-loader-versions.ts` | NEW — `useLoaderVersions(runtime)` lazy fetch + cache mirror of `useMcVersions` |
| `components/icons/Copy.tsx` | NEW — small inline SVG |

---

## Decisions locked from spec §9

1. NeoForge/Forge cascading pickers locked. Fabric stays single-picker.
2. Auto-restart on memory save **off**; toast wording stays "applies on next start".
3. Mods auto-apply on create — backend spawns `mods_apply::run`.
4. Picked-mods list rendered below the "+ pre-pick mods" button (compact list, removable).
5. `loader_version` on `ModdedConfig` is `Option<String>`. None ⇒ env emits `LATEST` (back-compat for existing rows).

---

## Task order

Execute in this order. Each task ends with a commit. Stop at section boundaries (lane breaks) for an eyeball check.

### Lane A — backend cross-cutting refactors

#### Task A1: Extract memory env helper

**Files:**
- Create: `backend/src/modpack/memory.rs`
- Modify: `backend/src/modpack/mod.rs`
- Test: `backend/src/modpack/memory.rs` (inline `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write failing unit test**

In `backend/src/modpack/memory.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_memory_env_4096() {
        let env = build_memory_env(4096);
        let init = env.iter().find(|e| e.name == "INIT_MEMORY").unwrap();
        let max = env.iter().find(|e| e.name == "MAX_MEMORY").unwrap();
        let gc = env.iter().find(|e| e.name == "JVM_XX_OPTS").unwrap();
        assert_eq!(init.value.as_deref(), Some("1024M")); // 4096/4 == 1024
        assert_eq!(max.value.as_deref(), Some("4096M"));
        assert!(gc.value.is_some());
    }

    #[test]
    fn init_memory_floor_at_1024() {
        let env = build_memory_env(2048); // 2048/4 == 512, floor to 1024
        let init = env.iter().find(|e| e.name == "INIT_MEMORY").unwrap();
        assert_eq!(init.value.as_deref(), Some("1024M"));
    }
}
```

- [ ] **Step 2: Run test, verify failure**

```
cd backend && cargo test --lib modpack::memory -- --nocapture
```

Expected: compile error / module not declared.

- [ ] **Step 3: Implement**

```rust
// backend/src/modpack/memory.rs
use k8s_openapi::api::core::v1::EnvVar;

use crate::modpack::env_kv;

pub const IDLE_GC_OPTS: &str = "-XX:+UnlockExperimentalVMOptions -XX:+UseG1GC \
    -XX:G1NewSizePercent=20 -XX:G1ReservePercent=20 \
    -XX:MaxGCPauseMillis=50 -XX:G1HeapRegionSize=32M";

pub fn init_memory_mi(memory_mi: u32) -> u32 {
    (memory_mi / 4).max(1024)
}

pub fn build_memory_env(memory_mi: u32) -> Vec<EnvVar> {
    vec![
        env_kv("INIT_MEMORY", &format!("{}M", init_memory_mi(memory_mi))),
        env_kv("MAX_MEMORY", &format!("{}M", memory_mi)),
        env_kv("JVM_XX_OPTS", IDLE_GC_OPTS),
    ]
}
```

Add `pub mod memory;` to `backend/src/modpack/mod.rs`. Verify the existing `IDLE_GC_OPTS` const lives somewhere in `modpack/` (e.g. in an existing provider) — if so, move it here and re-export from the original location, or import from here. (Look at `vanilla.rs:64-66` for the current home.)

- [ ] **Step 4: Run test, verify pass**

```
cargo test --lib modpack::memory -- --nocapture
```

- [ ] **Step 5: Commit**

```bash
git add backend/src/modpack/memory.rs backend/src/modpack/mod.rs
git commit -m "refactor(modpack): extract build_memory_env helper"
```

---

#### Task A2: Replace inlined memory env in providers

**Files:**
- Modify: `backend/src/modpack/vanilla.rs`, `modded.rs`, `modrinth.rs`, `curseforge.rs`, `paper.rs`

- [ ] **Step 1: Replace each provider's inline memory env**

For each of the five files, find the inline pattern (around `vanilla.rs:64-66`, `modded.rs:160-165`, `modrinth.rs:119-124`, `curseforge.rs:228-234`, `paper.rs:73-80`) which today looks like:

```rust
env_kv("INIT_MEMORY", &format!("{}M", init_memory_mi(memory_mi))),
env_kv("MAX_MEMORY", &format!("{}M", memory_mi)),
env_kv("JVM_XX_OPTS", IDLE_GC_OPTS),
```

Replace with:

```rust
let mut env = crate::modpack::memory::build_memory_env(memory_mi);
// then push the rest of the env
env.push(env_kv("VERSION", ...));
// ... continue per-provider env additions
```

Adjust the surrounding `vec![...]` builders to use `Vec::extend` / `push` patterns. Existing tests should still pass.

- [ ] **Step 2: Run all backend tests**

```
cargo test --all
```

Expected: all green.

- [ ] **Step 3: Run clippy**

```
cargo clippy --all-targets --features serve-dir -- -D warnings
cargo clippy --all-targets --features embed -- -D warnings
```

- [ ] **Step 4: Commit**

```bash
git add backend/src/modpack/{vanilla,modded,modrinth,curseforge,paper}.rs
git commit -m "refactor(modpack): use build_memory_env in all providers"
```

---

#### Task A3: Promote `patch_statefulset_env` to `k8s_patches.rs`

**Files:**
- Create: `backend/src/k8s_patches.rs`
- Modify: `backend/src/lib.rs` (or `main.rs` mod tree), `backend/src/modpack/orchestrator.rs`

- [ ] **Step 1: Move the function verbatim**

Cut `async fn patch_statefulset_env(...)` from `orchestrator.rs:435-465` into a new `backend/src/k8s_patches.rs`. Make it `pub`. Keep the existing signature `pub async fn patch_statefulset_env(kube: &kube::Client, namespace: &str, server_id: &str, env: &[EnvVar]) -> Result<(), kube::Error>`. Imports needed: `kube::Api`, `k8s_openapi::api::apps::v1::StatefulSet`, `k8s_openapi::api::core::v1::EnvVar`, `serde_json::json`, `kube::api::PatchParams`, `kube::api::Patch`.

- [ ] **Step 2: Wire module declaration**

Add `pub mod k8s_patches;` near the other top-level modules in `backend/src/lib.rs` (or `main.rs`).

- [ ] **Step 3: Update import in orchestrator**

In `orchestrator.rs`, replace the inlined private fn with `use crate::k8s_patches::patch_statefulset_env;` near the top.

- [ ] **Step 4: Run tests**

```
cargo test --all
cargo clippy --all-targets --features serve-dir -- -D warnings
```

Expected: no behaviour change, all tests still pass.

- [ ] **Step 5: Commit**

```bash
git add backend/src/k8s_patches.rs backend/src/lib.rs backend/src/modpack/orchestrator.rs
git commit -m "refactor(k8s): promote patch_statefulset_env to k8s_patches module"
```

---

#### Task A4: Memory env applied on settings save (B#3)

**Files:**
- Modify: `backend/src/routes/servers/settings.rs`
- Test: `backend/tests/settings_memory.rs` (new)

- [ ] **Step 1: Write the failing integration test**

```rust
// backend/tests/settings_memory.rs
mod common;

use anvil_backend::routes::servers::settings::SettingsRequest;
// ... project-specific test harness imports; use whatever the existing tests/ uses

#[tokio::test]
async fn settings_patch_memory_updates_statefulset_env() {
    let (state, _) = common::test_state().await;
    let id = common::seed_vanilla_server(&state, "ts-mem", 4096).await;

    let req = SettingsRequest { memory_mi: Some(8192), ..Default::default() };
    common::patch_settings(&state, &id, req).await.unwrap();

    let ss = common::fetch_statefulset(&state, &id).await;
    let env = common::container_env(&ss, "mc");
    assert_eq!(common::env_value(&env, "INIT_MEMORY"), Some("2048M".to_string())); // 8192/4
    assert_eq!(common::env_value(&env, "MAX_MEMORY"), Some("8192M".to_string()));
}
```

If a test harness doesn't exist for this style, look at `backend/tests/` for an existing integration test and mirror its setup. If only unit-level tests exist, write a `#[tokio::test]` that uses `kube::Client` against an `envtest` or local kind cluster — fall back to manual repro if the project's test runner can't reach a kube API in CI.

- [ ] **Step 2: Run the test, verify failure**

```
cargo test --test settings_memory -- --nocapture
```

Expected: fails because the StatefulSet env still has the old memory.

- [ ] **Step 3: Implement**

In `backend/src/routes/servers/settings.rs`, after the existing `if let Some(m) = req.memory_mi { sqlx::query("UPDATE servers SET memory_mi = ? ...") }` block (around `:67-73`), add:

```rust
if let Some(m) = req.memory_mi {
    // 1. SQL update (existing).
    sqlx::query("UPDATE servers SET memory_mi = ? WHERE id = ?")
        .bind(m).bind(&id).execute(&state.pool).await?;

    // 2. Rebuild the runtime's full env with the new memory.
    let new_env = build_full_env_for_running_runtime(&state, &id, m as u32).await?;

    // 3. Patch the StatefulSet env in-place.
    crate::k8s_patches::patch_statefulset_env(
        &state.kube,
        &state.mc_namespace,
        &id,
        &new_env,
    ).await
    .map_err(|e| AppError::Internal {
        code: "statefulset_patch_failed",
        message: format!("memory PATCH wrote SQLite but failed to patch StatefulSet env: {e}"),
    })?;
}
```

`build_full_env_for_running_runtime` lives in `settings.rs` for now (single caller). It loads the server row, dispatches on `source_kind`, builds the appropriate provider config with the new `memory_mi`, and calls that provider's `extra_env(&ProviderContext { server_id, memory_mi })`. Sketch:

```rust
async fn build_full_env_for_running_runtime(
    state: &AppState,
    server_id: &str,
    memory_mi: u32,
) -> Result<Vec<EnvVar>, AppError> {
    let row: (String, String) = sqlx::query_as(
        "SELECT source_kind, source_config FROM servers WHERE id = ?")
        .bind(server_id).fetch_one(&state.pool).await?;
    let provider = crate::modpack::from_db(&row.0, &row.1)?;
    Ok(provider.extra_env(&crate::modpack::ProviderContext {
        server_id, memory_mi,
    }))
}
```

(Look up the actual `from_db` signature — it's in `modpack/mod.rs` per the survey. If the function name differs, adjust accordingly.)

- [ ] **Step 4: Run test, verify pass**

```
cargo test --test settings_memory -- --nocapture
cargo clippy --all-targets --features serve-dir -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add backend/src/routes/servers/settings.rs backend/tests/settings_memory.rs
git commit -m "fix(settings): patch StatefulSet env when memory_mi changes"
```

---

#### Task A5: Frontend refreshable `ServerDetailContext`

**Files:**
- Modify: `frontend/app/lib/server-detail-context.ts`
- Modify: `frontend/app/servers/ServerDetailView.tsx`

- [ ] **Step 1: Widen the context type**

```ts
// frontend/app/lib/server-detail-context.ts
"use client";

import { createContext, useContext } from "react";

import type { ServerDetail } from "./api";

export interface ServerDetailValue {
  detail: ServerDetail;
  refresh: () => void;
}

export const ServerDetailContext = createContext<ServerDetailValue | null>(null);

export function useServerDetail(): ServerDetailValue {
  const v = useContext(ServerDetailContext);
  if (!v) {
    throw new Error("useServerDetail must be used inside the layout provider");
  }
  return v;
}

export function useServerDetailCtx(): ServerDetail {
  return useServerDetail().detail;
}
```

`useServerDetailCtx()` retains the legacy shape so unmodified consumers (Overview, Console, Players, Files, Settings tabs) keep working.

- [ ] **Step 2: Wire `refresh` in ServerDetailView**

In `ServerDetailView.tsx`, the existing `useEffect(() => { fetchServerDetail(name) ... })` block already owns the fetch. Pull the fetch body into a `refresh = useCallback(...)` and pass `{ detail, refresh }` through the provider:

```tsx
const refresh = useCallback(() => {
  // existing fetchServerDetail body, setState etc.
}, [name]);

useEffect(() => { refresh(); }, [refresh]);

// ...
<ServerDetailContext.Provider value={{ detail, refresh }}>
  ...
</ServerDetailContext.Provider>
```

- [ ] **Step 3: Typecheck + lint**

```
cd frontend && pnpm typecheck && pnpm lint
```

Expected: green (no consumer changed yet — they all use `useServerDetailCtx()`).

- [ ] **Step 4: Commit**

```bash
git add frontend/app/lib/server-detail-context.ts frontend/app/servers/ServerDetailView.tsx
git commit -m "refactor(server-detail): expose refresh via context"
```

---

### Lane B — NeoForge / Forge loader endpoint + per-runtime version env

#### Task B1: Add `quick-xml` dep + AppState cache slot

**Files:**
- Modify: `backend/Cargo.toml`
- Modify: `backend/src/state.rs` (or wherever AppState lives — verify)

- [ ] **Step 1: Add dep**

```bash
cd backend
cargo add quick-xml --features serialize
```

- [ ] **Step 2: Add cache slot**

```rust
// backend/src/state.rs (or similar)
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Debug, Clone, serde::Serialize)]
pub struct LoaderVersions {
    pub mc_versions: Vec<String>,
    pub by_mc: std::collections::BTreeMap<String, Vec<String>>,
}

pub type LoaderVersionCache =
    Arc<Mutex<std::collections::HashMap<&'static str, (LoaderVersions, Instant)>>>;

// In AppState:
// pub loader_version_cache: LoaderVersionCache,
```

In `main.rs` wiring, init with `Arc::new(Mutex::new(HashMap::new()))`.

- [ ] **Step 3: Build, verify clean**

```
cargo build
cargo clippy --all-targets --features serve-dir -- -D warnings
```

- [ ] **Step 4: Commit**

```bash
git add backend/Cargo.toml backend/Cargo.lock backend/src/state.rs backend/src/main.rs
git commit -m "feat(state): add LoaderVersionCache slot + quick-xml dep"
```

---

#### Task B2: Maven-metadata XML parser unit tests + impl

**Files:**
- Create: `backend/src/routes/runtimes.rs`
- Create: `backend/tests/fixtures/neoforge_maven_metadata.xml`
- Create: `backend/tests/fixtures/forge_maven_metadata.xml`

- [ ] **Step 1: Drop fixture files**

`tests/fixtures/neoforge_maven_metadata.xml` (sample 5-version snippet):

```xml
<?xml version="1.0" encoding="UTF-8"?>
<metadata>
  <groupId>net.neoforged</groupId>
  <artifactId>neoforge</artifactId>
  <versioning>
    <release>21.4.81</release>
    <versions>
      <version>21.4.81</version>
      <version>21.4.80</version>
      <version>21.1.182</version>
      <version>21.1.181</version>
      <version>20.6.121</version>
    </versions>
  </versioning>
</metadata>
```

`tests/fixtures/forge_maven_metadata.xml`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<metadata>
  <groupId>net.minecraftforge</groupId>
  <artifactId>forge</artifactId>
  <versioning>
    <versions>
      <version>1.21.4-54.1.0</version>
      <version>1.21.4-54.0.50</version>
      <version>1.21.1-52.0.20</version>
      <version>1.20.1-47.3.0</version>
    </versions>
  </versioning>
</metadata>
```

- [ ] **Step 2: Write failing parser tests in `routes/runtimes.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const NEOFORGE_FIXTURE: &str = include_str!("../../tests/fixtures/neoforge_maven_metadata.xml");
    const FORGE_FIXTURE: &str = include_str!("../../tests/fixtures/forge_maven_metadata.xml");

    #[test]
    fn parse_neoforge_groups_by_mc_version() {
        let v = parse_neoforge(NEOFORGE_FIXTURE).expect("parse");
        assert_eq!(v.mc_versions, vec!["1.21.4", "1.21.1", "1.20.6"]);
        assert_eq!(v.by_mc.get("1.21.4").unwrap(), &vec!["21.4.81", "21.4.80"]);
    }

    #[test]
    fn parse_forge_groups_by_mc_prefix() {
        let v = parse_forge(FORGE_FIXTURE).expect("parse");
        assert_eq!(v.mc_versions, vec!["1.21.4", "1.21.1", "1.20.1"]);
        assert_eq!(
            v.by_mc.get("1.21.4").unwrap(),
            &vec!["1.21.4-54.1.0", "1.21.4-54.0.50"]
        );
    }
}
```

- [ ] **Step 3: Run tests, verify failure**

```
cargo test --lib routes::runtimes -- --nocapture
```

Expected: compile error (functions not defined).

- [ ] **Step 4: Implement parsers**

```rust
// backend/src/routes/runtimes.rs
use std::collections::BTreeMap;
use anyhow::{Result, anyhow};
use serde::Deserialize;
use crate::state::LoaderVersions;

#[derive(Debug, Deserialize)]
struct MavenMetadata {
    versioning: MavenVersioning,
}
#[derive(Debug, Deserialize)]
struct MavenVersioning {
    versions: MavenVersions,
}
#[derive(Debug, Deserialize)]
struct MavenVersions {
    #[serde(rename = "version", default)]
    version: Vec<String>,
}

fn fetch_versions(xml: &str) -> Result<Vec<String>> {
    let m: MavenMetadata = quick_xml::de::from_str(xml)?;
    Ok(m.versioning.versions.version)
}

pub fn parse_neoforge(xml: &str) -> Result<LoaderVersions> {
    let raws = fetch_versions(xml)?;
    let mut by_mc: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for raw in raws {
        if raw.contains("-beta") { continue; }
        // 21.4.81 -> 1.21.4
        let parts: Vec<&str> = raw.splitn(3, '.').collect();
        if parts.len() < 2 { continue; }
        let mc = format!("1.{}.{}", parts[0], parts[1]);
        by_mc.entry(mc).or_default().push(raw);
    }
    let mut mc_versions: Vec<String> = by_mc.keys().cloned().collect();
    sort_mc_desc(&mut mc_versions);
    for v in by_mc.values_mut() { v.sort_by(|a, b| b.cmp(a)); }
    Ok(LoaderVersions { mc_versions, by_mc })
}

pub fn parse_forge(xml: &str) -> Result<LoaderVersions> {
    let raws = fetch_versions(xml)?;
    let mut by_mc: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for raw in raws {
        let Some((mc, _loader)) = raw.split_once('-') else { continue; };
        by_mc.entry(mc.to_string()).or_default().push(raw);
    }
    let mut mc_versions: Vec<String> = by_mc.keys().cloned().collect();
    sort_mc_desc(&mut mc_versions);
    for v in by_mc.values_mut() { v.sort_by(|a, b| b.cmp(a)); }
    Ok(LoaderVersions { mc_versions, by_mc })
}

fn sort_mc_desc(v: &mut Vec<String>) {
    v.sort_by(|a, b| {
        let pa: Vec<u32> = a.split('.').filter_map(|s| s.parse().ok()).collect();
        let pb: Vec<u32> = b.split('.').filter_map(|s| s.parse().ok()).collect();
        pb.cmp(&pa)
    });
}
```

- [ ] **Step 5: Run tests, verify pass**

```
cargo test --lib routes::runtimes
```

- [ ] **Step 6: Commit**

```bash
git add backend/src/routes/runtimes.rs backend/tests/fixtures/
git commit -m "feat(runtimes): parse maven-metadata for neoforge + forge"
```

---

#### Task B3: Loader versions endpoint with cache + integration test

**Files:**
- Modify: `backend/src/routes/runtimes.rs` (add handler)
- Modify: `backend/src/routes/mod.rs` (register route)
- Test: `backend/tests/loader_versions.rs`

- [ ] **Step 1: Write failing integration test**

```rust
// backend/tests/loader_versions.rs
mod common;

#[tokio::test]
async fn loader_versions_neoforge_returns_grouping() {
    let (state, _) = common::test_state().await;
    // Stub the upstream HTTP — depends on existing test harness; if no HTTP
    // mock is available, prime the cache directly via a test-only helper.
    common::prime_loader_cache(&state, "neoforge", common::neoforge_fixture()).await;

    let resp = common::get(&state, "/api/runtimes/neoforge/versions").await;
    assert_eq!(resp.status, 200);
    let body: serde_json::Value = resp.json();
    assert!(body["mc_versions"].as_array().unwrap().len() > 0);
    assert!(body["by_mc"]["1.21.4"].as_array().is_some());
}

#[tokio::test]
async fn loader_versions_unknown_runtime_404() {
    let (state, _) = common::test_state().await;
    let resp = common::get(&state, "/api/runtimes/fabric/versions").await;
    assert_eq!(resp.status, 404);
}
```

If `common::prime_loader_cache` and `common::get` aren't already in `tests/common/`, add them — minimal helpers around the AppState's cache and an axum `Router`.

- [ ] **Step 2: Run test, verify failure**

```
cargo test --test loader_versions
```

- [ ] **Step 3: Implement handler + cache integration**

```rust
// backend/src/routes/runtimes.rs (add to existing file)
use axum::{extract::{Path, State}, Json};
use std::time::{Duration, Instant};
use crate::state::AppState;
use crate::error::AppError;

const CACHE_TTL: Duration = Duration::from_secs(3600);
const NEOFORGE_URL: &str =
    "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml";
const FORGE_URL: &str =
    "https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml";

pub async fn handle_versions(
    Path(runtime): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<LoaderVersions>, AppError> {
    let key: &'static str = match runtime.as_str() {
        "neoforge" => "neoforge",
        "forge"    => "forge",
        _ => return Err(AppError::NotFound { code: "unknown_runtime" }),
    };
    if let Some(v) = read_cache(&state.loader_version_cache, key) {
        return Ok(Json(v));
    }
    let url = if key == "neoforge" { NEOFORGE_URL } else { FORGE_URL };
    let xml = reqwest::get(url).await?.error_for_status()?.text().await?;
    let parsed = if key == "neoforge" {
        parse_neoforge(&xml)?
    } else {
        parse_forge(&xml)?
    };
    write_cache(&state.loader_version_cache, key, &parsed);
    Ok(Json(parsed))
}

fn read_cache(cache: &LoaderVersionCache, key: &str) -> Option<LoaderVersions> {
    let g = cache.lock().ok()?;
    let (v, ts) = g.get(key)?;
    if ts.elapsed() < CACHE_TTL { Some(v.clone()) } else { None }
}
fn write_cache(cache: &LoaderVersionCache, key: &'static str, v: &LoaderVersions) {
    if let Ok(mut g) = cache.lock() {
        g.insert(key, (v.clone(), Instant::now()));
    }
}
```

In `routes/mod.rs`:

```rust
.route("/api/runtimes/{runtime}/versions", get(runtimes::handle_versions))
```

- [ ] **Step 4: Run tests, verify pass**

```
cargo test --test loader_versions
cargo clippy --all-targets --features serve-dir -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add backend/src/routes/runtimes.rs backend/src/routes/mod.rs backend/tests/loader_versions.rs
git commit -m "feat(api): GET /api/runtimes/{runtime}/versions"
```

---

#### Task B4: `ModdedConfig.loader_version` + env emission

**Files:**
- Modify: `backend/src/modpack/modded.rs`
- Test: inline `#[cfg(test)]` in same file

- [ ] **Step 1: Add field**

In `ModdedConfig`:

```rust
#[derive(...)]
pub struct ModdedConfig {
    pub runtime: Runtime,
    pub mc_version: String,
    #[serde(default)]
    pub loader_version: Option<String>,
    pub mods: Vec<ModEntry>,
    pub pending: Vec<PendingOp>,
}
```

`#[serde(default)]` keeps existing rows decoding cleanly.

- [ ] **Step 2: Write failing unit tests**

```rust
#[cfg(test)]
mod env_tests {
    use super::*;

    fn cfg(runtime: Runtime, loader: Option<&str>) -> ModdedConfig {
        ModdedConfig {
            runtime, mc_version: "1.21.4".into(),
            loader_version: loader.map(String::from),
            mods: vec![], pending: vec![],
        }
    }

    fn ctx() -> ProviderContext<'static> {
        ProviderContext { server_id: "id", memory_mi: 4096 }
    }

    #[test]
    fn fabric_no_loader_env() {
        let r = ModdedRuntime { config: cfg(Runtime::Fabric, None) };
        let env = r.extra_env(&ctx());
        assert!(env.iter().all(|e| e.name != "FORGE_VERSION" && e.name != "NEOFORGE_VERSION"));
    }
    #[test]
    fn forge_emits_forge_version() {
        let r = ModdedRuntime { config: cfg(Runtime::Forge, Some("1.21.4-54.1.0")) };
        let env = r.extra_env(&ctx());
        let v = env.iter().find(|e| e.name == "FORGE_VERSION").unwrap();
        assert_eq!(v.value.as_deref(), Some("1.21.4-54.1.0"));
    }
    #[test]
    fn neoforge_with_no_loader_falls_back_to_latest() {
        let r = ModdedRuntime { config: cfg(Runtime::NeoForge, None) };
        let env = r.extra_env(&ctx());
        let v = env.iter().find(|e| e.name == "NEOFORGE_VERSION").unwrap();
        assert_eq!(v.value.as_deref(), Some("LATEST"));
    }
}
```

- [ ] **Step 3: Implement env emission**

In `extra_env()` (around `modded.rs:155-173`):

```rust
match self.config.runtime {
    Runtime::Fabric => {} // no extra
    Runtime::Forge => env.push(env_kv(
        "FORGE_VERSION",
        self.config.loader_version.as_deref().unwrap_or("LATEST"),
    )),
    Runtime::NeoForge => env.push(env_kv(
        "NEOFORGE_VERSION",
        self.config.loader_version.as_deref().unwrap_or("LATEST"),
    )),
}
```

- [ ] **Step 4: Run tests, verify pass**

```
cargo test --lib modpack::modded
```

- [ ] **Step 5: Commit**

```bash
git add backend/src/modpack/modded.rs
git commit -m "feat(modded): emit FORGE_VERSION/NEOFORGE_VERSION from loader_version"
```

---

#### Task B5: `CreateServerRequest.modded.loader_version` + frontend wiring

**Files:**
- Modify: `backend/src/routes/servers/create.rs`
- Modify: `frontend/app/lib/api.ts`
- Modify: `frontend/app/servers/new/page.tsx`
- Create: `frontend/app/lib/use-loader-versions.ts`

- [ ] **Step 1: Backend — accept loader_version in request**

In `create.rs`, the modded sub-request becomes:

```rust
#[derive(Deserialize)]
pub struct ModdedCreate {
    pub runtime: Runtime,
    #[serde(default)]
    pub initial_mods: Vec<ModEntry>,
    #[serde(default)]
    pub loader_version: Option<String>,
}
```

When persisting `ModdedConfig` (around `:507-513`), populate `loader_version: cfg.loader_version`.

- [ ] **Step 2: Frontend — `fetchLoaderVersions` + Zod**

In `frontend/app/lib/api.ts`:

```ts
const loaderVersionsSchema = z.object({
  mc_versions: z.array(z.string()),
  by_mc: z.record(z.string(), z.array(z.string())),
});
export type LoaderVersions = z.infer<typeof loaderVersionsSchema>;

export async function fetchLoaderVersions(
  runtime: "forge" | "neoforge",
  signal?: AbortSignal,
): Promise<LoaderVersions> {
  const res = await fetch(`/api/runtimes/${runtime}/versions`, { signal });
  if (!res.ok) throw await ApiError.fromResponse(res);
  return loaderVersionsSchema.parse(await res.json());
}
```

Add `loader_version: z.string().optional()` to `createServerSchema.modded`.

- [ ] **Step 3: Frontend — `useLoaderVersions` hook**

```ts
// frontend/app/lib/use-loader-versions.ts
"use client";
import { useEffect, useState } from "react";
import { fetchLoaderVersions, type LoaderVersions } from "./api";

const cache = new Map<string, Promise<LoaderVersions>>();

export function useLoaderVersions(
  runtime: "forge" | "neoforge" | null,
): LoaderVersions | null {
  const [v, setV] = useState<LoaderVersions | null>(null);
  useEffect(() => {
    if (runtime === null) { setV(null); return; }
    let p = cache.get(runtime);
    if (!p) { p = fetchLoaderVersions(runtime); cache.set(runtime, p); }
    let alive = true;
    p.then((r) => { if (alive) setV(r); }).catch(() => { /* surface in UI elsewhere */ });
    return () => { alive = false; };
  }, [runtime]);
  return v;
}
```

- [ ] **Step 4: Frontend — cascading pickers in create form**

In `frontend/app/servers/new/page.tsx`, in the modded branch (`:351-389`), after the runtime selector and before the existing `McVersionPicker`:

```tsx
const loaderRuntime = draft.runtime === "forge" || draft.runtime === "neoforge"
  ? draft.runtime : null;
const loaderVs = useLoaderVersions(loaderRuntime);

// MC picker — when loaderRuntime is set, source from loaderVs?.mc_versions instead
const mcOptions = loaderRuntime !== null
  ? (loaderVs?.mc_versions ?? [])
  : (versions?.versions ?? []);

// after the runtime selector:
{loaderRuntime !== null && loaderVs === null && (
  <p className="text-text-faint text-[11px]">loading {loaderRuntime} versions…</p>
)}
<McVersionPicker
  value={draft.mc_version}
  onChange={switchMcWithGuard}
  versions={mcOptions}
  showFallbackWarning={!loaderRuntime && versions?.source === "fallback"}
/>
{loaderRuntime !== null && draft.mc_version !== null && (
  <div>
    <label className="...">{loaderRuntime} version</label>
    <select
      value={draft.loader_version ?? ""}
      onChange={(e) => set("loader_version", e.target.value === "" ? null : e.target.value)}
    >
      {(loaderVs?.by_mc[draft.mc_version] ?? []).map((v) => (
        <option key={v} value={v}>{v}</option>
      ))}
    </select>
  </div>
)}
```

Add `loader_version: null` to `INITIAL`. In the request submit (`:255-262`), add:

```ts
modded: {
  runtime: draft.runtime,
  initial_mods: draft.initial_mods,
  ...(draft.loader_version !== null ? { loader_version: draft.loader_version } : {}),
},
```

In `switchRuntimeWithGuard` and `switchMcWithGuard`, also set `loader_version` to null on switch.

- [ ] **Step 5: Tests + manual repro**

```
cd backend && cargo test --all
cd frontend && pnpm typecheck && pnpm lint
cd backend && cargo run --features serve-dir
# In another terminal: cd frontend && pnpm build
# Open the create form, switch runtime to neoforge, verify the version picker
```

- [ ] **Step 6: Commit**

```bash
git add backend/src/routes/servers/create.rs frontend/app/lib/api.ts frontend/app/lib/use-loader-versions.ts frontend/app/servers/new/page.tsx
git commit -m "feat(create): cascading mc + loader pickers for forge/neoforge"
```

---

### Lane C — small frontend UX

#### Task C1: Fabric runtime default on type=modded (B#7)

**Files:**
- Modify: `frontend/app/servers/new/page.tsx`

- [ ] **Step 1: Fix the lie**

In the type onChange (around `:322-333`):

```tsx
onChange={(v) => {
  set("type", v);
  if (v !== "modpack") {
    set("curseforge", null);
    set("modrinth", null);
  }
  if (v !== "modded") {
    set("runtime", null);
    set("initial_mods", []);
    set("loader_version", null);
  } else {
    set("runtime", "fabric");          // NEW: default
    set("initial_mods", []);
    set("loader_version", null);
  }
}}
```

In the runtime SegmentedControl (`:358`), drop `?? "fabric"`:

```tsx
value={draft.runtime}        // was: draft.runtime ?? "fabric"
```

Add a non-null assertion or guard since `runtime` is always non-null when type is modded. Use a helper:

```tsx
draft.type === "modded" && draft.runtime !== null && (
  <SegmentedControl value={draft.runtime} ... />
)
```

- [ ] **Step 2: Manual repro**

```
cd frontend && pnpm build
cd ../backend && cargo run --features serve-dir
# Visit /servers/new, click "modded", verify runtime is fabric and "+ pre-pick mods" enables once mc picked.
```

- [ ] **Step 3: Commit**

```bash
git add frontend/app/servers/new/page.tsx
git commit -m "fix(create): set runtime to fabric by default when type becomes modded"
```

---

#### Task C2: Discard pending mod refreshes view (B#11)

**Files:**
- Modify: `frontend/app/servers/tabs/ModsBody.tsx`

- [ ] **Step 1: Switch to `useServerDetail`**

In `ModsBody.tsx`, swap the import:

```tsx
import { useServerDetail } from "../../lib/server-detail-context";
const { detail, refresh } = useServerDetail();
```

In `addPendingMod`, `removePendingMod`, `discardPending`: after the promise resolves and BEFORE the toast, call `refresh()`:

```tsx
const discardPending = (idx: number): void => {
  removePendingMod(detail.id, idx)
    .then(() => {
      refresh();
      toast.push("discarded", "success");
    })
    .catch((err: unknown) => { /* unchanged */ });
};
```

Repeat the pattern in the `PaperPluginsBody` inner component (same file or its body).

- [ ] **Step 2: Manual repro**

Add a pending mod, then discard it, watch the list update without page navigation.

- [ ] **Step 3: Commit**

```bash
git add frontend/app/servers/tabs/ModsBody.tsx
git commit -m "fix(mods): refresh detail context after pending-op mutations"
```

---

#### Task C3: Paper tab labelled "plugins" (B#13a)

**Files:**
- Modify: `frontend/app/servers/ServerDetailView.tsx`

- [ ] **Step 1: Conditional label**

At `:177-182`:

```tsx
{
  id: "mods",
  label: detail.source_kind === "paper" ? "plugins" : "mods",
  href: tabHref("mods"),
  ...(detail.update_available ? { mark: true } : {}),
},
```

Tab id stays `"mods"` — URL routing untouched.

- [ ] **Step 2: Manual repro**

Visit a paper server, verify the tab reads "plugins". Visit a modded server, verify it still reads "mods".

- [ ] **Step 3: Commit**

```bash
git add frontend/app/servers/ServerDetailView.tsx
git commit -m "fix(tabs): paper servers display mods tab as 'plugins'"
```

---

#### Task C4: Copy-IP button (C#2)

**Files:**
- Create: `frontend/app/components/icons/Copy.tsx`
- Modify: `frontend/app/servers/tabs/OverviewBody.tsx`

- [ ] **Step 1: Inline SVG icon**

```tsx
// frontend/app/components/icons/Copy.tsx
export function CopyIcon(): JSX.Element {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor"
         strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <rect x="4" y="4" width="9" height="9" rx="1" />
      <path d="M3 11V4a1 1 0 0 1 1-1h7" />
    </svg>
  );
}
```

- [ ] **Step 2: Wire in OverviewBody**

```tsx
// frontend/app/servers/tabs/OverviewBody.tsx
import { CopyIcon } from "../../components/icons/Copy";
import { useToast } from "../../components/Toast";

// inside component:
const toast = useToast();
const onCopy = (): void => {
  if (!detail.endpoint) return;
  const addr = `${detail.endpoint.host}:${detail.endpoint.port.toString()}`;
  void navigator.clipboard.writeText(addr).then(
    () => toast.push("copied", "success"),
    () => toast.push("clipboard unavailable", "error"),
  );
};

// connection card:
<Card header="connection">
  <div className="flex items-center gap-2">
    <pre className="font-mono text-[12px] text-text-body">
      {detail.endpoint
        ? `${detail.endpoint.host}:${detail.endpoint.port.toString()}`
        : "address pending…"}
    </pre>
    {detail.endpoint && (
      <button
        type="button"
        onClick={onCopy}
        aria-label="copy address"
        className="rounded p-1 text-text-faint hover:text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
      >
        <CopyIcon />
      </button>
    )}
  </div>
</Card>
```

- [ ] **Step 3: Manual repro**

```
cd frontend && pnpm build
cd ../backend && cargo run --features serve-dir
# On a server detail page, click the icon, paste somewhere, verify host:port content.
```

- [ ] **Step 4: Commit**

```bash
git add frontend/app/components/icons/Copy.tsx frontend/app/servers/tabs/OverviewBody.tsx
git commit -m "feat(overview): copy address button"
```

---

#### Task C5: Picked-mods list during create (C#12)

**Files:**
- Modify: `frontend/app/servers/new/page.tsx`

- [ ] **Step 1: Render the list**

After the "+ pre-pick mods" button block (`:373-388`):

```tsx
{draft.initial_mods.length > 0 && (
  <ul className="mt-2 flex flex-col gap-1">
    {draft.initial_mods.map((m, i) => (
      <li
        key={`${m.provider}:${m.version_id}`}
        className="flex items-center gap-2 rounded-md border border-border bg-surface px-2 py-1"
      >
        <span className="font-mono text-[12px] text-text-body">{m.project_name}</span>
        <span className="font-mono text-[11px] text-text-faint">{m.version_name}</span>
        <button
          type="button"
          onClick={() => {
            set(
              "initial_mods",
              draft.initial_mods.filter((_, j) => j !== i),
            );
          }}
          aria-label={`remove ${m.project_name}`}
          className="ml-auto rounded p-1 text-text-faint hover:text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
        >
          ×
        </button>
      </li>
    ))}
  </ul>
)}
```

- [ ] **Step 2: Manual repro**

Pick 3 mods → list renders → remove one → list updates.

- [ ] **Step 3: Commit**

```bash
git add frontend/app/servers/new/page.tsx
git commit -m "feat(create): show picked-mods list with remove buttons"
```

---

#### Task C6: Auto-apply mods on create (C#10)

**Files:**
- Modify: `backend/src/routes/servers/create.rs`

- [ ] **Step 1: Spawn apply Job after create**

After SQLite insert and StatefulSet creation (after `:507-513`), inside the modded source-kind branch:

```rust
if !cfg.initial_mods.is_empty() {
    if let Some(guard) = crate::modpack::UpdateGuard::try_acquire(
        &server_id,
        state.update_locks.clone(),
        state.update_phase_buses.clone(),
    ) {
        let task_state = state.clone();
        let task_id = server_id.clone();
        tokio::spawn(async move {
            crate::modpack::mods_apply::run(
                task_state,
                task_id,
                guard,
                crate::modpack::SyncTarget::Mods,
            ).await;
        });
    } else {
        tracing::warn!(server.id = %server_id, "apply guard unavailable on create; user can apply manually");
    }
}
```

(Verify the exact `UpdateGuard::try_acquire` signature against `routes/servers/mods.rs:170-179`. Adjust imports.)

- [ ] **Step 2: Integration test**

```rust
// backend/tests/create_auto_apply.rs
#[tokio::test]
async fn create_with_initial_mods_spawns_apply() {
    let (state, _) = common::test_state().await;
    let req = common::create_modded_request_with_mods(2);
    let resp = common::post_create(&state, req).await.unwrap();
    let id = resp.id;

    // Apply Job should appear within ~5s
    let job_name = common::wait_for_apply_job(&state, &id, std::time::Duration::from_secs(5))
        .await
        .expect("apply job spawned");
    assert!(job_name.starts_with("mod-sync-mc-"));
}
```

- [ ] **Step 3: Verify**

```
cargo test --test create_auto_apply
cargo clippy --all-targets --features serve-dir -- -D warnings
```

- [ ] **Step 4: Commit**

```bash
git add backend/src/routes/servers/create.rs backend/tests/create_auto_apply.rs
git commit -m "feat(create): auto-apply initial_mods on creation"
```

---

## Verification

After all tasks land:

- [ ] `cd backend && cargo fmt --all`
- [ ] `cargo clippy --all-targets --features serve-dir -- -D warnings`
- [ ] `cargo clippy --all-targets --features embed -- -D warnings`
- [ ] `cargo test --all`
- [ ] `cd frontend && pnpm typecheck && pnpm lint && pnpm build`
- [ ] Single-binary smoke: `cd backend && cargo run --release --features embed` → curl `/health` and a server detail
- [ ] Manual repro of every FE item (C1–C5 above)

---

## Implementation prompt

Paste into a fresh Claude Code session inside `/home/hch/gitlab/anvil`:

```
Implement the plan at docs/superpowers/plans/2026-05-06-anvil-bugs-and-small-ux-impl.md.

Use the superpowers:executing-plans skill (or superpowers:subagent-driven-development if you'd
prefer one fresh subagent per task). Follow tasks A1 → A5 → B1 → B5 → C1 → C6 in order. The
spec at docs/superpowers/specs/2026-05-06-anvil-bugs-and-small-ux-design.md is the design
authority — refer to it when the plan elides a detail.

Run the verification commands at the end before reporting done. Commit per task in conventional
commits style. Read frontend/AGENTS.md before touching frontend code (Next.js 16 has breaking
changes from your training data).
```
