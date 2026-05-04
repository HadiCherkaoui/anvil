//! `GET /api/cluster/mc-versions` — cached Mojang version manifest.
//!
//! Release versions only, last 20. 24-hour TTL via the `AppState` cache slot.
//! Offline fallback to a hardcoded baseline so the panel stays usable when
//! the Mojang CDN is unreachable.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::AppState;
use crate::error::AppError;
use crate::validation::KNOWN_MC_VERSIONS;

/// Cache slot held in [`AppState`].
pub type McVersionsCache = Arc<Mutex<Option<(Vec<String>, Instant)>>>;

/// Maximum number of release versions returned to clients.
pub const MAX_VERSIONS: usize = 20;
/// How long to keep a fetched manifest before re-fetching.
const CACHE_TTL: Duration = Duration::from_hours(24);
/// Mojang version manifest URL.
const MANIFEST_URL: &str = "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json";
/// Per-fetch HTTP timeout.
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);

/// Returns a fresh, empty cache slot for use at startup.
#[must_use]
pub fn new_cache() -> McVersionsCache {
    Arc::new(Mutex::new(None))
}

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

/// Response body for `GET /api/cluster/mc-versions`.
#[derive(Debug, Serialize)]
pub struct McVersionsResponse {
    /// Release versions, most recent first, capped at [`MAX_VERSIONS`].
    pub versions: Vec<String>,
    /// `"mojang"` when freshly fetched or cache-hit; `"fallback"` when the
    /// Mojang manifest was unreachable and the hardcoded baseline is served.
    pub source: &'static str,
}

/// Parses the Mojang manifest JSON into a release-only, capped version list.
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
        .take(MAX_VERSIONS)
        .collect();
    out.shrink_to_fit();
    Ok(out)
}

/// Reads the cache, returning `Some` when fresh.
pub async fn cached(cache: &McVersionsCache) -> Option<Vec<String>> {
    let guard = cache.lock().await;
    guard.as_ref().and_then(|(versions, at)| {
        if at.elapsed() < CACHE_TTL {
            Some(versions.clone())
        } else {
            None
        }
    })
}

/// Fetches the manifest, parses it, populates the cache, and returns the list.
///
/// On any HTTP / parse failure returns `Err`; callers decide whether to
/// surface the error or fall back to [`KNOWN_MC_VERSIONS`].
async fn fetch_and_store(cache: &McVersionsCache) -> anyhow::Result<Vec<String>> {
    let body = reqwest::Client::new()
        .get(MANIFEST_URL)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let versions = parse_manifest(&body)?;
    let mut guard = cache.lock().await;
    *guard = Some((versions.clone(), Instant::now()));
    Ok(versions)
}

/// Handler for `GET /api/cluster/mc-versions`.
///
/// # Errors
///
/// Never errors — Mojang failures degrade to the fallback list with HTTP 200.
pub async fn handle(State(state): State<AppState>) -> Result<Json<McVersionsResponse>, AppError> {
    if let Some(cached) = cached(&state.mc_versions_cache).await {
        return Ok(Json(McVersionsResponse {
            versions: cached,
            source: "mojang",
        }));
    }
    match fetch_and_store(&state.mc_versions_cache).await {
        Ok(versions) => Ok(Json(McVersionsResponse {
            versions,
            source: "mojang",
        })),
        Err(e) => {
            tracing::warn!(error = %e, "mc-versions: mojang fetch failed; serving fallback");
            Ok(Json(McVersionsResponse {
                versions: KNOWN_MC_VERSIONS.iter().map(|s| (*s).to_owned()).collect(),
                source: "fallback",
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn empty_versions_is_ok() {
        let json = r#"{"latest": {"release": "1.21.4"}, "versions": []}"#;
        let v = parse_manifest(json).expect("parses");
        assert!(v.is_empty());
    }

    #[test]
    fn cap_enforced_at_max_versions() {
        let mut versions = Vec::new();
        for i in 0..(MAX_VERSIONS + 5) {
            versions.push(format!(r#"{{"id":"v{i}","type":"release"}}"#));
        }
        let json = format!(
            r#"{{"latest":{{"release":"v0"}},"versions":[{}]}}"#,
            versions.join(",")
        );
        let v = parse_manifest(&json).expect("parses");
        assert_eq!(v.len(), MAX_VERSIONS);
        assert_eq!(v[0], "v0");
    }

    #[test]
    fn invalid_json_errors() {
        assert!(parse_manifest("not json").is_err());
    }
}
