# Anvil v2 Sub-project B — Mod Ecosystem Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship Modrinth provider, runtime registry (Fabric/Forge/NeoForge/Paper via itzg TYPE), unified CF + Modrinth catalog, and individual-mod pending+apply flow on `modded` servers.

**Architecture:** `ModpackProvider` trait reshape decouples from `CurseForgeClient`; methods take a `ModpackHttp<'_>` borrow holding both CF (Option) and Modrinth (always-on) clients. Five `source_kind`s. Mod-sync is a new FSM mirroring M5's update FSM (UpdateGuard, snapshot lock, WS bus). No new images, no new DB migration.

**Tech Stack:** Rust 1.83 · axum 0.8 · kube-rs · sqlx (SQLite) · Next.js 16 (`output: 'export'`) · Tailwind v4 · Zod.

**Spec:** `docs/superpowers/specs/2026-05-03-anvil-v2-mod-ecosystem-design.md`

---

## File Structure

**Created (backend):**

| Path | Responsibility |
|---|---|
| `backend/src/modpack/mr_client.rs` | Modrinth HTTP client (mirrors `cf_client.rs` shape) |
| `backend/src/modpack/modrinth.rs` | `ModrinthServerPack` provider (`.mrpack` modpacks via itzg AUTO_MODRINTH) |
| `backend/src/modpack/modded.rs` | `ModdedRuntime` provider (Fabric/Forge/NeoForge + explicit modlist) |
| `backend/src/modpack/paper.rs` | `PaperServerProvider` (itzg TYPE=PAPER) |
| `backend/src/modpack/mods_apply.rs` | Mod-sync FSM (`announce → stop → sync → start → verify`) |
| `backend/src/routes/catalog.rs` | `GET /api/catalog/search` and `GET /api/catalog/projects/{provider}/{id}/versions` |
| `backend/src/routes/servers/mods.rs` | Pending CRUD + apply route + apply WS stream |

**Created (frontend):**

| Path | Responsibility |
|---|---|
| `frontend/app/components/CatalogSheet.tsx` | Catalog search UI (wraps `Sheet`) |
| `frontend/app/components/ApplySheet.tsx` | Mod-apply FSM viewer (parameterize `UpdateSheet` if 1:1) |
| `frontend/app/components/ModRow.tsx` | Installed-mod row (modded servers) |
| `frontend/app/components/PendingRow.tsx` | Pending-op row |
| `frontend/app/lib/use-mod-apply-stream.ts` | Apply WS hook (mirrors `update-stream.ts`) |

**Modified (backend):** `modpack/mod.rs`, `modpack/curseforge.rs`, `modpack/vanilla.rs`, `modpack/orchestrator.rs`, `modpack/poller.rs`, `modpack/jobs.rs`, `lib.rs`, `main.rs`, `config.rs`, `routes/mod.rs`, `routes/cluster.rs`, `routes/servers/mod.rs`, `routes/servers/create.rs`, `routes/servers/update.rs`, `validation.rs`.

**Modified (frontend):** `lib/api.ts`, `servers/tabs/ModsBody.tsx`, `servers/new/page.tsx`, `components/BuildSlip.tsx`.

---

## Phase 1 — Trait reshape (decouple from CF, widen IDs)

Goal: Make `ModpackProvider` work for both CF (numeric IDs) and Modrinth (string IDs). Changes ripple through the orchestrator, poller, and the public update-route wire shape.

### Task 1.1: Widen `VersionInfo.id` and `ModpackProvider::project_id` to String

**Files:**
- Modify: `backend/src/modpack/mod.rs`

- [ ] **Step 1: Update `VersionInfo` and trait**

In `backend/src/modpack/mod.rs`, change:

```rust
pub struct VersionInfo {
    /// Opaque upstream version id (CF: stringified u32; Modrinth: 8-char base62).
    pub id: String,
    pub name: String,
    pub download_url: String,
}
```

And the trait:

```rust
fn project_id(&self) -> Option<String> {
    None
}
```

(Was `Option<u32>`.)

- [ ] **Step 2: Add `ModpackHttp` borrow struct above the trait**

```rust
/// HTTP context handed to provider methods so they can reach the right upstream.
#[derive(Debug)]
pub struct ModpackHttp<'a> {
    pub cf: Option<&'a CurseForgeClient>,
    pub mr: &'a mr_client::ModrinthClient,
}
```

(Add `pub mod mr_client;` next to `pub mod cf_client;` even though the file doesn't exist yet — Phase 2 creates it. To keep this task compile-clean, defer the `mr` field until Phase 2 Task 2.1; for now stub `ModpackHttp { cf: Option<&CurseForgeClient> }` only and grow it next phase.)

Concretely for Task 1.1, ship:

```rust
#[derive(Debug)]
pub struct ModpackHttp<'a> {
    pub cf: Option<&'a CurseForgeClient>,
}
```

- [ ] **Step 3: Update trait method signatures**

```rust
async fn latest(&self, http: &ModpackHttp<'_>) -> Result<Option<VersionInfo>>;
async fn fetch_url(&self, http: &ModpackHttp<'_>, version: &VersionInfo) -> Result<String>;
```

- [ ] **Step 4: Run `cargo check`**

```bash
cd /home/hadi/gitlab/anvil/backend && cargo check
```

Expected: many errors in `curseforge.rs`, `vanilla.rs`, `orchestrator.rs`, `poller.rs` — that's what the next tasks fix.

### Task 1.2: Adapt `CurseForgeServerPack`

**Files:**
- Modify: `backend/src/modpack/curseforge.rs`

- [ ] **Step 1: Update `project_id` and `latest`/`fetch_url` to new signatures**

Change the impl:

```rust
fn project_id(&self) -> Option<String> {
    Some(self.config.project_id.to_string())
}

async fn latest(&self, http: &ModpackHttp<'_>) -> Result<Option<VersionInfo>> {
    let cf = http.cf.ok_or_else(|| anyhow!("CurseForge client unavailable"))?;
    let files = cf.list_files(self.config.project_id).await?;
    Ok(self.pick_latest(&files))
}

async fn fetch_url(&self, http: &ModpackHttp<'_>, version: &VersionInfo) -> Result<String> {
    let cf = http.cf.ok_or_else(|| anyhow!("CurseForge client unavailable"))?;
    let id_u32: u32 = version.id.parse().context("CF version id not numeric")?;
    let files = cf.list_files(self.config.project_id).await?;
    let f = files
        .iter()
        .find(|f| f.id == id_u32)
        .ok_or_else(|| anyhow!("file id {} not found in project files", version.id))?;
    f.download_url.clone().ok_or_else(|| {
        anyhow!(
            "file {} has no download_url (project disabled API distribution)",
            version.id
        )
    })
}
```

Update imports: add `Context as _`.

- [ ] **Step 2: Update `pick_latest` to emit string ids**

```rust
candidates.first().map(|f| VersionInfo {
    id: f.id.to_string(),
    name: f.display_name.clone(),
    download_url: f.download_url.clone().unwrap_or_default(),
})
```

- [ ] **Step 3: Update tests**

Existing tests assert `pick_latest(&files).unwrap().id` equals integers. Convert to strings:

```rust
assert_eq!(p.pick_latest(&files).unwrap().id, "2");
```

Apply to all `pick_latest_*` tests.

- [ ] **Step 4: Run `cargo test --package anvil --lib modpack::curseforge`**

```bash
cd /home/hadi/gitlab/anvil/backend && cargo test --lib modpack::curseforge
```

Expected: all CF unit tests pass.

### Task 1.3: Adapt `VanillaProvider`

**Files:**
- Modify: `backend/src/modpack/vanilla.rs`

- [ ] **Step 1: Update method signatures**

```rust
async fn latest(&self, _http: &ModpackHttp<'_>) -> Result<Option<VersionInfo>> {
    Ok(None)
}

async fn fetch_url(&self, _http: &ModpackHttp<'_>, _version: &VersionInfo) -> Result<String> {
    unreachable!("orchestrator must never call fetch_url on a vanilla provider")
}
```

- [ ] **Step 2: Update import** — replace `super::CurseForgeClient` with `super::ModpackHttp` in the `use` line.

- [ ] **Step 3: Run `cargo check --lib`**

```bash
cd /home/hadi/gitlab/anvil/backend && cargo check --lib
```

Expected: orchestrator.rs and poller.rs still error — fixed in next task.

### Task 1.4: Adapt orchestrator + update route

**Files:**
- Modify: `backend/src/modpack/orchestrator.rs`
- Modify: `backend/src/routes/servers/update.rs`

- [ ] **Step 1: Change `run`'s `target_version_id` parameter to `String`**

```rust
pub async fn run(state: AppState, server_id: String, target_version_id: String, guard: UpdateGuard) {
```

Pass through to `run_inner` similarly; `pick_target_version` takes `&str`:

```rust
async fn pick_target_version(
    provider: &dyn ModpackProvider,
    http: &ModpackHttp<'_>,
    target_version_id: &str,
) -> Result<VersionInfo> {
    if let Some(latest) = provider.latest(http).await?
        && latest.id == target_version_id
    {
        return Ok(latest);
    }
    let project_id = provider
        .project_id()
        .ok_or_else(|| anyhow!("provider {} has no project id", provider.kind()))?;
    // CF-specific cache hit; Modrinth provider's `latest` already populated its own.
    if provider.kind() == "curseforge" {
        let cf = http.cf.ok_or_else(|| anyhow!("CF client unavailable"))?;
        let project_id_u32: u32 = project_id.parse().context("CF project_id not numeric")?;
        let target_id_u32: u32 = target_version_id.parse().context("target version id not numeric for CF")?;
        let files = cf.list_files(project_id_u32).await?;
        let f = files
            .iter()
            .find(|f| f.id == target_id_u32)
            .ok_or_else(|| anyhow!("file id {target_version_id} not in project files"))?;
        return Ok(VersionInfo {
            id: f.id.to_string(),
            name: f.display_name.clone(),
            download_url: f.download_url.clone().unwrap_or_default(),
        });
    }
    // Modrinth path lands in Phase 4; for now bail.
    bail!("unsupported provider for target lookup: {}", provider.kind());
}
```

- [ ] **Step 2: Build `ModpackHttp` at call sites**

Replace the `cf` extraction with:

```rust
let cf = state.cf_client.as_ref();
let http = ModpackHttp { cf: cf.map(Arc::as_ref) };
```

Then use `&http` in `provider.latest(&http).await?` and `provider.fetch_url(&http, &version).await?`.

- [ ] **Step 3: Update `update.rs` route to take `String` version_id at the wire**

```rust
#[derive(Debug, Default, Deserialize)]
pub struct UpdateRequest {
    #[serde(default)]
    pub version_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UpdateResponse {
    pub status: &'static str,
    pub server_id: String,
    pub target_version_id: String,
}
```

Change the `latest_id` lookup:

```rust
let target_version_id: String = if let Some(v) = req.version_id {
    v
} else {
    let row: Option<(i64,)> = sqlx::query_as("SELECT latest_id FROM modpack_versions WHERE server_id = ?")
        .bind(&id)
        .fetch_optional(&state.pool)
        .await?;
    row.ok_or(AppError::Conflict {
        code: "no_update_target",
        message: "no version_id supplied and no cached latest version available".to_owned(),
    })?
    .0
    .to_string()
};
```

(`modpack_versions.latest_id` stays an INTEGER column for CF; Modrinth's poller will write `latest_id` as the hash of the string id, OR we ALTER the column. For B we keep INTEGER and store CF ids; Modrinth pack support uses a separate path documented in Phase 4. For now the route surfaces strings to the client.)

Actually — keep `modpack_versions.latest_id` as INTEGER. Modrinth modpack version polling stores `latest_id = 0` as a sentinel and the real id lives in `latest_name`. Phase 4 Task 4.1 documents this convention. So `update.rs` looks up CF rows the same as before, casting `i64 → String` at output.

- [ ] **Step 4: Update `target_version_id` to `String` everywhere it's spawned**

```rust
let task_state = state.clone();
let task_id = id.clone();
let task_target = target_version_id.clone();
tokio::spawn(async move {
    orchestrator::run(task_state, task_id, task_target, guard).await;
});
```

JSON shape:

```rust
Json(json!({
    "status": "started",
    "server_id": id,
    "target_version_id": target_version_id,
}))
```

### Task 1.5: Adapt poller

**Files:**
- Modify: `backend/src/modpack/poller.rs`

- [ ] **Step 1: Build `ModpackHttp` and tighten WHERE clause**

```rust
let cf = state.cf_client.as_ref();
let http = ModpackHttp { cf: cf.map(Arc::as_ref) };

let rows = sqlx::query(
    "SELECT id, source_kind, source_config FROM servers
     WHERE source_kind IN ('curseforge','modrinth')",
)
.fetch_all(&state.pool)
.await?;
```

Use `provider.latest(&http).await` instead of `provider.latest(cf).await`.

- [ ] **Step 2: Update auto-apply call**

```rust
orchestrator::run(state.clone(), id.clone(), latest.id.clone(), guard).await;
```

(`latest.id` is now a `String`.)

- [ ] **Step 3: Adapt `latest.id` SQL bind**

`modpack_versions.latest_id` is INTEGER. For CF rows we parse the string back to `i64`; for Modrinth we'd write 0. Wrap:

```rust
let latest_id_int: i64 = latest.id.parse().unwrap_or(0);
```

- [ ] **Step 4: Adapt the skip-list comparison and current-id check**

```rust
let current_id_str = cfg
    .get("current_version_id")
    .map(|v| match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => String::new(),
    })
    .unwrap_or_default();

let skipped = skip_list.iter().any(|s| s == &latest.id || s == &latest.name);

if latest.id == current_id_str || auto_mode == "never" || skipped {
    // ...
}
```

### Task 1.6: Quality gate + commit

- [ ] **Step 1: Run `cargo test --all`**

```bash
cd /home/hadi/gitlab/anvil/backend && cargo test --all
```

Expected: green.

- [ ] **Step 2: Run clippy (both feature flavours)**

```bash
cd /home/hadi/gitlab/anvil/backend && \
  cargo clippy --all-targets --features serve-dir -- -D warnings && \
  cargo clippy --all-targets --features embed -- -D warnings
```

Expected: green.

- [ ] **Step 3: Run fmt check**

```bash
cd /home/hadi/gitlab/anvil/backend && cargo fmt --check
```

- [ ] **Step 4: Commit**

```bash
cd /home/hadi/gitlab/anvil && git add backend/src/modpack backend/src/routes/servers/update.rs && \
git commit -m "$(cat <<'EOF'
refactor(modpack): widen version_id to String, introduce ModpackHttp

Decouples ModpackProvider from CurseForgeClient: methods now take a
ModpackHttp<'_> borrow that will hold both CF and Modrinth clients
(Modrinth lands next phase). VersionInfo.id and ModpackProvider::
project_id widen from u32/numeric to String so Modrinth's opaque
8-char base62 ids fit. Update wire shape (POST /api/servers/:id/update
target_version_id) likewise widens — only B ships before any external
client of this API exists.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2 — `ModrinthClient`

Goal: Typed HTTP client for `api.modrinth.com/v2`, mirroring `CurseForgeClient` shape (Arc<Inner>, mutex-guarded HashMap cache, polite User-Agent).

### Task 2.1: Skeleton + types

**Files:**
- Create: `backend/src/modpack/mr_client.rs`
- Modify: `backend/src/modpack/mod.rs` (add `pub mod mr_client;` and grow `ModpackHttp`)

- [ ] **Step 1: Write `mr_client.rs` skeleton with the API types**

```rust
//! Modrinth API client (`api.modrinth.com/v2`).
//!
//! No auth required; sets a polite `User-Agent` per Modrinth's API docs.
//! `list_versions` is cached for an hour, mirroring `CurseForgeClient`'s
//! `list_files` cache. `search` is not cached (each query is distinct).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, USER_AGENT};
use serde::Deserialize;
use tokio::sync::Mutex;

const MR_API: &str = "https://api.modrinth.com/v2";
const MR_USER_AGENT: &str = concat!(
    "anvil/",
    env!("CARGO_PKG_VERSION"),
    " (https://gitlab.cherkaoui.ch/HadiCherkaoui/anvil)"
);
const CACHE_TTL: Duration = Duration::from_secs(60 * 60);

/// Project metadata.
#[derive(Debug, Clone, Deserialize)]
pub struct MrProject {
    pub id: String,
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub project_type: String, // "mod" | "modpack" | "plugin" | ...
    #[serde(default)]
    pub loaders: Vec<String>,
    #[serde(default)]
    pub game_versions: Vec<String>,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub followers: u64,
}

/// One version entry from `/project/{id}/version`.
#[derive(Debug, Clone, Deserialize)]
pub struct MrVersion {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub version_number: String,
    /// "release" | "beta" | "alpha"
    pub version_type: String,
    pub loaders: Vec<String>,
    pub game_versions: Vec<String>,
    pub date_published: String,
    pub files: Vec<MrFile>,
}

/// One file inside an `MrVersion`.
#[derive(Debug, Clone, Deserialize)]
pub struct MrFile {
    pub url: String,
    pub filename: String,
    pub primary: bool,
    #[serde(default)]
    pub hashes: MrHashes,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MrHashes {
    #[serde(default)]
    pub sha512: Option<String>,
    #[serde(default)]
    pub sha1: Option<String>,
}

/// Search hit from `/search`.
#[derive(Debug, Clone, Deserialize)]
pub struct MrSearchHit {
    pub project_id: String,
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub project_type: String,
    pub display_categories: Vec<String>,
    pub versions: Vec<String>,
    pub downloads: u64,
    pub follows: u64,
    pub icon_url: Option<String>,
    #[serde(default)]
    pub author: String,
    pub date_modified: String,
}

#[derive(Debug, Deserialize)]
struct SearchEnvelope {
    hits: Vec<MrSearchHit>,
}

/// Search query parameters.
#[derive(Debug, Default)]
pub struct SearchQuery<'a> {
    pub query: &'a str,
    /// "mod" | "modpack" | "plugin"
    pub project_type: &'a str,
    pub loader: Option<&'a str>,
    pub game_version: Option<&'a str>,
    pub limit: u32,
    pub offset: u32,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    fetched_at: Instant,
    versions: Arc<Vec<MrVersion>>,
}

#[derive(Debug, Clone)]
pub struct ModrinthClient {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    http: reqwest::Client,
    cache: Mutex<HashMap<String, CacheEntry>>,
}

impl ModrinthClient {
    /// Builds a client with the polite `User-Agent`.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `reqwest::Client` fails to build.
    pub fn new() -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static(MR_USER_AGENT));
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()
            .context("building Modrinth reqwest client")?;
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                cache: Mutex::new(HashMap::new()),
            }),
        })
    }
}
```

- [ ] **Step 2: Wire into `modpack/mod.rs`**

Add `pub mod mr_client;` next to the other module declarations and grow `ModpackHttp`:

```rust
pub use mr_client::ModrinthClient;

#[derive(Debug)]
pub struct ModpackHttp<'a> {
    pub cf: Option<&'a CurseForgeClient>,
    pub mr: &'a ModrinthClient,
}
```

- [ ] **Step 3: Build still passes**

```bash
cd /home/hadi/gitlab/anvil/backend && cargo check --lib
```

Expected: lib compiles. Existing call sites that build `ModpackHttp { cf: ... }` will start failing because `mr` is required — that's Phase 3. For now, gate this task by checking that `mr_client.rs` compiles standalone:

```bash
cd /home/hadi/gitlab/anvil/backend && cargo check --lib 2>&1 | grep -v "missing field .mr." | head -30
```

(We accept the missing-field errors — they're caller-side and Phase 3 fixes them.)

### Task 2.2: `project()` with fixture test

**Files:**
- Modify: `backend/src/modpack/mr_client.rs`

- [ ] **Step 1: Add `project` method**

```rust
impl ModrinthClient {
    /// Fetches project metadata for `id_or_slug`.
    ///
    /// # Errors
    ///
    /// Returns an error if the project does not exist or the request fails.
    pub async fn project(&self, id_or_slug: &str) -> Result<MrProject> {
        let url = format!("{MR_API}/project/{id_or_slug}");
        let resp = self.inner.http.get(&url).send().await?;
        let status = resp.status();
        if status.as_u16() == 404 {
            bail!("Modrinth project {id_or_slug:?} not found");
        }
        if !status.is_success() {
            bail!("Modrinth GET {url} failed: HTTP {status}");
        }
        resp.json::<MrProject>().await.context("decoding /project")
    }
}
```

- [ ] **Step 2: Add a parse-fixture unit test (no network)**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_project_response() {
        let body = r#"{
            "id": "AANobbMI",
            "slug": "sodium",
            "title": "Sodium",
            "project_type": "mod",
            "loaders": ["fabric"],
            "game_versions": ["1.21.1", "1.21.0"],
            "icon_url": "https://cdn.modrinth.com/data/AANobbMI/icon.png",
            "downloads": 12345678,
            "followers": 5000
        }"#;
        let p: MrProject = serde_json::from_str(body).expect("parses");
        assert_eq!(p.id, "AANobbMI");
        assert_eq!(p.slug, "sodium");
        assert_eq!(p.project_type, "mod");
        assert_eq!(p.loaders, vec!["fabric"]);
    }
}
```

- [ ] **Step 3: `cargo test --lib modpack::mr_client::tests::parses_project_response`**

Expected: PASS.

### Task 2.3: `list_versions()` with cache

**Files:**
- Modify: `backend/src/modpack/mr_client.rs`

- [ ] **Step 1: Add the method + helpers**

```rust
impl ModrinthClient {
    /// Returns the cached or freshly-fetched version list for `project_id`.
    ///
    /// # Errors
    ///
    /// Returns an error if the upstream call fails.
    pub async fn list_versions(&self, project_id: &str) -> Result<Arc<Vec<MrVersion>>> {
        if let Some(cached) = self.peek_cache(project_id).await {
            return Ok(cached);
        }
        let url = format!("{MR_API}/project/{project_id}/version");
        let resp = self.inner.http.get(&url).send().await?;
        let status = resp.status();
        if status.as_u16() == 404 {
            bail!("Modrinth project {project_id:?} not found");
        }
        if !status.is_success() {
            bail!("Modrinth GET {url} failed: HTTP {status}");
        }
        let versions: Vec<MrVersion> = resp.json().await.context("decoding /version")?;
        let entry = CacheEntry {
            fetched_at: Instant::now(),
            versions: Arc::new(versions),
        };
        let cloned = Arc::clone(&entry.versions);
        self.inner
            .cache
            .lock()
            .await
            .insert(project_id.to_owned(), entry);
        Ok(cloned)
    }

    async fn peek_cache(&self, project_id: &str) -> Option<Arc<Vec<MrVersion>>> {
        let cache = self.inner.cache.lock().await;
        let entry = cache.get(project_id)?;
        if entry.fetched_at.elapsed() < CACHE_TTL {
            Some(Arc::clone(&entry.versions))
        } else {
            None
        }
    }
}
```

- [ ] **Step 2: Add a parse-fixture test**

```rust
#[test]
fn parses_version_response() {
    let body = r#"[{
        "id": "8VJ4TfX1",
        "project_id": "AANobbMI",
        "name": "Sodium 0.5.13 for 1.21.1",
        "version_number": "mc1.21.1-0.5.13",
        "version_type": "release",
        "loaders": ["fabric"],
        "game_versions": ["1.21.1"],
        "date_published": "2026-01-01T00:00:00Z",
        "files": [{
            "url": "https://cdn.modrinth.com/data/AANobbMI/versions/8VJ4TfX1/sodium-fabric-0.5.13.jar",
            "filename": "sodium-fabric-0.5.13.jar",
            "primary": true,
            "hashes": {"sha512": "abc"}
        }]
    }]"#;
    let v: Vec<MrVersion> = serde_json::from_str(body).expect("parses");
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].id, "8VJ4TfX1");
    assert_eq!(v[0].files[0].filename, "sodium-fabric-0.5.13.jar");
    assert_eq!(v[0].files[0].hashes.sha512.as_deref(), Some("abc"));
}
```

- [ ] **Step 3: `cargo test --lib modpack::mr_client`**

Expected: PASS.

### Task 2.4: `search()` and `version()`

**Files:**
- Modify: `backend/src/modpack/mr_client.rs`

- [ ] **Step 1: Add `search` (uses facets)**

```rust
impl ModrinthClient {
    /// Searches projects with the given facets.
    ///
    /// # Errors
    ///
    /// Returns an error if the upstream call fails.
    pub async fn search(&self, q: &SearchQuery<'_>) -> Result<Vec<MrSearchHit>> {
        let mut facets: Vec<Vec<String>> = vec![vec![format!("project_type:{}", q.project_type)]];
        if let Some(l) = q.loader {
            facets.push(vec![format!("categories:{l}")]);
        }
        if let Some(v) = q.game_version {
            facets.push(vec![format!("versions:{v}")]);
        }
        let facets_json =
            serde_json::to_string(&facets).context("serializing search facets")?;
        let limit = if q.limit == 0 { 20 } else { q.limit };
        let resp = self
            .inner
            .http
            .get(format!("{MR_API}/search"))
            .query(&[
                ("query", q.query),
                ("facets", facets_json.as_str()),
                ("limit", &limit.to_string()),
                ("offset", &q.offset.to_string()),
                ("index", "relevance"),
            ])
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!("Modrinth /search failed: HTTP {}", resp.status());
        }
        let env: SearchEnvelope = resp.json().await.context("decoding /search")?;
        Ok(env.hits)
    }

    /// Fetches a single version by id.
    ///
    /// # Errors
    ///
    /// Returns an error if the version does not exist or the call fails.
    pub async fn version(&self, version_id: &str) -> Result<MrVersion> {
        let url = format!("{MR_API}/version/{version_id}");
        let resp = self.inner.http.get(&url).send().await?;
        if resp.status().as_u16() == 404 {
            bail!("Modrinth version {version_id:?} not found");
        }
        if !resp.status().is_success() {
            bail!("Modrinth GET {url} failed: HTTP {}", resp.status());
        }
        resp.json::<MrVersion>().await.context("decoding /version/{id}")
    }
}
```

- [ ] **Step 2: Add fixture test for the search hit shape**

```rust
#[test]
fn parses_search_envelope() {
    let body = r#"{"hits": [{
        "project_id": "AANobbMI",
        "slug": "sodium",
        "title": "Sodium",
        "description": "Modern rendering engine",
        "project_type": "mod",
        "display_categories": ["fabric", "optimization"],
        "versions": ["1.21.1"],
        "downloads": 100,
        "follows": 10,
        "icon_url": null,
        "author": "jellysquid3",
        "date_modified": "2026-01-01T00:00:00Z"
    }], "offset": 0, "limit": 20, "total_hits": 1}"#;
    let env: SearchEnvelope = serde_json::from_str(body).expect("parses");
    assert_eq!(env.hits.len(), 1);
    assert_eq!(env.hits[0].project_id, "AANobbMI");
}
```

- [ ] **Step 3: `cargo test --lib modpack::mr_client`**

Expected: PASS.

### Task 2.5: Phase 2 commit

- [ ] **Step 1: Commit**

```bash
cd /home/hadi/gitlab/anvil && git add backend/src/modpack/mr_client.rs backend/src/modpack/mod.rs && \
git commit -m "$(cat <<'EOF'
feat(backend): ModrinthClient with project/version/search/list_versions

Mirrors CurseForgeClient shape: Arc<Inner> with reqwest::Client +
Mutex<HashMap> cache. CACHE_TTL=1h matches CF. search hits Modrinth's
/search endpoint with facet filters; project/version/list_versions
hit /project/{id}, /version/{id}, /project/{id}/version. Polite
User-Agent per Modrinth API docs.

ModpackHttp now carries both clients (cf optional, mr always-on)
matching the always-on Modrinth posture.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3 — AppState + config wiring

Goal: Make `state.mr_client` always available; relax the CF-key-gated PVC requirement; pass `ModpackHttp { cf, mr: &state.mr_client }` everywhere.

### Task 3.1: AppState + config

**Files:**
- Modify: `backend/src/lib.rs`
- Modify: `backend/src/config.rs`
- Modify: `backend/src/main.rs`

- [ ] **Step 1: Add `mr_client` to `AppState`**

In `backend/src/lib.rs` `AppState` struct:

```rust
pub mr_client: Arc<ModrinthClient>,
```

(Always present — no `Option`.) Update the `use` line:

```rust
use crate::modpack::{CurseForgeClient, ModrinthClient, orchestrator::UpdatePhase};
```

Update the `Debug` impl:

```rust
.field("mr_client", &"<mr>")
```

- [ ] **Step 2: Drop CF-key-gated PVC requirement**

In `backend/src/config.rs::from_env`, replace:

```rust
if cf_api_key.is_some() && modpack_snapshots_pvc.is_none() {
    bail!(
        "CF_API_KEY is set but ANVIL_MODPACK_SNAPSHOTS_PVC is not — modpack updates need a snapshots PVC"
    );
}
```

with:

```rust
if modpack_snapshots_pvc.is_none() {
    bail!(
        "ANVIL_MODPACK_SNAPSHOTS_PVC must be set — modpack/modded updates need a snapshots PVC"
    );
}
```

- [ ] **Step 3: Boot `ModrinthClient` in `main.rs`**

Add to imports:

```rust
use anvil::modpack::{self, CurseForgeClient, ModrinthClient};
```

In `main()`, before the `AppState` constructor:

```rust
let mr_client = Arc::new(
    ModrinthClient::new().context("constructing Modrinth client")?,
);
```

In the struct init:

```rust
mr_client,
```

Tighten the poller-spawn condition. `cf_client.is_some()` no longer gates the poller — the poller now also handles Modrinth servers. Replace:

```rust
if state.cf_client.is_some() {
    let poller_state = state.clone();
    tokio::spawn(async move {
        modpack::poller::run(poller_state).await;
    });
}
```

with (always spawn):

```rust
let poller_state = state.clone();
tokio::spawn(async move {
    modpack::poller::run(poller_state).await;
});
```

- [ ] **Step 4: Update poller startup safety**

The poller previously errored if `cf_client` was None. Soften: on each tick, still skip CF rows when `cf_client` is None (Modrinth rows still poll). In `backend/src/modpack/poller.rs::tick`, replace:

```rust
let cf = state
    .cf_client
    .as_ref()
    .ok_or_else(|| anyhow::anyhow!("CF disabled, poller should not be running"))?;
```

with:

```rust
let cf = state.cf_client.as_ref();
let http = ModpackHttp {
    cf: cf.map(Arc::as_ref),
    mr: state.mr_client.as_ref(),
};
```

And inside the loop, before calling `provider.latest(&http)`, skip CF rows when CF is disabled:

```rust
if source_kind == "curseforge" && cf.is_none() {
    continue;
}
```

- [ ] **Step 5: Update orchestrator's `ModpackHttp` build to include `mr`**

In `backend/src/modpack/orchestrator.rs`, find the `let cf = state.cf_client.as_ref()...` block and grow:

```rust
let http = ModpackHttp {
    cf: state.cf_client.as_deref(),
    mr: state.mr_client.as_ref(),
};
```

Use `&http` for both `provider.latest` and `provider.fetch_url` calls. Drop the now-unused `cf` local where applicable.

### Task 3.2: Quality gate + commit

- [ ] **Step 1: `cargo check --lib`**

```bash
cd /home/hadi/gitlab/anvil/backend && cargo check --lib
```

Expected: green.

- [ ] **Step 2: `cargo test --all`**

```bash
cd /home/hadi/gitlab/anvil/backend && cargo test --all
```

Expected: green.

- [ ] **Step 3: clippy + fmt**

```bash
cd /home/hadi/gitlab/anvil/backend && \
  cargo clippy --all-targets --features serve-dir -- -D warnings && \
  cargo clippy --all-targets --features embed -- -D warnings && \
  cargo fmt --check
```

Expected: green.

- [ ] **Step 4: Commit**

```bash
cd /home/hadi/gitlab/anvil && git add backend/src && \
git commit -m "$(cat <<'EOF'
feat(backend): mr_client always-on in AppState; poller no longer CF-gated

ModrinthClient is constructed unconditionally at boot and lives on
state.mr_client (Arc, no Option). ANVIL_MODPACK_SNAPSHOTS_PVC becomes
unconditionally required since Modrinth (always-on) needs it. The
hourly poller now spawns regardless of CF state and skips CF rows
in-loop when cf_client is None.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---


## Phase 4 — New providers

Goal: Add `ModrinthServerPack`, `ModdedRuntime`, `PaperServerProvider`. Wire into `from_db`.

### Task 4.1: `ModrinthServerPack` provider

**Files:**
- Create: `backend/src/modpack/modrinth.rs`
- Modify: `backend/src/modpack/mod.rs` (add `pub mod modrinth;` + `pub use`)

- [ ] **Step 1: Write the provider**

```rust
//! Modrinth `.mrpack` provider.
//!
//! Reuses `itzg/minecraft-server:java21` with `TYPE=AUTO_MODRINTH` —
//! itzg's launcher handles `.mrpack` unzip + loader install.

use std::time::Duration;

use anyhow::{Result, anyhow};
use k8s_openapi::api::core::v1::EnvVar;
use serde::{Deserialize, Serialize};

use super::curseforge::{AutoUpdateMode, Channel};
use super::vanilla::env_kv;
use super::{ModpackHttp, ModpackProvider, ProviderContext, VersionInfo};

const MR_IMAGE: &str = "itzg/minecraft-server:java21";
const MR_BOOT_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Persisted Modrinth modpack config (lives in `servers.source_config`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Modrinth project id (8-char base62) or slug.
    pub project_id: String,
    pub channel: Channel,
    #[serde(default)]
    pub version_skip: Vec<String>,
    #[serde(default)]
    pub force_version: Option<String>,
    pub current_version_id: String,
    pub current_version_name: String,
    #[serde(default)]
    pub auto_update_mode: AutoUpdateMode,
}

/// Modrinth modpack provider.
#[derive(Debug, Clone)]
pub struct ModrinthServerPack {
    config: Config,
}

impl ModrinthServerPack {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Picks the newest version matching the channel + skip list.
    fn pick_latest(&self, versions: &[super::mr_client::MrVersion]) -> Option<VersionInfo> {
        let mut candidates: Vec<&super::mr_client::MrVersion> = versions
            .iter()
            .filter(|v| match (&self.config.channel, v.version_type.as_str()) {
                (Channel::Release, "release") => true,
                (Channel::Beta, "release" | "beta") => true,
                (Channel::Alpha, _) => true,
                _ => false,
            })
            .filter(|v| {
                !self
                    .config
                    .version_skip
                    .iter()
                    .any(|s| s == &v.id || s == &v.name)
            })
            .filter(|v| v.files.iter().any(|f| f.primary))
            .collect();
        candidates.sort_by(|a, b| b.date_published.cmp(&a.date_published));
        candidates.first().map(|v| {
            let primary = v.files.iter().find(|f| f.primary).expect("checked above");
            VersionInfo {
                id: v.id.clone(),
                name: v.name.clone(),
                download_url: primary.url.clone(),
            }
        })
    }
}

#[async_trait::async_trait]
impl ModpackProvider for ModrinthServerPack {
    fn kind(&self) -> &'static str {
        "modrinth"
    }

    fn project_id(&self) -> Option<String> {
        Some(self.config.project_id.clone())
    }

    fn pod_image(&self) -> &str {
        MR_IMAGE
    }

    fn launch_command(&self) -> Option<Vec<String>> {
        None
    }

    fn extra_env(&self, ctx: &ProviderContext<'_>) -> Vec<EnvVar> {
        vec![
            env_kv("EULA", "TRUE"),
            env_kv("TYPE", "AUTO_MODRINTH"),
            env_kv("MODRINTH_PROJECT", &self.config.project_id),
            env_kv("MODRINTH_DOWNLOAD_DEPENDENCIES", "required"),
            env_kv("MEMORY", &format!("{}M", ctx.memory_mi)),
            env_kv("ENABLE_RCON", "true"),
            super::vanilla::env_secret(
                "RCON_PASSWORD",
                &format!("mc-{}-rcon", ctx.server_id),
                "password",
            ),
        ]
    }

    fn boot_timeout(&self) -> Duration {
        MR_BOOT_TIMEOUT
    }

    async fn latest(&self, http: &ModpackHttp<'_>) -> Result<Option<VersionInfo>> {
        let versions = http.mr.list_versions(&self.config.project_id).await?;
        Ok(self.pick_latest(&versions))
    }

    async fn fetch_url(&self, http: &ModpackHttp<'_>, version: &VersionInfo) -> Result<String> {
        let v = http.mr.version(&version.id).await?;
        let primary = v
            .files
            .iter()
            .find(|f| f.primary)
            .ok_or_else(|| anyhow!("Modrinth version {} has no primary file", version.id))?;
        Ok(primary.url.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::super::mr_client::{MrFile, MrHashes, MrVersion};
    use super::*;

    fn mr_v(id: &str, name: &str, vtype: &str, date: &str) -> MrVersion {
        MrVersion {
            id: id.to_owned(),
            project_id: "p".to_owned(),
            name: name.to_owned(),
            version_number: name.to_owned(),
            version_type: vtype.to_owned(),
            loaders: vec!["fabric".to_owned()],
            game_versions: vec!["1.21.1".to_owned()],
            date_published: date.to_owned(),
            files: vec![MrFile {
                url: format!("https://example/{id}.mrpack"),
                filename: format!("{id}.mrpack"),
                primary: true,
                hashes: MrHashes::default(),
            }],
        }
    }

    fn pack(channel: Channel, skip: Vec<String>) -> ModrinthServerPack {
        ModrinthServerPack::new(Config {
            project_id: "AANobbMI".to_owned(),
            channel,
            version_skip: skip,
            force_version: None,
            current_version_id: String::new(),
            current_version_name: String::new(),
            auto_update_mode: AutoUpdateMode::Notify,
        })
    }

    #[test]
    fn pick_latest_picks_newest_release() {
        let p = pack(Channel::Release, vec![]);
        let vs = vec![
            mr_v("a", "old", "release", "2026-01-01T00:00:00Z"),
            mr_v("b", "new", "release", "2026-02-01T00:00:00Z"),
        ];
        assert_eq!(p.pick_latest(&vs).unwrap().id, "b");
    }

    #[test]
    fn pick_latest_release_rejects_beta() {
        let p = pack(Channel::Release, vec![]);
        let vs = vec![mr_v("a", "beta-only", "beta", "2026-01-01T00:00:00Z")];
        assert!(p.pick_latest(&vs).is_none());
    }

    #[test]
    fn pick_latest_honours_skip_list_by_id() {
        let p = pack(Channel::Release, vec!["b".to_owned()]);
        let vs = vec![
            mr_v("a", "old", "release", "2026-01-01T00:00:00Z"),
            mr_v("b", "new", "release", "2026-02-01T00:00:00Z"),
        ];
        assert_eq!(p.pick_latest(&vs).unwrap().id, "a");
    }

    #[test]
    fn provider_kind_is_modrinth() {
        assert_eq!(pack(Channel::Release, vec![]).kind(), "modrinth");
    }
}
```

- [ ] **Step 2: Make `vanilla.rs` helpers `pub(super)` so modrinth.rs can reuse**

In `backend/src/modpack/vanilla.rs`, change the visibility of `env_kv` and `env_secret`:

```rust
pub(super) fn env_kv(name: &str, value: &str) -> EnvVar {
    // ...
}

pub(super) fn env_secret(name: &str, secret_name: &str, key: &str) -> EnvVar {
    // ...
}
```

- [ ] **Step 3: `pub use` in `mod.rs`**

```rust
pub mod modrinth;
pub use modrinth::ModrinthServerPack;
```

- [ ] **Step 4: `cargo test --lib modpack::modrinth`**

Expected: PASS.

### Task 4.2: `ModdedRuntime` provider

**Files:**
- Create: `backend/src/modpack/modded.rs`
- Modify: `backend/src/modpack/mod.rs`

- [ ] **Step 1: Write the provider**

```rust
//! Modded runtime provider — Fabric / Forge / NeoForge with explicit modlist.
//!
//! itzg/minecraft-server with TYPE switching. The mod jars are NOT delivered
//! via itzg's MODS env — anvil's mod-sync Job is the sole writer to /data/mods.

use std::time::Duration;

use anyhow::{Result, anyhow};
use k8s_openapi::api::core::v1::EnvVar;
use serde::{Deserialize, Serialize};

use super::vanilla::{env_kv, env_secret};
use super::{ModpackHttp, ModpackProvider, ProviderContext, VersionInfo};

const MODDED_IMAGE: &str = "itzg/minecraft-server:java21";
const MODDED_BOOT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// One installed mod (persisted in `source_config.mods`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModEntry {
    pub provider: String,
    pub project_id: String,
    pub project_slug: String,
    pub project_name: String,
    pub version_id: String,
    pub version_name: String,
    pub filename: String,
    pub download_url: String,
    #[serde(default)]
    pub sha512: Option<String>,
}

/// One pending change in the modlist draft.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PendingOp {
    Add { mod_entry: ModEntry },
    Remove { filename: String },
    Bump {
        filename: String,
        to_version_id: String,
        to_version_name: String,
        to_filename: String,
        to_download_url: String,
        #[serde(default)]
        to_sha512: Option<String>,
    },
}

/// Loader runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    Fabric,
    Forge,
    NeoForge,
}

impl Runtime {
    fn type_env(self) -> &'static str {
        match self {
            Self::Fabric => "FABRIC",
            Self::Forge => "FORGE",
            Self::NeoForge => "NEOFORGE",
        }
    }
}

/// Persisted modded config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub runtime: Runtime,
    pub mc_version: String,
    #[serde(default)]
    pub mods: Vec<ModEntry>,
    #[serde(default)]
    pub pending: Vec<PendingOp>,
}

#[derive(Debug, Clone)]
pub struct ModdedRuntime {
    config: Config,
}

impl ModdedRuntime {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Returns the modlist that would result from applying every pending op
    /// in order, replacing the current one.
    #[must_use]
    pub fn desired_mods(&self) -> Vec<ModEntry> {
        let mut out = self.config.mods.clone();
        for op in &self.config.pending {
            match op {
                PendingOp::Add { mod_entry } => {
                    out.retain(|m| m.filename != mod_entry.filename);
                    out.push(mod_entry.clone());
                }
                PendingOp::Remove { filename } => {
                    out.retain(|m| m.filename != *filename);
                }
                PendingOp::Bump {
                    filename,
                    to_version_id,
                    to_version_name,
                    to_filename,
                    to_download_url,
                    to_sha512,
                } => {
                    if let Some(idx) = out.iter().position(|m| m.filename == *filename) {
                        let m = &mut out[idx];
                        m.version_id = to_version_id.clone();
                        m.version_name = to_version_name.clone();
                        m.filename = to_filename.clone();
                        m.download_url = to_download_url.clone();
                        m.sha512 = to_sha512.clone();
                    }
                }
            }
        }
        out
    }
}

#[async_trait::async_trait]
impl ModpackProvider for ModdedRuntime {
    fn kind(&self) -> &'static str {
        "modded"
    }

    fn pod_image(&self) -> &str {
        MODDED_IMAGE
    }

    fn launch_command(&self) -> Option<Vec<String>> {
        None
    }

    fn extra_env(&self, ctx: &ProviderContext<'_>) -> Vec<EnvVar> {
        vec![
            env_kv("EULA", "TRUE"),
            env_kv("TYPE", self.config.runtime.type_env()),
            env_kv("VERSION", &self.config.mc_version),
            env_kv("MEMORY", &format!("{}M", ctx.memory_mi)),
            env_kv("ENABLE_RCON", "true"),
            env_secret(
                "RCON_PASSWORD",
                &format!("mc-{}-rcon", ctx.server_id),
                "password",
            ),
        ]
    }

    fn boot_timeout(&self) -> Duration {
        MODDED_BOOT_TIMEOUT
    }

    async fn latest(&self, _http: &ModpackHttp<'_>) -> Result<Option<VersionInfo>> {
        Ok(None) // mods are pinned; per-mod update polling is a follow-up.
    }

    async fn fetch_url(&self, _http: &ModpackHttp<'_>, _version: &VersionInfo) -> Result<String> {
        Err(anyhow!("modded runtime has no pack-level upstream"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> ModEntry {
        ModEntry {
            provider: "modrinth".to_owned(),
            project_id: format!("{name}-id"),
            project_slug: name.to_owned(),
            project_name: name.to_owned(),
            version_id: format!("{name}-v"),
            version_name: format!("{name}-1.0"),
            filename: format!("{name}.jar"),
            download_url: format!("https://example/{name}.jar"),
            sha512: None,
        }
    }

    fn cfg(mods: Vec<ModEntry>, pending: Vec<PendingOp>) -> Config {
        Config {
            runtime: Runtime::Fabric,
            mc_version: "1.21.1".to_owned(),
            mods,
            pending,
        }
    }

    #[test]
    fn desired_with_no_pending_returns_current() {
        let m = ModdedRuntime::new(cfg(vec![entry("sodium"), entry("lithium")], vec![]));
        assert_eq!(m.desired_mods().len(), 2);
    }

    #[test]
    fn desired_applies_remove() {
        let m = ModdedRuntime::new(cfg(
            vec![entry("sodium"), entry("lithium")],
            vec![PendingOp::Remove {
                filename: "sodium.jar".to_owned(),
            }],
        ));
        let d = m.desired_mods();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].filename, "lithium.jar");
    }

    #[test]
    fn desired_applies_add() {
        let m = ModdedRuntime::new(cfg(
            vec![],
            vec![PendingOp::Add {
                mod_entry: entry("sodium"),
            }],
        ));
        let d = m.desired_mods();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].filename, "sodium.jar");
    }

    #[test]
    fn desired_applies_bump() {
        let m = ModdedRuntime::new(cfg(
            vec![entry("sodium")],
            vec![PendingOp::Bump {
                filename: "sodium.jar".to_owned(),
                to_version_id: "newv".to_owned(),
                to_version_name: "sodium-2.0".to_owned(),
                to_filename: "sodium-2.0.jar".to_owned(),
                to_download_url: "https://example/sodium-2.0.jar".to_owned(),
                to_sha512: None,
            }],
        ));
        let d = m.desired_mods();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].filename, "sodium-2.0.jar");
        assert_eq!(d[0].version_id, "newv");
    }

    #[test]
    fn extra_env_emits_type_for_runtime() {
        let m = ModdedRuntime::new(cfg(vec![], vec![]));
        let ctx = ProviderContext {
            server_id: "abc",
            memory_mi: 4096,
        };
        let env = m.extra_env(&ctx);
        let t = env.iter().find(|e| e.name == "TYPE").unwrap();
        assert_eq!(t.value.as_deref(), Some("FABRIC"));
        let v = env.iter().find(|e| e.name == "VERSION").unwrap();
        assert_eq!(v.value.as_deref(), Some("1.21.1"));
    }
}
```

- [ ] **Step 2: `pub use` in `mod.rs`**

```rust
pub mod modded;
pub use modded::{ModdedRuntime, Runtime as ModdedRuntimeKind, ModEntry, PendingOp};
```

- [ ] **Step 3: `cargo test --lib modpack::modded`**

Expected: PASS.

### Task 4.3: `PaperServerProvider`

**Files:**
- Create: `backend/src/modpack/paper.rs`
- Modify: `backend/src/modpack/mod.rs`

- [ ] **Step 1: Write the provider**

```rust
//! Paper provider — itzg with TYPE=PAPER.

use std::time::Duration;

use anyhow::{Result, anyhow};
use k8s_openapi::api::core::v1::EnvVar;
use serde::{Deserialize, Serialize};

use super::vanilla::{env_kv, env_secret};
use super::{ModpackHttp, ModpackProvider, ProviderContext, VersionInfo};

const PAPER_IMAGE: &str = "itzg/minecraft-server:java21";
const PAPER_BOOT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub mc_version: String,
    #[serde(default)]
    pub paper_build: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PaperServerProvider {
    config: Config,
}

impl PaperServerProvider {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }
}

#[async_trait::async_trait]
impl ModpackProvider for PaperServerProvider {
    fn kind(&self) -> &'static str {
        "paper"
    }

    fn pod_image(&self) -> &str {
        PAPER_IMAGE
    }

    fn launch_command(&self) -> Option<Vec<String>> {
        None
    }

    fn extra_env(&self, ctx: &ProviderContext<'_>) -> Vec<EnvVar> {
        let mut env = vec![
            env_kv("EULA", "TRUE"),
            env_kv("TYPE", "PAPER"),
            env_kv("VERSION", &self.config.mc_version),
            env_kv("MEMORY", &format!("{}M", ctx.memory_mi)),
            env_kv("ENABLE_RCON", "true"),
            env_secret(
                "RCON_PASSWORD",
                &format!("mc-{}-rcon", ctx.server_id),
                "password",
            ),
        ];
        if let Some(build) = self.config.paper_build.as_deref() {
            env.push(env_kv("PAPER_BUILD", build));
        }
        env
    }

    fn boot_timeout(&self) -> Duration {
        PAPER_BOOT_TIMEOUT
    }

    async fn latest(&self, _http: &ModpackHttp<'_>) -> Result<Option<VersionInfo>> {
        Ok(None)
    }

    async fn fetch_url(&self, _http: &ModpackHttp<'_>, _v: &VersionInfo) -> Result<String> {
        Err(anyhow!("paper has no pack-level upstream"))
    }
}
```

- [ ] **Step 2: `pub use` in `mod.rs`**

```rust
pub mod paper;
pub use paper::PaperServerProvider;
```

### Task 4.4: `from_db` arms

**Files:**
- Modify: `backend/src/modpack/mod.rs`

- [ ] **Step 1: Extend `from_db`**

```rust
pub fn from_db(source_kind: &str, source_config: &str) -> Result<Provider> {
    match source_kind {
        "vanilla" => Ok(Box::new(VanillaProvider::new())),
        "curseforge" => {
            let cfg: curseforge::Config = serde_json::from_str(source_config)
                .map_err(|e| anyhow::anyhow!("source_config not valid CurseForge JSON: {e}"))?;
            Ok(Box::new(CurseForgeServerPack::new(cfg)))
        }
        "modrinth" => {
            let cfg: modrinth::Config = serde_json::from_str(source_config)
                .map_err(|e| anyhow::anyhow!("source_config not valid Modrinth JSON: {e}"))?;
            Ok(Box::new(ModrinthServerPack::new(cfg)))
        }
        "modded" => {
            let cfg: modded::Config = serde_json::from_str(source_config)
                .map_err(|e| anyhow::anyhow!("source_config not valid modded JSON: {e}"))?;
            Ok(Box::new(ModdedRuntime::new(cfg)))
        }
        "paper" => {
            let cfg: paper::Config = serde_json::from_str(source_config)
                .map_err(|e| anyhow::anyhow!("source_config not valid paper JSON: {e}"))?;
            Ok(Box::new(PaperServerProvider::new(cfg)))
        }
        other => Err(anyhow::anyhow!("unknown source_kind {other:?}")),
    }
}
```

- [ ] **Step 2: Add `from_db` test for each new kind**

```rust
#[test]
fn from_db_returns_modrinth() {
    let cfg = r#"{
        "project_id": "AANobbMI",
        "channel": "release",
        "current_version_id": "",
        "current_version_name": ""
    }"#;
    let p = from_db("modrinth", cfg).expect("modrinth");
    assert_eq!(p.kind(), "modrinth");
}

#[test]
fn from_db_returns_modded() {
    let cfg = r#"{"runtime": "fabric", "mc_version": "1.21.1"}"#;
    let p = from_db("modded", cfg).expect("modded");
    assert_eq!(p.kind(), "modded");
}

#[test]
fn from_db_returns_paper() {
    let cfg = r#"{"mc_version": "1.21.1"}"#;
    let p = from_db("paper", cfg).expect("paper");
    assert_eq!(p.kind(), "paper");
}
```

- [ ] **Step 3: Wire orchestrator's `pick_target_version` Modrinth path**

Update the bail in Phase 1 Task 1.4 to handle modrinth:

```rust
match provider.kind() {
    "curseforge" => {
        let cf = http.cf.ok_or_else(|| anyhow!("CF client unavailable"))?;
        // ... CF path as before
    }
    "modrinth" => {
        let v = http.mr.version(target_version_id).await?;
        let primary = v.files.iter().find(|f| f.primary)
            .ok_or_else(|| anyhow!("Modrinth version {target_version_id} has no primary file"))?;
        return Ok(VersionInfo {
            id: v.id.clone(),
            name: v.name.clone(),
            download_url: primary.url.clone(),
        });
    }
    other => bail!("unsupported provider for target lookup: {other}"),
}
```

### Task 4.5: Phase 4 quality gate + commit

- [ ] **Step 1: `cargo test --all`**

Expected: green.

- [ ] **Step 2: clippy + fmt**

```bash
cd /home/hadi/gitlab/anvil/backend && \
  cargo clippy --all-targets --features serve-dir -- -D warnings && \
  cargo clippy --all-targets --features embed -- -D warnings && \
  cargo fmt --check
```

- [ ] **Step 3: Commit**

```bash
git add backend/src/modpack && \
git commit -m "$(cat <<'EOF'
feat(backend): Modrinth, Modded, Paper providers + from_db arms

ModrinthServerPack — itzg AUTO_MODRINTH; channel-aware version pick.
ModdedRuntime — itzg TYPE switch (FABRIC/FORGE/NEOFORGE), modlist +
pending-op draft model with desired_mods() folding adds/removes/bumps.
PaperServerProvider — itzg TYPE=PAPER. All four reuse vanilla's
env_kv/env_secret helpers (now pub(super)).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 5 — Validation additions

### Task 5.1: New validators

**Files:**
- Modify: `backend/src/validation.rs`

- [ ] **Step 1: Add five validators**

```rust
const KNOWN_RUNTIMES: &[&str] = &["fabric", "forge", "neoforge", "paper"];
const KNOWN_CATALOG_PROVIDERS: &[&str] = &["curseforge", "modrinth"];
const SEARCH_QUERY_MAX_LEN: usize = 100;
const MOD_FILENAME_MAX_LEN: usize = 200;

/// Validates a runtime discriminator.
///
/// # Errors
/// Returns `BadRequest{code:"runtime_invalid"}` when not in the allowed set.
pub fn validate_runtime(r: &str) -> Result<(), AppError> {
    if KNOWN_RUNTIMES.contains(&r) {
        Ok(())
    } else {
        Err(AppError::BadRequest {
            code: "runtime_invalid",
            message: format!("runtime {r:?} not in {KNOWN_RUNTIMES:?}"),
        })
    }
}

/// Validates a Modrinth project id (8-char base62) or slug.
///
/// # Errors
/// Returns `BadRequest{code:"modrinth_id_invalid"}` on shape mismatch.
pub fn validate_modrinth_id_or_slug(s: &str) -> Result<(), AppError> {
    let len = s.len();
    if (1..=40).contains(&len) {
        let is_id = s.len() == 8 && s.chars().all(|c| c.is_ascii_alphanumeric());
        let is_slug = s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
        if is_id || is_slug {
            return Ok(());
        }
    }
    Err(AppError::BadRequest {
        code: "modrinth_id_invalid",
        message: format!("modrinth id/slug {s:?} invalid"),
    })
}

/// Validates a search query.
///
/// # Errors
/// Returns `BadRequest{code:"search_query_invalid"}`.
pub fn validate_search_query(q: &str) -> Result<(), AppError> {
    let trimmed = q.trim();
    if trimmed.is_empty() || trimmed.len() > SEARCH_QUERY_MAX_LEN {
        return Err(AppError::BadRequest {
            code: "search_query_invalid",
            message: format!("query must be 1..={SEARCH_QUERY_MAX_LEN} chars"),
        });
    }
    Ok(())
}

/// Validates a catalog provider discriminator.
///
/// # Errors
/// Returns `BadRequest{code:"catalog_provider_invalid"}`.
pub fn validate_catalog_provider(p: &str) -> Result<(), AppError> {
    if KNOWN_CATALOG_PROVIDERS.contains(&p) {
        Ok(())
    } else {
        Err(AppError::BadRequest {
            code: "catalog_provider_invalid",
            message: format!("provider {p:?} not in {KNOWN_CATALOG_PROVIDERS:?}"),
        })
    }
}

/// Validates a mod filename. Defends the sync Job's `rm` from path injection.
///
/// # Errors
/// Returns `BadRequest{code:"mod_filename_invalid"}`.
pub fn validate_mod_filename(name: &str) -> Result<(), AppError> {
    let len = name.len();
    if !(1..=MOD_FILENAME_MAX_LEN).contains(&len)
        || name.contains('/')
        || !name.ends_with(".jar")
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
    {
        return Err(AppError::BadRequest {
            code: "mod_filename_invalid",
            message: format!("filename {name:?} must be a basename ending .jar with [A-Za-z0-9._+-]"),
        });
    }
    Ok(())
}
```

- [ ] **Step 2: Add unit tests**

```rust
#[test]
fn runtime_validator() {
    for r in KNOWN_RUNTIMES {
        assert!(validate_runtime(r).is_ok());
    }
    for r in ["", "vanilla", "FABRIC", "spongeforge"] {
        assert!(validate_runtime(r).is_err());
    }
}

#[test]
fn modrinth_id_or_slug_validator() {
    assert!(validate_modrinth_id_or_slug("AANobbMI").is_ok());
    assert!(validate_modrinth_id_or_slug("sodium").is_ok());
    assert!(validate_modrinth_id_or_slug("more-than-eight-but-slug").is_ok());
    assert!(validate_modrinth_id_or_slug("").is_err());
    assert!(validate_modrinth_id_or_slug("UPPER").is_err());
    assert!(validate_modrinth_id_or_slug("space slug").is_err());
}

#[test]
fn search_query_validator() {
    assert!(validate_search_query("sodium").is_ok());
    assert!(validate_search_query("").is_err());
    assert!(validate_search_query("    ").is_err());
    assert!(validate_search_query(&"a".repeat(101)).is_err());
}

#[test]
fn catalog_provider_validator() {
    assert!(validate_catalog_provider("curseforge").is_ok());
    assert!(validate_catalog_provider("modrinth").is_ok());
    assert!(validate_catalog_provider("vanilla").is_err());
}

#[test]
fn mod_filename_validator() {
    assert!(validate_mod_filename("sodium-fabric-0.5.13+mc1.21.1.jar").is_ok());
    assert!(validate_mod_filename("../etc/passwd").is_err());
    assert!(validate_mod_filename("sodium.zip").is_err());
    assert!(validate_mod_filename(".jar").is_ok()); // 4 chars, all valid
    assert!(validate_mod_filename(&format!("{}.jar", "a".repeat(200))).is_err());
}
```

- [ ] **Step 3: `cargo test --lib validation`**

Expected: PASS.

### Task 5.2: Phase 5 commit

- [ ] **Step 1: Commit**

```bash
cd /home/hadi/gitlab/anvil && git add backend/src/validation.rs && \
git commit -m "$(cat <<'EOF'
feat(validation): runtime / modrinth-id / search-query / mod-filename

Five new validators for B's surfaces. validate_mod_filename is the
load-bearing one: it gates DB writes so the sync Job's `rm -f
/data/mods/$base` cannot be tricked by a slash- or dot-injected
filename arriving through the catalog API.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 6 — Catalog API

Goal: `GET /api/catalog/search` (modpack/mod) and `GET /api/catalog/projects/{provider}/{id}/versions`. Plus `modrinth_enabled` on capabilities.

### Task 6.1: Catalog handler skeleton + types

**Files:**
- Create: `backend/src/routes/catalog.rs`
- Modify: `backend/src/routes/mod.rs` (mount)

- [ ] **Step 1: Write the handler module**

```rust
//! `/api/catalog/*` — unified CF + Modrinth catalog.

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::AppError;
use crate::modpack::cf_client::CfProject;
use crate::modpack::mr_client::{MrSearchHit, MrVersion, SearchQuery};
use crate::validation::{
    validate_catalog_provider, validate_modrinth_id_or_slug, validate_runtime,
    validate_search_query,
};

const CF_GAME_ID_MINECRAFT: u32 = 432;

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    /// "mod" | "modpack"
    #[serde(rename = "type")]
    pub kind: String,
    pub q: String,
    #[serde(default)]
    pub loader: Option<String>,
    #[serde(default)]
    pub mc: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct CatalogHit {
    pub provider: &'static str,
    pub project_id: String,
    pub slug: String,
    pub name: String,
    pub summary: String,
    pub icon_url: Option<String>,
    pub downloads: u64,
    pub follows: u64,
    pub project_type: String,
    pub loaders: Vec<String>,
    pub game_versions: Vec<String>,
    pub author: Option<String>,
    pub updated: String,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub results: Vec<CatalogHit>,
}

/// `GET /api/catalog/search`
///
/// # Errors
/// - 400 invalid params
pub async fn search(
    State(state): State<AppState>,
    Query(p): Query<SearchParams>,
) -> Result<Json<SearchResponse>, AppError> {
    if p.kind != "mod" && p.kind != "modpack" {
        return Err(AppError::BadRequest {
            code: "catalog_type_invalid",
            message: format!("type must be mod or modpack, got {:?}", p.kind),
        });
    }
    validate_search_query(&p.q)?;
    if let Some(l) = p.loader.as_deref() {
        validate_runtime(l)?;
    }

    let limit = p.limit.unwrap_or(20).clamp(1, 50);
    let offset = p.offset.unwrap_or(0);

    let mut results: Vec<CatalogHit> = Vec::new();

    // Modrinth — both modpack and mod queries hit it.
    let mr_q = SearchQuery {
        query: &p.q,
        project_type: if p.kind == "mod" { "mod" } else { "modpack" },
        loader: p.loader.as_deref(),
        game_version: p.mc.as_deref(),
        limit,
        offset,
    };
    match state.mr_client.search(&mr_q).await {
        Ok(hits) => results.extend(hits.into_iter().map(modrinth_hit_to_catalog)),
        Err(e) => tracing::warn!(error = %e, "modrinth search failed"),
    }

    // CurseForge — only for modpacks (and only when configured).
    if p.kind == "modpack"
        && let Some(cf) = state.cf_client.as_ref()
    {
        match cf_search(cf, &p.q, limit).await {
            Ok(projects) => results.extend(projects.into_iter().map(cf_project_to_catalog)),
            Err(e) => tracing::warn!(error = %e, "curseforge search failed"),
        }
    }

    // Sort by downloads desc as a single combined heuristic.
    results.sort_by(|a, b| b.downloads.cmp(&a.downloads));

    Ok(Json(SearchResponse { results }))
}

fn modrinth_hit_to_catalog(h: MrSearchHit) -> CatalogHit {
    CatalogHit {
        provider: "modrinth",
        project_id: h.project_id,
        slug: h.slug,
        name: h.title,
        summary: h.description,
        icon_url: h.icon_url,
        downloads: h.downloads,
        follows: h.follows,
        project_type: h.project_type,
        loaders: h
            .display_categories
            .into_iter()
            .filter(|c| matches!(c.as_str(), "fabric" | "forge" | "neoforge" | "paper" | "quilt"))
            .collect(),
        game_versions: h.versions,
        author: Some(h.author),
        updated: h.date_modified,
    }
}

fn cf_project_to_catalog(p: CfProject) -> CatalogHit {
    CatalogHit {
        provider: "curseforge",
        project_id: p.id.to_string(),
        slug: p.slug,
        name: p.name,
        summary: String::new(),
        icon_url: None,
        downloads: 0,
        follows: 0,
        project_type: "modpack".to_owned(),
        loaders: vec![],
        game_versions: vec![],
        author: None,
        updated: String::new(),
    }
}

async fn cf_search(
    cf: &crate::modpack::CurseForgeClient,
    q: &str,
    _limit: u32,
) -> anyhow::Result<Vec<CfProject>> {
    // Reuse resolve_slug shape with searchFilter; cf_client.rs does not yet
    // expose a dedicated search method — extend it if/when needed. For B
    // we lean on Modrinth-rich results and fall back to slug-paste for CF.
    // Concretely: try resolving the query as a slug; if it matches, return
    // it as a single hit.
    match cf.resolve_slug(q).await {
        Ok(p) => Ok(vec![p]),
        Err(_) => Ok(vec![]),
    }
}
```

(CurseForge search returns thin results for B — slug-paste remains the primary CF discovery path, and Modrinth carries the discoverable surface. Full CF search support extends `cf_client.rs` and is left for B.1.)

- [ ] **Step 2: Versions endpoint**

```rust
#[derive(Debug, Deserialize)]
pub struct VersionsParams {
    #[serde(default)]
    pub loader: Option<String>,
    #[serde(default)]
    pub mc: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CatalogVersion {
    pub version_id: String,
    pub version_name: String,
    pub channel: String,
    pub loaders: Vec<String>,
    pub game_versions: Vec<String>,
    pub date_published: String,
    pub primary_filename: String,
    pub primary_url: String,
    pub primary_sha512: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct VersionsResponse {
    pub versions: Vec<CatalogVersion>,
}

/// `GET /api/catalog/projects/{provider}/{id}/versions`
///
/// # Errors
/// - 400 invalid provider/id
/// - 502 upstream error
pub async fn versions(
    State(state): State<AppState>,
    Path((provider, id)): Path<(String, String)>,
    Query(p): Query<VersionsParams>,
) -> Result<Json<VersionsResponse>, AppError> {
    validate_catalog_provider(&provider)?;
    if let Some(l) = p.loader.as_deref() {
        validate_runtime(l)?;
    }

    let versions = match provider.as_str() {
        "modrinth" => {
            validate_modrinth_id_or_slug(&id)?;
            let raw = state
                .mr_client
                .list_versions(&id)
                .await
                .map_err(|e| AppError::BadRequest {
                    code: "modrinth_unavailable",
                    message: format!("modrinth list_versions: {e}"),
                })?;
            raw.iter()
                .filter(|v| {
                    p.loader
                        .as_deref()
                        .is_none_or(|l| v.loaders.iter().any(|x| x == l))
                })
                .filter(|v| {
                    p.mc.as_deref()
                        .is_none_or(|mc| v.game_versions.iter().any(|x| x == mc))
                })
                .filter_map(|v| {
                    let primary = v.files.iter().find(|f| f.primary)?;
                    Some(mr_version_to_catalog(v, primary))
                })
                .collect()
        }
        "curseforge" => {
            let project_id: u32 = id.parse().map_err(|_| AppError::BadRequest {
                code: "cf_id_invalid",
                message: format!("CurseForge id must be numeric, got {id:?}"),
            })?;
            let cf = state.cf_client.as_ref().ok_or(AppError::BadRequest {
                code: "cf_disabled",
                message: "CurseForge support is not enabled".to_owned(),
            })?;
            let files = cf
                .list_files(project_id)
                .await
                .map_err(|e| AppError::BadRequest {
                    code: "cf_unavailable",
                    message: format!("curseforge list_files: {e}"),
                })?;
            files.iter().map(cf_file_to_catalog).collect()
        }
        _ => unreachable!("validated above"),
    };

    Ok(Json(VersionsResponse { versions }))
}

fn mr_version_to_catalog(v: &MrVersion, primary: &crate::modpack::mr_client::MrFile) -> CatalogVersion {
    CatalogVersion {
        version_id: v.id.clone(),
        version_name: v.name.clone(),
        channel: v.version_type.clone(),
        loaders: v.loaders.clone(),
        game_versions: v.game_versions.clone(),
        date_published: v.date_published.clone(),
        primary_filename: primary.filename.clone(),
        primary_url: primary.url.clone(),
        primary_sha512: primary.hashes.sha512.clone(),
    }
}

fn cf_file_to_catalog(f: &crate::modpack::cf_client::CfFile) -> CatalogVersion {
    let channel = match f.release_type {
        1 => "release",
        2 => "beta",
        _ => "alpha",
    };
    CatalogVersion {
        version_id: f.id.to_string(),
        version_name: f.display_name.clone(),
        channel: channel.to_owned(),
        loaders: vec![],
        game_versions: vec![],
        date_published: f.file_date.clone(),
        primary_filename: f.display_name.clone(),
        primary_url: f.download_url.clone().unwrap_or_default(),
        primary_sha512: None,
    }
}
```

- [ ] **Step 3: Mount in `routes/mod.rs`**

In the `protected` Router chain:

```rust
.route("/api/catalog/search", get(catalog::search))
.route(
    "/api/catalog/projects/{provider}/{id}/versions",
    get(catalog::versions),
)
```

Add `pub mod catalog;` next to the other module declarations.

- [ ] **Step 4: Build check**

```bash
cd /home/hadi/gitlab/anvil/backend && cargo check --lib
```

Expected: green.

### Task 6.2: `modrinth_enabled` on capabilities

**Files:**
- Modify: `backend/src/routes/cluster.rs`

- [ ] **Step 1: Add field**

```rust
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize)]
pub struct ClusterCapabilities {
    pub loadbalancer: bool,
    pub nodeport: bool,
    pub clusterip: bool,
    pub available_storage_classes: Vec<String>,
    pub default_storage_class: Option<String>,
    pub cf_api_key_present: bool,
    /// Modrinth is API-key-free; always available when the panel can reach modrinth.com.
    pub modrinth_enabled: bool,
    pub available_cpu_cores: f64,
}
```

- [ ] **Step 2: Set in handler**

```rust
let caps = ClusterCapabilities {
    // ... existing fields
    modrinth_enabled: true,
    available_cpu_cores,
};
```

### Task 6.3: Phase 6 commit

- [ ] **Step 1: `cargo test --all` + clippy + fmt**

Expected: green.

- [ ] **Step 2: Commit**

```bash
cd /home/hadi/gitlab/anvil && git add backend/src && \
git commit -m "$(cat <<'EOF'
feat(api): /api/catalog/search + /api/catalog/projects/{provider}/{id}/versions

Unified search across Modrinth (always-on) + CurseForge (when
configured) for modpacks; Modrinth-only for individual mods. Strict
loader/mc facets on mod search prevent incompatible mods from
reaching the install button.

Versions endpoint surfaces per-version loaders / mc / channel /
primary file URL+sha512 so the install picker has everything it
needs without a second round-trip.

Adds modrinth_enabled:true to GET /api/cluster/capabilities.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 7 — Mods CRUD + apply FSM

Goal: pending-op CRUD + apply orchestrator + WS stream. Mirrors M5's update FSM with a slimmer phase set and no rollback.

### Task 7.1: `build_mod_sync_job` in `jobs.rs`

**Files:**
- Modify: `backend/src/modpack/jobs.rs`

- [ ] **Step 1: Add the builder + script**

After `build_swap_job`:

```rust
/// Builds the mod-sync Job — wipes any /data/mods/*.jar not in KEEP_FILENAMES,
/// then downloads any DESIRED_URLS jar that isn't already present. Verifies
/// sha512 when supplied.
#[must_use]
pub fn build_mod_sync_job(
    server_id: &str,
    ts: i64,
    namespace: &str,
    keep_filenames: &[&str],
    desired_urls: &[(&str, &str, Option<&str>)],
) -> Job {
    let resource_name = format!("mc-{server_id}");
    let pvc_name = format!("data-{resource_name}-0");
    let job_name = format!("mod-sync-{resource_name}-{ts}");

    let keep = keep_filenames.join("\n");
    let desired = desired_urls
        .iter()
        .map(|(filename, url, sha)| format!("{filename}\t{url}\t{}", sha.unwrap_or("")))
        .collect::<Vec<_>>()
        .join("\n");

    let env = vec![
        env_kv("KEEP_FILENAMES", &keep),
        env_kv("DESIRED_URLS", &desired),
    ];

    let container = Container {
        name: "sync".to_owned(),
        image: Some(ALPINE_IMAGE.to_owned()),
        command: Some(vec![
            "sh".to_owned(),
            "-c".to_owned(),
            MOD_SYNC_SCRIPT.to_owned(),
        ]),
        env: Some(env),
        volume_mounts: Some(vec![VolumeMount {
            name: "data".to_owned(),
            mount_path: "/data".to_owned(),
            ..VolumeMount::default()
        }]),
        ..Container::default()
    };

    job(
        &job_name,
        namespace,
        labels(server_id, "mod-sync"),
        container,
        vec![data_volume(&pvc_name)],
    )
}

const MOD_SYNC_SCRIPT: &str = r#"
set -eu
apk add --no-cache curl >/dev/null

mkdir -p /data/mods

# 1. Build the keep-set in a temp file.
echo "$KEEP_FILENAMES" > /tmp/keep.txt

# 2. Remove any jar in /data/mods/ that isn't in the keep set.
for jar in /data/mods/*.jar; do
  [ -e "$jar" ] || continue
  base=$(basename "$jar")
  if ! grep -qxF "$base" /tmp/keep.txt; then
    echo "removing $base"
    rm -f "$jar"
  fi
done

# 3. Download every DESIRED_URLS line whose filename isn't yet present.
echo "$DESIRED_URLS" | while IFS="$(printf '\t')" read -r filename url sha; do
  [ -z "$filename" ] && continue
  target="/data/mods/$filename"
  if [ -e "$target" ]; then
    continue
  fi
  echo "fetching $filename"
  curl -fL "$url" -o "$target.tmp"
  if [ -n "$sha" ]; then
    echo "$sha  $target.tmp" | sha512sum -c -
  fi
  mv "$target.tmp" "$target"
done

echo "mod-sync complete"
"#;
```

- [ ] **Step 2: Add a unit test**

```rust
#[test]
fn mod_sync_job_carries_keep_and_desired_in_env() {
    let j = build_mod_sync_job(
        "abc",
        1,
        "mc",
        &["sodium.jar", "lithium.jar"],
        &[("iris.jar", "https://example/iris.jar", Some("ffff"))],
    );
    let env = j.spec.unwrap().template.spec.unwrap().containers[0]
        .env
        .clone()
        .unwrap();
    let keep = env.iter().find(|e| e.name == "KEEP_FILENAMES").unwrap();
    let desired = env.iter().find(|e| e.name == "DESIRED_URLS").unwrap();
    assert!(keep.value.as_deref().unwrap().contains("sodium.jar"));
    assert!(desired.value.as_deref().unwrap().contains("iris.jar\thttps://example/iris.jar\tffff"));
}

#[test]
fn mod_sync_job_uses_data_pvc_name() {
    let j = build_mod_sync_job("abc", 1, "mc", &[], &[]);
    let v = j.spec.unwrap().template.spec.unwrap().volumes.unwrap();
    let data = v.iter().find(|x| x.name == "data").unwrap();
    let pvc = data.persistent_volume_claim.as_ref().unwrap();
    assert_eq!(pvc.claim_name, "data-mc-abc-0");
}
```

- [ ] **Step 3: `cargo test --lib modpack::jobs::tests::mod_sync`**

Expected: PASS.

### Task 7.2: Apply FSM (`mods_apply.rs`)

**Files:**
- Create: `backend/src/modpack/mods_apply.rs`
- Modify: `backend/src/modpack/mod.rs` (add `pub mod mods_apply;`)

- [ ] **Step 1: Write the FSM**

```rust
//! Mod-sync FSM for `modded` servers.
//!
//! Runs from a click on the Mods tab `[apply now]` button. Re-uses
//! `UpdateGuard` + `snapshot_pvc_lock` + the WS-bus pattern. No backup;
//! mods/ is recoverable by clicking apply again.

use anyhow::{Context as _, Result, anyhow, bail};
use chrono::Utc;
use kube::Api;
use kube::api::{DeleteParams, PostParams};
use serde::Serialize;
use serde_json::json;
use tokio::time::sleep;

use crate::AppState;
use crate::modpack::guard::UpdateGuard;
use crate::modpack::jobs::build_mod_sync_job;
use crate::modpack::modded::{Config as ModdedConfig, ModdedRuntime};
use crate::modpack::orchestrator::UpdatePhase;
use crate::routes::servers::create::insert_audit;
use std::time::Duration;

const POD_TERMINATE_TIMEOUT: Duration = Duration::from_secs(90);
const SYNC_JOB_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const POD_RUNNING_TIMEOUT: Duration = Duration::from_secs(120);

/// Phases for the mod-sync run. Reuses `UpdatePhase` for WS-shape parity
/// (`Syncing` is encoded as `Swapping` so the existing UpdateSheet phase
/// list keeps working without code duplication; rename in B.1 if needed).
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModsApplyPhase {
    Queued,
    Stopping,
    Syncing,
    Starting,
    Verifying,
    Succeeded,
    Failed,
}

/// Kicks off the mod-sync FSM. Long-running task; spawned by the route
/// handler; drops the guard on completion.
pub async fn run(state: AppState, server_id: String, guard: UpdateGuard) {
    let outcome = run_inner(&state, &server_id, &guard).await;
    match outcome {
        Ok(()) => {
            guard.emit(UpdatePhase::Succeeded);
            tracing::info!(server.id = %server_id, "mod-sync succeeded");
        }
        Err(err) => {
            guard.emit(UpdatePhase::Failed);
            tracing::error!(server.id = %server_id, err = %err, "mod-sync failed");
            let now = Utc::now().timestamp();
            let _ = insert_audit(
                &state.pool,
                &server_id,
                "mods_apply_failed",
                Some(json!({"err": err.to_string()})),
                now,
            )
            .await;
        }
    }
}

async fn run_inner(state: &AppState, server_id: &str, guard: &UpdateGuard) -> Result<()> {
    let now = Utc::now().timestamp();
    insert_audit(&state.pool, server_id, "mods_apply_started", None, now).await?;

    // Load config.
    let row: (String, String) =
        sqlx::query_as("SELECT source_kind, source_config FROM servers WHERE id = ?")
            .bind(server_id)
            .fetch_one(&state.pool)
            .await
            .with_context(|| format!("loading source for {server_id}"))?;
    if row.0 != "modded" {
        bail!("mods_apply only valid for modded servers (got {})", row.0);
    }
    let cfg: ModdedConfig =
        serde_json::from_str(&row.1).context("source_config not modded JSON")?;
    let runtime = ModdedRuntime::new(cfg.clone());
    let desired = runtime.desired_mods();

    if cfg.pending.is_empty() {
        bail!("no pending changes to apply");
    }

    // Acquire the global Job lock.
    let permit = state.snapshot_pvc_lock.lock().await;

    // Stop.
    guard.emit(UpdatePhase::Stopping);
    crate::modpack::orchestrator_helpers::scale_to(&state.kube, &state.mc_namespace, server_id, 0)
        .await?;
    crate::modpack::orchestrator_helpers::wait_pod_gone(
        &state.kube,
        &state.mc_namespace,
        server_id,
        POD_TERMINATE_TIMEOUT,
    )
    .await?;

    // Sync mods.
    guard.emit(UpdatePhase::Swapping); // reuse Swapping as the sync phase
    let keep: Vec<&str> = desired.iter().map(|m| m.filename.as_str()).collect();
    let urls: Vec<(&str, &str, Option<&str>)> = desired
        .iter()
        .map(|m| {
            (
                m.filename.as_str(),
                m.download_url.as_str(),
                m.sha512.as_deref(),
            )
        })
        .collect();
    let ts = Utc::now().timestamp();
    let sync_job = build_mod_sync_job(server_id, ts, &state.mc_namespace, &keep, &urls);
    let job_name = sync_job
        .metadata
        .name
        .clone()
        .ok_or_else(|| anyhow!("sync Job missing name"))?;
    let jobs: Api<k8s_openapi::api::batch::v1::Job> =
        Api::namespaced(state.kube.clone(), &state.mc_namespace);
    if jobs.get_opt(&job_name).await?.is_some() {
        let _ = jobs.delete(&job_name, &DeleteParams::default()).await;
        sleep(Duration::from_secs(1)).await;
    }
    jobs.create(&PostParams::default(), &sync_job)
        .await
        .with_context(|| format!("creating Job {job_name}"))?;
    crate::modpack::orchestrator_helpers::wait_job(
        &state.kube,
        &state.mc_namespace,
        &job_name,
        SYNC_JOB_TIMEOUT,
    )
    .await?;

    drop(permit);

    // Start + verify.
    guard.emit(UpdatePhase::Starting);
    crate::modpack::orchestrator_helpers::scale_to(&state.kube, &state.mc_namespace, server_id, 1)
        .await?;

    guard.emit(UpdatePhase::Verifying);
    crate::modpack::orchestrator_helpers::wait_pod_running(
        &state.kube,
        &state.mc_namespace,
        server_id,
        POD_RUNNING_TIMEOUT,
    )
    .await?;
    crate::modpack::orchestrator_helpers::wait_for_done_marker(
        &state.kube,
        &state.mc_namespace,
        server_id,
        Duration::from_secs(10 * 60),
    )
    .await?;

    // Persist: replace mods, clear pending.
    let mut new_cfg = cfg;
    new_cfg.mods = desired;
    new_cfg.pending = Vec::new();
    let new_raw = serde_json::to_string(&new_cfg)?;
    sqlx::query("UPDATE servers SET source_config = ? WHERE id = ?")
        .bind(&new_raw)
        .bind(server_id)
        .execute(&state.pool)
        .await?;

    let now = Utc::now().timestamp();
    insert_audit(
        &state.pool,
        server_id,
        "mods_apply_succeeded",
        Some(json!({"mods": new_cfg.mods.len()})),
        now,
    )
    .await?;
    Ok(())
}
```

- [ ] **Step 2: Extract orchestrator helpers**

The mods_apply FSM reuses `scale_to`, `wait_pod_gone`, `wait_pod_running`, `wait_job`, `wait_for_done_marker` from `orchestrator.rs`. Promote those from `async fn` private to a `pub(crate) mod orchestrator_helpers` co-located in the same file or split out:

In `backend/src/modpack/orchestrator.rs`, change `async fn scale_to`, `wait_pod_gone`, `wait_pod_running`, `spawn_job`, `wait_job`, `wait_for_done_marker` to `pub(crate) async fn`. Then create:

```rust
// At the top of orchestrator.rs:
pub mod orchestrator_helpers {
    pub use super::{scale_to, wait_pod_gone, wait_pod_running, wait_job, wait_for_done_marker};
}
```

Or simpler: `pub(crate) use` them from the parent module. Pick whichever the codebase style prefers — for now, mark each helper `pub(crate)` and add `pub(crate) use` aliases:

```rust
pub(crate) mod orchestrator_helpers {
    pub use super::{scale_to, wait_for_done_marker, wait_job, wait_pod_gone, wait_pod_running};
}
```

- [ ] **Step 3: `cargo check --lib`**

```bash
cd /home/hadi/gitlab/anvil/backend && cargo check --lib
```

Expected: green.

### Task 7.3: Mods CRUD + apply route + WS

**Files:**
- Create: `backend/src/routes/servers/mods.rs`
- Modify: `backend/src/routes/servers/mod.rs` (add `pub mod mods;`)
- Modify: `backend/src/routes/mod.rs` (mount routes)

- [ ] **Step 1: Write the route module**

```rust
//! `/api/servers/{id}/mods*` — modlist editing + apply.

use axum::Json;
use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use bytes::Bytes;
use chrono::Utc;
use futures_util::sink::SinkExt as _;
use futures_util::stream::{SplitSink, SplitStream, StreamExt as _};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::{oneshot, watch};
use tokio::time::{MissedTickBehavior, interval};

use crate::AppState;
use crate::error::AppError;
use crate::modpack::guard::UpdateGuard;
use crate::modpack::modded::{Config as ModdedConfig, ModEntry, PendingOp};
use crate::modpack::mods_apply;
use crate::modpack::orchestrator::UpdatePhase;
use crate::validation::validate_mod_filename;

const HEARTBEAT: Duration = Duration::from_secs(30);

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PendingOpRequest {
    Add { mod_entry: ModEntry },
    Remove { filename: String },
    Bump {
        filename: String,
        to_version_id: String,
        to_version_name: String,
        to_filename: String,
        to_download_url: String,
        #[serde(default)]
        to_sha512: Option<String>,
    },
}

impl From<PendingOpRequest> for PendingOp {
    fn from(r: PendingOpRequest) -> Self {
        match r {
            PendingOpRequest::Add { mod_entry } => PendingOp::Add { mod_entry },
            PendingOpRequest::Remove { filename } => PendingOp::Remove { filename },
            PendingOpRequest::Bump {
                filename,
                to_version_id,
                to_version_name,
                to_filename,
                to_download_url,
                to_sha512,
            } => PendingOp::Bump {
                filename,
                to_version_id,
                to_version_name,
                to_filename,
                to_download_url,
                to_sha512,
            },
        }
    }
}

/// `POST /api/servers/{id}/mods` — append a pending op.
///
/// # Errors
/// - 404 if server missing
/// - 400 not_modded
/// - 400 mod_filename_invalid
pub async fn add_pending(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(req): Json<PendingOpRequest>,
) -> Result<StatusCode, AppError> {
    // Validate filename(s).
    match &req {
        PendingOpRequest::Add { mod_entry } => validate_mod_filename(&mod_entry.filename)?,
        PendingOpRequest::Remove { filename } => validate_mod_filename(filename)?,
        PendingOpRequest::Bump {
            filename,
            to_filename,
            ..
        } => {
            validate_mod_filename(filename)?;
            validate_mod_filename(to_filename)?;
        }
    }

    let mut cfg = load_modded_cfg(&state, &id).await?;
    cfg.pending.push(req.into());
    save_modded_cfg(&state, &id, &cfg).await?;
    let now = Utc::now().timestamp();
    let _ = crate::routes::servers::create::insert_audit(
        &state.pool,
        &id,
        "mods_pending_add",
        Some(json!({"pending_count": cfg.pending.len()})),
        now,
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/servers/{id}/mods/pending/{idx}` — drop one pending op.
///
/// # Errors
/// - 404 server missing or idx out of range
pub async fn remove_pending(
    Path((id, idx)): Path<(String, usize)>,
    State(state): State<AppState>,
) -> Result<StatusCode, AppError> {
    let mut cfg = load_modded_cfg(&state, &id).await?;
    if idx >= cfg.pending.len() {
        return Err(AppError::NotFound);
    }
    cfg.pending.remove(idx);
    save_modded_cfg(&state, &id, &cfg).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
pub struct ApplyResponse {
    pub status: &'static str,
    pub server_id: String,
    pub pending_count: usize,
}

/// `POST /api/servers/{id}/mods/apply` — kick the FSM.
///
/// # Errors
/// - 404 server missing
/// - 400 not_modded
/// - 409 nothing_pending or apply_in_progress
pub async fn apply(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<ApplyResponse>), AppError> {
    let cfg = load_modded_cfg(&state, &id).await?;
    if cfg.pending.is_empty() {
        return Err(AppError::Conflict {
            code: "nothing_pending",
            message: "no pending mod changes to apply".to_owned(),
        });
    }
    let pending_count = cfg.pending.len();

    let Some(guard) = UpdateGuard::try_acquire(
        &id,
        state.update_locks.clone(),
        state.update_phase_buses.clone(),
    ) else {
        return Err(AppError::Conflict {
            code: "apply_in_progress",
            message: "an update or apply is already running for this server".to_owned(),
        });
    };

    let task_state = state.clone();
    let task_id = id.clone();
    tokio::spawn(async move {
        mods_apply::run(task_state, task_id, guard).await;
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(ApplyResponse {
            status: "started",
            server_id: id,
            pending_count,
        }),
    ))
}

/// `GET /api/servers/{id}/mods/apply/stream` — WS phase stream.
/// Mirrors update_stream's frame shape for frontend reuse.
///
/// # Errors
/// - 404 server missing
pub async fn apply_stream(
    Path(id): Path<String>,
    State(state): State<AppState>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, AppError> {
    let _row = crate::routes::servers::get::fetch_server_row(&state.pool, &id).await?;
    Ok(upgrade.on_upgrade(move |socket| run_ws(socket, state, id)))
}

async fn load_modded_cfg(state: &AppState, id: &str) -> Result<ModdedConfig, AppError> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT source_kind, source_config FROM servers WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    let (kind, raw) = row.ok_or(AppError::NotFound)?;
    if kind != "modded" {
        return Err(AppError::BadRequest {
            code: "not_modded",
            message: "modlist endpoints only apply to modded servers".to_owned(),
        });
    }
    serde_json::from_str(&raw).map_err(|e| {
        AppError::Internal(anyhow::anyhow!("source_config not modded JSON: {e}"))
    })
}

async fn save_modded_cfg(
    state: &AppState,
    id: &str,
    cfg: &ModdedConfig,
) -> Result<(), AppError> {
    let raw = serde_json::to_string(cfg).map_err(|e| AppError::Internal(e.into()))?;
    sqlx::query("UPDATE servers SET source_config = ? WHERE id = ?")
        .bind(&raw)
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum Frame {
    Hello { phase: UpdatePhase },
    Progress { phase: UpdatePhase },
    Done { result: DoneResult },
    End { reason: &'static str },
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
enum DoneResult {
    Succeeded,
    Failed,
}

impl Frame {
    fn into_message(self) -> Message {
        let payload = serde_json::to_string(&self).expect("Frame serialization is infallible");
        Message::Text(Utf8Bytes::from(payload))
    }
}

fn terminal(p: UpdatePhase) -> Option<DoneResult> {
    match p {
        UpdatePhase::Succeeded => Some(DoneResult::Succeeded),
        UpdatePhase::Failed | UpdatePhase::RolledBack => Some(DoneResult::Failed),
        _ => None,
    }
}

async fn run_ws(socket: WebSocket, state: AppState, id: String) {
    let (sender, receiver) = socket.split();
    let (close_tx, close_rx) = oneshot::channel::<()>();
    let read_task = tokio::spawn(watch_close(receiver, close_tx));
    write_loop(sender, state, id, close_rx).await;
    read_task.abort();
}

async fn watch_close(mut rx: SplitStream<WebSocket>, close_tx: oneshot::Sender<()>) {
    while let Some(msg) = rx.next().await {
        match msg {
            Ok(Message::Close(_)) | Err(_) => break,
            _ => {}
        }
    }
    let _ = close_tx.send(());
}

async fn write_loop(
    mut sender: SplitSink<WebSocket, Message>,
    state: AppState,
    id: String,
    mut close_rx: oneshot::Receiver<()>,
) {
    let rx_opt: Option<watch::Receiver<UpdatePhase>> = state
        .update_phase_buses
        .lock()
        .expect("update_phase_buses poisoned")
        .get(&id)
        .cloned();

    let Some(mut rx) = rx_opt else {
        let _ = sender.send(Frame::Hello { phase: UpdatePhase::Queued }.into_message()).await;
        let _ = sender.send(Frame::End { reason: "no-apply-in-progress" }.into_message()).await;
        let _ = sender.send(Message::Close(Some(CloseFrame { code: 1000, reason: Utf8Bytes::from("") }))).await;
        return;
    };

    let current = *rx.borrow_and_update();
    if sender.send(Frame::Hello { phase: current }.into_message()).await.is_err() {
        return;
    }
    if let Some(result) = terminal(current) {
        let _ = sender.send(Frame::Done { result }.into_message()).await;
        return;
    }

    let mut hb = interval(HEARTBEAT);
    hb.set_missed_tick_behavior(MissedTickBehavior::Skip);
    hb.tick().await;

    loop {
        tokio::select! {
            biased;
            _ = &mut close_rx => return,
            _ = hb.tick() => {
                if sender.send(Message::Ping(Bytes::new())).await.is_err() {
                    return;
                }
            }
            changed = rx.changed() => {
                if changed.is_err() {
                    let _ = sender.send(Frame::Done { result: DoneResult::Failed }.into_message()).await;
                    return;
                }
                let phase = *rx.borrow_and_update();
                if sender.send(Frame::Progress { phase }.into_message()).await.is_err() {
                    return;
                }
                if let Some(result) = terminal(phase) {
                    let _ = sender.send(Frame::Done { result }.into_message()).await;
                    return;
                }
            }
        }
    }
}
```

- [ ] **Step 2: Wire into `routes/servers/mod.rs` and `routes/mod.rs`**

In `backend/src/routes/servers/mod.rs`, add `pub mod mods;` next to the others.

In `backend/src/routes/mod.rs`, in `protected`:

```rust
.route("/api/servers/{id}/mods", post(servers::mods::add_pending))
.route(
    "/api/servers/{id}/mods/pending/{idx}",
    axum::routing::delete(servers::mods::remove_pending),
)
.route("/api/servers/{id}/mods/apply", post(servers::mods::apply))
.route(
    "/api/servers/{id}/mods/apply/stream",
    get(servers::mods::apply_stream),
)
```

- [ ] **Step 3: `cargo build` + tests**

```bash
cd /home/hadi/gitlab/anvil/backend && cargo build && cargo test --all
```

Expected: green.

### Task 7.4: Phase 7 commit

- [ ] **Step 1: clippy + fmt**

```bash
cd /home/hadi/gitlab/anvil/backend && \
  cargo clippy --all-targets --features serve-dir -- -D warnings && \
  cargo clippy --all-targets --features embed -- -D warnings && \
  cargo fmt --check
```

- [ ] **Step 2: Commit**

```bash
cd /home/hadi/gitlab/anvil && git add backend/src && \
git commit -m "$(cat <<'EOF'
feat(api): mods pending CRUD + apply FSM + WS stream

build_mod_sync_job: alpine + curl that wipes any /data/mods/*.jar
not in KEEP_FILENAMES then downloads any DESIRED_URLS line whose
filename isn't present, verifying sha512 when supplied. Defends path
traversal at the validation layer (validate_mod_filename).

mods_apply::run is the FSM: stop → sync Job → start → verify, reusing
UpdateGuard / snapshot_pvc_lock / WS bus. Phase wire shape mirrors
update_stream so the frontend reuses one phase viewer.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 8 — Server-create extensions

Goal: `POST /api/servers` accepts `paper`, `modded`, `modrinth` in addition to `vanilla`/`curseforge`.

### Task 8.1: Extend create handler

**Files:**
- Modify: `backend/src/routes/servers/create.rs`

- [ ] **Step 1: Extend `CreateRequest`**

```rust
#[derive(Debug, Deserialize)]
pub struct CreateRequest {
    // ...existing fields
    pub server_type: Option<String>,
    /// Sub-config for `server_type=curseforge`.
    #[serde(default)]
    pub curseforge: Option<CurseForgeCreateConfig>,
    /// Sub-config for `server_type=modrinth`.
    #[serde(default)]
    pub modrinth: Option<ModrinthCreateConfig>,
    /// Sub-config for `server_type=modded`.
    #[serde(default)]
    pub modded: Option<ModdedCreateConfig>,
}

#[derive(Debug, Deserialize)]
pub struct ModrinthCreateConfig {
    pub project_id: String,
    pub channel: Channel,
}

#[derive(Debug, Deserialize)]
pub struct ModdedCreateConfig {
    pub runtime: String, // "fabric" | "forge" | "neoforge"
    #[serde(default)]
    pub initial_mods: Vec<crate::modpack::modded::ModEntry>,
}
```

- [ ] **Step 2: Add the new arms in the dispatch**

```rust
const SERVER_TYPE_MODRINTH: &str = "modrinth";
const SERVER_TYPE_MODDED: &str = "modded";
const SERVER_TYPE_PAPER: &str = "paper";

// In the validation `match`:
let server_type = server_type.unwrap_or_else(|| SERVER_TYPE_VANILLA.to_owned());
match server_type.as_str() {
    SERVER_TYPE_VANILLA
    | SERVER_TYPE_CURSEFORGE
    | SERVER_TYPE_MODRINTH
    | SERVER_TYPE_MODDED
    | SERVER_TYPE_PAPER => {}
    other => {
        return Err(AppError::BadRequest {
            code: "server_type_invalid",
            message: format!("server_type {other:?} not supported"),
        });
    }
}
```

In the `resolved = match server_type.as_str()` block, add:

```rust
SERVER_TYPE_MODRINTH => resolve_modrinth(&state, modrinth).await?,
SERVER_TYPE_MODDED => resolve_modded(&state, mc_version, modded).await?,
SERVER_TYPE_PAPER => resolve_paper(&state, mc_version).await?,
```

- [ ] **Step 3: Implement `resolve_modrinth`**

```rust
async fn resolve_modrinth(
    state: &AppState,
    cfg: Option<ModrinthCreateConfig>,
) -> Result<ResolvedSource, AppError> {
    use crate::modpack::modrinth::{Config as MrPackConfig, ModrinthServerPack};
    use crate::modpack::ModpackHttp;

    let cfg = cfg.ok_or(AppError::BadRequest {
        code: "modrinth_config_missing",
        message: "modrinth.{project_id, channel} required for server_type=modrinth".to_owned(),
    })?;
    crate::validation::validate_modrinth_id_or_slug(&cfg.project_id)?;

    let provisional = ModrinthServerPack::new(MrPackConfig {
        project_id: cfg.project_id.clone(),
        channel: cfg.channel,
        version_skip: Vec::new(),
        force_version: None,
        current_version_id: String::new(),
        current_version_name: String::new(),
        auto_update_mode: AutoUpdateMode::Notify,
    });
    let http = ModpackHttp {
        cf: state.cf_client.as_deref(),
        mr: state.mr_client.as_ref(),
    };
    let pick = provisional
        .latest(&http)
        .await
        .map_err(|e| AppError::BadRequest {
            code: "modrinth_unavailable",
            message: format!("modrinth lookup: {e}"),
        })?
        .ok_or(AppError::BadRequest {
            code: "no_modpack_versions",
            message: format!("project {:?} has no matching versions", cfg.project_id),
        })?;

    let stored_cfg = MrPackConfig {
        project_id: cfg.project_id,
        channel: cfg.channel,
        version_skip: Vec::new(),
        force_version: None,
        current_version_id: pick.id.clone(),
        current_version_name: pick.name.clone(),
        auto_update_mode: AutoUpdateMode::Notify,
    };
    let source_config =
        serde_json::to_string(&stored_cfg).map_err(|e| AppError::Internal(e.into()))?;

    Ok(ResolvedSource {
        provider: Box::new(ModrinthServerPack::new(stored_cfg)),
        mc_version: pick.name,
        source_kind: SERVER_TYPE_MODRINTH,
        source_config,
    })
}
```

- [ ] **Step 4: Implement `resolve_modded` + `resolve_paper`**

```rust
async fn resolve_modded(
    _state: &AppState,
    mc_version: Option<String>,
    cfg: Option<ModdedCreateConfig>,
) -> Result<ResolvedSource, AppError> {
    use crate::modpack::modded::{Config as ModdedCfg, ModdedRuntime, Runtime};

    let cfg = cfg.ok_or(AppError::BadRequest {
        code: "modded_config_missing",
        message: "modded.{runtime} required for server_type=modded".to_owned(),
    })?;
    crate::validation::validate_runtime(&cfg.runtime)?;
    let runtime = match cfg.runtime.as_str() {
        "fabric" => Runtime::Fabric,
        "forge" => Runtime::Forge,
        "neoforge" => Runtime::NeoForge,
        other => {
            return Err(AppError::BadRequest {
                code: "runtime_invalid",
                message: format!("runtime {other:?} not allowed for modded servers"),
            });
        }
    };
    let mc_v = mc_version.ok_or(AppError::BadRequest {
        code: "mc_version_required",
        message: "mc_version is required for modded servers".to_owned(),
    })?;
    // Re-validate filenames in initial_mods before persisting.
    for m in &cfg.initial_mods {
        crate::validation::validate_mod_filename(&m.filename)?;
    }
    let stored = ModdedCfg {
        runtime,
        mc_version: mc_v.clone(),
        mods: cfg.initial_mods.clone(),
        pending: Vec::new(),
    };
    let source_config =
        serde_json::to_string(&stored).map_err(|e| AppError::Internal(e.into()))?;
    Ok(ResolvedSource {
        provider: Box::new(ModdedRuntime::new(stored)),
        mc_version: mc_v,
        source_kind: SERVER_TYPE_MODDED,
        source_config,
    })
}

async fn resolve_paper(
    state: &AppState,
    mc_version: Option<String>,
) -> Result<ResolvedSource, AppError> {
    use crate::modpack::paper::{Config as PaperCfg, PaperServerProvider};

    let mc_v = mc_version.ok_or(AppError::BadRequest {
        code: "mc_version_required",
        message: "mc_version is required for paper servers".to_owned(),
    })?;
    crate::validation::validate_mc_version(state, &mc_v).await?;
    let stored = PaperCfg {
        mc_version: mc_v.clone(),
        paper_build: None,
    };
    let source_config =
        serde_json::to_string(&stored).map_err(|e| AppError::Internal(e.into()))?;
    Ok(ResolvedSource {
        provider: Box::new(PaperServerProvider::new(stored)),
        mc_version: mc_v,
        source_kind: SERVER_TYPE_PAPER,
        source_config,
    })
}
```

- [ ] **Step 5: Threading mods initial_mods**

The modded resolve pre-loads `mods` from the request; `mod-sync` Job hasn't run yet. The server's first `start` will boot with no mod jars in /data/mods because /data/mods doesn't exist. Two clean options:

(a) Add a "force-sync-on-first-start" semantic: when starting a modded server with `mods.len() > 0` and no jars on disk, run the sync FSM before scaling the StatefulSet. Adds startup logic.

(b) Document: modded servers created with initial_mods must click `[apply]` once before they boot correctly. The Mods tab shows an "initial sync required" notice when `pending == [] && jars-on-disk-unknown`.

Pick (b) for B (anti-OE — relies on the user pressing apply). Actually cleaner: the create handler converts `initial_mods` into `pending: [Add{...}, Add{...}]` so the Mods tab naturally shows "N pending changes" and the user clicks `[apply now]` to install them. Implement this:

```rust
let pending: Vec<crate::modpack::modded::PendingOp> = cfg
    .initial_mods
    .iter()
    .map(|m| crate::modpack::modded::PendingOp::Add {
        mod_entry: m.clone(),
    })
    .collect();
let stored = ModdedCfg {
    runtime,
    mc_version: mc_v.clone(),
    mods: Vec::new(),
    pending,
};
```

(Replacing the prior `mods: cfg.initial_mods.clone()` line.)

- [ ] **Step 6: `cargo test --all`**

Expected: green (existing CF + vanilla tests still pass).

### Task 8.2: Phase 8 commit

```bash
git add backend/src/routes/servers/create.rs && \
git commit -m "$(cat <<'EOF'
feat(api): POST /api/servers accepts paper, modded, modrinth types

Modrinth type resolves the latest matching version via ModrinthClient
at create-time and persists the picked version+download_url. Modded
type stores runtime + mc_version + a pending list of Add ops (one per
initial_mods entry) so the Mods tab shows the "N pending — apply now"
banner naturally on first load. Paper just snapshots mc_version.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 9 — Frontend schemas + API functions

### Task 9.1: Extend Zod schemas

**Files:**
- Modify: `frontend/app/lib/api.ts`

- [ ] **Step 1: Extend sourceKindSchema and clusterCapabilitiesSchema**

```ts
export const sourceKindSchema = z.enum([
    "vanilla",
    "curseforge",
    "modrinth",
    "modded",
    "paper",
]);

export const clusterCapabilitiesSchema = z.object({
    loadbalancer: z.boolean(),
    nodeport: z.boolean(),
    clusterip: z.boolean(),
    available_storage_classes: z.array(z.string()),
    default_storage_class: z.string().nullable(),
    cf_api_key_present: z.boolean().default(false),
    modrinth_enabled: z.boolean().default(true),
    available_cpu_cores: z.number().nonnegative().default(0),
});
```

- [ ] **Step 2: Widen update wire shapes (target_version_id is now String)**

```ts
export const updateStartResponseSchema = z.object({
    status: z.string(),
    server_id: z.string(),
    target_version_id: z.string(),
});
```

And `applyUpdate`:

```ts
export async function applyUpdate(
    id: string,
    versionId?: string,
): Promise<UpdateStartResponse> {
    const body =
        versionId !== undefined
            ? JSON.stringify({ version_id: versionId })
            : JSON.stringify({});
    // ...same as before
}
```

- [ ] **Step 3: Add catalog schemas + functions**

```ts
export const catalogProviderSchema = z.enum(["curseforge", "modrinth"]);

export const catalogHitSchema = z.object({
    provider: catalogProviderSchema,
    project_id: z.string(),
    slug: z.string(),
    name: z.string(),
    summary: z.string().default(""),
    icon_url: z.string().nullable(),
    downloads: z.number().int().nonnegative(),
    follows: z.number().int().nonnegative(),
    project_type: z.string(),
    loaders: z.array(z.string()).default([]),
    game_versions: z.array(z.string()).default([]),
    author: z.string().nullable().default(null),
    updated: z.string().default(""),
});

export const catalogSearchResponseSchema = z.object({
    results: z.array(catalogHitSchema),
});

export const catalogVersionSchema = z.object({
    version_id: z.string(),
    version_name: z.string(),
    channel: z.string(),
    loaders: z.array(z.string()).default([]),
    game_versions: z.array(z.string()).default([]),
    date_published: z.string(),
    primary_filename: z.string(),
    primary_url: z.string(),
    primary_sha512: z.string().nullable().default(null),
});

export const catalogVersionsResponseSchema = z.object({
    versions: z.array(catalogVersionSchema),
});

export type CatalogHit = z.infer<typeof catalogHitSchema>;
export type CatalogVersion = z.infer<typeof catalogVersionSchema>;
export type CatalogProvider = z.infer<typeof catalogProviderSchema>;

export interface CatalogSearchParams {
    type: "mod" | "modpack";
    q: string;
    loader?: "fabric" | "forge" | "neoforge" | "paper";
    mc?: string;
    limit?: number;
    offset?: number;
}

export async function searchCatalog(
    params: CatalogSearchParams,
    signal?: AbortSignal,
): Promise<readonly CatalogHit[]> {
    const sp = new URLSearchParams({ type: params.type, q: params.q });
    if (params.loader !== undefined) sp.set("loader", params.loader);
    if (params.mc !== undefined) sp.set("mc", params.mc);
    if (params.limit !== undefined) sp.set("limit", params.limit.toString());
    if (params.offset !== undefined) sp.set("offset", params.offset.toString());
    const init: RequestInit = signal ? { signal } : {};
    const res = await fetch(`/api/catalog/search?${sp.toString()}`, init);
    const body = await jsonOrThrow(res, catalogSearchResponseSchema);
    return body.results;
}

export async function fetchCatalogVersions(
    provider: CatalogProvider,
    id: string,
    opts: { loader?: string; mc?: string } = {},
    signal?: AbortSignal,
): Promise<readonly CatalogVersion[]> {
    const sp = new URLSearchParams();
    if (opts.loader !== undefined) sp.set("loader", opts.loader);
    if (opts.mc !== undefined) sp.set("mc", opts.mc);
    const qs = sp.toString();
    const url = `/api/catalog/projects/${encodeURIComponent(provider)}/${encodeURIComponent(id)}/versions${qs.length > 0 ? `?${qs}` : ""}`;
    const init: RequestInit = signal ? { signal } : {};
    const res = await fetch(url, init);
    const body = await jsonOrThrow(res, catalogVersionsResponseSchema);
    return body.versions;
}
```

- [ ] **Step 4: Mods schemas + functions**

```ts
export const modEntrySchema = z.object({
    provider: catalogProviderSchema,
    project_id: z.string(),
    project_slug: z.string(),
    project_name: z.string(),
    version_id: z.string(),
    version_name: z.string(),
    filename: z.string(),
    download_url: z.string(),
    sha512: z.string().nullable().default(null),
});

export const modPendingOpSchema = z.discriminatedUnion("op", [
    z.object({ op: z.literal("add"), mod_entry: modEntrySchema }),
    z.object({ op: z.literal("remove"), filename: z.string() }),
    z.object({
        op: z.literal("bump"),
        filename: z.string(),
        to_version_id: z.string(),
        to_version_name: z.string(),
        to_filename: z.string(),
        to_download_url: z.string(),
        to_sha512: z.string().nullable().default(null),
    }),
]);

export const moddedConfigSchema = z.object({
    runtime: z.enum(["fabric", "forge", "neoforge"]),
    mc_version: z.string(),
    mods: z.array(modEntrySchema).default([]),
    pending: z.array(modPendingOpSchema).default([]),
});

export type ModEntry = z.infer<typeof modEntrySchema>;
export type ModPendingOp = z.infer<typeof modPendingOpSchema>;
export type ModdedConfig = z.infer<typeof moddedConfigSchema>;

export const modsApplyResponseSchema = z.object({
    status: z.string(),
    server_id: z.string(),
    pending_count: z.number().int().nonnegative(),
});

export async function addPendingMod(
    serverId: string,
    op: ModPendingOp,
): Promise<void> {
    const validated = modPendingOpSchema.parse(op);
    const res = await fetch(
        `/api/servers/${encodeURIComponent(serverId)}/mods`,
        {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(validated),
        },
    );
    await noContentOrThrow(res);
}

export async function removePendingMod(
    serverId: string,
    idx: number,
): Promise<void> {
    const res = await fetch(
        `/api/servers/${encodeURIComponent(serverId)}/mods/pending/${idx.toString()}`,
        { method: "DELETE" },
    );
    await noContentOrThrow(res);
}

export async function applyMods(
    serverId: string,
): Promise<{ status: string; server_id: string; pending_count: number }> {
    const res = await fetch(
        `/api/servers/${encodeURIComponent(serverId)}/mods/apply`,
        { method: "POST" },
    );
    return jsonOrThrow(res, modsApplyResponseSchema);
}
```

- [ ] **Step 5: Extend createServerRequestSchema**

```ts
export const modrinthCreateSchema = z.object({
    project_id: z.string().min(1).max(40),
    channel: cfChannelSchema,
});

export const moddedCreateSchema = z.object({
    runtime: z.enum(["fabric", "forge", "neoforge"]),
    initial_mods: z.array(modEntrySchema).default([]),
});

export const createServerRequestSchema = z.object({
    name: z.string().regex(NAME_REGEX, "lowercase letters, digits, '-' (1-63 chars)"),
    mc_version: z.string().optional(),
    memory_mi: z.number().int().min(1024).max(16_384),
    cpu_millicores: z.number().int().min(250).max(16_000),
    exposure_mode: exposureModeSchema.optional(),
    storage_class: z.string().optional(),
    storage_size_gi: z.number().int().min(10).max(500).optional(),
    server_type: sourceKindSchema.optional(),
    curseforge: curseforgeCreateSchema.optional(),
    modrinth: modrinthCreateSchema.optional(),
    modded: moddedCreateSchema.optional(),
});
```

### Task 9.2: `use-mod-apply-stream` hook

**Files:**
- Create: `frontend/app/lib/use-mod-apply-stream.ts`

- [ ] **Step 1: Mirror `update-stream.ts` shape**

```ts
"use client";

import { useEffect, useRef, useState } from "react";

import type { UpdatePhase } from "./update-stream";

export interface ModApplyStream {
    status: "connecting" | "open" | "reconnecting" | "closed";
    phase: UpdatePhase | null;
    result: "succeeded" | "failed" | null;
    endedReason: string | null;
}

const INITIAL: ModApplyStream = {
    status: "connecting",
    phase: null,
    result: null,
    endedReason: null,
};

export function useModApplyStream(serverId: string | null): ModApplyStream {
    const [state, setState] = useState<ModApplyStream>(INITIAL);
    const cancelled = useRef(false);

    useEffect(() => {
        if (serverId === null) {
            setState(INITIAL);
            return undefined;
        }
        cancelled.current = false;
        const url = `${window.location.protocol === "https:" ? "wss:" : "ws:"}//${window.location.host}/api/servers/${encodeURIComponent(serverId)}/mods/apply/stream`;
        let socket: WebSocket | null = null;
        let backoff = 1_000;
        const connect = (): void => {
            if (cancelled.current) return;
            socket = new WebSocket(url);
            socket.onopen = (): void => {
                setState((s) => ({ ...s, status: "open" }));
            };
            socket.onmessage = (ev): void => {
                try {
                    const frame: unknown = JSON.parse(String(ev.data));
                    if (typeof frame === "object" && frame !== null && "type" in frame) {
                        const f = frame as { type: string; phase?: UpdatePhase; result?: "succeeded" | "failed"; reason?: string };
                        if (f.type === "hello" || f.type === "progress") {
                            setState((s) => ({ ...s, phase: f.phase ?? s.phase }));
                        } else if (f.type === "done") {
                            setState((s) => ({ ...s, result: f.result ?? null }));
                        } else if (f.type === "end") {
                            setState((s) => ({ ...s, endedReason: f.reason ?? null }));
                        }
                    }
                } catch {
                    // ignore malformed frames
                }
            };
            socket.onerror = (): void => {
                if (cancelled.current) return;
                setState((s) => ({ ...s, status: "reconnecting" }));
            };
            socket.onclose = (): void => {
                if (cancelled.current) return;
                setState((s) => ({ ...s, status: "reconnecting" }));
                window.setTimeout(connect, backoff);
                backoff = Math.min(backoff * 2, 30_000);
            };
        };
        connect();
        return () => {
            cancelled.current = true;
            socket?.close();
            setState(INITIAL);
        };
    }, [serverId]);

    return state;
}
```

### Task 9.3: Phase 9 quality gate + commit

- [ ] **Step 1: `pnpm typecheck && pnpm lint`**

```bash
cd /home/hadi/gitlab/anvil/frontend && pnpm typecheck && pnpm lint
```

Expected: green (existing callsites of `applyUpdate` will need updating where they pass numeric ids — search and fix any).

- [ ] **Step 2: Commit**

```bash
cd /home/hadi/gitlab/anvil && git add frontend && \
git commit -m "$(cat <<'EOF'
feat(frontend): schemas + API for catalog + mods CRUD + apply WS

sourceKindSchema gains 3 variants. clusterCapabilitiesSchema gains
modrinth_enabled. New catalog hit/version schemas + searchCatalog /
fetchCatalogVersions functions. Mods pending CRUD (addPendingMod /
removePendingMod) + applyMods kicker + useModApplyStream hook
mirroring useUpdateStream's shape so ApplySheet can reuse the
phase-list rendering.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 10 — `CatalogSheet` component

### Task 10.1: Component skeleton

**Files:**
- Create: `frontend/app/components/CatalogSheet.tsx`

- [ ] **Step 1: Write the component**

```tsx
"use client";

import { useEffect, useState, type ReactElement } from "react";

import {
    ApiError,
    fetchCatalogVersions,
    searchCatalog,
    type CatalogHit,
    type CatalogProvider,
    type CatalogVersion,
} from "../lib/api";

import { Button } from "./Button";
import { SegmentedControl } from "./SegmentedControl";
import { Sheet } from "./Sheet";
import { Skeleton } from "./Skeleton";

type Mode = "modpack" | "mod";
type Loader = "fabric" | "forge" | "neoforge";

export interface CatalogPick {
    hit: CatalogHit;
    version: CatalogVersion;
}

interface Props {
    isOpen: boolean;
    onClose: () => void;
    mode: Mode;
    loader?: Loader;
    mc?: string;
    onPick: (pick: CatalogPick) => void;
}

export function CatalogSheet({
    isOpen,
    onClose,
    mode,
    loader,
    mc,
    onPick,
}: Props): ReactElement {
    const [q, setQ] = useState("");
    const [hits, setHits] = useState<readonly CatalogHit[]>([]);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [activeHit, setActiveHit] = useState<CatalogHit | null>(null);
    const [versions, setVersions] = useState<readonly CatalogVersion[]>([]);

    useEffect(() => {
        if (!isOpen) return;
        setQ("");
        setHits([]);
        setError(null);
        setActiveHit(null);
        setVersions([]);
    }, [isOpen]);

    useEffect(() => {
        if (!isOpen || q.trim().length === 0) return undefined;
        const ctrl = new AbortController();
        const t = window.setTimeout(() => {
            setBusy(true);
            setError(null);
            const params: Parameters<typeof searchCatalog>[0] = {
                type: mode,
                q: q.trim(),
            };
            if (mode === "mod" && loader !== undefined) params.loader = loader;
            if (mode === "mod" && mc !== undefined) params.mc = mc;
            searchCatalog(params, ctrl.signal)
                .then((r) => {
                    setHits(r);
                })
                .catch((err: unknown) => {
                    if (err instanceof DOMException && err.name === "AbortError") return;
                    setError(
                        err instanceof ApiError
                            ? `${err.code}: ${err.message}`
                            : err instanceof Error
                                ? err.message
                                : "search failed",
                    );
                })
                .finally(() => {
                    setBusy(false);
                });
        }, 300);
        return () => {
            ctrl.abort();
            window.clearTimeout(t);
        };
    }, [isOpen, q, mode, loader, mc]);

    const onPickHit = (hit: CatalogHit): void => {
        setActiveHit(hit);
        setVersions([]);
        const ctrl = new AbortController();
        fetchCatalogVersions(
            hit.provider,
            hit.project_id,
            { ...(loader !== undefined ? { loader } : {}), ...(mc !== undefined ? { mc } : {}) },
            ctrl.signal,
        )
            .then(setVersions)
            .catch((err: unknown) => {
                setError(
                    err instanceof ApiError
                        ? `${err.code}: ${err.message}`
                        : err instanceof Error
                            ? err.message
                            : "version fetch failed",
                );
            });
    };

    return (
        <Sheet
            isOpen={isOpen}
            onClose={onClose}
            title={mode === "mod" ? "browse mods" : "browse modpacks"}
            width={720}
        >
            <div className="flex h-full flex-col">
                <div className="border-b border-border-soft px-5 py-3">
                    <input
                        value={q}
                        onChange={(e) => {
                            setQ(e.target.value);
                            setActiveHit(null);
                        }}
                        placeholder={mode === "mod" ? "search mods" : "search modpacks"}
                        className="w-full rounded-md border border-border bg-bg px-3 py-2 font-mono text-[13px] text-text-body placeholder:text-text-faint focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
                        spellCheck={false}
                    />
                    {mode === "mod" && (
                        <p className="mt-2 font-mono text-[11px] text-text-faint">
                            filtered to {loader ?? "any loader"} ·{" "}
                            {mc ?? "any minecraft version"}
                        </p>
                    )}
                </div>

                {activeHit !== null ? (
                    <div className="flex-1 overflow-y-auto px-5 py-3">
                        <button
                            type="button"
                            onClick={() => {
                                setActiveHit(null);
                            }}
                            className="mb-3 font-mono text-[11px] text-text-muted hover:text-text-body"
                        >
                            ← back to results
                        </button>
                        <h3 className="font-mono text-[14px] text-text-primary">
                            {activeHit.name}
                        </h3>
                        <p className="mt-1 font-mono text-[12px] text-text-muted">
                            pick a version
                        </p>
                        <ul className="mt-3 flex flex-col gap-1">
                            {versions.length === 0 && (
                                <li className="font-mono text-[12px] text-text-faint">
                                    no compatible versions
                                </li>
                            )}
                            {versions.map((v) => (
                                <li
                                    key={v.version_id}
                                    className="flex items-center justify-between border-b border-border-soft py-2 font-mono text-[12px]"
                                >
                                    <div>
                                        <span className="text-text-body">{v.version_name}</span>
                                        <span className="ml-2 text-text-faint">{v.channel}</span>
                                    </div>
                                    <Button
                                        variant="primary"
                                        onClick={() => {
                                            onPick({ hit: activeHit, version: v });
                                            onClose();
                                        }}
                                    >
                                        install
                                    </Button>
                                </li>
                            ))}
                        </ul>
                    </div>
                ) : (
                    <div className="flex-1 overflow-y-auto">
                        {busy &&
                            Array.from({ length: 4 }).map((_, i) => (
                                <Skeleton key={i} variant="row" className="mx-5 my-2 h-12" />
                            ))}
                        {!busy && error !== null && (
                            <p className="px-5 py-3 font-mono text-[12px] text-state-error">
                                {error}
                            </p>
                        )}
                        {!busy && error === null && q.trim().length > 0 && hits.length === 0 && (
                            <p className="px-5 py-3 font-mono text-[12px] text-text-faint">
                                no results
                            </p>
                        )}
                        {!busy && q.trim().length === 0 && (
                            <p className="px-5 py-3 font-mono text-[12px] text-text-faint">
                                start typing to search
                            </p>
                        )}
                        <ul>
                            {hits.map((h) => (
                                <li
                                    key={`${h.provider}:${h.project_id}`}
                                    className="group flex items-center gap-3 border-b border-border-soft px-5 py-2"
                                >
                                    <span
                                        className="h-3.5 w-1 rounded-sm"
                                        style={{
                                            background:
                                                h.provider === "modrinth"
                                                    ? "var(--color-source-modrinth)"
                                                    : "var(--color-source-curseforge)",
                                        }}
                                    />
                                    <div className="flex-1">
                                        <p className="font-mono text-[13px] text-text-body">
                                            {h.name}
                                        </p>
                                        <p className="font-mono text-[11px] text-text-muted">
                                            {h.author ?? ""} · {h.downloads.toLocaleString()} downloads
                                        </p>
                                    </div>
                                    <Button
                                        onClick={() => {
                                            onPickHit(h);
                                        }}
                                    >
                                        pick
                                    </Button>
                                </li>
                            ))}
                        </ul>
                    </div>
                )}
            </div>
        </Sheet>
    );
}
```

- [ ] **Step 2: `pnpm typecheck && pnpm lint`**

Expected: green.

### Task 10.2: Phase 10 commit

```bash
cd /home/hadi/gitlab/anvil && git add frontend/app/components/CatalogSheet.tsx && \
git commit -m "$(cat <<'EOF'
feat(frontend): CatalogSheet — search + version-picker for mods/modpacks

Two-step UX inside the right slide-over: search hits with source-marker
bars (copper for CF, modrinth-green for MR), click `pick` to load
versions for that project, click `install` on a version to land it
in the parent (caller decides what onPick does). Strict facets when
mode='mod' (loader+mc are non-negotiable).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 11 — Mods tab body

### Task 11.1: ApplySheet component

**Files:**
- Create: `frontend/app/components/ApplySheet.tsx`

- [ ] **Step 1: Mirror `UpdateSheet` with the shorter phase set**

```tsx
"use client";

import type { ReactElement } from "react";

import { useModApplyStream } from "../lib/use-mod-apply-stream";
import type { UpdatePhase } from "../lib/update-stream";
import { cn } from "../lib/cn";

import { Sheet } from "./Sheet";

const ORDER: ReadonlyArray<UpdatePhase> = [
    "queued",
    "stopping",
    "swapping", // surfaces as 'syncing' visually
    "starting",
    "verifying",
    "succeeded",
];

const LABELS: Record<UpdatePhase, string> = {
    queued: "queued",
    announcing: "announcing",
    stopping: "stopping",
    "backing-up": "backing up",
    swapping: "syncing mods",
    starting: "starting",
    verifying: "verifying",
    succeeded: "succeeded",
    "rolling-back": "rolling back",
    "rolled-back": "rolled back",
    failed: "failed",
};

interface Props {
    serverId: string | null;
    isOpen: boolean;
    onClose: () => void;
}

export function ApplySheet({ serverId, isOpen, onClose }: Props): ReactElement {
    const stream = useModApplyStream(isOpen ? serverId : null);
    const activeIdx = stream.phase ? ORDER.indexOf(stream.phase) : -1;

    return (
        <Sheet isOpen={isOpen} onClose={onClose} title="apply mods" width={640}>
            <div className="p-5">
                <ol className="flex flex-col gap-2 font-mono text-[12px]">
                    {ORDER.map((p, i) => {
                        const reached = activeIdx >= 0 && i <= activeIdx;
                        const active = stream.phase === p;
                        return (
                            <li
                                key={p}
                                className={cn(
                                    "flex items-center gap-3",
                                    reached ? "text-text-body" : "text-text-faint",
                                    active && "text-accent",
                                )}
                            >
                                <span
                                    className={cn(
                                        "h-1.5 w-1.5 rounded-full",
                                        active
                                            ? "bg-accent"
                                            : reached
                                                ? "bg-state-running"
                                                : "bg-text-faint",
                                    )}
                                />
                                {LABELS[p]}
                            </li>
                        );
                    })}
                </ol>
                {stream.result !== null && (
                    <p className="mt-4 font-mono text-[12px] text-text-body">
                        result · {stream.result}
                    </p>
                )}
                {stream.endedReason !== null && (
                    <p className="mt-1 font-mono text-[12px] text-state-error">
                        {stream.endedReason}
                    </p>
                )}
                {stream.status === "reconnecting" && (
                    <p className="mt-2 font-mono text-[11px] text-text-muted">
                        reconnecting…
                    </p>
                )}
            </div>
        </Sheet>
    );
}
```

### Task 11.2: ModsBody — modded full UX

**Files:**
- Modify: `frontend/app/servers/tabs/ModsBody.tsx`

- [ ] **Step 1: Replace placeholder with type branching**

```tsx
"use client";

import { useState, type ReactElement } from "react";

import {
    ApiError,
    addPendingMod,
    applyMods,
    moddedConfigSchema,
    removePendingMod,
    type ModEntry,
    type ModPendingOp,
    type ModdedConfig,
} from "../../lib/api";
import { useServerDetailCtx } from "../../lib/server-detail-context";

import { ApplySheet } from "../../components/ApplySheet";
import { Button } from "../../components/Button";
import { Card } from "../../components/Card";
import { CatalogSheet, type CatalogPick } from "../../components/CatalogSheet";
import { useToast } from "../../components/Toast";

export function ModsBody(): ReactElement {
    const detail = useServerDetailCtx();
    const toast = useToast();
    const [browseOpen, setBrowseOpen] = useState(false);
    const [applyOpen, setApplyOpen] = useState(false);

    if (detail.source_kind === "vanilla") {
        return (
            <Card>
                <p className="font-mono text-[12px] text-text-muted">
                    vanilla servers don&apos;t support mods.
                </p>
            </Card>
        );
    }

    if (detail.source_kind === "paper") {
        return (
            <Card>
                <p className="font-mono text-[12px] text-text-muted">
                    paper plugin browsing arrives later. install plugins via
                    an external file manager for now.
                </p>
            </Card>
        );
    }

    if (
        detail.source_kind === "curseforge"
        || detail.source_kind === "modrinth"
    ) {
        return (
            <Card header={`bundled in ${detail.mc_version}`}>
                <p className="font-mono text-[12px] text-text-muted">
                    pack-driven · changes get wiped at next pack update. view
                    mods/ for now via an external file manager.
                </p>
            </Card>
        );
    }

    // modded
    const cfgParse = moddedConfigSchema.safeParse(detail.source_config);
    if (!cfgParse.success) {
        return (
            <Card>
                <p className="font-mono text-[12px] text-state-error">
                    source_config did not parse as a modded config
                </p>
            </Card>
        );
    }
    const cfg: ModdedConfig = cfgParse.data;

    const onPick = (pick: CatalogPick): void => {
        const entry: ModEntry = {
            provider: pick.hit.provider,
            project_id: pick.hit.project_id,
            project_slug: pick.hit.slug,
            project_name: pick.hit.name,
            version_id: pick.version.version_id,
            version_name: pick.version.version_name,
            filename: pick.version.primary_filename,
            download_url: pick.version.primary_url,
            sha512: pick.version.primary_sha512,
        };
        const op: ModPendingOp = { op: "add", mod_entry: entry };
        addPendingMod(detail.id, op)
            .then(() => {
                toast.push(`queued · ${entry.project_name}`, "success");
            })
            .catch((err: unknown) => {
                toast.push(
                    `queue failed · ${err instanceof ApiError ? err.code : "unknown"}`,
                    "error",
                );
            });
    };

    const removeInstalled = (filename: string): void => {
        const op: ModPendingOp = { op: "remove", filename };
        addPendingMod(detail.id, op)
            .then(() => {
                toast.push(`queued removal · ${filename}`, "success");
            })
            .catch((err: unknown) => {
                toast.push(
                    `queue failed · ${err instanceof ApiError ? err.code : "unknown"}`,
                    "error",
                );
            });
    };

    const discardPending = (idx: number): void => {
        removePendingMod(detail.id, idx)
            .then(() => {
                toast.push("discarded", "success");
            })
            .catch((err: unknown) => {
                toast.push(
                    `discard failed · ${err instanceof ApiError ? err.code : "unknown"}`,
                    "error",
                );
            });
    };

    const onApply = (): void => {
        applyMods(detail.id)
            .then(() => {
                setApplyOpen(true);
            })
            .catch((err: unknown) => {
                toast.push(
                    `apply failed · ${err instanceof ApiError ? err.code : "unknown"}`,
                    "error",
                );
            });
    };

    return (
        <>
            <Card>
                <div className="flex items-baseline justify-between">
                    <p className="font-mono text-[13px] text-text-primary">
                        {cfg.mods.length} installed
                        {cfg.pending.length > 0 && (
                            <span className="ml-3 text-state-warning">
                                · {cfg.pending.length} pending
                            </span>
                        )}
                    </p>
                    <Button
                        onClick={() => {
                            setBrowseOpen(true);
                        }}
                    >
                        + add mods
                    </Button>
                </div>

                <ul className="mt-4 flex flex-col">
                    {cfg.mods.length === 0 && (
                        <li className="py-2 font-mono text-[12px] text-text-faint">
                            no mods installed yet — click `+ add mods` to start.
                        </li>
                    )}
                    {cfg.mods.map((m) => (
                        <li
                            key={m.filename}
                            className="group flex items-center justify-between border-b border-border-soft py-2 font-mono text-[12px]"
                        >
                            <div className="flex items-center gap-3">
                                <span
                                    className="h-3.5 w-1 rounded-sm"
                                    style={{
                                        background:
                                            m.provider === "modrinth"
                                                ? "var(--color-source-modrinth)"
                                                : "var(--color-source-curseforge)",
                                    }}
                                />
                                <span className="text-text-body">{m.project_name}</span>
                                <span className="text-text-faint">{m.version_name}</span>
                            </div>
                            <button
                                type="button"
                                onClick={() => {
                                    removeInstalled(m.filename);
                                }}
                                className="opacity-0 transition-opacity hover:text-state-error focus-visible:opacity-100 group-hover:opacity-100"
                            >
                                remove
                            </button>
                        </li>
                    ))}
                </ul>

                {cfg.pending.length > 0 && (
                    <>
                        <p className="mt-6 font-mono text-[11px] uppercase tracking-wider text-text-muted">
                            pending changes
                        </p>
                        <ul className="mt-2 flex flex-col">
                            {cfg.pending.map((op, i) => (
                                <li
                                    key={`${op.op}-${i}`}
                                    className="group flex items-center justify-between border-b border-border-soft py-2 font-mono text-[12px]"
                                >
                                    <PendingLabel op={op} />
                                    <button
                                        type="button"
                                        onClick={() => {
                                            discardPending(i);
                                        }}
                                        className="opacity-0 transition-opacity hover:text-state-error focus-visible:opacity-100 group-hover:opacity-100"
                                    >
                                        discard
                                    </button>
                                </li>
                            ))}
                        </ul>
                        <div className="mt-4 flex justify-end gap-2">
                            <Button onClick={onApply} variant="primary">
                                apply now
                            </Button>
                        </div>
                    </>
                )}
            </Card>

            <CatalogSheet
                isOpen={browseOpen}
                onClose={() => {
                    setBrowseOpen(false);
                }}
                mode="mod"
                loader={cfg.runtime}
                mc={cfg.mc_version}
                onPick={onPick}
            />
            <ApplySheet
                serverId={detail.id}
                isOpen={applyOpen}
                onClose={() => {
                    setApplyOpen(false);
                }}
            />
        </>
    );
}

function PendingLabel({ op }: { op: ModPendingOp }): ReactElement {
    if (op.op === "add") {
        return (
            <span>
                <span className="mr-2 text-state-running">+</span>
                add · {op.mod_entry.project_name} {op.mod_entry.version_name}
            </span>
        );
    }
    if (op.op === "remove") {
        return (
            <span>
                <span className="mr-2 text-state-error">−</span>
                remove · {op.filename}
            </span>
        );
    }
    return (
        <span>
            <span className="mr-2 text-accent">↑</span>
            bump · {op.filename} → {op.to_version_name}
        </span>
    );
}
```

### Task 11.3: Phase 11 quality gate + commit

- [ ] **Step 1: `pnpm typecheck && pnpm lint && pnpm build`**

Expected: green.

- [ ] **Step 2: Commit**

```bash
cd /home/hadi/gitlab/anvil && git add frontend && \
git commit -m "$(cat <<'EOF'
feat(frontend): ModsBody full content + ApplySheet

Branches by source_kind: modded gets the full installed/pending list
with [+ add mods] (opens CatalogSheet pre-filtered to runtime+mc),
hover-reveal remove on installed rows, hover-reveal discard on pending,
and [apply now] kicks /mods/apply and opens ApplySheet with the
streaming phase list. CF/Modrinth show read-only "bundled" copy. Paper
shows the deferred-plugin placeholder. Vanilla case is defensive only
(parent hides the tab).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 12 — Create page extensions

### Task 12.1: Type SegmentedControl + new sections

**Files:**
- Modify: `frontend/app/servers/new/page.tsx`
- Modify: `frontend/app/components/BuildSlip.tsx`

- [ ] **Step 1: Extend `TYPE_OPTIONS` and `CreateType`**

In `BuildSlip.tsx`:

```tsx
export type CreateType = "vanilla" | "paper" | "modpack" | "modded";

export interface CreateDraft {
    name: string;
    type: CreateType;
    mc_version: string | null;
    cpu_millicores: number;
    memory_mi: number;
    storage_size_gi: number;
    storage_class: string | null;
    exposure_mode: ExposureMode;
    server_type: SourceKind;
    curseforge: { project_id: number; channel: CfChannel } | null;
    modrinth: { project_id: string; channel: CfChannel } | null;
    runtime: "fabric" | "forge" | "neoforge" | null;
    initial_mods: ModEntry[];
}
```

Update `BuildSlip` to render `runtime` (when type=modded) and `initial_mods.length` count.

- [ ] **Step 2: Update create page TYPE_OPTIONS**

```tsx
const TYPE_OPTIONS: ReadonlyArray<{ value: CreateType; label: string }> = [
    { value: "vanilla", label: "vanilla" },
    { value: "paper", label: "paper" },
    { value: "modpack", label: "modpack" },
    { value: "modded", label: "modded" },
];
```

- [ ] **Step 3: Section 03 branching**

Replace the existing Section 03 body with a four-arm branch:

```tsx
{draft.type === "vanilla" && /* existing vanilla mc_version select */}
{draft.type === "paper" && /* same mc_version select */}
{draft.type === "modpack" && /* CF or Modrinth picker via CatalogSheet (modpack mode) */}
{draft.type === "modded" && /* runtime SegmentedControl + mc + optional pre-pick mods */}
```

For `modded`:

```tsx
<div className="flex flex-col gap-3">
    <div>
        <label className="mb-1 block font-mono text-[11px] uppercase tracking-wider text-text-muted">
            runtime
        </label>
        <SegmentedControl
            ariaLabel="modded runtime"
            value={draft.runtime ?? "fabric"}
            onChange={(v) => {
                if (draft.initial_mods.length > 0) {
                    if (!window.confirm(`switching runtime clears ${draft.initial_mods.length.toString()} picked mods. continue?`)) return;
                }
                set("runtime", v);
                set("initial_mods", []);
            }}
            options={[
                { value: "fabric", label: "fabric" },
                { value: "forge", label: "forge" },
                { value: "neoforge", label: "neoforge" },
            ]}
        />
    </div>
    <div>
        <label className="mb-1 block font-mono text-[11px] uppercase tracking-wider text-text-muted">
            minecraft version
        </label>
        <select
            value={draft.mc_version ?? ""}
            onChange={(e) => {
                if (draft.initial_mods.length > 0 && e.target.value !== draft.mc_version) {
                    if (!window.confirm(`switching MC version clears ${draft.initial_mods.length.toString()} picked mods. continue?`)) return;
                }
                set("mc_version", e.target.value === "" ? null : e.target.value);
                set("initial_mods", []);
            }}
            className="rounded-md border border-border bg-bg px-2 py-1.5 font-mono text-[12px] text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
        >
            <option value="">— select —</option>
            {(versions?.versions ?? []).map((v) => (
                <option key={v} value={v}>{v}</option>
            ))}
        </select>
    </div>
    <div className="flex items-center gap-2">
        <Button
            onClick={() => {
                if (draft.runtime !== null && draft.mc_version !== null) {
                    setBrowseOpen(true);
                }
            }}
            disabled={draft.runtime === null || draft.mc_version === null}
        >
            + pre-pick mods
        </Button>
        <span className="font-mono text-[11px] text-text-faint">
            {draft.initial_mods.length} picked
        </span>
    </div>
</div>
```

For `modpack`:

```tsx
<div className="flex flex-col gap-3">
    <SegmentedControl
        ariaLabel="provider"
        value={draft.modrinth !== null ? "modrinth" : "curseforge"}
        onChange={(v) => {
            if (v === "curseforge") {
                set("modrinth", null);
            } else {
                set("curseforge", null);
            }
        }}
        options={[
            { value: "curseforge", label: "curseforge" },
            { value: "modrinth", label: "modrinth" },
        ]}
    />
    {/* slug input + resolve (existing CF flow), or `[browse]` button */}
    <Button
        onClick={() => {
            setBrowseOpen(true);
        }}
    >
        browse
    </Button>
</div>
```

(Wire CatalogSheet with `mode="modpack"` and an `onPick` that fills `curseforge` or `modrinth` based on the picked hit's provider.)

- [ ] **Step 4: Add CatalogSheet to the create page**

```tsx
const [browseOpen, setBrowseOpen] = useState(false);

const onCatalogPick = (pick: CatalogPick): void => {
    if (draft.type === "modpack") {
        if (pick.hit.provider === "modrinth") {
            set("modrinth", { project_id: pick.hit.project_id, channel: "release" });
            set("curseforge", null);
            set("mc_version", pick.version.version_name);
        } else {
            const idNum = Number.parseInt(pick.hit.project_id, 10);
            if (!Number.isNaN(idNum)) {
                set("curseforge", { project_id: idNum, channel: "release" });
                set("modrinth", null);
                set("mc_version", pick.version.version_name);
            }
        }
    } else if (draft.type === "modded") {
        const entry: ModEntry = {
            provider: pick.hit.provider,
            project_id: pick.hit.project_id,
            project_slug: pick.hit.slug,
            project_name: pick.hit.name,
            version_id: pick.version.version_id,
            version_name: pick.version.version_name,
            filename: pick.version.primary_filename,
            download_url: pick.version.primary_url,
            sha512: pick.version.primary_sha512,
        };
        set("initial_mods", [...draft.initial_mods, entry]);
    }
};

<CatalogSheet
    isOpen={browseOpen}
    onClose={() => { setBrowseOpen(false); }}
    mode={draft.type === "modded" ? "mod" : "modpack"}
    {...(draft.type === "modded" && draft.runtime !== null ? { loader: draft.runtime } : {})}
    {...(draft.type === "modded" && draft.mc_version !== null ? { mc: draft.mc_version } : {})}
    onPick={onCatalogPick}
/>
```

- [ ] **Step 5: Update submit handler**

```tsx
const isPaper = draft.type === "paper";
const isModpack = draft.type === "modpack" && (draft.curseforge !== null || draft.modrinth !== null);
const isModded = draft.type === "modded" && draft.runtime !== null;

const request: CreateServerRequest = {
    name: draft.name,
    memory_mi: draft.memory_mi,
    cpu_millicores: draft.cpu_millicores,
    exposure_mode: draft.exposure_mode,
    storage_size_gi: draft.storage_size_gi,
    ...(draft.storage_class !== null && draft.storage_class !== ""
        ? { storage_class: draft.storage_class }
        : {}),
    ...(draft.mc_version !== null ? { mc_version: draft.mc_version } : {}),
    server_type: isPaper
        ? "paper"
        : isModpack
            ? draft.modrinth !== null ? "modrinth" : "curseforge"
            : isModded
                ? "modded"
                : "vanilla",
    ...(draft.curseforge !== null && draft.modrinth === null
        ? { curseforge: { project_id: draft.curseforge.project_id, channel: draft.curseforge.channel } }
        : {}),
    ...(draft.modrinth !== null
        ? { modrinth: { project_id: draft.modrinth.project_id, channel: draft.modrinth.channel } }
        : {}),
    ...(isModded && draft.runtime !== null
        ? { modded: { runtime: draft.runtime, initial_mods: draft.initial_mods } }
        : {}),
};
```

Update `INITIAL` and `missing` validation to cover the new types.

- [ ] **Step 6: `pnpm typecheck && pnpm lint && pnpm build`**

Expected: green.

### Task 12.2: Phase 12 commit

```bash
cd /home/hadi/gitlab/anvil && git add frontend && \
git commit -m "$(cat <<'EOF'
feat(frontend): create page — paper, modpack-via-browse, modded types

TYPE_OPTIONS extends to 4. Modpack section gains a [browse] button
opening CatalogSheet in modpack mode (CF + Modrinth merged); picking
auto-fills the matching sub-config and the version_name as mc_version.
Modded section ships runtime SegmentedControl + mc-version select +
a [+ pre-pick mods] button opening CatalogSheet pre-filtered to the
runtime+mc. Switching runtime/mc with picked mods prompts before
clearing. Submit branches across all five new server_type values.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 13 — Smoke verification

### Task 13.1: Backend gates

- [ ] **Step 1: `cargo test --all`**

```bash
cd /home/hadi/gitlab/anvil/backend && cargo test --all
```

Expected: all unit + integration tests pass.

- [ ] **Step 2: `cargo clippy` both feature flavors**

```bash
cd /home/hadi/gitlab/anvil/backend && \
  cargo clippy --all-targets --features serve-dir -- -D warnings && \
  cargo clippy --all-targets --features embed -- -D warnings
```

Expected: no warnings.

- [ ] **Step 3: `cargo fmt --check`**

```bash
cd /home/hadi/gitlab/anvil/backend && cargo fmt --check
```

Expected: clean.

### Task 13.2: Frontend gates

- [ ] **Step 1: typecheck + lint + build**

```bash
cd /home/hadi/gitlab/anvil/frontend && pnpm typecheck && pnpm lint && pnpm build
```

Expected: green; `out/` produced.

### Task 13.3: Manual smoke against the cluster

(Runs against the homelab — not part of CI; document as the human-driven gate.)

- [ ] **Step 1: Build and run the panel locally with serve-dir**

```bash
cd /home/hadi/gitlab/anvil/frontend && pnpm build
cd ../backend && \
  ANVIL_MC_STORAGE_CLASS=tank \
  ANVIL_LB_SUPPORTED=false \
  ANVIL_NODE_HOST=192.168.1.10 \
  ANVIL_OIDC_ISSUER_URL=https://auth.cherkaoui.ch/application/o/anvil/ \
  ANVIL_OIDC_CLIENT_ID=... \
  ANVIL_OIDC_CLIENT_SECRET=... \
  ANVIL_OIDC_REDIRECT_URL=http://localhost:8080/api/auth/callback \
  ANVIL_SESSION_KEY="$(openssl rand -base64 48)" \
  ANVIL_MODPACK_SNAPSHOTS_PVC=mc-snapshots \
  cargo run --features serve-dir
```

- [ ] **Step 2: Verify modded create end-to-end**

Open `http://localhost:8080`, sign in via Authentik, click `[+ new]`. Pick `modded` → fabric → 1.21.1 → click `[+ pre-pick mods]` → search `sodium`, pick the latest version → close Sheet → submit. Watch detail-page Mods tab show `0 installed · 1 pending`. Click `[apply now]`. The ApplySheet streams `stopping → syncing mods → starting → verifying → succeeded`. Check `kubectl exec mc-{id}-0 -- ls /data/mods` shows the jar.

- [ ] **Step 3: Verify Modrinth modpack create**

Re-open create page. Pick `modpack` → toggle to `modrinth` → `[browse]` → search a Modrinth pack (e.g. `simply-optimized`) → pick → submit. Watch the new server boot via `TYPE=AUTO_MODRINTH`.

- [ ] **Step 4: Verify Paper boot**

Create a `paper` server. Confirm `kubectl get sts mc-{id} -o yaml` shows `TYPE=PAPER` env. Boot. RCON `say hi` works. Mods tab shows the deferred-plugin placeholder.

- [ ] **Step 5: Verify CF regression**

Create an existing-style CF server (paste an ATM-11 slug). Confirm it still creates, starts, and updates the same as before B.

- [ ] **Step 6: Verify strict facets**

On the modded server, open the Mods tab → `[+ add mods]` → search `optifine`. Should return zero hits (Fabric/1.21.1 has no optifine port). Search `sodium` → returns hits.

---

## Self-Review Checklist

Run before declaring B complete:

- [ ] Every spec section §1-§13 maps to at least one task in this plan.
- [ ] No task references a function/type defined nowhere.
- [ ] Quality gates green at end of each phase commit.
- [ ] Manual smoke (Phase 13.3) green against the live cluster.
- [ ] Spec's §10 verification list (mirrored in Phase 13.3) green item by item.

