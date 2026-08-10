// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Modrinth API client (`api.modrinth.com/v2`).
//!
//! No auth required; sets a polite `User-Agent` per Modrinth's API docs.
//! `list_versions` is cached for an hour, mirroring `CurseForgeClient`'s
//! `list_files` cache. `search` is not cached (each query is distinct).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, RETRY_AFTER, USER_AGENT};
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::{Level, event};

/// Maximum retries for transient upstream errors (429 / 5xx).
const MAX_RETRIES: u32 = 3;
/// Initial backoff between retries; doubles each attempt up to [`BACKOFF_CAP`].
const BACKOFF_INITIAL: Duration = Duration::from_secs(2);
/// Hard cap on a single backoff sleep. Modrinth occasionally returns
/// `Retry-After` values minutes long; we honour them but ceil here.
const BACKOFF_CAP: Duration = Duration::from_mins(1);
/// Fallback when `Retry-After` is an HTTP-date we don't bother parsing.
const RETRY_AFTER_HTTP_DATE_FALLBACK: Duration = Duration::from_secs(30);

/// Send `req` with retry on 429 + 5xx. Honours `Retry-After` (seconds form)
/// and falls back to capped exponential backoff. Returns the first
/// successful response or the last error.
async fn with_retry(
    http: &reqwest::Client,
    build: impl Fn() -> reqwest::RequestBuilder,
) -> Result<reqwest::Response> {
    let mut backoff = BACKOFF_INITIAL;
    let mut last_status: Option<reqwest::StatusCode> = None;
    for attempt in 0..=MAX_RETRIES {
        let req = build().build().context("building Modrinth request")?;
        match http.execute(req).await {
            Ok(resp) => {
                let status = resp.status();
                let retryable = status.as_u16() == 429 || status.is_server_error();
                if !retryable || attempt == MAX_RETRIES {
                    return Ok(resp);
                }
                let wait = parse_retry_after(resp.headers().get(RETRY_AFTER)).unwrap_or(backoff);
                event!(
                    name: "anvil.modpack.mr.retry",
                    Level::WARN,
                    status = %status,
                    attempt,
                    wait_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
                    "Modrinth transient error; retrying",
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
                    name: "anvil.modpack.mr.retry",
                    Level::WARN,
                    err = %err,
                    attempt,
                    "Modrinth network error; retrying",
                );
                sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_CAP);
            }
        }
    }
    // Unreachable: the loop always returns on the final iteration.
    if let Some(status) = last_status {
        bail!("Modrinth retry exhausted at HTTP {status}");
    }
    bail!("Modrinth retry exhausted")
}

fn parse_retry_after(hv: Option<&HeaderValue>) -> Option<Duration> {
    let raw = hv?.to_str().ok()?.trim();
    if let Ok(secs) = raw.parse::<u64>() {
        return Some(Duration::from_secs(secs).min(BACKOFF_CAP));
    }
    // HTTP-date form (RFC 7231); we don't parse it — fall back to a sane default.
    Some(RETRY_AFTER_HTTP_DATE_FALLBACK)
}

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
/// Soft cap on cache entries before TTL-based eviction kicks in.
const CACHE_MAX_ENTRIES: usize = 256;

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
    #[serde(default)]
    pub dependencies: Vec<MrDependency>,
}

/// One entry in [`MrVersion::dependencies`].
///
/// `dependency_type` is `"required"` | `"optional"` | `"incompatible"` | `"embedded"`.
#[derive(Debug, Clone, Deserialize)]
pub struct MrDependency {
    pub version_id: Option<String>,
    pub project_id: Option<String>,
    pub file_name: Option<String>,
    pub dependency_type: String,
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
        let resp = with_retry(&self.inner.http, || self.inner.http.get(&url)).await?;
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
        let resp = with_retry(&self.inner.http, || self.inner.http.get(&url)).await?;
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
        let mut cache = self.inner.cache.lock().await;
        if cache.len() >= CACHE_MAX_ENTRIES {
            evict(&mut cache);
        }
        cache.insert(project_id.to_owned(), entry);
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
        let limit_s = limit.to_string();
        let offset_s = q.offset.to_string();
        let resp = with_retry(&self.inner.http, || {
            self.inner.http.get(format!("{MR_API}/search")).query(&[
                ("query", q.query),
                ("facets", facets_json.as_str()),
                ("limit", limit_s.as_str()),
                ("offset", offset_s.as_str()),
                ("index", "relevance"),
            ])
        })
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
        let resp = with_retry(&self.inner.http, || self.inner.http.get(&url)).await?;
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

/// Evicts expired entries first; if still over capacity, drops the oldest
/// half by `fetched_at`. Caller holds the cache lock.
fn evict(cache: &mut HashMap<String, CacheEntry>) {
    cache.retain(|_, e| e.fetched_at.elapsed() < CACHE_TTL);
    if cache.len() < CACHE_MAX_ENTRIES {
        return;
    }
    let mut by_age: Vec<(String, Instant)> = cache
        .iter()
        .map(|(k, v)| (k.clone(), v.fetched_at))
        .collect();
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
    fn parses_dependency_array() {
        let body = r#"{
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
        let v: MrVersion = serde_json::from_str(body).expect("parses");
        assert_eq!(v.dependencies.len(), 2);
        assert_eq!(v.dependencies[0].project_id.as_deref(), Some("fabric-api"));
        assert_eq!(v.dependencies[0].dependency_type, "required");
        assert_eq!(v.dependencies[1].version_id.as_deref(), Some("ver-x"));
        assert_eq!(v.dependencies[1].dependency_type, "optional");
    }

    #[test]
    fn missing_dependency_array_is_empty() {
        let body = r#"{
            "id": "v", "project_id": "p", "name": "X", "version_number": "1",
            "version_type": "release", "loaders": [], "game_versions": [],
            "date_published": "2026-01-01T00:00:00Z", "files": []
        }"#;
        let v: MrVersion = serde_json::from_str(body).expect("parses");
        assert!(v.dependencies.is_empty());
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
