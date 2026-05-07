//! `CurseForge` HTTP client with a 1h per-project file-list cache.
//!
//! Calls land on `api.curseforge.com/v1`; the API key comes from the
//! `CF_API_KEY` env (mounted from the `cf-api-key` Secret). Responses for
//! `/mods/{id}/files` are cached for an hour so the hourly poller and any
//! ad-hoc requests share one upstream call per project.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, RETRY_AFTER};
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::{Level, event};

/// Maximum retries for transient upstream errors (429 / 5xx).
const MAX_RETRIES: u32 = 3;
const BACKOFF_INITIAL: Duration = Duration::from_secs(2);
const BACKOFF_CAP: Duration = Duration::from_mins(1);
const RETRY_AFTER_HTTP_DATE_FALLBACK: Duration = Duration::from_secs(30);

async fn with_retry(
    http: &reqwest::Client,
    build: impl Fn() -> reqwest::RequestBuilder,
) -> Result<reqwest::Response> {
    let mut backoff = BACKOFF_INITIAL;
    let mut last_status: Option<reqwest::StatusCode> = None;
    for attempt in 0..=MAX_RETRIES {
        let req = build().build().context("building CurseForge request")?;
        match http.execute(req).await {
            Ok(resp) => {
                let status = resp.status();
                let retryable = status.as_u16() == 429 || status.is_server_error();
                if !retryable || attempt == MAX_RETRIES {
                    return Ok(resp);
                }
                let wait = parse_retry_after(resp.headers().get(RETRY_AFTER)).unwrap_or(backoff);
                event!(
                    name: "anvil.modpack.cf.retry",
                    Level::WARN,
                    status = %status,
                    attempt,
                    wait_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
                    "CurseForge transient error; retrying",
                );
                last_status = Some(status);
                sleep(wait).await;
                backoff = (backoff * 2).min(BACKOFF_CAP);
            }
            Err(err) => {
                if attempt == MAX_RETRIES {
                    return Err(anyhow::Error::from(err));
                }
                event!(
                    name: "anvil.modpack.cf.retry",
                    Level::WARN,
                    err = %err,
                    attempt,
                    "CurseForge network error; retrying",
                );
                sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_CAP);
            }
        }
    }
    if let Some(status) = last_status {
        bail!("CurseForge retry exhausted at HTTP {status}");
    }
    bail!("CurseForge retry exhausted")
}

fn parse_retry_after(hv: Option<&HeaderValue>) -> Option<Duration> {
    let raw = hv?.to_str().ok()?.trim();
    if let Ok(secs) = raw.parse::<u64>() {
        return Some(Duration::from_secs(secs).min(BACKOFF_CAP));
    }
    Some(RETRY_AFTER_HTTP_DATE_FALLBACK)
}

/// `CurseForge` API base URL.
const CF_API: &str = "https://api.curseforge.com/v1";

/// Minecraft game id on `CurseForge` — used to scope project searches.
const MINECRAFT_GAME_ID: u32 = 432;

/// `CurseForge` class id for Minecraft modpacks. Without this filter
/// `/mods/search` returns mods, modpacks, worlds and resource packs mixed.
const MODPACK_CLASS_ID: u32 = 4471;

/// File-list cache TTL.
const CACHE_TTL: Duration = Duration::from_hours(1);
/// Soft cap on cache entries before TTL-based eviction kicks in.
const CACHE_MAX_ENTRIES: usize = 256;

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
    /// File id of the linked server-pack file, when the project uploads
    /// the server pack as a sibling file rather than including it in the
    /// regular listing. Most modern modpacks (ATM-11, FTB, etc.) take this
    /// route: every "main" file has `is_server_pack: false` and points here.
    /// Resolve via [`CurseForgeClient::file`].
    #[serde(rename = "serverPackFileId", default)]
    pub server_pack_file_id: Option<u32>,
    /// Direct download URL — may be null when the project disabled API distribution.
    #[serde(rename = "downloadUrl")]
    pub download_url: Option<String>,
    /// Unix-second upload timestamp (we expose ISO 8601 from the API field
    /// `fileDate`); kept as the raw string and parsed only when needed.
    #[serde(rename = "fileDate", default)]
    pub file_date: String,
    /// Actual disk filename (e.g. `mymod-1.2.3.jar`). Distinct from
    /// `displayName`. Used to construct `ModEntry.filename` when resolving
    /// dependencies; older code paths fall back to `display_name`.
    #[serde(rename = "fileName", default)]
    pub file_name: String,
    /// CF interleaves MC versions and loader labels in this single array
    /// (e.g. `["1.21.4", "Forge", "Fabric"]`). Kept for the modpack
    /// version listing; individual mods don't go through this client.
    #[serde(rename = "gameVersions", default)]
    pub game_versions: Vec<String>,
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
    /// One-line description shown in catalog cards.
    #[serde(default)]
    pub summary: String,
    /// Total downloads across all files.
    #[serde(rename = "downloadCount", default)]
    pub download_count: u64,
    /// `CurseForge` "thumbs up" count — surfaced as `follows` in the
    /// merged catalog so the panel can sort / display popularity.
    #[serde(rename = "thumbsUpCount", default)]
    pub thumbs_up_count: u64,
    /// Logo image; only the URL is consumed.
    #[serde(default)]
    pub logo: Option<CfLogo>,
    /// Project authors. The catalog UI shows the first.
    #[serde(default)]
    pub authors: Vec<CfAuthor>,
    /// Per-file index used to derive the loader/game-version chips in
    /// the catalog UI without re-paginating `/mods/{id}/files`.
    #[serde(rename = "latestFilesIndexes", default)]
    pub latest_files_indexes: Vec<CfFileIndex>,
    /// ISO 8601 last-modified timestamp.
    #[serde(rename = "dateModified", default)]
    pub date_modified: String,
}

/// `CurseForge` logo wrapper — only `url` is used.
#[derive(Debug, Clone, Deserialize)]
pub struct CfLogo {
    pub url: String,
}

/// `CurseForge` author entry — only `name` is used.
#[derive(Debug, Clone, Deserialize)]
pub struct CfAuthor {
    pub name: String,
}

/// One row of `CfProject.latest_files_indexes`. The id mapping comes from
/// the `CurseForge` `modLoaderType` enum: 1 `Forge`, 4 `Fabric`, 5 `Quilt`,
/// 6 `NeoForge`.
#[derive(Debug, Clone, Deserialize)]
pub struct CfFileIndex {
    #[serde(rename = "gameVersion")]
    pub game_version: String,
    #[serde(rename = "modLoader", default)]
    pub mod_loader: Option<u8>,
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
        let mut cache = self.inner.cache.lock().await;
        if cache.len() >= CACHE_MAX_ENTRIES {
            evict(&mut cache);
        }
        cache.insert(project_id, entry);
        Ok(cloned)
    }

    /// Fetches one file by id. Used to resolve linked server-pack files
    /// that don't appear in the regular `/mods/{id}/files` listing.
    ///
    /// No caching — invoked at most once per server-create or update swap.
    ///
    /// # Errors
    ///
    /// Returns an error if the file does not exist or the upstream call fails.
    pub async fn file(&self, project_id: u32, file_id: u32) -> Result<CfFile> {
        let url = format!("{CF_API}/mods/{project_id}/files/{file_id}");
        let resp = with_retry(&self.inner.http, || self.inner.http.get(&url)).await?;
        let status = resp.status();
        if status.as_u16() == 404 {
            bail!("CurseForge file {file_id} not found in project {project_id}");
        }
        if !status.is_success() {
            bail!("CurseForge GET {url} failed: HTTP {status}");
        }
        let wrap: Envelope<CfFile> = resp
            .json()
            .await
            .context("decoding /mods/{id}/files/{id}")?;
        Ok(wrap.data)
    }

    /// Returns project metadata for `project_id`. No caching — the resolve
    /// endpoint is invoked once per server-create, not on a hot path.
    ///
    /// # Errors
    ///
    /// Returns an error if the project does not exist or the upstream call fails.
    pub async fn project(&self, project_id: u32) -> Result<CfProject> {
        let url = format!("{CF_API}/mods/{project_id}");
        let resp = with_retry(&self.inner.http, || self.inner.http.get(&url)).await?;
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

    /// Searches `CurseForge` modpacks by free-text query. Returns up to
    /// `page_size` hits starting at `index`, sorted by `CurseForge`'s
    /// default ranking (popularity-weighted).
    ///
    /// # Errors
    ///
    /// Returns an error if the upstream call fails or the response shape
    /// is not what we expect.
    pub async fn search(&self, query: &str, page_size: u32, index: u32) -> Result<Vec<CfProject>> {
        let url = format!("{CF_API}/mods/search");
        let game_id_s = MINECRAFT_GAME_ID.to_string();
        let class_id_s = MODPACK_CLASS_ID.to_string();
        let page_size_s = page_size.to_string();
        let index_s = index.to_string();
        let resp = with_retry(&self.inner.http, || {
            self.inner.http.get(&url).query(&[
                ("gameId", game_id_s.as_str()),
                ("classId", class_id_s.as_str()),
                ("searchFilter", query),
                ("pageSize", page_size_s.as_str()),
                ("index", index_s.as_str()),
            ])
        })
        .await?;
        if !resp.status().is_success() {
            bail!("CurseForge search failed: HTTP {}", resp.status());
        }
        let wrap: Envelope<Vec<CfProject>> = resp.json().await.context("decoding /mods/search")?;
        Ok(wrap.data)
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
        // Bumped from 500 — long-lived modpacks (ATM, FTB) easily exceed
        // ten pages of historical builds, and `target file not in first
        // 500` was silently truncating selectable versions.
        const MAX_FILES: usize = 2000;

        let url = format!("{CF_API}/mods/{project_id}/files");
        let mut all: Vec<CfFile> = Vec::new();
        let mut index: u32 = 0;
        loop {
            let page_size_s = PAGE_SIZE.to_string();
            let index_s = index.to_string();
            let resp = with_retry(&self.inner.http, || {
                self.inner.http.get(&url).query(&[
                    ("pageSize", page_size_s.as_str()),
                    ("index", index_s.as_str()),
                ])
            })
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

/// Evicts expired entries first; if still over capacity, drops the oldest
/// half by `fetched_at`. Caller holds the cache lock.
fn evict(cache: &mut HashMap<u32, CacheEntry>) {
    cache.retain(|_, e| e.fetched_at.elapsed() < CACHE_TTL);
    if cache.len() < CACHE_MAX_ENTRIES {
        return;
    }
    let mut by_age: Vec<(u32, Instant)> = cache.iter().map(|(k, v)| (*k, v.fetched_at)).collect();
    by_age.sort_by_key(|(_, t)| *t);
    let drop_n = cache.len() - (CACHE_MAX_ENTRIES / 2);
    for (k, _) in by_age.into_iter().take(drop_n) {
        cache.remove(&k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cf_file_ignores_dependencies_field() {
        // The dependencies field used to be parsed for the per-mod CF
        // dep resolver; that path is gone (mods are Modrinth-only).
        // CF still emits the field — serde must keep tolerating it.
        let body = r#"{
            "id": 1, "displayName": "X", "releaseType": 1,
            "dependencies": [{ "modId": 200, "relationType": 3 }]
        }"#;
        let f: CfFile = serde_json::from_str(body).expect("parses");
        assert_eq!(f.id, 1);
    }
}
