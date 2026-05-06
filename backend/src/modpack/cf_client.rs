//! `CurseForge` HTTP client with a 1h per-project file-list cache.
//!
//! Calls land on `api.curseforge.com/v1`; the API key comes from the
//! `CF_API_KEY` env (mounted from the `cf-api-key` Secret). Responses for
//! `/mods/{id}/files` are cached for an hour so the hourly poller and any
//! ad-hoc requests share one upstream call per project.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context as _, Result};
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT};
use serde::Deserialize;
use tokio::sync::Mutex;

/// `CurseForge` API base URL.
const CF_API: &str = "https://api.curseforge.com/v1";

/// Minecraft game id on `CurseForge` — used to scope project searches.
const MINECRAFT_GAME_ID: u32 = 432;

/// `CurseForge` class id for Minecraft modpacks. Without this filter
/// `/mods/search` returns mods, modpacks, worlds and resource packs mixed.
const MODPACK_CLASS_ID: u32 = 4471;

/// File-list cache TTL.
const CACHE_TTL: Duration = Duration::from_hours(1);

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
    /// (e.g. `["1.21.4", "Forge", "Fabric"]`). Used for dep-resolver
    /// compatibility filtering.
    #[serde(rename = "gameVersions", default)]
    pub game_versions: Vec<String>,
    /// Required / optional / incompatible / etc. relations to other projects.
    #[serde(default)]
    pub dependencies: Vec<CfDependency>,
}

/// One entry in [`CfFile::dependencies`].
///
/// `relation_type` mapping per `CurseForge`: `1` embedded, `2` optional,
/// `3` required, `4` tool, `5` incompatible, `6` include.
#[derive(Debug, Clone, Deserialize)]
pub struct CfDependency {
    #[serde(rename = "modId")]
    pub mod_id: u32,
    #[serde(rename = "relationType")]
    pub relation_type: u8,
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
        self.inner.cache.lock().await.insert(project_id, entry);
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
        let resp = self.inner.http.get(&url).send().await?;
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
        let resp = self
            .inner
            .http
            .get(&url)
            .query(&[
                ("gameId", MINECRAFT_GAME_ID.to_string().as_str()),
                ("classId", MODPACK_CLASS_ID.to_string().as_str()),
                ("searchFilter", query),
                ("pageSize", page_size.to_string().as_str()),
                ("index", index.to_string().as_str()),
            ])
            .send()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cf_file_with_dependencies() {
        let body = r#"{
            "id": 1,
            "displayName": "X",
            "releaseType": 1,
            "downloadUrl": "https://example.com/x.jar",
            "dependencies": [
                { "modId": 200, "relationType": 3 },
                { "modId": 201, "relationType": 2 },
                { "modId": 202, "relationType": 5 },
                { "modId": 203, "relationType": 6 }
            ]
        }"#;
        let f: CfFile = serde_json::from_str(body).expect("parses");
        assert_eq!(f.dependencies.len(), 4);
        assert_eq!(f.dependencies[0].mod_id, 200);
        assert_eq!(f.dependencies[0].relation_type, 3);
        assert_eq!(f.dependencies[1].relation_type, 2);
        assert_eq!(f.dependencies[3].relation_type, 6);
    }

    #[test]
    fn missing_dependencies_array_is_empty() {
        let body = r#"{
            "id": 1, "displayName": "X", "releaseType": 1
        }"#;
        let f: CfFile = serde_json::from_str(body).expect("parses");
        assert!(f.dependencies.is_empty());
    }
}
