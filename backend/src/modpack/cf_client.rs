//! `CurseForge` HTTP client with a 1h per-project file-list cache.
//!
//! Calls land on `api.curseforge.com/v1`; the API key comes from the
//! `CF_API_KEY` env (mounted from the `cf-api-key` Secret). Responses for
//! `/mods/{id}/files` are cached for an hour so the hourly poller and any
//! ad-hoc requests share one upstream call per project.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue};
use serde::Deserialize;
use tokio::sync::Mutex;

/// `CurseForge` API base URL.
const CF_API: &str = "https://api.curseforge.com/v1";

/// Minecraft game id on `CurseForge` — used to scope project searches.
const MINECRAFT_GAME_ID: u32 = 432;

/// File-list cache TTL.
const CACHE_TTL: Duration = Duration::from_secs(60 * 60);

/// Upstream `CurseForge` file (one entry of `/mods/{id}/files.data`).
///
/// Captures only the fields the panel uses; the API returns ~30 more.
#[derive(Debug, Clone, Deserialize)]
pub struct CfFile {
    /// File identifier (used as the version id in `modpack_versions`).
    pub id: u32,
    /// Display name (`"All The Mods 11 - 4.4 - Server Pack"`).
    #[serde(rename = "displayName")]
    pub display_name: String,
    /// Release type (1 = Release, 2 = Beta, 3 = Alpha).
    #[serde(rename = "releaseType")]
    pub release_type: u8,
    /// `true` for files marked as a server pack.
    #[serde(rename = "isServerPack", default)]
    pub is_server_pack: bool,
    /// Direct download URL — may be null when the project disabled API distribution.
    #[serde(rename = "downloadUrl")]
    pub download_url: Option<String>,
    /// Unix-second upload timestamp (we expose ISO 8601 from the API field
    /// `fileDate`); kept as the raw string and parsed only when needed.
    #[serde(rename = "fileDate", default)]
    pub file_date: String,
}

/// Project metadata returned by `/mods/search` and `/mods/{id}`.
#[derive(Debug, Clone, Deserialize)]
pub struct CfProject {
    /// Project id (the `project_id` we persist).
    pub id: u32,
    /// Project name.
    pub name: String,
    /// URL slug (`"all-the-mods-11"`).
    pub slug: String,
}

/// One-field generic envelope for `CurseForge` API responses (everything
/// returns `{"data": …}` or `{"data": [...], "pagination": {...}}`; we only
/// look at `data`).
#[derive(Deserialize)]
struct Envelope<T> {
    data: T,
}

/// Cached `Vec<CfFile>` entry.
#[derive(Debug, Clone)]
struct CacheEntry {
    fetched_at: Instant,
    files: Arc<Vec<CfFile>>,
}

/// `CurseForge` HTTP client.
///
/// Cheap to clone via `Arc` wrapping. The cache is `Mutex`-guarded; contention
/// is not a concern because the panel runs at homelab scale (~5 servers, one
/// poll per hour).
#[derive(Debug, Clone)]
pub struct CurseForgeClient {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    http: reqwest::Client,
    cache: Mutex<HashMap<u32, CacheEntry>>,
}

impl CurseForgeClient {
    /// Builds a client authenticated with the supplied API key.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying `reqwest::Client` fails to build
    /// (e.g. invalid headers).
    pub fn new(api_key: &str) -> Result<Self> {
        let mut headers = HeaderMap::new();
        let mut auth =
            HeaderValue::from_str(api_key).context("CF_API_KEY contains non-ASCII bytes")?;
        auth.set_sensitive(true);
        headers.insert("x-api-key", auth);
        headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(30))
            .build()
            .context("building CurseForge reqwest client")?;

        Ok(Self {
            inner: Arc::new(Inner {
                http,
                cache: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// Returns the cached or freshly-fetched file list for `project_id`.
    ///
    /// # Errors
    ///
    /// Returns an error if the upstream call fails or the response shape is
    /// not what the panel expects.
    pub async fn list_files(&self, project_id: u32) -> Result<Arc<Vec<CfFile>>> {
        if let Some(cached) = self.peek_cache(project_id).await {
            return Ok(cached);
        }
        let files = self.fetch_files(project_id).await?;
        let entry = CacheEntry {
            fetched_at: Instant::now(),
            files: Arc::new(files),
        };
        let cloned = Arc::clone(&entry.files);
        self.inner.cache.lock().await.insert(project_id, entry);
        Ok(cloned)
    }

    /// Returns project metadata for `project_id`. No caching — the resolve
    /// endpoint is invoked once per server-create, not on a hot path.
    ///
    /// # Errors
    ///
    /// Returns an error if the project does not exist or the upstream call fails.
    pub async fn project(&self, project_id: u32) -> Result<CfProject> {
        let url = format!("{CF_API}/mods/{project_id}");
        let resp = self.inner.http.get(&url).send().await?;
        let status = resp.status();
        if status.as_u16() == 404 {
            bail!("CurseForge project {project_id} not found");
        }
        if !status.is_success() {
            bail!("CurseForge GET {url} failed: HTTP {status}");
        }
        let wrap: Envelope<CfProject> = resp.json().await.context("decoding /mods/{id}")?;
        Ok(wrap.data)
    }

    /// Resolves a project slug (e.g. `"all-the-mods-11"`) to a project id.
    ///
    /// # Errors
    ///
    /// Returns an error if no Minecraft project matches the slug or the upstream
    /// call fails.
    pub async fn resolve_slug(&self, slug: &str) -> Result<CfProject> {
        let url = format!("{CF_API}/mods/search");
        let resp = self
            .inner
            .http
            .get(&url)
            .query(&[
                ("gameId", MINECRAFT_GAME_ID.to_string().as_str()),
                ("slug", slug),
            ])
            .send()
            .await?;
        if !resp.status().is_success() {
            bail!("CurseForge slug search failed: HTTP {}", resp.status());
        }
        let wrap: Envelope<Vec<CfProject>> = resp.json().await.context("decoding /mods/search")?;
        wrap.data
            .into_iter()
            .find(|p| p.slug == slug)
            .ok_or_else(|| anyhow!("no Minecraft project with slug {slug:?}"))
    }

    async fn peek_cache(&self, project_id: u32) -> Option<Arc<Vec<CfFile>>> {
        let cache = self.inner.cache.lock().await;
        let entry = cache.get(&project_id)?;
        if entry.fetched_at.elapsed() < CACHE_TTL {
            Some(Arc::clone(&entry.files))
        } else {
            None
        }
    }

    async fn fetch_files(&self, project_id: u32) -> Result<Vec<CfFile>> {
        const PAGE_SIZE: u32 = 50;
        const MAX_FILES: usize = 500;

        let url = format!("{CF_API}/mods/{project_id}/files");
        let mut all: Vec<CfFile> = Vec::new();
        let mut index: u32 = 0;
        loop {
            let resp = self
                .inner
                .http
                .get(&url)
                .query(&[
                    ("pageSize", PAGE_SIZE.to_string().as_str()),
                    ("index", index.to_string().as_str()),
                ])
                .send()
                .await?;
            let status = resp.status();
            if status.as_u16() == 404 {
                bail!("CurseForge project {project_id} not found");
            }
            if !status.is_success() {
                bail!("CurseForge GET {url} failed: HTTP {status}");
            }
            let wrap: Envelope<Vec<CfFile>> =
                resp.json().await.context("decoding /mods/{id}/files")?;
            let n = wrap.data.len();
            all.extend(wrap.data);
            if n < PAGE_SIZE as usize || all.len() >= MAX_FILES {
                break;
            }
            index += PAGE_SIZE;
        }
        all.truncate(MAX_FILES);
        Ok(all)
    }
}
