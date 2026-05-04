//! Modrinth API client (`api.modrinth.com/v2`).
//!
//! No auth required; sets a polite `User-Agent` per Modrinth's API docs.
//! `list_versions` is cached for an hour, mirroring `CurseForgeClient`'s
//! `list_files` cache. `search` is not cached (each query is distinct).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, USER_AGENT};
use serde::Deserialize;
use tokio::sync::Mutex;

/// Modrinth API base URL.
const MR_API: &str = "https://api.modrinth.com/v2";
/// Polite User-Agent — Modrinth's API docs ask for one.
const MR_USER_AGENT: &str = concat!(
    "anvil/",
    env!("CARGO_PKG_VERSION"),
    " (https://gitlab.cherkaoui.ch/HadiCherkaoui/anvil)"
);
/// Version-list cache TTL.
const CACHE_TTL: Duration = Duration::from_hours(1);

/// Project metadata from `/project/{id_or_slug}`.
#[derive(Debug, Clone, Deserialize)]
pub struct MrProject {
    pub id: String,
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// `"mod"` | `"modpack"` | `"plugin"` | …
    #[serde(default)]
    pub project_type: String,
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

/// One version entry from `/project/{id}/version` or `/version/{id}`.
#[derive(Debug, Clone, Deserialize)]
pub struct MrVersion {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub version_number: String,
    /// `"release"` | `"beta"` | `"alpha"`
    pub version_type: String,
    pub loaders: Vec<String>,
    pub game_versions: Vec<String>,
    pub date_published: String,
    pub files: Vec<MrFile>,
}

/// One file inside an [`MrVersion`].
#[derive(Debug, Clone, Deserialize)]
pub struct MrFile {
    pub url: String,
    pub filename: String,
    pub primary: bool,
    #[serde(default)]
    pub hashes: MrHashes,
}

/// File hashes returned by Modrinth.
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
    /// `"mod"` | `"modpack"` | `"plugin"`
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

/// Modrinth HTTP client. Cheap to clone via `Arc` wrapping.
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
    /// Builds a Modrinth client with the polite `User-Agent`.
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
        let versions: Vec<MrVersion> = resp.json().await.context("decoding /version list")?;
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
        let facets_json = serde_json::to_string(&facets).context("serializing search facets")?;
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
        resp.json::<MrVersion>()
            .await
            .context("decoding /version/{id}")
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
}
