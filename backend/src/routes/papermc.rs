//! `GET /api/papermc/versions` — supported Paper Minecraft versions.
//!
//! itzg's `TYPE=PAPER` rejects MC versions Paper doesn't ship for, so the
//! create form needs to know the Paper-supported subset of the Mojang
//! manifest. `PaperMC`'s official API (`api.papermc.io/v2/projects/paper`)
//! lists every version with at least one Paper build. The result is
//! cached for 1 hour and falls back to a stale cache (then a hardcoded
//! baseline) when the upstream is briefly unreachable.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::AppError;

/// In-memory cache slot held in [`crate::AppState`].
pub type PaperVersionsCache = Arc<Mutex<Option<(Vec<String>, Instant)>>>;

/// Maximum number of versions surfaced to the frontend. Paper supports
/// every patch release back to 1.8 — keeping the dropdown short.
pub const MAX_VERSIONS: usize = 25;

/// How long to keep a parsed listing before re-fetching.
const CACHE_TTL: Duration = Duration::from_hours(1);
/// Per-fetch HTTP timeout.
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);
/// `PaperMC` v2 project endpoint.
const PROJECT_URL: &str = "https://api.papermc.io/v2/projects/paper";

/// Returns a fresh, empty cache slot for use at startup.
#[must_use]
pub fn new_cache() -> PaperVersionsCache {
    Arc::new(Mutex::new(None))
}

#[derive(Deserialize)]
struct ProjectResponse {
    /// `PaperMC` returns versions in ascending order; we reverse before
    /// capping so the dropdown shows newest first.
    versions: Vec<String>,
}

/// Response body for `GET /api/papermc/versions`.
#[derive(Debug, Serialize)]
pub struct PaperVersionsResponse {
    /// Paper-supported MC versions, newest first, capped at [`MAX_VERSIONS`].
    pub versions: Vec<String>,
    /// `"papermc"` on cache hit / fresh fetch; `"fallback"` when the
    /// `PaperMC` API was unreachable AND no cache is available, in which
    /// case the response carries the hardcoded baseline.
    pub source: &'static str,
}

/// Hardcoded fallback for when both the upstream and the cache are unavailable.
/// Updated periodically; the UI labels these as "fallback" so the user knows.
const FALLBACK_VERSIONS: &[&str] = &[
    "1.21.4", "1.21.3", "1.21.1", "1.20.6", "1.20.4", "1.20.2", "1.20.1", "1.19.4", "1.18.2",
];

/// Parses the `PaperMC` project response into a newest-first capped list.
///
/// # Errors
///
/// Returns the underlying [`serde_json::Error`] if the body shape is wrong.
pub fn parse_project(body: &str) -> Result<Vec<String>, serde_json::Error> {
    let p: ProjectResponse = serde_json::from_str(body)?;
    let mut out = p.versions;
    out.reverse();
    out.truncate(MAX_VERSIONS);
    Ok(out)
}

fn cached(cache: &PaperVersionsCache) -> Option<Vec<String>> {
    let g = cache.lock().ok()?;
    g.as_ref().and_then(|(v, at)| {
        if at.elapsed() < CACHE_TTL {
            Some(v.clone())
        } else {
            None
        }
    })
}

/// Returns the cached value regardless of freshness — used as the second
/// fallback step when the upstream is unreachable mid-incident.
fn stale_cached(cache: &PaperVersionsCache) -> Option<Vec<String>> {
    let g = cache.lock().ok()?;
    g.as_ref().map(|(v, _)| v.clone())
}

async fn fetch_and_store(cache: &PaperVersionsCache) -> Result<Vec<String>> {
    let body = reqwest::Client::new()
        .get(PROJECT_URL)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let versions = parse_project(&body)?;
    if let Ok(mut g) = cache.lock() {
        *g = Some((versions.clone(), Instant::now()));
    }
    Ok(versions)
}

/// Returns true when `mc_version` is in the Paper-supported list, hitting
/// cache first and falling back to a fresh fetch. Best-effort — on any
/// error we accept the version (the create still hits itzg, which fails
/// later if the version really doesn't exist) so a transient `PaperMC`
/// outage doesn't block server creation.
pub async fn is_supported(cache: &PaperVersionsCache, mc_version: &str) -> bool {
    if let Some(v) = cached(cache) {
        return v.iter().any(|s| s == mc_version);
    }
    match fetch_and_store(cache).await {
        Ok(v) => v.iter().any(|s| s == mc_version),
        Err(_) => true,
    }
}

/// Handler for `GET /api/papermc/versions`.
///
/// # Errors
///
/// Never errors — upstream failures degrade to stale cache, then to the
/// hardcoded fallback list, both with HTTP 200.
pub async fn handle(
    State(state): State<AppState>,
) -> Result<Json<PaperVersionsResponse>, AppError> {
    if let Some(v) = cached(&state.papermc_cache) {
        return Ok(Json(PaperVersionsResponse {
            versions: v,
            source: "papermc",
        }));
    }
    match fetch_and_store(&state.papermc_cache).await {
        Ok(v) => Ok(Json(PaperVersionsResponse {
            versions: v,
            source: "papermc",
        })),
        Err(e) => {
            tracing::warn!(error = %e, "papermc fetch failed; serving cache or fallback");
            if let Some(stale) = stale_cached(&state.papermc_cache) {
                return Ok(Json(PaperVersionsResponse {
                    versions: stale,
                    source: "papermc",
                }));
            }
            Ok(Json(PaperVersionsResponse {
                versions: FALLBACK_VERSIONS.iter().map(|s| (*s).to_owned()).collect(),
                source: "fallback",
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_project_reverses_and_caps() {
        let body = r#"{"project_id":"paper","versions":["1.18","1.19","1.20","1.21"]}"#;
        let v = parse_project(body).expect("parse");
        assert_eq!(v, vec!["1.21", "1.20", "1.19", "1.18"]);
    }

    #[test]
    fn cap_enforced_at_max_versions() {
        let mut versions = Vec::new();
        for i in 0..(MAX_VERSIONS + 5) {
            versions.push(format!("\"v{i}\""));
        }
        let body = format!("{{\"versions\":[{}]}}", versions.join(","));
        let v = parse_project(&body).expect("parse");
        assert_eq!(v.len(), MAX_VERSIONS);
        // Reversed: newest (highest index) first.
        assert_eq!(v[0], format!("v{}", MAX_VERSIONS + 4));
    }

    #[test]
    fn invalid_json_errors() {
        assert!(parse_project("not json").is_err());
    }

    #[test]
    fn fallback_versions_non_empty() {
        assert!(!FALLBACK_VERSIONS.is_empty());
    }
}
