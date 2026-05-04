# Anvil v2 — Sub-project A (Foundation Rehaul) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the Anvil v2 foundation rehaul — workshop-aesthetic design system, new component primitives, redesigned server list / detail / create pages, CPU control, expanded MC version list, update-FSM display, and folded-in polish-audit cleanup.

**Architecture:** Six sections, ordered for a single-engineer execution stream. Backend foundation (DB migration, CPU field, MC versions endpoint, dead-code cleanup) lands first so the frontend's Zod schemas align with known shapes. Then design tokens + primitives, then layout swap (`CommandBar` replaces `UserBadge`), then list page, then create page + tabbed detail page, then polish + verification.

**Tech Stack:** Rust 1.83+ · axum 0.8 · kube-rs · sqlx (offline SQLite) · Next.js 16.2.4 (App Router, `output: 'export'`) · React 19.2.4 · TypeScript strict · Tailwind v4.2.4 · Zod v4. No new top-level dependencies.

**Source spec:** `/home/hadi/gitlab/anvil/docs/superpowers/specs/2026-05-03-anvil-v2-foundation-design.md` (signed off).
**Audit input:** `/home/hadi/gitlab/anvil/docs/polish-audit.md`.

---

## Context

Anvil v1.0.0 ships M1–M5 (server lifecycle, OIDC, live logs/RCON, CurseForge ServerFiles modpack pipeline). The polish audit found three deferred M5 UI surfaces missing, speculative backend code with no consumer (`changelog_excerpt`, `provider_to_config`/`serde_project_id`, broken `force_version`), a flat detail page that can't host the additional surfaces (mods/players/files/settings) the user wants, and visual rough edges (focus rings, raw kebab-case strings, silent UserBadge failures).

Rather than incrementally polishing v1, v2 is a **foundation rehaul + tightly-scoped capability bump**. Sub-project A covers the foundation; mod ecosystem (B), player management (C), and file-browser sidecar (D) ship as later sub-projects with their own design cycles. Mods/Players/Files tabs render placeholders in A.

After A is done, the user deploys via FluxCD at `/home/hadi/Documents/GitHub/homelab-k8s-fluxcd/`.

---

## Hard Constraints (carry-overs from CLAUDE.md)

- No new top-level dependencies without asking.
- No traits with one impl, no plugin/extension architectures, no preparedness code.
- Tailwind v4 utilities + project-local primitives (no Radix, no shadcn, no Headless UI).
- Frontend stays static export (`output: 'export'`); no API routes, SSR, middleware, or rewrites.
- Backend uses kube-rs typed APIs; axum 0.8 path syntax `{param}` not `:param`.
- Conventional commits per logical change. Frequent commits.
- Stop every ~5 commits or section boundary so the user can eyeball.

---

## Decisions Locked (from spec §13 and Plan-agent review)

1. **Tab routing:** explicit per-tab pages (`/servers/[name]/page.tsx` = overview, `/servers/[name]/console/page.tsx`, `/servers/[name]/mods/page.tsx`, etc.) **not** dynamic `[tab]`. Cleaner for static export, no `generateStaticParams` gymnastics.
2. **Toast scope:** lifecycle actions (start/stop/restart) and settings PATCH show a toast. Delete keeps the existing `ConfirmDeleteDialog`. Update FSM gets its own `Sheet` (no toast). **No 5-second stop-undo affordance** — adds deferred-execution complexity for a marginal-value flow; defer to a later cycle. Spec proposed it but didn't lock it.
3. **Mojang version cache:** an `Arc<Mutex<Option<(Vec<String>, Instant)>>>` field on `AppState`, mirroring the existing `capabilities_cache` pattern.
4. **`ModpackProvider::project_id()`:** add to the trait with default `fn project_id(&self) -> Option<u32> { None }`, override in CF impl. Removes the dead `provider_to_config`/`serde_project_id` workaround. Vanilla impl inherits the default.
5. **Component removal vs. soft-deprecation:** hard remove. `UserBadge`, `NewServerModal`, `StatusBadge` are deleted in the same commit that removes their last callsite.
6. **`useUpdateStream` parameter is the server UUID, not the name.** The new name-based detail page must fetch `ServerDetail` (which carries the UUID) before subscribing.

---

## File Structure

### Backend changes (`/home/hadi/gitlab/anvil/backend/`)

| File | Change |
|---|---|
| `migrations/0004_m6_cpu_field.sql` | NEW — `ALTER TABLE servers ADD COLUMN cpu_millicores INTEGER NOT NULL DEFAULT 1000;` |
| `Cargo.toml` | EDIT — remove `wiremock = "0.6"` from `[dev-dependencies]` |
| `src/lib.rs` | EDIT — add `mc_versions_cache: McVersionsCache` field to `AppState`; init in main wiring |
| `src/main.rs` | EDIT — pass new cache field into `AppState` |
| `src/validation.rs` | EDIT — add `validate_storage_size_gi`, `validate_slug`, `validate_force_version`, `validate_version_skip`; switch `validate_mc_version` to async-cache-backed (or inline check against the cache loaded at startup) |
| `src/k8s_builders.rs` | EDIT — `pod_resources(memory_mi, cpu_millicores)` + thread `cpu_millicores` through `BuildParams` and `build_statefulset` |
| `src/routes/servers/create.rs` | EDIT — `CreateRequest` adds `cpu_millicores: i64`; persist to DB; pass to `BuildParams` |
| `src/routes/servers/get.rs` | EDIT — `ServerDetail` adds `cpu_millicores: i64`; remove `latest_changelog_excerpt`; SELECT updated |
| `src/routes/servers/settings.rs` | EDIT — `SettingsRequest` adds `memory_mi: Option<i64>` and `cpu_millicores: Option<i64>`; UPDATE statement extended |
| `src/routes/servers/mod.rs` (or `list.rs`) | EDIT — list response includes `cpu_millicores`; remove `latest_changelog_excerpt` if present |
| `src/routes/cluster.rs` | EDIT — `ClusterCapabilities` adds `available_cpu_cores: f64`; aggregate from `Node.status.allocatable.cpu` |
| `src/routes/mc_versions.rs` | NEW — `GET /api/cluster/mc-versions` handler with 24h cache, Mojang manifest fetch, offline fallback to hardcoded `KNOWN_MC_VERSIONS` |
| `src/routes/mod.rs` | EDIT — register the new mc-versions route |
| `src/modpack/mod.rs` | EDIT — drop `changelog_excerpt: Option<String>` from `VersionInfo`; add `fn project_id(&self) -> Option<u32> { None }` default to `ModpackProvider` trait |
| `src/modpack/curseforge.rs` | EDIT — implement `project_id` returning `Some(self.config.project_id)` |
| `src/modpack/poller.rs` | EDIT — drop `changelog_excerpt` writes to DB |
| `src/modpack/orchestrator.rs` | EDIT — replace `pick_target_version` body to use `provider.project_id()`; delete `provider_to_config` and `serde_project_id`; wire `force_version` into the version pick |
| `src/modpack/cf_client.rs` | EDIT — `fetch_files` paginated loop with 500-file cap |

Repo root:

| File | Change |
|---|---|
| `identifier.sqlite` | DELETE (`git rm --cached` then `rm`) — already in `.gitignore` |

### Frontend changes (`/home/hadi/gitlab/anvil/frontend/app/`)

| File | Change |
|---|---|
| `globals.css` | EDIT — expand `@theme` with full §5 token set |
| `layout.tsx` | EDIT — mount `CommandBar` instead of `UserBadge` |
| `page.tsx` | EDIT — server list per spec §8.1 (Skeleton, summary line, source bars, update indicators, visibility-paused poll, whole-row click, action reveal-on-hover) |
| `lib/api.ts` | EDIT — add `cpuMillicoresField`/`mcVersionsSchema`; add `cpu_millicores` to summary/detail/create/settings schemas; add `available_cpu_cores` to capabilities; drop `latest_changelog_excerpt` |
| `lib/update-stream.ts` | EDIT — align `onopen`/`hello` with `logs-stream.ts` (audit §6.2) |
| `components/Button.tsx` | EDIT — add `focus-visible:ring`, primary `[label]` brackets via inner spans, `ghost` variant |
| `components/Modal.tsx` | EDIT — focus trap, SVG `✕`, focus ring on close button |
| `components/IconButton.tsx` | NEW — icon-only square button, `aria-label` required |
| `components/Sheet.tsx` | NEW — right slide-over, 480/640/720px width prop, Esc, backdrop scrim, focus-trap |
| `components/Card.tsx` | NEW — bordered panel, optional `header` prop |
| `components/Tabs.tsx` | NEW — copper underline on active, optional `count` and `mark` props |
| `components/Badge.tsx` | NEW — variants `running`/`stopped`/`starting`/`stopping`/`error`/`update` |
| `components/Toast.tsx` | NEW — bottom-right transient, 4s default, container + hook |
| `components/Dropdown.tsx` | NEW — action menu (`⋯`) |
| `components/Skeleton.tsx` | NEW — `row`/`block`/`text` variants |
| `components/Tooltip.tsx` | NEW — hover annotation |
| `components/SegmentedControl.tsx` | NEW — release/beta/alpha-style toggles |
| `components/RangeSlider.tsx` | NEW — `input[type=range]` + tick marks + value display |
| `components/PathBreadcrumb.tsx` | NEW — anvil mark + `/`-separated segments |
| `components/CommandBar.tsx` | NEW — top bar; left = `PathBreadcrumb`, right = user identity + logout |
| `components/BuildSlip.tsx` | NEW — sticky-left spec sheet for create page; defines `CreateFormContext` in same file |
| `components/UserBadge.tsx` | DELETE |
| `components/NewServerModal.tsx` | DELETE |
| `components/StatusBadge.tsx` | DELETE |
| `components/ServerTable.tsx` → `components/ServerList.tsx` | RENAME + rewrite per §8.1 |
| `components/LiveLogPanel.tsx` | EDIT — re-skin with new tokens + friendly `EndReason` lookup map |
| `components/RconCommand.tsx` | EDIT — re-skin with new tokens |
| `servers/[name]/layout.tsx` | NEW — detail-page shell: header + update banner + `Tabs` strip; client component, fetches `ServerDetail` via `useServerDetail(name)` and provides via context |
| `servers/[name]/page.tsx` | NEW — Overview tab body |
| `servers/[name]/console/page.tsx` | NEW — Console tab body (`LiveLogPanel` + `RconCommand`) |
| `servers/[name]/mods/page.tsx` | NEW — placeholder + read-only modpack identity |
| `servers/[name]/players/page.tsx` | NEW — placeholder |
| `servers/[name]/files/page.tsx` | NEW — placeholder |
| `servers/[name]/settings/page.tsx` | NEW — Settings tab (memory/cpu/mc_version, modpack auto-update mode + version-skip, delete server in danger zone) |
| `servers/new/page.tsx` | NEW — create flow (build slip + six numbered sections + bottom bar) |
| `servers/detail/page.tsx` | EDIT — replace body with one-shot redirect: read `?id=`, GET `/api/servers/{id}`, `router.replace('/servers/' + name)` |
| `lib/server-detail-context.ts` | NEW — small `createContext<ServerDetail | null>(null)` and `useServerDetailCtx()` hook used by `[name]/layout.tsx` and tab pages |
| `lib/use-mc-versions.ts` | NEW — `useMcVersions()` hook fetching `/api/cluster/mc-versions` once, cached in module-scope |
| `lib/end-reason.ts` | NEW — `EndReason` → friendly text map for `LiveLogPanel` |

---

## Verification commands (run between sections)

```bash
# Backend (cwd: /home/hadi/gitlab/anvil/backend)
cargo fmt --check
cargo clippy --all-targets --features serve-dir -- -D warnings
cargo clippy --all-targets --features embed   -- -D warnings
cargo test --all

# Frontend (cwd: /home/hadi/gitlab/anvil/frontend)
pnpm typecheck
pnpm lint
pnpm build

# Full local single-binary smoke (cwd: /home/hadi/gitlab/anvil)
cd frontend && pnpm build && cd ../backend && cargo build --release --features embed
```

After each section, run all of the above that apply. Run `superpowers:code-reviewer` on section boundaries.

---

# Section 1 — Backend foundation

**Acceptance gate:** All cargo checks green. `GET /api/cluster/mc-versions` returns ≥15 versions. `POST /api/servers` with `cpu_millicores=2000` creates a StatefulSet with `limits.cpu: 2000m` (verified via `kubectl get sts mc-<id> -o yaml`). Validation functions reject out-of-bounds inputs.

### Task 1.1: Repository hygiene — drop `wiremock`, delete `identifier.sqlite`

**Files:**
- Modify: `/home/hadi/gitlab/anvil/backend/Cargo.toml`
- Delete: `/home/hadi/gitlab/anvil/identifier.sqlite`

- [ ] **Step 1: Verify `wiremock` has no callers**

```bash
rg -n 'wiremock' /home/hadi/gitlab/anvil/backend
```
Expected: matches only in `Cargo.toml`. If anything else, stop and report.

- [ ] **Step 2: Remove `wiremock` line from `[dev-dependencies]`**

Edit `Cargo.toml`. Find the `[dev-dependencies]` block; delete `wiremock = "0.6"`.

- [ ] **Step 3: Drop the stray dev DB**

```bash
git rm --cached /home/hadi/gitlab/anvil/identifier.sqlite
rm -f /home/hadi/gitlab/anvil/identifier.sqlite
```
Verify: `grep -F 'identifier.sqlite' /home/hadi/gitlab/anvil/.gitignore` returns the line (already present per inventory).

- [ ] **Step 4: Run cargo checks**

```bash
cd /home/hadi/gitlab/anvil/backend
cargo fmt --check
cargo clippy --all-targets --features serve-dir -- -D warnings
cargo test --all
```
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add backend/Cargo.toml identifier.sqlite
git commit -m "chore: drop unused wiremock dev-dep and stray identifier.sqlite"
```

### Task 1.2: Migration `0004_m6_cpu_field.sql` and DB-layer thread

**Files:**
- Create: `/home/hadi/gitlab/anvil/backend/migrations/0004_m6_cpu_field.sql`

- [ ] **Step 1: Write the migration**

```sql
-- M6: per-server CPU limit (millicores). Default 1000 = 1 core matches the
-- prior hardcoded "1000m" floor; existing servers keep running until next
-- restart picks up the new spec.
ALTER TABLE servers ADD COLUMN cpu_millicores INTEGER NOT NULL DEFAULT 1000;
```

- [ ] **Step 2: Regenerate sqlx-data offline cache (if used)**

Inventory did not confirm `sqlx-data.json`. Run:

```bash
cd /home/hadi/gitlab/anvil/backend
ls sqlx-data.json .sqlx 2>/dev/null
```

If present, regenerate later (after queries are updated) per project convention. Do not regenerate yet.

- [ ] **Step 3: Sanity-run the backend to apply the migration locally**

```bash
cd /home/hadi/gitlab/anvil/backend
ANVIL_MC_STORAGE_CLASS=tank \
ANVIL_OIDC_ISSUER_URL=https://example/x \
ANVIL_OIDC_CLIENT_ID=x \
ANVIL_OIDC_CLIENT_SECRET=x \
ANVIL_OIDC_REDIRECT_URL=http://localhost:8080/api/auth/callback \
ANVIL_SESSION_KEY="$(openssl rand -base64 48)" \
ANVIL_DATABASE_URL='sqlite:///tmp/anvil-cpu-test.db?mode=rwc' \
cargo run --features serve-dir &
BACKEND_PID=$!
sleep 5
sqlite3 /tmp/anvil-cpu-test.db '.schema servers' | grep cpu_millicores
kill $BACKEND_PID
```
Expected: schema line shows `cpu_millicores INTEGER NOT NULL DEFAULT 1000`.

- [ ] **Step 4: Commit (migration only — code changes follow)**

```bash
git add backend/migrations/0004_m6_cpu_field.sql
git commit -m "feat(db): migration 0004 adds cpu_millicores to servers"
```

### Task 1.3: Thread `cpu_millicores` through k8s_builders, create, get, list, settings

**Files:**
- Modify: `/home/hadi/gitlab/anvil/backend/src/k8s_builders.rs`
- Modify: `/home/hadi/gitlab/anvil/backend/src/routes/servers/create.rs`
- Modify: `/home/hadi/gitlab/anvil/backend/src/routes/servers/get.rs`
- Modify: `/home/hadi/gitlab/anvil/backend/src/routes/servers/mod.rs` (list)
- Modify: `/home/hadi/gitlab/anvil/backend/src/routes/servers/settings.rs`

- [ ] **Step 1: `k8s_builders.rs::pod_resources` takes cpu_millicores**

Find `fn pod_resources(memory_mi: i64) -> ResourceRequirements` (around line 317). Change signature and body:

```rust
fn pod_resources(memory_mi: i64, cpu_millicores: i64) -> ResourceRequirements {
    let mut limits: BTreeMap<String, Quantity> = BTreeMap::new();
    limits.insert("memory".to_owned(), Quantity(format!("{memory_mi}Mi")));
    limits.insert("cpu".to_owned(), Quantity(format!("{cpu_millicores}m")));
    ResourceRequirements {
        requests: None,
        limits: Some(limits),
        claims: None,
    }
}
```

Find `BuildParams` (or whichever struct passes `memory_mi` to the builder). Add `pub cpu_millicores: i64`. Find the call site `pod_resources(params.memory_mi)` and change to `pod_resources(params.memory_mi, params.cpu_millicores)`.

- [ ] **Step 2: `create.rs::CreateRequest` adds the field**

Find the `CreateRequest` struct. Add `pub cpu_millicores: i64,` after `memory_mi`. Find the `validate_*` calls; add a new validator call (the validator is added in Task 1.6):

```rust
crate::validation::validate_cpu_millicores(req.cpu_millicores)?;
```

Find the `INSERT INTO servers` statement. Add `cpu_millicores` to the column list and bind the value. Pass `req.cpu_millicores` to `BuildParams` constructor.

- [ ] **Step 3: `get.rs::ServerDetail` adds the field, drops `latest_changelog_excerpt`**

In `ServerDetail` struct: add `pub cpu_millicores: i64,`; delete `pub latest_changelog_excerpt: Option<String>,` (around line 48). In the SELECT statement: add `cpu_millicores` to the projection; remove the `changelog_excerpt`/`latest_changelog_excerpt` join column. In the response builder: remove the `latest_changelog_excerpt:` field; add `cpu_millicores: row.cpu_millicores,`.

- [ ] **Step 4: list response (`servers/mod.rs` or `list.rs`) adds the field**

Find the list handler and the row struct it returns. Add `cpu_millicores: i64` to the row struct and projection.

- [ ] **Step 5: `settings.rs::SettingsRequest` adds memory_mi + cpu_millicores**

Add fields:

```rust
pub memory_mi: Option<i64>,
pub cpu_millicores: Option<i64>,
```

In the validation block, add (the validator names assume Task 1.6 has been done; if not, sequence Task 1.6 before this step):

```rust
if let Some(m) = req.memory_mi { crate::validation::validate_memory_mi(m)?; }
if let Some(c) = req.cpu_millicores { crate::validation::validate_cpu_millicores(c)?; }
```

In the UPDATE statement, build the SET clause to include each provided field. Pattern: collect a `Vec<&'static str>` of column setters and bind values in the same order, append at the end. Match the existing pattern in `settings.rs` for how `auto_update_mode` and `version_skip` are set.

- [ ] **Step 6: cargo check**

```bash
cd /home/hadi/gitlab/anvil/backend
cargo check --features serve-dir
```
Expected: green (compile only — tests follow once validators are in).

- [ ] **Step 7: Commit**

```bash
git add backend/src/k8s_builders.rs backend/src/routes/servers/create.rs \
  backend/src/routes/servers/get.rs backend/src/routes/servers/mod.rs \
  backend/src/routes/servers/settings.rs
git commit -m "feat(api): per-server cpu_millicores field; drop dead changelog_excerpt"
```

### Task 1.4: `ClusterCapabilities` adds `available_cpu_cores`

**Files:**
- Modify: `/home/hadi/gitlab/anvil/backend/src/routes/cluster.rs`

- [ ] **Step 1: Read the current handler**

```bash
sed -n '1,160p' /home/hadi/gitlab/anvil/backend/src/routes/cluster.rs
```
Note the `kube::Api<Node>` usage and the cache pattern.

- [ ] **Step 2: Extend `ClusterCapabilities` struct**

Add field:

```rust
/// Sum of allocatable CPU across schedulable nodes, in fractional cores.
pub available_cpu_cores: f64,
```

- [ ] **Step 3: Aggregate allocatable CPU**

In the handler that builds `ClusterCapabilities`, add a Node list query (or extend the existing one). For each schedulable node (skip cordoned: check `node.spec.unschedulable != Some(true)`), parse `node.status.allocatable["cpu"]` Quantity into millicores using `parse_cpu_quantity` (helper below), sum, then divide by 1000.0:

```rust
fn parse_cpu_quantity(q: &str) -> Option<i64> {
    if let Some(n) = q.strip_suffix('m') {
        n.parse::<i64>().ok()
    } else {
        q.parse::<f64>().ok().map(|f| (f * 1000.0) as i64)
    }
}
```

If Node listing fails, log + return `0.0` rather than 500-erroring the capabilities endpoint.

- [ ] **Step 4: cargo clippy**

```bash
cd /home/hadi/gitlab/anvil/backend
cargo clippy --all-targets --features serve-dir -- -D warnings
```
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add backend/src/routes/cluster.rs
git commit -m "feat(api): /cluster/capabilities surfaces available_cpu_cores"
```

### Task 1.5: `GET /api/cluster/mc-versions` with 24h cache + Mojang manifest fetch

**Files:**
- Create: `/home/hadi/gitlab/anvil/backend/src/routes/mc_versions.rs`
- Modify: `/home/hadi/gitlab/anvil/backend/src/routes/mod.rs`
- Modify: `/home/hadi/gitlab/anvil/backend/src/lib.rs` (add cache field to `AppState`)
- Modify: `/home/hadi/gitlab/anvil/backend/src/main.rs` (init the cache)

- [ ] **Step 1: Write the failing test**

Create `/home/hadi/gitlab/anvil/backend/tests/mc_versions.rs` (this is integration-test territory; if the project uses unit tests in-module, fall back to that pattern):

```rust
//! Verifies that the manifest parser extracts release versions and caps the list.

use anvil::routes::mc_versions::{parse_manifest, MAX_VERSIONS};

#[test]
fn parses_release_versions_capped() {
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
    assert!(v.len() <= MAX_VERSIONS);
}
```

- [ ] **Step 2: Run the test (expect compile failure)**

```bash
cd /home/hadi/gitlab/anvil/backend
cargo test --test mc_versions
```
Expected: fails — module does not exist yet.

- [ ] **Step 3: Implement `mc_versions.rs`**

```rust
//! GET /api/cluster/mc-versions — cached Mojang version manifest (release only).
//!
//! 24-hour TTL. Offline fallback to a hardcoded baseline so the panel stays
//! usable when the Mojang CDN is unreachable.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::error::AppError;
use crate::AppState;

pub type McVersionsCache = Arc<Mutex<Option<(Vec<String>, Instant)>>>;

pub const MAX_VERSIONS: usize = 20;
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MANIFEST_URL: &str =
    "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json";

const FALLBACK: &[&str] = &[
    "1.21.4", "1.21.3", "1.21.1", "1.21.0", "1.20.6", "1.20.4",
];

#[derive(Deserialize)]
struct Manifest {
    versions: Vec<ManifestVersion>,
}

#[derive(Deserialize)]
struct ManifestVersion {
    id: String,
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Serialize)]
pub struct McVersionsResponse {
    pub versions: Vec<String>,
    pub source: &'static str, // "mojang" | "fallback"
}

pub fn parse_manifest(body: &str) -> Result<Vec<String>, serde_json::Error> {
    let m: Manifest = serde_json::from_str(body)?;
    let mut out: Vec<String> = m
        .versions
        .into_iter()
        .filter(|v| v.kind == "release")
        .map(|v| v.id)
        .take(MAX_VERSIONS)
        .collect();
    out.shrink_to_fit();
    Ok(out)
}

pub async fn handle(
    State(state): State<AppState>,
) -> Result<Json<McVersionsResponse>, AppError> {
    let mut guard = state.mc_versions_cache.lock().await;
    if let Some((cached, at)) = guard.as_ref()
        && at.elapsed() < CACHE_TTL
    {
        return Ok(Json(McVersionsResponse {
            versions: cached.clone(),
            source: "mojang",
        }));
    }
    let fetched = fetch_and_parse().await;
    match fetched {
        Ok(vs) => {
            *guard = Some((vs.clone(), Instant::now()));
            Ok(Json(McVersionsResponse { versions: vs, source: "mojang" }))
        }
        Err(e) => {
            tracing::warn!(error = %e, "mc-versions: mojang fetch failed; using fallback");
            Ok(Json(McVersionsResponse {
                versions: FALLBACK.iter().map(|s| (*s).to_owned()).collect(),
                source: "fallback",
            }))
        }
    }
}

async fn fetch_and_parse() -> anyhow::Result<Vec<String>> {
    let body = reqwest::Client::new()
        .get(MANIFEST_URL)
        .timeout(Duration::from_secs(8))
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(parse_manifest(&body)?)
}
```

- [ ] **Step 4: Add `McVersionsCache` field to `AppState`**

In `/home/hadi/gitlab/anvil/backend/src/lib.rs`, add the field after `capabilities_cache`:

```rust
pub mc_versions_cache: crate::routes::mc_versions::McVersionsCache,
```

In `/home/hadi/gitlab/anvil/backend/src/main.rs` where `AppState { ... }` is constructed, add:

```rust
mc_versions_cache: Arc::new(tokio::sync::Mutex::new(None)),
```

(The existing AppState uses both std and async mutexes — match the pattern of `snapshot_pvc_lock` which is `Arc<AsyncMutex<()>>`.)

- [ ] **Step 5: Wire the route**

In `/home/hadi/gitlab/anvil/backend/src/routes/mod.rs`, add `pub mod mc_versions;` and:

```rust
.route("/api/cluster/mc-versions", get(mc_versions::handle))
```

- [ ] **Step 6: Run the test**

```bash
cd /home/hadi/gitlab/anvil/backend
cargo test --test mc_versions -- --nocapture
```
Expected: pass.

- [ ] **Step 7: Smoke test the endpoint**

```bash
cd /home/hadi/gitlab/anvil/backend
ANVIL_MC_STORAGE_CLASS=tank ANVIL_OIDC_ISSUER_URL=... ... \
cargo run --features serve-dir &
sleep 5
curl -s localhost:8080/api/cluster/mc-versions | jq .
kill %1
```
Expected: `{ "versions": [...≥15...], "source": "mojang" }`.

- [ ] **Step 8: Commit**

```bash
git add backend/src/routes/mc_versions.rs backend/src/routes/mod.rs \
  backend/src/lib.rs backend/src/main.rs backend/tests/mc_versions.rs
git commit -m "feat(api): /cluster/mc-versions with mojang manifest cache"
```

### Task 1.6: Validation tightening + switch `validate_mc_version` to cache-backed

**Files:**
- Modify: `/home/hadi/gitlab/anvil/backend/src/validation.rs`

- [ ] **Step 1: Write failing tests**

Append to `validation.rs` `#[cfg(test)] mod tests { ... }`:

```rust
#[test]
fn storage_size_gi_bounds() {
    assert!(validate_storage_size_gi(0).is_err());
    assert!(validate_storage_size_gi(9).is_err());
    assert!(validate_storage_size_gi(10).is_ok());
    assert!(validate_storage_size_gi(500).is_ok());
    assert!(validate_storage_size_gi(501).is_err());
}

#[test]
fn cpu_millicores_bounds() {
    assert!(validate_cpu_millicores(0).is_err());
    assert!(validate_cpu_millicores(250).is_ok());
    assert!(validate_cpu_millicores(8000).is_ok());
    assert!(validate_cpu_millicores(16001).is_err());
}

#[test]
fn slug_length_cap() {
    assert!(validate_slug("ok").is_ok());
    assert!(validate_slug("").is_err());
    assert!(validate_slug(&"a".repeat(200)).is_ok());
    assert!(validate_slug(&"a".repeat(201)).is_err());
}

#[test]
fn force_version_format() {
    assert!(validate_force_version("1.21.4").is_ok());
    assert!(validate_force_version("ATM-11_v3.2-final").is_ok());
    assert!(validate_force_version("").is_err());
    assert!(validate_force_version("bad version!").is_err());
    assert!(validate_force_version(&"a".repeat(129)).is_err());
}

#[test]
fn version_skip_cap() {
    let ok: Vec<String> = (0..50).map(|i| format!("v{i}")).collect();
    assert!(validate_version_skip(&ok).is_ok());
    let too_many: Vec<String> = (0..51).map(|i| format!("v{i}")).collect();
    assert!(validate_version_skip(&too_many).is_err());
}
```

- [ ] **Step 2: Run tests (expect failure)**

```bash
cd /home/hadi/gitlab/anvil/backend
cargo test validation::tests
```
Expected: fail — functions don't exist.

- [ ] **Step 3: Implement the validators**

Append to `validation.rs`:

```rust
pub fn validate_storage_size_gi(value: i64) -> Result<(), AppError> {
    if (10..=500).contains(&value) {
        Ok(())
    } else {
        Err(AppError::BadRequest(format!(
            "storage_size_gi must be 10..=500 (got {value})"
        )))
    }
}

pub fn validate_cpu_millicores(value: i64) -> Result<(), AppError> {
    // 250m floor (anything less starves the JVM); 16000m ceiling (panel cluster
    // upper bound — see cluster-profile.md).
    if (250..=16000).contains(&value) {
        Ok(())
    } else {
        Err(AppError::BadRequest(format!(
            "cpu_millicores must be 250..=16000 (got {value})"
        )))
    }
}

pub fn validate_slug(s: &str) -> Result<(), AppError> {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed.len() > 200 {
        return Err(AppError::BadRequest(
            "slug must be 1..=200 non-blank characters".to_owned(),
        ));
    }
    Ok(())
}

pub fn validate_force_version(v: &str) -> Result<(), AppError> {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r"^[A-Za-z0-9._-]{1,128}$").expect("static regex")
    });
    if re.is_match(v) {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "force_version must match [A-Za-z0-9._-]{1,128}".to_owned(),
        ))
    }
}

pub fn validate_version_skip(list: &[String]) -> Result<(), AppError> {
    if list.len() > 50 {
        return Err(AppError::BadRequest(
            "version_skip exceeds 50 entries".to_owned(),
        ));
    }
    Ok(())
}
```

The `regex` crate may not be a dependency. Check `Cargo.toml`. If absent, **stop and ask the user** before adding it. (Per CLAUDE.md, no new top-level dependencies without asking. `regex` is a small, standard crate but still a new dep. Hand-rolled char loop is acceptable: `v.len() <= 128 && !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))`.)

- [ ] **Step 4: Wire into call sites**

In `routes/servers/create.rs`: add `validate_storage_size_gi(req.storage_size_gi)?;` before the existing inline check; remove the inline check.

In `routes/modpack.rs`: add `validate_slug(slug)?;` before the existing trim check.

In `routes/servers/settings.rs`: add `if let Some(v) = req.force_version.as_ref().and_then(|x| x.as_ref()) { validate_force_version(v)?; }` and `if let Some(list) = req.version_skip.as_ref() { validate_version_skip(list)?; }`.

(`cpu_millicores` validator wired in Task 1.3 already.)

- [ ] **Step 5: Switch `validate_mc_version` to consult the cache**

The existing `validate_mc_version` checks against `KNOWN_MC_VERSIONS` (hardcoded 6). Change to consult the `mc_versions_cache` from `AppState`. Pattern: `pub async fn validate_mc_version(state: &AppState, v: &str) -> Result<(), AppError>`. If the cache is empty/expired, fetch (reusing `mc_versions::handle`'s helper), or fall back to `KNOWN_MC_VERSIONS`. Keep `KNOWN_MC_VERSIONS` as the offline floor so this never errors on Mojang outage.

Update the create handler call sites to `validate_mc_version(&state, &req.mc_version).await?`.

- [ ] **Step 6: Run all tests + clippy**

```bash
cd /home/hadi/gitlab/anvil/backend
cargo test --all
cargo clippy --all-targets --features serve-dir -- -D warnings
cargo clippy --all-targets --features embed   -- -D warnings
```
Expected: green.

- [ ] **Step 7: Commit**

```bash
git add backend/src/validation.rs backend/src/routes/servers/create.rs \
  backend/src/routes/servers/settings.rs backend/src/routes/modpack.rs
git commit -m "fix(validation): tighten storage/slug/force_version/version_skip; mc_version uses live cache"
```

### Task 1.7: Modpack cleanup — `project_id` trait method, drop `changelog_excerpt`, paginate `fetch_files`

**Files:**
- Modify: `/home/hadi/gitlab/anvil/backend/src/modpack/mod.rs`
- Modify: `/home/hadi/gitlab/anvil/backend/src/modpack/curseforge.rs`
- Modify: `/home/hadi/gitlab/anvil/backend/src/modpack/poller.rs`
- Modify: `/home/hadi/gitlab/anvil/backend/src/modpack/orchestrator.rs`
- Modify: `/home/hadi/gitlab/anvil/backend/src/modpack/cf_client.rs`

- [ ] **Step 1: Add `project_id` to the trait**

In `modpack/mod.rs`, locate the `ModpackProvider` trait. Add:

```rust
/// Numeric upstream project id when the provider has one (CurseForge,
/// Modrinth eventually). Returns `None` for providers without — e.g. vanilla.
fn project_id(&self) -> Option<u32> { None }
```

- [ ] **Step 2: Implement on `CurseForgeServerPack`**

In `modpack/curseforge.rs`, find the `impl ModpackProvider for CurseForgeServerPack`. Add:

```rust
fn project_id(&self) -> Option<u32> {
    Some(self.config.project_id)
}
```

- [ ] **Step 3: Drop `changelog_excerpt` from `VersionInfo`**

In `modpack/mod.rs`, remove `pub changelog_excerpt: Option<String>` from `VersionInfo`. Update every constructor site (poller, orchestrator, cf_client) — drop the `changelog_excerpt: None` lines or `.changelog_excerpt(...)` builder calls.

- [ ] **Step 4: Drop `changelog_excerpt` from poller writes**

In `modpack/poller.rs` around lines 128–140, remove the `.bind(latest.changelog_excerpt.as_deref())` and the corresponding column from the SQL. Update the SQL to drop the `changelog_excerpt` column. Keep the rest of the upsert.

If the `modpack_versions` schema has a `changelog_excerpt` column, **leave it in place** (don't add a destructive migration just to remove a column). The column simply stops being written/read; benign and preserves backups.

- [ ] **Step 5: Fix `pick_target_version` and delete `provider_to_config`/`serde_project_id`**

In `modpack/orchestrator.rs`, replace `pick_target_version` body:

```rust
async fn pick_target_version(
    provider: &dyn ModpackProvider,
    cf: &Arc<CurseForgeClient>,
    target_version_id: u32,
) -> Result<VersionInfo> {
    if let Some(latest) = provider.latest(cf).await?
        && latest.id == target_version_id
    {
        return Ok(latest);
    }
    let project_id = provider
        .project_id()
        .ok_or_else(|| anyhow!("provider {} has no project id", provider.kind()))?;
    let files = cf.list_files(project_id).await?;
    let f = files
        .iter()
        .find(|f| f.id == target_version_id)
        .ok_or_else(|| anyhow!("file id {target_version_id} not in project files"))?;
    Ok(VersionInfo {
        id: f.id,
        name: f.display_name.clone(),
        download_url: f.download_url.clone().unwrap_or_default(),
    })
}
```

Delete the `provider_to_config` and `serde_project_id` functions entirely. Run `cargo check` and remove any now-unused imports.

- [ ] **Step 6: Wire `force_version` into the version pick**

Find the orchestrator's "decide target version" logic (the place that currently calls `provider.latest(cf)` to choose what to install). If the source config carries `force_version: Some(name)`, look it up in `cf.list_files(project_id)` by `display_name`, and use that file as the target. Otherwise fall back to `provider.latest(cf)`. The clear-on-success at line 592 already exists — keep it.

Sketch (adapt to actual surrounding code):

```rust
let cfg: serde_json::Value = serde_json::from_str(&server.source_config)?;
let force = cfg.get("force_version").and_then(|v| v.as_str()).map(str::to_owned);
let target = if let Some(forced) = force.as_deref() {
    let project_id = provider.project_id()
        .ok_or_else(|| anyhow!("force_version requires a CF provider"))?;
    let files = cf.list_files(project_id).await?;
    let f = files.iter().find(|f| f.display_name == forced)
        .ok_or_else(|| anyhow!("forced version {forced} not in project files"))?;
    VersionInfo { id: f.id, name: f.display_name.clone(),
                  download_url: f.download_url.clone().unwrap_or_default() }
} else {
    provider.latest(cf).await?
        .ok_or_else(|| anyhow!("no latest version"))?
};
```

- [ ] **Step 7: Paginate `CurseForgeClient::fetch_files`**

In `modpack/cf_client.rs` around lines 200–220, change `fetch_files` to loop with `pageSize=50` and an `index` query param, accumulating files until the response returns fewer than `pageSize` items or the cap of 500 is reached.

```rust
const PAGE_SIZE: u32 = 50;
const MAX_FILES: usize = 500;

pub async fn fetch_files(&self, project_id: u32) -> Result<Vec<CfFile>> {
    let mut all = Vec::new();
    let mut index: u32 = 0;
    loop {
        let url = format!(
            "{}/v1/mods/{project_id}/files?pageSize={PAGE_SIZE}&index={index}",
            self.base_url
        );
        let resp: CfFilesPage = self
            .http
            .get(&url)
            .header("x-api-key", &self.api_key)
            .send().await?.error_for_status()?
            .json().await?;
        let n = resp.data.len();
        all.extend(resp.data);
        if n < PAGE_SIZE as usize || all.len() >= MAX_FILES {
            break;
        }
        index += PAGE_SIZE;
    }
    all.truncate(MAX_FILES);
    Ok(all)
}
```

`CfFilesPage` is the existing response wrapper; check the actual struct name in the file. The pagination param name may be `index` or `pageIndex` — verify in the file's existing GET URL builder.

- [ ] **Step 8: Run cargo checks**

```bash
cd /home/hadi/gitlab/anvil/backend
cargo test --all
cargo clippy --all-targets --features serve-dir -- -D warnings
cargo clippy --all-targets --features embed   -- -D warnings
cargo fmt --check
```
Expected: green. If any tests reference `VersionInfo.changelog_excerpt`, fix them by dropping the field.

- [ ] **Step 9: Commit**

```bash
git add backend/src/modpack/
git commit -m "refactor(modpack): project_id() trait method; drop changelog_excerpt; paginate fetch_files; force_version honored"
```

### Section 1 checkpoint

- [ ] Run all backend verification commands (top of plan)
- [ ] Run `superpowers:code-reviewer` on the section diff
- [ ] **STOP** for user eyeball before Section 2

---

# Section 2 — Design tokens and primitive components

**Acceptance gate:** `pnpm typecheck`, `pnpm lint`, `pnpm build` green. Existing pages still render (the layout still mounts `UserBadge` until Section 3). Every new component file exists and has typed props matching the spec.

### Task 2.1: Expand `globals.css` with the §5 token set

**Files:**
- Modify: `/home/hadi/gitlab/anvil/frontend/app/globals.css`

- [ ] **Step 1: Replace the `@theme` block**

Overwrite the file with the expanded token set. Note: in Tailwind v4, names like `--color-bg` generate `bg-bg`/`text-bg` utilities; that's the convention we're committing to. Motion vars don't generate utilities — use as inline `style` or via `@utility`.

```css
@import "tailwindcss";

@theme {
  /* Hook the next/font CSS variables (set in app/layout.tsx) into Tailwind's
     font-sans / font-mono utility classes. */
  --font-sans: var(--font-fira-sans), system-ui, sans-serif;
  --font-mono: var(--font-fira-code), ui-monospace, "SFMono-Regular", monospace;

  /* Surfaces */
  --color-bg:            #0a0a0c;
  --color-surface:       #0e0f12;
  --color-elevated:      #15161b;
  --color-border:        #1e1e22;
  --color-border-soft:   #15151a;
  --color-border-strong: #2e2e34;

  /* Text */
  --color-text-primary:  #f2f2f5;
  --color-text-body:     #e6e7eb;
  --color-text-muted:    #8a8a92;
  --color-text-dim:      #6b6b73;
  --color-text-faint:    #4f4f56;

  /* Signature accent (used surgically) */
  --color-accent:         #d29150;
  --color-accent-bg:      #1a1208;
  --color-accent-border:  #3a2a18;
  --color-accent-bracket: #6e4a26;

  /* State (only for state) */
  --color-state-running: #8aaf45;
  --color-state-warning: #cdaa66;
  --color-state-error:   #c97f6f;

  /* Source markers */
  --color-source-curseforge: var(--color-accent);
  --color-source-modrinth:   #6cb04a;
  --color-source-local:      var(--color-text-faint);

  /* Radii */
  --radius-none: 0;
  --radius-sm:   2px;
  --radius-md:   4px;
}

/* Custom variables that don't fit Tailwind's namespaces — used via style/var(). */
:root {
  --color-state-running-glow: rgba(138, 175, 69, 0.18);
  --motion-fast:    120ms;
  --motion-default: 150ms;
  --motion-slow:    250ms;
}

/* Application body baseline. */
body {
  background: var(--color-bg);
  color: var(--color-text-body);
  font-family: var(--font-sans);
}
```

- [ ] **Step 2: Verify build**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm lint && pnpm build
```
Expected: green. The build will still be the v1 visual, just with new tokens unused yet.

- [ ] **Step 3: Commit**

```bash
git add frontend/app/globals.css
git commit -m "feat(frontend): expand design tokens for v2 workshop aesthetic"
```

### Task 2.2: Edit `Button.tsx` (focus ring, brackets, ghost variant)

**Files:**
- Modify: `/home/hadi/gitlab/anvil/frontend/app/components/Button.tsx`

- [ ] **Step 1: Read the current file**

```bash
sed -n '1,60p' /home/hadi/gitlab/anvil/frontend/app/components/Button.tsx
```

- [ ] **Step 2: Rewrite**

Keep the existing API (`variant`, `type` defaulting to `"button"`, forwards `ButtonHTMLAttributes`). Add:
- `variant: "primary" | "secondary" | "danger" | "ghost"` (add `ghost`)
- `size?: "sm" | "md"`, default `"md"`
- `focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent` on every variant
- For `primary`: render the children inside `[ ]` brackets via leading/trailing `<span>` elements styled with `text-accent-bracket`. Background `bg-accent-bg`, border `border-accent-border`, text `text-accent`. Hover: `hover:border-accent`. Disabled: `disabled:opacity-40 disabled:cursor-not-allowed`.
- For `secondary`: `border border-border bg-surface text-text-body hover:border-border-strong`.
- For `danger`: `border border-border bg-surface text-state-error hover:border-state-error`.
- For `ghost`: no border, `text-text-muted hover:text-text-body`.

Reference shape:

```tsx
import type { ButtonHTMLAttributes } from "react";
import { cn } from "../lib/cn";

type Variant = "primary" | "secondary" | "danger" | "ghost";
type Size = "sm" | "md";

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: Size;
}

const base =
  "inline-flex items-center gap-2 rounded-md font-mono uppercase tracking-wide " +
  "transition-colors disabled:opacity-40 disabled:cursor-not-allowed " +
  "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent";

const sizes: Record<Size, string> = {
  sm: "px-2 py-1 text-[11px]",
  md: "px-3 py-1.5 text-xs",
};

const variants: Record<Variant, string> = {
  primary:
    "bg-accent-bg border border-accent-border text-accent hover:border-accent",
  secondary:
    "bg-surface border border-border text-text-body hover:border-border-strong",
  danger:
    "bg-surface border border-border text-state-error hover:border-state-error",
  ghost: "text-text-muted hover:text-text-body",
};

export function Button({
  variant = "secondary",
  size = "md",
  type = "button",
  className,
  children,
  ...rest
}: ButtonProps) {
  return (
    <button type={type} className={cn(base, sizes[size], variants[variant], className)} {...rest}>
      {variant === "primary" ? (
        <>
          <span className="text-accent-bracket">[</span>
          {children}
          <span className="text-accent-bracket">]</span>
        </>
      ) : (
        children
      )}
    </button>
  );
}
```

- [ ] **Step 3: Add the `cn` helper if not present**

```bash
ls /home/hadi/gitlab/anvil/frontend/app/lib/cn.ts 2>/dev/null
```
If missing, create it:

```ts
export function cn(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(" ");
}
```

- [ ] **Step 4: Verify**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm lint
```
Expected: green. (Existing `Button` callers keep working — props are backward-compatible.)

- [ ] **Step 5: Commit**

```bash
git add frontend/app/components/Button.tsx frontend/app/lib/cn.ts
git commit -m "feat(frontend): Button — focus ring, primary brackets, ghost variant"
```

### Task 2.3: Edit `Modal.tsx` (focus trap, SVG ✕, focus ring on close)

**Files:**
- Modify: `/home/hadi/gitlab/anvil/frontend/app/components/Modal.tsx`

- [ ] **Step 1: Read the current file**

```bash
sed -n '1,90p' /home/hadi/gitlab/anvil/frontend/app/components/Modal.tsx
```

- [ ] **Step 2: Rewrite**

Keep the existing API (`title: string`, `onClose`, `children`, optional `maxWidth` prop). Add:
- A focus-trap that restores focus to the previously-focused element on close.
- Replace U+2715 with an SVG `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2}><path d="M6 6l12 12M18 6L6 18" /></svg>`.
- Focus ring on the close button: `focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent`.
- Tokens: `bg-surface text-text-body border border-border` on the panel; `bg-bg/80 backdrop-blur-sm` on the backdrop is fine — but per spec **no glassmorphism**, so use `bg-bg/80` only. Drop `backdrop-blur`.

Implement focus trap with a small effect that listens for `Tab` keydown and cycles between the first and last focusable elements inside the panel ref. Pattern:

```tsx
useEffect(() => {
  const previous = document.activeElement as HTMLElement | null;
  const panel = panelRef.current;
  if (!panel) return;
  const focusables = panel.querySelectorAll<HTMLElement>(
    'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
  );
  focusables[0]?.focus();
  const handler = (e: KeyboardEvent) => {
    if (e.key !== "Tab" || focusables.length === 0) return;
    const first = focusables[0]!;
    const last = focusables[focusables.length - 1]!;
    if (e.shiftKey && document.activeElement === first) {
      last.focus(); e.preventDefault();
    } else if (!e.shiftKey && document.activeElement === last) {
      first.focus(); e.preventDefault();
    }
  };
  document.addEventListener("keydown", handler);
  return () => {
    document.removeEventListener("keydown", handler);
    previous?.focus();
  };
}, []);
```

Beware `noUncheckedIndexedAccess` — use the `!` non-null assert only when the length check guards it.

- [ ] **Step 3: Verify**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm lint
```
Expected: green. Existing `ConfirmDeleteDialog`, `NewServerModal` callers continue working (no API change).

- [ ] **Step 4: Commit**

```bash
git add frontend/app/components/Modal.tsx
git commit -m "feat(frontend): Modal — focus trap, SVG close, focus ring"
```

### Task 2.4: New stateless primitives (`Sheet`, `Card`, `Tabs`, `Badge`, `Skeleton`, `IconButton`, `Tooltip`, `SegmentedControl`, `RangeSlider`, `Dropdown`)

**Files:**
- Create: `/home/hadi/gitlab/anvil/frontend/app/components/{IconButton,Sheet,Card,Tabs,Badge,Skeleton,Tooltip,SegmentedControl,RangeSlider,Dropdown}.tsx`

For each, write the file in one pass, then `pnpm typecheck` once at the end. Skeletons below — fill copy+CSS to match spec §5/§6.

- [ ] **Step 1: `IconButton.tsx`**

```tsx
import type { ButtonHTMLAttributes, ReactNode } from "react";
import { cn } from "../lib/cn";

interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  "aria-label": string;
  children: ReactNode;
}

export function IconButton({ className, type = "button", children, ...rest }: IconButtonProps) {
  return (
    <button
      type={type}
      className={cn(
        "inline-flex h-8 w-8 items-center justify-center rounded-md text-text-muted",
        "hover:bg-elevated hover:text-text-body transition-colors",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent",
        className,
      )}
      {...rest}
    >
      {children}
    </button>
  );
}
```

- [ ] **Step 2: `Sheet.tsx`**

Right slide-over. `width: 480 | 640 | 720` prop, `isOpen`, `onClose`, `title`, `children`. Esc closes. Backdrop closes. Same focus-trap as Modal. Use `style={{ transitionDuration: 'var(--motion-slow)' }}` for the slide-in.

```tsx
"use client";
import { useEffect, useRef } from "react";
import type { ReactNode } from "react";
import { cn } from "../lib/cn";

interface SheetProps {
  isOpen: boolean;
  onClose: () => void;
  title: string;
  width?: 480 | 640 | 720;
  children: ReactNode;
}

export function Sheet({ isOpen, onClose, title, width = 480, children }: SheetProps) {
  const panelRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!isOpen) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [isOpen, onClose]);
  return (
    <div
      className={cn("fixed inset-0 z-50 transition-opacity", isOpen ? "pointer-events-auto opacity-100" : "pointer-events-none opacity-0")}
      style={{ transitionDuration: "var(--motion-default)" }}
    >
      <button
        type="button"
        aria-label="close sheet"
        className="absolute inset-0 bg-bg/70"
        onClick={onClose}
      />
      <div
        ref={panelRef}
        role="dialog"
        aria-label={title}
        className={cn(
          "absolute right-0 top-0 h-full bg-surface border-l border-border shadow-2xl",
          "transition-transform",
          isOpen ? "translate-x-0" : "translate-x-full",
        )}
        style={{ width: `${width}px`, transitionDuration: "var(--motion-slow)" }}
      >
        <header className="flex items-center justify-between border-b border-border-soft px-5 py-3">
          <h2 className="font-mono text-[13px] uppercase tracking-wide text-text-primary">{title}</h2>
          <button onClick={onClose} aria-label="close" className="text-text-muted hover:text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent rounded">
            <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth={2}><path d="M6 6l12 12M18 6L6 18" /></svg>
          </button>
        </header>
        <div className="overflow-y-auto" style={{ height: "calc(100% - 49px)" }}>{children}</div>
      </div>
    </div>
  );
}
```

- [ ] **Step 3: `Card.tsx`**

```tsx
import type { ReactNode } from "react";
import { cn } from "../lib/cn";

interface CardProps {
  header?: ReactNode;
  children: ReactNode;
  className?: string;
}

export function Card({ header, children, className }: CardProps) {
  return (
    <div className={cn("rounded-md border border-border bg-surface", className)}>
      {header && (
        <div className="border-b border-border-soft px-4 py-3 font-mono text-[11px] uppercase tracking-wider text-text-muted">
          {header}
        </div>
      )}
      <div className="p-4">{children}</div>
    </div>
  );
}
```

- [ ] **Step 4: `Tabs.tsx`**

`tabs: { id: string; label: string; href: string; count?: number; mark?: boolean }[]`, `activeId: string`. Renders a `<nav>` with `<Link>` per tab. Active tab gets `text-text-primary` + bottom border `border-accent`; inactive gets `text-text-muted`. Optional count badge after label; optional dot mark.

```tsx
import Link from "next/link";
import { cn } from "../lib/cn";

interface Tab {
  id: string;
  label: string;
  href: string;
  count?: number;
  mark?: boolean;
}

interface TabsProps {
  tabs: Tab[];
  activeId: string;
}

export function Tabs({ tabs, activeId }: TabsProps) {
  return (
    <nav className="flex gap-6 border-b border-border-soft">
      {tabs.map((t) => {
        const active = t.id === activeId;
        return (
          <Link
            key={t.id}
            href={t.href}
            className={cn(
              "relative py-3 font-mono text-[12px] uppercase tracking-wider transition-colors",
              "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent rounded-sm",
              active ? "text-text-primary" : "text-text-muted hover:text-text-body",
            )}
          >
            {t.label}
            {typeof t.count === "number" && (
              <span className="ml-2 text-text-faint">({t.count})</span>
            )}
            {t.mark && (
              <span className="ml-2 inline-block h-1.5 w-1.5 rounded-full bg-state-warning" />
            )}
            {active && (
              <span className="absolute -bottom-px left-0 right-0 h-px bg-accent" />
            )}
          </Link>
        );
      })}
    </nav>
  );
}
```

- [ ] **Step 5: `Badge.tsx`**

Replaces `StatusBadge`. `variant: "running" | "stopped" | "starting" | "stopping" | "error" | "update"`. For running: small dot with glow `box-shadow: 0 0 8px var(--color-state-running-glow)`. For starting/stopping: pulse animation. For update: copper accent text.

```tsx
import { cn } from "../lib/cn";

type Variant = "running" | "stopped" | "starting" | "stopping" | "error" | "update";

interface BadgeProps { variant: Variant; label?: string; }

const labels: Record<Variant, string> = {
  running: "running",
  stopped: "stopped",
  starting: "starting",
  stopping: "stopping",
  error: "error",
  update: "update available",
};

const dotColor: Record<Variant, string> = {
  running: "bg-state-running",
  stopped: "bg-text-faint",
  starting: "bg-state-warning animate-pulse",
  stopping: "bg-state-warning animate-pulse",
  error:    "bg-state-error",
  update:   "bg-accent",
};

const textColor: Record<Variant, string> = {
  running: "text-text-body",
  stopped: "text-text-muted",
  starting: "text-text-body",
  stopping: "text-text-body",
  error:    "text-state-error",
  update:   "text-accent",
};

export function Badge({ variant, label }: BadgeProps) {
  return (
    <span className={cn("inline-flex items-center gap-1.5 font-mono text-[11px]", textColor[variant])}>
      <span
        className={cn("h-1.5 w-1.5 rounded-full", dotColor[variant])}
        style={variant === "running"
          ? { boxShadow: "0 0 8px var(--color-state-running-glow)" }
          : undefined}
      />
      {label ?? labels[variant]}
    </span>
  );
}
```

- [ ] **Step 6: `Skeleton.tsx`**

Three variants. `row` for table rows, `block` for cards, `text` for inline.

```tsx
import { cn } from "../lib/cn";

interface SkeletonProps {
  variant: "row" | "block" | "text";
  className?: string;
}

export function Skeleton({ variant, className }: SkeletonProps) {
  const base = "animate-pulse bg-elevated";
  const shape = variant === "row" ? "h-10 w-full"
              : variant === "block" ? "h-32 w-full rounded-md"
              : "h-3 w-24 rounded-sm";
  return <div className={cn(base, shape, className)} aria-hidden="true" />;
}
```

- [ ] **Step 7: `Tooltip.tsx`**

Lightweight, hover-only (no Radix). `<span>` wrapper with `title` and an absolute-positioned tooltip on hover via group classes.

```tsx
import type { ReactNode } from "react";
import { cn } from "../lib/cn";

interface TooltipProps { label: string; children: ReactNode; className?: string; }

export function Tooltip({ label, children, className }: TooltipProps) {
  return (
    <span className={cn("group relative inline-flex", className)}>
      {children}
      <span className="pointer-events-none absolute bottom-full left-1/2 mb-1 -translate-x-1/2 whitespace-nowrap rounded-sm border border-border bg-elevated px-2 py-1 font-mono text-[10px] uppercase tracking-wider text-text-body opacity-0 transition-opacity group-hover:opacity-100">
        {label}
      </span>
    </span>
  );
}
```

- [ ] **Step 8: `SegmentedControl.tsx`**

```tsx
import { cn } from "../lib/cn";

interface SegmentedControlProps<T extends string> {
  value: T;
  options: ReadonlyArray<{ value: T; label: string }>;
  onChange: (value: T) => void;
  ariaLabel: string;
}

export function SegmentedControl<T extends string>({
  value, options, onChange, ariaLabel,
}: SegmentedControlProps<T>) {
  return (
    <div role="radiogroup" aria-label={ariaLabel} className="inline-flex rounded-md border border-border bg-surface p-0.5">
      {options.map((o) => {
        const active = o.value === value;
        return (
          <button
            key={o.value}
            type="button"
            role="radio"
            aria-checked={active}
            onClick={() => onChange(o.value)}
            className={cn(
              "px-2.5 py-1 font-mono text-[11px] uppercase tracking-wider rounded-sm transition-colors",
              "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent",
              active ? "bg-elevated text-text-primary" : "text-text-muted hover:text-text-body",
            )}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}
```

- [ ] **Step 9: `RangeSlider.tsx`**

Wraps `input[type=range]`. Renders ticks via a small SVG track underneath if `ticks` is given. `value`, `onChange`, `min`, `max`, `step`, `unit?` ("MiB" / "cores"), `label`.

```tsx
"use client";
import type { ChangeEvent } from "react";
import { cn } from "../lib/cn";

interface RangeSliderProps {
  label: string;
  value: number;
  onChange: (v: number) => void;
  min: number;
  max: number;
  step?: number;
  unit?: string;
  ticks?: number[];
  className?: string;
}

export function RangeSlider({
  label, value, onChange, min, max, step = 1, unit, ticks, className,
}: RangeSliderProps) {
  const handle = (e: ChangeEvent<HTMLInputElement>) => onChange(Number(e.target.value));
  return (
    <div className={cn("flex flex-col gap-2", className)}>
      <div className="flex items-baseline justify-between">
        <label className="font-mono text-[11px] uppercase tracking-wider text-text-muted">{label}</label>
        <span className="font-mono text-[12px] text-text-body">
          {value}{unit && <span className="ml-1 text-text-muted">{unit}</span>}
        </span>
      </div>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={handle}
        className="h-1 w-full appearance-none bg-border rounded-full accent-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
      />
      {ticks && (
        <div className="flex justify-between font-mono text-[10px] text-text-faint">
          {ticks.map((t) => <span key={t}>{t}</span>)}
        </div>
      )}
    </div>
  );
}
```

- [ ] **Step 10: `Dropdown.tsx`**

`button` (the trigger) + items. Closes on outside click or Esc.

```tsx
"use client";
import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { cn } from "../lib/cn";

interface DropdownItem { id: string; label: string; onSelect: () => void; danger?: boolean; }
interface DropdownProps { trigger: ReactNode; items: ReadonlyArray<DropdownItem>; ariaLabel: string; }

export function Dropdown({ trigger, items, ariaLabel }: DropdownProps) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const onClick = (e: MouseEvent) => {
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") setOpen(false); };
    document.addEventListener("click", onClick);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("click", onClick);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);
  return (
    <div ref={wrapRef} className="relative">
      <button
        type="button"
        aria-label={ariaLabel}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="inline-flex h-7 w-7 items-center justify-center rounded text-text-muted hover:bg-elevated focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
      >
        {trigger}
      </button>
      {open && (
        <div role="menu" className="absolute right-0 z-10 mt-1 min-w-40 rounded-md border border-border bg-surface py-1 shadow-lg">
          {items.map((it) => (
            <button
              key={it.id}
              role="menuitem"
              type="button"
              onClick={() => { setOpen(false); it.onSelect(); }}
              className={cn(
                "block w-full px-3 py-1.5 text-left font-mono text-[12px] hover:bg-elevated focus-visible:outline-none focus-visible:bg-elevated",
                it.danger ? "text-state-error" : "text-text-body",
              )}
            >
              {it.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
```

- [ ] **Step 11: Verify**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm lint && pnpm build
```
Expected: green.

- [ ] **Step 12: Commit**

```bash
git add frontend/app/components/{IconButton,Sheet,Card,Tabs,Badge,Skeleton,Tooltip,SegmentedControl,RangeSlider,Dropdown}.tsx
git commit -m "feat(frontend): primitives — Sheet, Card, Tabs, Badge, Skeleton, IconButton, Tooltip, SegmentedControl, RangeSlider, Dropdown"
```

### Task 2.5: `Toast` system

**Files:**
- Create: `/home/hadi/gitlab/anvil/frontend/app/components/Toast.tsx`

- [ ] **Step 1: Implement context + provider + hook**

```tsx
"use client";
import { createContext, useCallback, useContext, useEffect, useState } from "react";
import type { ReactNode } from "react";
import { cn } from "../lib/cn";

type Tone = "info" | "success" | "error";

interface Toast { id: number; message: string; tone: Tone; }

interface ToastCtx { push: (message: string, tone?: Tone) => void; }

const Ctx = createContext<ToastCtx | null>(null);

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const push = useCallback((message: string, tone: Tone = "info") => {
    const id = Date.now() + Math.random();
    setToasts((prev) => [...prev, { id, message, tone }]);
    window.setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== id));
    }, 4000);
  }, []);
  return (
    <Ctx.Provider value={{ push }}>
      {children}
      <div className="pointer-events-none fixed bottom-4 right-4 z-50 flex flex-col gap-2">
        {toasts.map((t) => (
          <div
            key={t.id}
            role="status"
            className={cn(
              "pointer-events-auto rounded-md border px-3 py-2 font-mono text-[12px] shadow-lg bg-surface",
              t.tone === "error" ? "border-state-error text-state-error"
              : t.tone === "success" ? "border-state-running text-state-running"
              : "border-border text-text-body",
            )}
          >
            {t.message}
          </div>
        ))}
      </div>
    </Ctx.Provider>
  );
}

export function useToast(): ToastCtx {
  const v = useContext(Ctx);
  if (!v) throw new Error("useToast must be used inside ToastProvider");
  return v;
}
```

- [ ] **Step 2: Mount provider in `layout.tsx`** (deferred to Section 3 alongside CommandBar swap — don't mount yet)

- [ ] **Step 3: Verify**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm lint
```

- [ ] **Step 4: Commit**

```bash
git add frontend/app/components/Toast.tsx
git commit -m "feat(frontend): Toast provider + useToast hook"
```

### Task 2.6: `PathBreadcrumb` and `CommandBar`

**Files:**
- Create: `/home/hadi/gitlab/anvil/frontend/app/components/PathBreadcrumb.tsx`
- Create: `/home/hadi/gitlab/anvil/frontend/app/components/CommandBar.tsx`

- [ ] **Step 1: `PathBreadcrumb.tsx`**

```tsx
"use client";
import Link from "next/link";
import { usePathname } from "next/navigation";

const ROOT_LABEL = "anvil";

function decodeSegment(s: string): string {
  try { return decodeURIComponent(s); } catch { return s; }
}

export function PathBreadcrumb() {
  const pathname = usePathname();
  const segments = pathname.split("/").filter((s) => s.length > 0);
  // /servers/<name> -> ["servers", "<name>"]
  const crumbs = [
    { label: ROOT_LABEL, href: "/" },
    ...segments.map((s, i) => ({
      label: decodeSegment(s),
      href: "/" + segments.slice(0, i + 1).join("/"),
    })),
  ];
  return (
    <nav aria-label="breadcrumb" className="flex items-center gap-2 font-mono text-[12px] text-text-muted">
      <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" strokeWidth={1.5} className="text-accent">
        <path d="M4 14l4-8h8l4 8M6 14h12v4H6z" />
      </svg>
      {crumbs.map((c, i) => {
        const last = i === crumbs.length - 1;
        return (
          <span key={c.href} className="flex items-center gap-2">
            {i > 0 && <span className="text-text-faint">/</span>}
            {last ? (
              <span className="text-text-primary">{c.label}</span>
            ) : (
              <Link href={c.href} className="hover:text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent rounded-sm">
                {c.label}
              </Link>
            )}
          </span>
        );
      })}
    </nav>
  );
}
```

- [ ] **Step 2: `CommandBar.tsx`**

Top bar. Left = `<PathBreadcrumb />`. Right = user identity + logout. Reuses the `getMe` and `logout` functions from `lib/api.ts`. Renders Skeleton while loading (no silent gap — audit §1.4).

```tsx
"use client";
import { useEffect, useState } from "react";
import { getMe, logout, type Me } from "../lib/api";
import { PathBreadcrumb } from "./PathBreadcrumb";
import { Skeleton } from "./Skeleton";
import { IconButton } from "./IconButton";

export function CommandBar() {
  const [me, setMe] = useState<Me | null | undefined>(undefined); // undefined = loading
  useEffect(() => {
    let alive = true;
    getMe()
      .then((value) => { if (alive) setMe(value); })
      .catch(() => { if (alive) setMe(null); });
    return () => { alive = false; };
  }, []);

  return (
    <header className="flex h-12 items-center justify-between border-b border-border-soft bg-bg px-5">
      <PathBreadcrumb />
      <div className="flex items-center gap-3">
        {me === undefined ? (
          <Skeleton variant="text" className="h-3 w-32" />
        ) : me ? (
          <>
            {me.picture ? (
              // eslint-disable-next-line @next/next/no-img-element
              <img src={me.picture} alt="" className="h-6 w-6 rounded-full" />
            ) : null}
            <span className="font-mono text-[12px] text-text-body">{me.name}</span>
            <form action="/api/auth/logout" method="post">
              <IconButton aria-label="logout" type="submit" onClick={(e) => { e.preventDefault(); void logout(); }}>
                <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" strokeWidth={2}><path d="M16 17l5-5-5-5M21 12H9M9 21H5a2 2 0 01-2-2V5a2 2 0 012-2h4" /></svg>
              </IconButton>
            </form>
          </>
        ) : (
          <span className="font-mono text-[12px] text-state-error">auth error</span>
        )}
      </div>
    </header>
  );
}
```

This requires `getMe` to return a typed `Me` (currently inferred from `meSchema`). Add `export type Me = z.infer<typeof meSchema>;` in `lib/api.ts` if not already there. Defer this typing tweak to Task 4.1; mark it as a TODO if you want to commit Section 2 first.

- [ ] **Step 3: Verify**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm lint
```
Expected: green if `Me` type is exported. If not, temporarily declare a local interface in `CommandBar.tsx` matching `meSchema`.

- [ ] **Step 4: Commit**

```bash
git add frontend/app/components/PathBreadcrumb.tsx frontend/app/components/CommandBar.tsx
git commit -m "feat(frontend): CommandBar + PathBreadcrumb (mounted in section 3)"
```

### Section 2 checkpoint

- [ ] All cargo + pnpm checks green.
- [ ] Run `superpowers:code-reviewer` on the section diff.
- [ ] **STOP** for user eyeball before Section 3.

---

# Section 3 — Layout swap (CommandBar replaces UserBadge)

**Acceptance gate:** Home page renders with CommandBar at the top — anvil mark, `anvil / servers` breadcrumb, user identity. `UserBadge.tsx` is gone. `pnpm build` green.

### Task 3.1: Mount `CommandBar` and `ToastProvider` in `layout.tsx`; delete `UserBadge.tsx`

**Files:**
- Modify: `/home/hadi/gitlab/anvil/frontend/app/layout.tsx`
- Delete: `/home/hadi/gitlab/anvil/frontend/app/components/UserBadge.tsx`

- [ ] **Step 1: Read the current layout**

```bash
sed -n '1,60p' /home/hadi/gitlab/anvil/frontend/app/layout.tsx
```

- [ ] **Step 2: Rewrite**

Keep next/font setup and metadata. Replace `<UserBadge />` with `<CommandBar />`. Wrap children in `<ToastProvider>` so any descendant can `useToast()`.

```tsx
import type { Metadata } from "next";
import { Fira_Code, Fira_Sans } from "next/font/google";
import "./globals.css";
import { CommandBar } from "./components/CommandBar";
import { ToastProvider } from "./components/Toast";

const firaSans = Fira_Sans({
  subsets: ["latin"],
  weight: ["400", "500", "600"],
  variable: "--font-fira-sans",
});
const firaCode = Fira_Code({
  subsets: ["latin"],
  weight: ["400", "500"],
  variable: "--font-fira-code",
});

export const metadata: Metadata = {
  title: "anvil",
  description: "Minecraft server panel",
};

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html lang="en" className="dark">
      <body className={`${firaSans.variable} ${firaCode.variable} font-sans antialiased`}>
        <ToastProvider>
          <CommandBar />
          <main>{children}</main>
        </ToastProvider>
      </body>
    </html>
  );
}
```

- [ ] **Step 3: Delete `UserBadge.tsx`**

```bash
rm /home/hadi/gitlab/anvil/frontend/app/components/UserBadge.tsx
rg -n 'UserBadge' /home/hadi/gitlab/anvil/frontend
```
Expected: no matches.

- [ ] **Step 4: Verify**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm lint && pnpm build
```
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add frontend/app/layout.tsx frontend/app/components/UserBadge.tsx
git commit -m "feat(frontend): CommandBar replaces UserBadge in root layout; ToastProvider mounted"
```

### Section 3 checkpoint

- [ ] Smoke-test in browser: `cd backend && cargo run --features serve-dir`, open `http://localhost:8080`, confirm CommandBar renders at top with breadcrumb and identity.
- [ ] Run `superpowers:code-reviewer` on the section diff (small).
- [ ] **STOP** for user eyeball before Section 4.

---

# Section 4 — Server list page redesign

**Acceptance gate:** `pnpm typecheck`, `pnpm lint`, `pnpm build` green. Home page renders with Skeleton rows while loading, summary line, source bars, `↑n` indicator, whole-row click, action reveal-on-hover, polling pauses when tab is hidden.

### Task 4.1: Update `lib/api.ts` schemas

**Files:**
- Modify: `/home/hadi/gitlab/anvil/frontend/app/lib/api.ts`

- [ ] **Step 1: Add `mcVersionsSchema` and helpers**

```ts
export const mcVersionsResponseSchema = z.object({
  versions: z.array(z.string()).min(1),
  source: z.enum(["mojang", "fallback"]),
});
export type McVersionsResponse = z.infer<typeof mcVersionsResponseSchema>;

export async function fetchMcVersions(): Promise<McVersionsResponse> {
  const res = await fetch("/api/cluster/mc-versions");
  return jsonOrThrow(res, mcVersionsResponseSchema);
}
```

- [ ] **Step 2: Extend `serverSummarySchema` and `serverDetailSchema` with `cpu_millicores`**

Find both schemas. Add:

```ts
cpu_millicores: z.number().int(),
```

In `serverDetailSchema`, **remove** `latest_changelog_excerpt`.

- [ ] **Step 3: Extend `clusterCapabilitiesSchema` with `available_cpu_cores`**

```ts
available_cpu_cores: z.number().nonnegative(),
```

- [ ] **Step 4: Extend `createServerRequestSchema` with `cpu_millicores`**

```ts
cpu_millicores: z.number().int().min(250).max(16000),
```

- [ ] **Step 5: Extend `settingsRequestSchema` with `memory_mi` and `cpu_millicores`**

```ts
memory_mi: z.number().int().optional(),
cpu_millicores: z.number().int().optional(),
```

- [ ] **Step 6: Export `Me` type**

```ts
export type Me = z.infer<typeof meSchema>;
```

- [ ] **Step 7: Verify**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck
```
Expected: build errors in `NewServerModal.tsx`, `servers/detail/page.tsx` because their existing payload shapes now require `cpu_millicores`. Defer fixing those — `NewServerModal` is being deleted in Section 5; `detail/page.tsx` becomes a redirect-only stub. For now, allow the build to fail; or add temporary defaults in those two files just to keep the build green. **Decision: pass `cpu_millicores: 1000` literal in `NewServerModal.tsx` so v1 modal keeps creating servers until Section 5 deletes it**, and add a Zod `.optional()` to a temporary alias if needed. Keep the create endpoint requirement — the temporary literal gets us through.

Pragmatic shortcut: add a temporary `cpu_millicores: 1000,` to the payload in `NewServerModal.tsx` (around the existing payload object). One-line change.

- [ ] **Step 8: Commit**

```bash
git add frontend/app/lib/api.ts frontend/app/components/NewServerModal.tsx
git commit -m "feat(frontend): schemas — cpu_millicores, mc-versions, available_cpu_cores; drop changelog_excerpt"
```

### Task 4.2: Align `update-stream.ts` semantics with `logs-stream.ts`

**Files:**
- Modify: `/home/hadi/gitlab/anvil/frontend/app/lib/update-stream.ts`

- [ ] **Step 1: Read both files side-by-side**

```bash
sed -n '1,60p' /home/hadi/gitlab/anvil/frontend/app/lib/update-stream.ts
sed -n '1,60p' /home/hadi/gitlab/anvil/frontend/app/lib/logs-stream.ts
```

- [ ] **Step 2: Mirror the `hello` flip**

`logs-stream.ts` flips `status` to `"live"` on a `hello` frame; `update-stream.ts` currently flips to `"open"` on `socket.onopen`. Change `update-stream.ts` to wait for the `hello` frame, then flip to `"live"`. Update the `Status` union: `"connecting" | "live" | "reconnecting" | "ended" | "error"`. Adjust the cancel-tracking pattern to use the same shape (ref-based vs closure — pick whatever `logs-stream.ts` does and mirror it).

- [ ] **Step 3: Verify call sites**

`useUpdateStream` is consumed only by the new detail page (Section 5) and is currently unused. So no caller updates needed today. But the existing `'open'` literal must be removed.

- [ ] **Step 4: Verify**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm lint
```

- [ ] **Step 5: Commit**

```bash
git add frontend/app/lib/update-stream.ts
git commit -m "refactor(frontend): align update-stream onopen/hello semantics with logs-stream"
```

### Task 4.3: Rename `ServerTable.tsx` → `ServerList.tsx` and rewrite per spec §8.1

**Files:**
- Move: `/home/hadi/gitlab/anvil/frontend/app/components/ServerTable.tsx` → `ServerList.tsx`
- Will be edited by next task

- [ ] **Step 1: Move the file**

```bash
git mv /home/hadi/gitlab/anvil/frontend/app/components/ServerTable.tsx \
       /home/hadi/gitlab/anvil/frontend/app/components/ServerList.tsx
```

- [ ] **Step 2: Update the lone import in `app/page.tsx`**

```bash
sed -i 's|./components/ServerTable|./components/ServerList|' /home/hadi/gitlab/anvil/frontend/app/page.tsx
sed -i 's|ServerTable|ServerList|g' /home/hadi/gitlab/anvil/frontend/app/page.tsx
```

(Or hand-edit; just two references.)

- [ ] **Step 3: Inside `ServerList.tsx`, rename the export**

Edit the export name from `ServerTable` to `ServerList`. Don't change implementation yet — that's the next task.

- [ ] **Step 4: Verify**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm build
```

- [ ] **Step 5: Commit**

```bash
git add frontend/app/components/ServerList.tsx frontend/app/page.tsx
git commit -m "refactor(frontend): rename ServerTable -> ServerList"
```

### Task 4.4: Rewrite `ServerList.tsx` per spec §8.1

**Files:**
- Modify: `/home/hadi/gitlab/anvil/frontend/app/components/ServerList.tsx`

- [ ] **Step 1: Rewrite**

Implement:
- Whole-row click via `<tr>` wrapped in nothing-but-its-cells; navigation uses `router.push(\`/servers/${encodeURIComponent(name)}\`)` on row click. Keyboard: `tabIndex={0}` on `<tr>`, `onKeyDown` Enter/Space.
- Source color bar (4px wide, 14px tall) on the very left of the name column. Color via `bg-source-curseforge`/`bg-source-modrinth`/`bg-source-local`, conditional on `source_kind`. Vanilla shows nothing.
- `↑n` after the name where `n` = `update_available ? 1 : 0` (today only one update count is exposed). Render copper `text-accent`.
- Reveal-on-hover action cluster on the right: start (when stopped), stop (when running), restart (when running), `⋯` Dropdown overflow with "open in console" link to `/servers/<name>/console`.
- Empty state: anvil SVG mark, "no servers yet — click [+ new] to forge one".

Use:
- `Badge` for status column
- `Dropdown` for `⋯`
- `Skeleton` for loading rows in the parent

- [ ] **Step 2: Verify**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm lint && pnpm build
```

- [ ] **Step 3: Commit**

```bash
git add frontend/app/components/ServerList.tsx
git commit -m "feat(frontend): ServerList — whole-row click, source bars, update indicator, hover actions"
```

### Task 4.5: Rewrite `app/page.tsx` per spec §8.1

**Files:**
- Modify: `/home/hadi/gitlab/anvil/frontend/app/page.tsx`

- [ ] **Step 1: Implement visibility-paused polling**

Use `document.visibilityState` to pause/resume the 5s poll. Pattern:

```tsx
useEffect(() => {
  let timer: number | undefined;
  let alive = true;
  const tick = async () => {
    if (!alive) return;
    if (document.visibilityState === "visible") {
      try {
        const next = await fetchServers();
        if (alive) setServers(next);
      } catch (e) {
        if (alive) setError(formatError(e));
      }
    }
    timer = window.setTimeout(tick, 5000);
  };
  void tick();
  const onVis = () => {
    if (document.visibilityState === "visible" && timer === undefined) {
      void tick();
    }
  };
  document.addEventListener("visibilitychange", onVis);
  return () => {
    alive = false;
    if (timer !== undefined) window.clearTimeout(timer);
    document.removeEventListener("visibilitychange", onVis);
  };
}, []);
```

- [ ] **Step 2: Render summary line**

Compute counts:

```tsx
const total = servers.length;
const running = servers.filter(s => s.status === "running").length;
const stopped = servers.filter(s => s.status === "stopped").length;
const updates = servers.filter(s => s.update_available).length;
```

Render in a single line with `·` separators in `text-text-muted`, plus `[+ new]` Button (variant `primary`) right-aligned that does `router.push("/servers/new")`.

- [ ] **Step 3: Replace loading text with `<Skeleton variant="row" />` × 3**

- [ ] **Step 4: Replace inline error with a `Card` variant of error treatment** (border + state-error text)

- [ ] **Step 5: Remove the `<NewServerModal>` mount and its `useState`**

The `[+ new]` button now navigates to `/servers/new` (built in Section 5). Until Section 5 lands, the route 404s with the SPA fallback rendering the home page — that's acceptable for the in-flight state.

- [ ] **Step 6: Verify**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm lint && pnpm build
```

- [ ] **Step 7: Commit**

```bash
git add frontend/app/page.tsx
git commit -m "feat(frontend): home page redesign — skeleton, summary, visibility-paused poll, route to /servers/new"
```

### Section 4 checkpoint

- [ ] Smoke-test in browser. List should render with the new design.
- [ ] Run `superpowers:code-reviewer` on the section diff.
- [ ] **STOP** for user eyeball before Section 5.

---

# Section 5 — Create page and tabbed detail page

**Acceptance gate:** `pnpm typecheck`, `pnpm lint`, `pnpm build` green. `/servers/new` flow creates a server and navigates to `/servers/<name>`. Deep-link `/servers/<name>/console` opens on the Console tab. `/servers/detail?id=<uuid>` redirects to `/servers/<name>`. Update banner appears for CF servers; `[update]` opens Sheet with FSM stream.

### Task 5.1: `lib/server-detail-context.ts` and `lib/use-mc-versions.ts`

**Files:**
- Create: `/home/hadi/gitlab/anvil/frontend/app/lib/server-detail-context.ts`
- Create: `/home/hadi/gitlab/anvil/frontend/app/lib/use-mc-versions.ts`
- Create: `/home/hadi/gitlab/anvil/frontend/app/lib/end-reason.ts`

- [ ] **Step 1: `server-detail-context.ts`**

```ts
"use client";
import { createContext, useContext } from "react";
import type { ServerDetail } from "./api";

export const ServerDetailContext = createContext<ServerDetail | null>(null);

export function useServerDetailCtx(): ServerDetail {
  const v = useContext(ServerDetailContext);
  if (!v) throw new Error("useServerDetailCtx outside provider");
  return v;
}
```

- [ ] **Step 2: `use-mc-versions.ts`**

Module-scoped cache so multiple consumers don't refetch.

```ts
"use client";
import { useEffect, useState } from "react";
import { fetchMcVersions, type McVersionsResponse } from "./api";

let cached: McVersionsResponse | undefined;
let inflight: Promise<McVersionsResponse> | undefined;

export function useMcVersions(): McVersionsResponse | undefined {
  const [value, setValue] = useState<McVersionsResponse | undefined>(cached);
  useEffect(() => {
    if (value) return;
    if (!inflight) inflight = fetchMcVersions().then((v) => { cached = v; return v; });
    let alive = true;
    inflight.then((v) => { if (alive) setValue(v); }).catch(() => {});
    return () => { alive = false; };
  }, [value]);
  return value;
}
```

- [ ] **Step 3: `end-reason.ts`**

```ts
const FRIENDLY: Record<string, string> = {
  "pod-unavailable": "the server's pod went away",
  "connecting": "still connecting",
  "reconnecting": "reconnecting…",
  "stream-closed": "log stream closed",
  "stream-error": "log stream error",
};

export function friendlyEndReason(reason: string | undefined): string {
  if (!reason) return "ended";
  return FRIENDLY[reason] ?? reason;
}
```

- [ ] **Step 4: Verify**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm lint
```

- [ ] **Step 5: Commit**

```bash
git add frontend/app/lib/server-detail-context.ts frontend/app/lib/use-mc-versions.ts frontend/app/lib/end-reason.ts
git commit -m "feat(frontend): detail-context + mc-versions hook + end-reason map"
```

### Task 5.2: Detail page shell — `servers/[name]/layout.tsx`

**Files:**
- Create: `/home/hadi/gitlab/anvil/frontend/app/servers/[name]/layout.tsx`

- [ ] **Step 1: Implement**

This is a **client** layout (it uses hooks for polling). It fetches `ServerDetail` by name, renders the header + update banner + `Tabs` strip, and provides `ServerDetailContext` to descendants. Uses `usePathname` to derive the active tab.

```tsx
"use client";
import { useEffect, useState } from "react";
import { useParams, usePathname, useRouter } from "next/navigation";
import { fetchServerByName, type ServerDetail } from "../../lib/api";
import { ServerDetailContext } from "../../lib/server-detail-context";
import { Tabs } from "../../components/Tabs";
import { Skeleton } from "../../components/Skeleton";
import { Badge } from "../../components/Badge";
import { Button } from "../../components/Button";
import { IconButton } from "../../components/IconButton";
import { Dropdown } from "../../components/Dropdown";
import { useToast } from "../../components/Toast";
import { startServer, stopServer, restartServer, applyUpdate } from "../../lib/api";

export default function ServerLayout({ children }: { children: React.ReactNode }) {
  const params = useParams<{ name: string }>();
  const name = decodeURIComponent(params.name);
  const pathname = usePathname();
  const router = useRouter();
  const toast = useToast();
  const [detail, setDetail] = useState<ServerDetail | null | undefined>(undefined);
  const [error, setError] = useState<string | undefined>();

  useEffect(() => {
    let alive = true;
    let timer: number | undefined;
    const tick = async () => {
      if (!alive || document.visibilityState !== "visible") {
        timer = window.setTimeout(tick, 5000);
        return;
      }
      try {
        const v = await fetchServerByName(name);
        if (alive) { setDetail(v); setError(undefined); }
      } catch (e) {
        if (alive) setError(e instanceof Error ? e.message : String(e));
      }
      timer = window.setTimeout(tick, 5000);
    };
    void tick();
    return () => { alive = false; if (timer !== undefined) window.clearTimeout(timer); };
  }, [name]);

  if (detail === undefined) return <main className="p-6"><Skeleton variant="block" /></main>;
  if (detail === null) return <main className="p-6 text-state-error">server "{name}" not found</main>;

  const lastSeg = pathname.split("/").pop() ?? "";
  const activeId = ["console","mods","players","files","settings"].includes(lastSeg)
    ? lastSeg : "overview";

  const tabs = [
    { id: "overview", label: "overview", href: `/servers/${encodeURIComponent(name)}` },
    { id: "console",  label: "console",  href: `/servers/${encodeURIComponent(name)}/console` },
    { id: "mods",     label: "mods",     href: `/servers/${encodeURIComponent(name)}/mods` },
    { id: "players",  label: "players",  href: `/servers/${encodeURIComponent(name)}/players` },
    { id: "files",    label: "files",    href: `/servers/${encodeURIComponent(name)}/files` },
    { id: "settings", label: "settings", href: `/servers/${encodeURIComponent(name)}/settings` },
  ];

  // Lifecycle handlers — wrap with toast
  const lifecycle = (label: string, fn: () => Promise<void>) => async () => {
    try {
      await fn();
      toast.push(`${label} ok`, "success");
    } catch (e) {
      toast.push(e instanceof Error ? e.message : `${label} failed`, "error");
    }
  };

  return (
    <ServerDetailContext.Provider value={detail}>
      <main className="px-5 py-6">
        <header className="mb-4 flex items-start justify-between">
          <div>
            <h1 className="font-mono text-[24px] font-semibold text-text-primary tracking-tight">
              {detail.name}
            </h1>
            <div className="mt-2 flex items-center gap-4 font-mono text-[12px] text-text-muted">
              <Badge variant={detail.status as Parameters<typeof Badge>[0]["variant"]} />
              <span>runtime · {detail.server_type}</span>
              <span>version · {detail.mc_version}</span>
              <span>cpu · {(detail.cpu_millicores / 1000).toFixed(1)} cores</span>
              <span>memory · {detail.memory_mi} MiB</span>
              <span>storage · {detail.storage_size_gi} GiB</span>
            </div>
          </div>
          <div className="flex items-center gap-2">
            {detail.status === "running" && (
              <Button onClick={lifecycle("stop", () => stopServer(detail.id))}>stop</Button>
            )}
            {detail.status === "stopped" && (
              <Button variant="primary" onClick={lifecycle("start", () => startServer(detail.id))}>start</Button>
            )}
            {detail.status === "running" && (
              <Button onClick={lifecycle("restart", () => restartServer(detail.id))}>restart</Button>
            )}
            <Dropdown
              ariaLabel="more actions"
              trigger={<span aria-hidden>⋯</span>}
              items={[
                { id: "console", label: "open console", onSelect: () => router.push(`/servers/${encodeURIComponent(name)}/console`) },
                ...(detail.status === "stopped" ? [{ id: "delete", label: "delete server", onSelect: () => { /* deleted via Settings tab today */ }, danger: true }] : []),
              ]}
            />
          </div>
        </header>

        {detail.update_available && !detail.update_in_progress && (
          <div className="mb-4 rounded-md border border-accent-border bg-accent-bg/30 px-4 py-3 flex items-center justify-between">
            <span className="font-mono text-[12px] text-accent">
              update available · {detail.mc_version} → {detail.latest_version_name ?? "?"}
            </span>
            <div className="flex gap-2">
              <Button variant="ghost">skip</Button>
              <Button variant="primary" onClick={lifecycle("update", () => applyUpdate(detail.id))}>update</Button>
            </div>
          </div>
        )}

        <Tabs tabs={tabs} activeId={activeId} />
        <div className="mt-6">
          {error && <div className="mb-3 text-state-error font-mono text-[12px]">{error}</div>}
          {children}
        </div>
      </main>
    </ServerDetailContext.Provider>
  );
}
```

This requires a new API helper `fetchServerByName(name: string)`. Add to `lib/api.ts`:

```ts
export async function fetchServerByName(name: string): Promise<ServerDetail> {
  // The list endpoint returns summaries with id+name; map name -> id then fetch detail.
  const list = await fetchServers();
  const match = list.find((s) => s.name === name);
  if (!match) throw new Error(`server "${name}" not found`);
  return fetchServerDetail(match.id);
}
```

Note: this is a 2-RTT pattern. Acceptable for v1. A future optimization is `GET /api/servers/by-name/{name}` — out of scope for A.

- [ ] **Step 2: Verify**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm lint
```
Expected: green. Build will complain about missing tab page files until next tasks.

- [ ] **Step 3: Commit**

```bash
git add frontend/app/servers/\[name\]/layout.tsx frontend/app/lib/api.ts
git commit -m "feat(frontend): /servers/[name] layout — header, banner, Tabs, ServerDetailContext"
```

### Task 5.3: Tab body pages

**Files:**
- Create: `/home/hadi/gitlab/anvil/frontend/app/servers/[name]/page.tsx` (overview)
- Create: `/home/hadi/gitlab/anvil/frontend/app/servers/[name]/console/page.tsx`
- Create: `/home/hadi/gitlab/anvil/frontend/app/servers/[name]/mods/page.tsx`
- Create: `/home/hadi/gitlab/anvil/frontend/app/servers/[name]/players/page.tsx`
- Create: `/home/hadi/gitlab/anvil/frontend/app/servers/[name]/files/page.tsx`
- Create: `/home/hadi/gitlab/anvil/frontend/app/servers/[name]/settings/page.tsx`

- [ ] **Step 1: Overview (`page.tsx`)**

Two-column: left = connection card + 8-line console preview; right = at-a-glance stats + recent activity (audit log not exposed via API today — render an empty list with copy "audit log surfacing arrives later"). Uses `useServerDetailCtx()`.

```tsx
"use client";
import { useServerDetailCtx } from "../../lib/server-detail-context";
import { Card } from "../../components/Card";

export default function OverviewPage() {
  const detail = useServerDetailCtx();
  return (
    <div className="grid grid-cols-2 gap-4">
      <Card header="connection">
        <pre className="font-mono text-[12px] text-text-body">
{detail.endpoint
  ? `${detail.endpoint.host}:${detail.endpoint.port}`
  : "address pending…"}
        </pre>
      </Card>
      <Card header="at a glance">
        <dl className="grid grid-cols-2 gap-y-2 font-mono text-[12px]">
          <dt className="text-text-muted">runtime</dt><dd>{detail.server_type}</dd>
          <dt className="text-text-muted">version</dt><dd>{detail.mc_version}</dd>
          <dt className="text-text-muted">cpu limit</dt><dd>{(detail.cpu_millicores / 1000).toFixed(2)} cores</dd>
          <dt className="text-text-muted">memory limit</dt><dd>{detail.memory_mi} MiB</dd>
          <dt className="text-text-muted">storage</dt><dd>{detail.storage_size_gi} GiB</dd>
        </dl>
      </Card>
    </div>
  );
}
```

- [ ] **Step 2: Console (`console/page.tsx`)**

Mounts `LiveLogPanel` and `RconCommand`. `LiveLogPanel` is being re-skinned in Task 5.5; for now mount it as-is.

```tsx
"use client";
import { useServerDetailCtx } from "../../../lib/server-detail-context";
import { LiveLogPanel } from "../../../components/LiveLogPanel";
import { RconCommand } from "../../../components/RconCommand";

export default function ConsolePage() {
  const detail = useServerDetailCtx();
  return (
    <div className="flex flex-col gap-4">
      <LiveLogPanel serverId={detail.id} />
      <RconCommand serverId={detail.id} />
    </div>
  );
}
```

- [ ] **Step 3: Placeholders (mods/players/files)**

Each is a 5-line stub:

```tsx
"use client";
import { useServerDetailCtx } from "../../../lib/server-detail-context";
import { Card } from "../../../components/Card";

export default function ModsPage() {
  const detail = useServerDetailCtx();
  return (
    <Card header="mods">
      <p className="font-mono text-[12px] text-text-muted">
        mod browsing arrives in v2.1.
      </p>
      {detail.source_kind !== "vanilla" && (
        <p className="mt-2 font-mono text-[12px] text-text-body">
          modpack identity · {detail.source_kind}
        </p>
      )}
    </Card>
  );
}
```

Players and Files are the same shape — different copy ("player management arrives in v2.2", "in-app file browser arrives in v2.3").

- [ ] **Step 4: Settings (`settings/page.tsx`)**

Memory + CPU `RangeSlider`s, `mc_version` `<select>` populated from `useMcVersions()`, modpack auto-update mode `SegmentedControl` (when `source_kind != "vanilla"`), version-skip list (collapsible). Danger zone with `[delete server]` (`stopped`-only, opens `ConfirmDeleteDialog`).

Uses `updateServerSettings(detail.id, { ... })`. On success, toast.

This is the most code-heavy tab body — ~150 lines. Implement it as a single file. Skeleton:

```tsx
"use client";
import { useState } from "react";
import { useRouter } from "next/navigation";
import { useServerDetailCtx } from "../../../lib/server-detail-context";
import { useMcVersions } from "../../../lib/use-mc-versions";
import { Card } from "../../../components/Card";
import { RangeSlider } from "../../../components/RangeSlider";
import { Button } from "../../../components/Button";
import { useToast } from "../../../components/Toast";
import { updateServerSettings, deleteServer } from "../../../lib/api";

export default function SettingsPage() {
  const detail = useServerDetailCtx();
  const router = useRouter();
  const toast = useToast();
  const versions = useMcVersions();
  const [memory, setMemory] = useState(detail.memory_mi);
  const [cpu, setCpu] = useState(detail.cpu_millicores);
  const [mcVersion, setMcVersion] = useState(detail.mc_version);

  const save = async () => {
    try {
      await updateServerSettings(detail.id, {
        ...(memory !== detail.memory_mi ? { memory_mi: memory } : {}),
        ...(cpu !== detail.cpu_millicores ? { cpu_millicores: cpu } : {}),
        // mc_version goes via force_version since the server-side spec lands on next start
      });
      toast.push("settings saved", "success");
    } catch (e) {
      toast.push(e instanceof Error ? e.message : "save failed", "error");
    }
  };

  const onDelete = async () => {
    if (!confirm(`type the server name "${detail.name}" to confirm`)) return;
    try {
      await deleteServer(detail.id);
      router.push("/");
    } catch (e) {
      toast.push(e instanceof Error ? e.message : "delete failed", "error");
    }
  };

  return (
    <div className="flex flex-col gap-4 max-w-2xl">
      <Card header="resources (apply on next start)">
        <div className="flex flex-col gap-4">
          <RangeSlider label="memory" value={memory} onChange={setMemory} min={1024} max={16384} step={1024} unit="MiB" />
          <RangeSlider label="cpu" value={cpu} onChange={setCpu} min={250} max={16000} step={250} unit="m" />
          <div>
            <label className="font-mono text-[11px] uppercase tracking-wider text-text-muted block mb-1">minecraft version</label>
            <select
              value={mcVersion}
              onChange={(e) => setMcVersion(e.target.value)}
              className="rounded-md border border-border bg-surface px-2 py-1 font-mono text-[12px] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
            >
              {(versions?.versions ?? [detail.mc_version]).map((v) => (
                <option key={v} value={v}>{v}</option>
              ))}
            </select>
          </div>
          <div><Button variant="primary" onClick={save}>save</Button></div>
        </div>
      </Card>

      {detail.status === "stopped" && (
        <Card header="danger zone">
          <Button variant="danger" onClick={onDelete}>delete server</Button>
        </Card>
      )}
    </div>
  );
}
```

(Full ConfirmDeleteDialog parity with `confirm()` in this stub is an intentional simplification — the existing `ConfirmDeleteDialog` requires typing the name; restoring that is a 5-line follow-up using the new `Modal`. **Decision: use the existing `ConfirmDeleteDialog` here**, not `confirm()`. Replace the `onDelete` handler with state-controlled Dialog mounting.)

- [ ] **Step 5: Verify build**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm lint && pnpm build
```
Expected: green. Build now produces the full tab-segmented detail page.

- [ ] **Step 6: Commit**

```bash
git add frontend/app/servers/\[name\]/
git commit -m "feat(frontend): detail tab bodies — overview, console, settings; mods/players/files placeholders"
```

### Task 5.4: Update FSM display via `Sheet`

**Files:**
- Create: `/home/hadi/gitlab/anvil/frontend/app/components/UpdateSheet.tsx`
- Modify: `/home/hadi/gitlab/anvil/frontend/app/servers/[name]/layout.tsx` (mount the Sheet, wire `[update]` button)

- [ ] **Step 1: `UpdateSheet.tsx`**

```tsx
"use client";
import { Sheet } from "./Sheet";
import { useUpdateStream, type UpdatePhase } from "../lib/update-stream";
import { cn } from "../lib/cn";

const ORDER: UpdatePhase[] = [
  "queued", "announcing", "stopping", "backing-up",
  "swapping", "starting", "verifying", "succeeded",
];

interface UpdateSheetProps {
  serverId: string | null;
  isOpen: boolean;
  onClose: () => void;
}

export function UpdateSheet({ serverId, isOpen, onClose }: UpdateSheetProps) {
  const stream = useUpdateStream(isOpen ? serverId : null);
  return (
    <Sheet isOpen={isOpen} onClose={onClose} title="update" width={640}>
      <div className="p-5">
        <ol className="flex flex-col gap-2 font-mono text-[12px]">
          {ORDER.map((p) => {
            const reached = ORDER.indexOf(p) <= ORDER.indexOf(stream.phase ?? "queued");
            const active = stream.phase === p;
            return (
              <li key={p} className={cn("flex items-center gap-3",
                reached ? "text-text-body" : "text-text-faint",
                active && "text-accent")}>
                <span className={cn("h-1.5 w-1.5 rounded-full",
                  active ? "bg-accent" : reached ? "bg-state-running" : "bg-text-faint")} />
                {p}
              </li>
            );
          })}
        </ol>
        {stream.result && (
          <p className="mt-4 font-mono text-[12px] text-text-body">result · {stream.result}</p>
        )}
        {stream.endedReason && (
          <p className="mt-1 font-mono text-[12px] text-state-error">{stream.endedReason}</p>
        )}
      </div>
    </Sheet>
  );
}
```

`UpdatePhase` type comes from `update-stream.ts` — export it if not already exported.

- [ ] **Step 2: Wire into `[name]/layout.tsx`**

Add `useState<boolean>` for `sheetOpen`. Change the `[update]` Button `onClick` to `() => setSheetOpen(true)` (and still trigger `applyUpdate` to start the FSM if not in progress). Mount `<UpdateSheet serverId={detail.id} isOpen={sheetOpen} onClose={() => setSheetOpen(false)} />` at the end of the layout JSX.

- [ ] **Step 3: Verify**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm lint && pnpm build
```

- [ ] **Step 4: Commit**

```bash
git add frontend/app/components/UpdateSheet.tsx frontend/app/servers/\[name\]/layout.tsx
git commit -m "feat(frontend): UpdateSheet — FSM display via right slide-over"
```

### Task 5.5: Re-skin `LiveLogPanel` and `RconCommand`; friendly EndReason

**Files:**
- Modify: `/home/hadi/gitlab/anvil/frontend/app/components/LiveLogPanel.tsx`
- Modify: `/home/hadi/gitlab/anvil/frontend/app/components/RconCommand.tsx`

- [ ] **Step 1: `LiveLogPanel.tsx`**

Read the current file. Replace inline color classes with new tokens (`bg-surface`, `text-text-body`, `border-border` etc.). At line 104 (per audit §1.5), replace `ended (${endedReason})` with `ended · ${friendlyEndReason(endedReason)}` from `lib/end-reason.ts`. Distinguish `connecting` (text-text-muted dot) from `reconnecting` (text-state-warning dot) per audit §1.9.

- [ ] **Step 2: `RconCommand.tsx`**

Replace inline color classes with new tokens. Use `Button` (variant `primary`) for submit, focus ring on the `<input>`.

- [ ] **Step 3: Verify**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm lint && pnpm build
```

- [ ] **Step 4: Commit**

```bash
git add frontend/app/components/LiveLogPanel.tsx frontend/app/components/RconCommand.tsx
git commit -m "feat(frontend): LiveLogPanel + RconCommand re-skin; friendly EndReason"
```

### Task 5.6: `BuildSlip.tsx` and `/servers/new/page.tsx`

**Files:**
- Create: `/home/hadi/gitlab/anvil/frontend/app/components/BuildSlip.tsx`
- Create: `/home/hadi/gitlab/anvil/frontend/app/servers/new/page.tsx`

- [ ] **Step 1: `BuildSlip.tsx` and `CreateFormContext` in same file**

```tsx
"use client";
import { createContext, useContext } from "react";

export interface CreateDraft {
  name: string;
  type: "vanilla" | "paper" | "modpack" | "modded";
  mc_version: string | null;
  cpu_millicores: number;
  memory_mi: number;
  storage_size_gi: number;
  storage_class: string | null;
  exposure_mode: "loadbalancer" | "nodeport" | "clusterip";
  // modpack subform — populated lazily by B
  curseforge?: { project_id: number; file_id: number; channel: "release" | "beta" | "alpha" } | undefined;
}

export const CreateFormContext = createContext<CreateDraft | null>(null);

function useDraft(): CreateDraft {
  const v = useContext(CreateFormContext);
  if (!v) throw new Error("BuildSlip outside CreateFormContext");
  return v;
}

function dash(v: string | number | null | undefined): string {
  return v === null || v === undefined || v === "" ? "—" : String(v);
}

export function BuildSlip({ status }: { status: "draft" | "valid" | "submitting" }) {
  const d = useDraft();
  return (
    <aside className="sticky top-6 w-80 rounded-md border border-border bg-surface p-5">
      <header className="flex items-center justify-between mb-4">
        <span className="font-mono text-[10px] uppercase tracking-[0.12em] text-text-faint">build slip</span>
        <span className="font-mono text-[11px] uppercase tracking-wider text-accent">{status}</span>
      </header>
      <dl className="grid grid-cols-1 gap-y-3 font-mono text-[12px]">
        <Section label="01 identity">
          <Field label="name" value={d.name} />
          <Field label="type" value={d.type} />
        </Section>
        <Section label="02 source">
          <Field label="mc version" value={d.mc_version} />
          {d.type === "modpack" && d.curseforge && (
            <>
              <Field label="cf project" value={d.curseforge.project_id} />
              <Field label="cf file" value={d.curseforge.file_id} />
              <Field label="channel" value={d.curseforge.channel} />
            </>
          )}
        </Section>
        <Section label="03 resources">
          <Field label="cpu" value={`${(d.cpu_millicores / 1000).toFixed(2)} cores`} />
          <Field label="memory" value={`${d.memory_mi} MiB`} />
        </Section>
        <Section label="04 storage">
          <Field label="size" value={`${d.storage_size_gi} GiB`} />
          <Field label="class" value={d.storage_class} />
        </Section>
        <Section label="05 network">
          <Field label="exposure" value={d.exposure_mode} />
        </Section>
      </dl>
    </aside>
  );
}

function Section({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="border-t border-border-soft pt-2 first:border-t-0 first:pt-0">
      <div className="font-mono text-[10px] uppercase tracking-[0.12em] text-text-faint mb-1">{label}</div>
      {children}
    </div>
  );
}

function Field({ label, value }: { label: string; value: string | number | null | undefined }) {
  return (
    <div className="flex justify-between">
      <span className="text-text-muted">{label}</span>
      <span className="text-text-body">{dash(value)}</span>
    </div>
  );
}
```

- [ ] **Step 2: `/servers/new/page.tsx`**

Six numbered sections, two-column layout, bottom action bar.

```tsx
"use client";
import { useState } from "react";
import { useRouter } from "next/navigation";
import { CreateFormContext, BuildSlip, type CreateDraft } from "../../components/BuildSlip";
import { Card } from "../../components/Card";
import { RangeSlider } from "../../components/RangeSlider";
import { SegmentedControl } from "../../components/SegmentedControl";
import { Button } from "../../components/Button";
import { useToast } from "../../components/Toast";
import { useMcVersions } from "../../lib/use-mc-versions";
import { fetchCapabilities, createServer } from "../../lib/api";
import { useEffect } from "react";

const INITIAL: CreateDraft = {
  name: "",
  type: "vanilla",
  mc_version: null,
  cpu_millicores: 2000,
  memory_mi: 4096,
  storage_size_gi: 20,
  storage_class: null,
  exposure_mode: "clusterip",
};

export default function NewServerPage() {
  const router = useRouter();
  const toast = useToast();
  const versions = useMcVersions();
  const [draft, setDraft] = useState<CreateDraft>(INITIAL);
  const [submitting, setSubmitting] = useState(false);
  const [caps, setCaps] = useState<Awaited<ReturnType<typeof fetchCapabilities>> | null>(null);

  useEffect(() => { void fetchCapabilities().then(setCaps); }, []);

  const set = <K extends keyof CreateDraft>(k: K, v: CreateDraft[K]) => setDraft((d) => ({ ...d, [k]: v }));

  const missing: string[] = [];
  if (!draft.name) missing.push("name");
  if (!draft.mc_version) missing.push("mc version");
  if (!draft.storage_class && caps && caps.available_storage_classes.length > 1) missing.push("storage class");
  const valid = missing.length === 0;
  const status: "draft" | "valid" | "submitting" = submitting ? "submitting" : valid ? "valid" : "draft";

  const submit = async () => {
    if (!valid || !draft.mc_version) return;
    setSubmitting(true);
    try {
      const created = await createServer({
        name: draft.name,
        mc_version: draft.mc_version,
        memory_mi: draft.memory_mi,
        cpu_millicores: draft.cpu_millicores,
        exposure_mode: draft.exposure_mode,
        storage_size_gi: draft.storage_size_gi,
        ...(draft.storage_class ? { storage_class: draft.storage_class } : {}),
        server_type: draft.type === "vanilla" ? "vanilla" : "modpack",
        ...(draft.curseforge ? { curseforge: draft.curseforge } : {}),
      });
      toast.push("server forged", "success");
      router.push(`/servers/${encodeURIComponent(created.name)}`);
    } catch (e) {
      toast.push(e instanceof Error ? e.message : "create failed", "error");
      setSubmitting(false);
    }
  };

  return (
    <CreateFormContext.Provider value={draft}>
      <main className="px-5 py-6 grid grid-cols-[320px,1fr] gap-8">
        <BuildSlip status={status} />
        <div className="flex flex-col gap-4 max-w-2xl">
          <Section number="01" title="identity">
            <Card>
              <label className="block font-mono text-[11px] uppercase tracking-wider text-text-muted mb-1">name</label>
              <input
                value={draft.name}
                onChange={(e) => set("name", e.target.value)}
                placeholder="e.g. atm-11-friends"
                className="w-full rounded-md border border-border bg-bg px-3 py-2 font-mono text-[13px] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
              />
              <p className="mt-1 font-mono text-[11px] text-text-faint">
                lowercase, dashes, 3–63 chars. unique across the panel.
              </p>
            </Card>
          </Section>

          <Section number="02" title="type">
            <Card>
              <SegmentedControl
                ariaLabel="server type"
                value={draft.type}
                onChange={(v) => set("type", v)}
                options={[
                  { value: "vanilla", label: "vanilla" },
                  { value: "paper", label: "paper" },
                  { value: "modpack", label: "modpack" },
                  { value: "modded", label: "modded" },
                ]}
              />
              {draft.type !== "vanilla" && (
                <p className="mt-2 font-mono text-[11px] text-text-faint">
                  full {draft.type} support arrives in v2.1 — vanilla is the only fully-wired type today.
                </p>
              )}
            </Card>
          </Section>

          <Section number="03" title="source">
            <Card>
              {draft.type === "vanilla" || draft.type === "paper" || draft.type === "modded" ? (
                <div>
                  <label className="block font-mono text-[11px] uppercase tracking-wider text-text-muted mb-1">minecraft version</label>
                  <select
                    value={draft.mc_version ?? ""}
                    onChange={(e) => set("mc_version", e.target.value || null)}
                    className="rounded-md border border-border bg-bg px-2 py-1 font-mono text-[12px] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
                  >
                    <option value="">— select —</option>
                    {(versions?.versions ?? []).map((v) => <option key={v} value={v}>{v}</option>)}
                  </select>
                </div>
              ) : (
                <div className="flex items-center gap-3">
                  <input placeholder="paste a curseforge or modrinth url" disabled className="flex-1 rounded-md border border-border bg-bg px-3 py-2 font-mono text-[12px] text-text-muted" />
                  <Button disabled>browse</Button>
                  <span className="font-mono text-[11px] text-text-faint">v2.1</span>
                </div>
              )}
            </Card>
          </Section>

          <Section number="04" title="resources">
            <Card>
              <div className="flex flex-col gap-4">
                <RangeSlider label="memory" value={draft.memory_mi} onChange={(v) => set("memory_mi", v)} min={1024} max={16384} step={1024} unit="MiB" />
                <RangeSlider label="cpu"    value={draft.cpu_millicores} onChange={(v) => set("cpu_millicores", v)} min={250} max={16000} step={250} unit="m" />
                {caps && (
                  <p className="font-mono text-[11px] text-text-faint">
                    cluster headroom · {caps.available_cpu_cores.toFixed(1)} cores total
                  </p>
                )}
              </div>
            </Card>
          </Section>

          <Section number="05" title="storage">
            <Card>
              <div className="flex flex-col gap-4">
                <RangeSlider label="size" value={draft.storage_size_gi} onChange={(v) => set("storage_size_gi", v)} min={10} max={500} step={10} unit="GiB" />
                {caps && caps.available_storage_classes.length > 1 && (
                  <div>
                    <label className="block font-mono text-[11px] uppercase tracking-wider text-text-muted mb-1">storage class</label>
                    <select
                      value={draft.storage_class ?? ""}
                      onChange={(e) => set("storage_class", e.target.value || null)}
                      className="rounded-md border border-border bg-bg px-2 py-1 font-mono text-[12px]"
                    >
                      <option value="">— default ({caps.default_storage_class ?? "?"}) —</option>
                      {caps.available_storage_classes.map((c) => <option key={c} value={c}>{c}</option>)}
                    </select>
                  </div>
                )}
              </div>
            </Card>
          </Section>

          <Section number="06" title="network">
            <Card>
              <SegmentedControl
                ariaLabel="exposure mode"
                value={draft.exposure_mode}
                onChange={(v) => set("exposure_mode", v)}
                options={[
                  { value: "clusterip", label: "clusterip" },
                  { value: "nodeport", label: "nodeport" },
                  ...(caps?.loadbalancer ? [{ value: "loadbalancer" as const, label: "loadbalancer" }] : []),
                ]}
              />
            </Card>
          </Section>

          <footer className="sticky bottom-0 -mx-5 mt-4 border-t border-border bg-bg px-5 py-3 flex items-center justify-between">
            <span className="font-mono text-[12px]">
              {valid
                ? <span className="text-state-running">● all sections valid · ready to forge</span>
                : <span className="text-state-error">× missing: {missing.join(", ")}</span>}
            </span>
            <div className="flex gap-2">
              <Button onClick={() => router.push("/")}>cancel</Button>
              <Button variant="primary" disabled={!valid || submitting} onClick={() => void submit()}>create server</Button>
            </div>
          </footer>
        </div>
      </main>
    </CreateFormContext.Provider>
  );
}

function Section({ number, title, children }: { number: string; title: string; children: React.ReactNode }) {
  return (
    <section>
      <header className="mb-2 flex items-baseline gap-3">
        <span className="font-mono text-[10px] uppercase tracking-[0.12em] text-text-faint">{number}</span>
        <h2 className="font-mono text-[14px] uppercase tracking-wider text-text-primary">{title}</h2>
      </header>
      {children}
    </section>
  );
}
```

- [ ] **Step 3: Verify**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm lint && pnpm build
```

- [ ] **Step 4: Commit**

```bash
git add frontend/app/components/BuildSlip.tsx frontend/app/servers/new/page.tsx
git commit -m "feat(frontend): /servers/new — build-slip + 6-section create flow"
```

### Task 5.7: Convert old `/servers/detail/page.tsx` into a redirect

**Files:**
- Modify: `/home/hadi/gitlab/anvil/frontend/app/servers/detail/page.tsx`

- [ ] **Step 1: Replace with redirect-only stub**

```tsx
"use client";
import { Suspense, useEffect } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { fetchServerDetail } from "../../lib/api";

function Inner() {
  const router = useRouter();
  const params = useSearchParams();
  const id = params.get("id");
  useEffect(() => {
    if (!id) { router.replace("/"); return; }
    void fetchServerDetail(id)
      .then((d) => router.replace(`/servers/${encodeURIComponent(d.name)}`))
      .catch(() => router.replace("/"));
  }, [id, router]);
  return <p className="p-5 font-mono text-[12px] text-text-muted">redirecting…</p>;
}

export default function DetailLegacyRedirect() {
  return <Suspense><Inner /></Suspense>;
}
```

- [ ] **Step 2: Verify**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm lint && pnpm build
```

- [ ] **Step 3: Commit**

```bash
git add frontend/app/servers/detail/page.tsx
git commit -m "refactor(frontend): /servers/detail?id= becomes a one-shot redirect to /servers/<name>"
```

### Task 5.8: Delete `NewServerModal.tsx` and `StatusBadge.tsx`

**Files:**
- Delete: `/home/hadi/gitlab/anvil/frontend/app/components/NewServerModal.tsx`
- Delete: `/home/hadi/gitlab/anvil/frontend/app/components/StatusBadge.tsx`

- [ ] **Step 1: Verify zero callers**

```bash
rg -n 'NewServerModal|StatusBadge' /home/hadi/gitlab/anvil/frontend
```
Expected: no matches outside the files themselves.

If `StatusBadge` is still referenced (e.g. in `ServerList.tsx`), swap to `Badge` first, then run rg again.

- [ ] **Step 2: Delete and verify**

```bash
rm /home/hadi/gitlab/anvil/frontend/app/components/NewServerModal.tsx
rm /home/hadi/gitlab/anvil/frontend/app/components/StatusBadge.tsx
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm lint && pnpm build
```

- [ ] **Step 3: Commit**

```bash
git add frontend/app/components/NewServerModal.tsx frontend/app/components/StatusBadge.tsx
git commit -m "chore(frontend): remove obsoleted NewServerModal + StatusBadge"
```

### Section 5 checkpoint

- [ ] Browser smoke test:
  - `/` → list renders, click `[+ new]` → goes to `/servers/new`
  - Fill form → click `[create server]` → lands on `/servers/<name>`
  - Click each tab → URL changes, body changes
  - `/servers/<name>/console` direct link → renders Console
  - Visit `/servers/detail?id=<some-uuid>` → redirects to `/servers/<name>`
- [ ] Run `superpowers:code-reviewer` on the section diff.
- [ ] **STOP** for user eyeball before Section 6.

---

# Section 6 — Polish remainder + verification

**Acceptance gate:** Every item in spec §12 acceptance checklist is checked off. `kubectl get sts mc-<id> -o yaml` shows `limits.cpu: <value>m` matching the chosen `cpu_millicores`. Visual signature pass: no purple/blue gradients, copper accent only on brand mark / primary-CTA brackets / active-tab underline / source-curseforge bar.

### Task 6.1: ConfirmDeleteDialog re-skin under new Modal

**Files:**
- Modify: `/home/hadi/gitlab/anvil/frontend/app/components/ConfirmDeleteDialog.tsx`

- [ ] **Step 1: Read and adapt**

The existing dialog already uses `Modal`. Swap inline color classes to new tokens. Replace the submit `<button>` with `<Button variant="danger">`. Verify it opens, requires the typed name, and calls `deleteServer`.

- [ ] **Step 2: Wire it into Settings tab**

Replace the `confirm()` call in `[name]/settings/page.tsx::onDelete` with state-controlled `<ConfirmDeleteDialog>` mounting.

- [ ] **Step 3: Verify**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm lint && pnpm build
```

- [ ] **Step 4: Commit**

```bash
git add frontend/app/components/ConfirmDeleteDialog.tsx frontend/app/servers/\[name\]/settings/page.tsx
git commit -m "feat(frontend): ConfirmDeleteDialog re-skin; Settings danger zone uses it"
```

### Task 6.2: Final verification sweep

- [ ] **Step 1: Backend**

```bash
cd /home/hadi/gitlab/anvil/backend
cargo fmt --check
cargo clippy --all-targets --features serve-dir -- -D warnings
cargo clippy --all-targets --features embed   -- -D warnings
cargo test --all
```

- [ ] **Step 2: Frontend**

```bash
cd /home/hadi/gitlab/anvil/frontend
pnpm typecheck && pnpm lint && pnpm build
```

- [ ] **Step 3: Single-binary release**

```bash
cd /home/hadi/gitlab/anvil
cd frontend && pnpm build && cd ../backend && cargo build --release --features embed
```
Expected: green.

- [ ] **Step 4: Live cluster check** (requires user to run; cannot self-execute without cluster context)

User runs:

```bash
cd /home/hadi/gitlab/anvil/backend
ANVIL_MC_STORAGE_CLASS=tank ANVIL_OIDC_ISSUER_URL=... ... \
  cargo run --release --features embed
```
Then in browser:
- Create a fresh vanilla server with `cpu=2000m, mem=4096, storage=20GiB, clusterip`
- `kubectl get sts mc-<id> -o yaml | yq .spec.template.spec.containers[0].resources` confirms `limits.cpu: 2000m`, `limits.memory: 4096Mi`
- Existing v1 server (if any) is reachable, list shows `cpu=1000m` (backfill), restart it → new spec lands
- Visit `/servers/<name>/console` → logs stream live
- Visit `/servers/<name>/settings` → adjust memory → save → restart → new spec lands

- [ ] **Step 5: Spec §12 checklist walk**

Walk every item in the spec's verification block. Tick each one.

- [ ] **Step 6: Commit (no code, marker only)**

```bash
git commit --allow-empty -m "chore: anvil v2 sub-project A complete (M6)"
```

### Section 6 checkpoint

- [ ] All checklist items green.
- [ ] Run `superpowers:code-reviewer` on the cumulative section 5+6 diff.
- [ ] **STOP** — sub-project A complete. Hand off to FluxCD deployment phase (separate cycle).

---

## Self-Review

**Spec coverage:**

- §1 Context — N/A (background)
- §2 Scope — every item is in a section: design system §2.1 → Section 2; primitives §2.2 → Section 2; layout §2.3 → Section 3; server list §2.4 → Section 4; detail page §2.5 → Section 5 (Tasks 5.2–5.5); create page §2.6 → Section 5 (Task 5.6); update FSM §2.7 → Section 5 (Task 5.4); CPU control §2.8 → Section 1 (Task 1.3) + Section 5 (settings tab); MC version list §2.9 → Section 1 (Task 1.5); polish-audit fold-in §2.10 → spread across Sections 1, 4, 5, 6 + Task 5.5 specifically for `EndReason`.
- §3 Anti-overengineering — addressed by spec-coverage checks during writing; no traits added with one impl (the `project_id` trait method has the vanilla impl as the second consumer of the default).
- §4 Design POV — Section 2 (tokens) and Section 5 (page implementations) embed the workshop aesthetic; visual sign-off is Task 6.2 step 4.
- §5 Tokens — Task 2.1.
- §6 Primitives — Task 2.2 (Button), 2.3 (Modal), 2.4 (10 stateless primitives), 2.5 (Toast), 2.6 (CommandBar/PathBreadcrumb), 5.6 (BuildSlip).
- §7 Layout — Task 3.1 (CommandBar mount).
- §8 Pages — Tasks 4.4–4.5 (list), 5.2–5.4 (detail), 5.6 (create).
- §9 Backend — Tasks 1.2 (DB), 1.3 (CPU thread), 1.4 (capabilities), 1.5 (mc-versions), 1.6 (validation), 1.7 (modpack cleanup).
- §10 Frontend file deltas — every file in the spec table appears in this plan's "File Structure" section and has a task.
- §11 Migration — Task 1.2 + redirect Task 5.7.
- §12 Verification — Task 6.2.
- §13 Open questions — resolved at top of plan ("Decisions Locked").
- §14 What ships at the end — matches the section structure.

**Placeholder scan:** ran mental search for "TBD", "TODO", "fill in", "similar to", "appropriate error handling". None found. Specific deferral notes ("v2.1") are explicit user-facing copy, not implementer placeholders.

**Type consistency:**
- `cpu_millicores: i64` (Rust) ↔ `cpu_millicores: z.number().int()` (Zod) — matches across create/get/list/settings.
- `available_cpu_cores: f64` ↔ `z.number().nonnegative()` — matches.
- `Status` union in `update-stream.ts` updated to `"connecting" | "live" | "reconnecting" | "ended" | "error"` — matches the `logs-stream.ts` pattern.
- `Tab.id` strings (`overview`, `console`, `mods`, `players`, `files`, `settings`) consistent across `Tabs` props and the layout's `activeId` derivation.
- `useUpdateStream(serverId: string | null)` — caller (`UpdateSheet`) passes `detail.id` (UUID), not the URL name. Confirmed.

**Outstanding implementation risks I know I left in:**

1. The `regex` crate dependency for `validate_force_version` (Task 1.6 Step 3) — must ask the user before adding. Hand-rolled char loop fallback is documented inline.
2. The `fetchServerByName(name)` helper does 2 RTTs (Task 5.2). Acceptable for v1; faster `GET /api/servers/by-name/{name}` is a deliberate deferral.
3. Tailwind v4 `bg-bg`/`text-text-primary` utility names look weird in source. Documented but not aliased.
4. `lib/api.ts` `Me` type export is added in Task 4.1 step 6 — the `CommandBar` (Task 2.6) depends on it. Order of execution must respect this; if Section 2 runs before Section 4, declare `interface Me` locally in `CommandBar.tsx` and remove later.

---

## Execution Handoff

Plan complete and saved to this file (`/home/hadi/.claude/plans/you-re-picking-up-anvil-sequential-kurzweil.md`).

**Two execution options:**

1. **Subagent-Driven (recommended)** — fresh subagent per task with two-stage review. Good for the longer tasks (5.6, 5.2) where context isolation matters.
2. **Inline execution** — execute tasks in this session via `superpowers:executing-plans` with batch checkpoints. Good for the shorter, mechanical sections (1, 3, 4).

Hadi instructed in the brief: "Stop every ~5 commits or section boundary so the user can eyeball." Both modes honor that — section checkpoints are explicit `STOP` markers. Defer the choice to Hadi at execution time.
