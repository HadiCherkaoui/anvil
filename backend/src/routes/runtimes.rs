//! `GET /api/runtimes/{runtime}/versions` — Forge / `NeoForge` loader versions.
//!
//! Anvil scrapes the upstream Maven `maven-metadata.xml` for each runtime,
//! groups versions by their corresponding Minecraft version, and serves
//! the result with a 1-hour in-memory cache. Frontend uses the result to
//! render cascading MC ↔ loader pickers in the create form so users only
//! pick combinations that actually exist (`NeoForge` skips MC versions, so
//! the previous LATEST-everywhere flow installed-failed silently).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::AppError;

/// Parsed loader-version listing returned by the endpoint.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct LoaderVersions {
    /// MC versions known to have at least one loader release, newest first.
    pub mc_versions: Vec<String>,
    /// Loader versions per MC version, newest first.
    pub by_mc: BTreeMap<String, Vec<String>>,
}

/// In-memory cache slot held in [`crate::AppState`]. Keyed by runtime name
/// (`"forge"` / `"neoforge"`).
pub type LoaderVersionCache = Arc<Mutex<HashMap<&'static str, (LoaderVersions, Instant)>>>;

/// Returns a fresh, empty cache slot for use at startup.
#[must_use]
pub fn new_cache() -> LoaderVersionCache {
    Arc::new(Mutex::new(HashMap::new()))
}

/// How long parsed loader-version listings stay in the cache.
const CACHE_TTL: Duration = Duration::from_hours(1);
/// Per-fetch HTTP timeout.
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);
const NEOFORGE_URL: &str =
    "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml";
const FORGE_URL: &str =
    "https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml";

/// Test-only seeder so integration tests can prime the cache without
/// hitting the upstream maven.
#[doc(hidden)]
pub fn prime_cache(cache: &LoaderVersionCache, runtime: &'static str, parsed: LoaderVersions) {
    if let Ok(mut g) = cache.lock() {
        g.insert(runtime, (parsed, Instant::now()));
    }
}

/// Fetches `runtime`'s loader listing through the cache; returns a clone
/// of the parsed value. Used by the poller to detect newer loader
/// versions. `runtime` must be `"forge"` or `"neoforge"`.
///
/// # Errors
///
/// Returns the upstream HTTP / parse error if the cache is empty AND
/// the maven fetch fails. Stale-cache fallback mirrors the route handler.
pub async fn cached_or_fetch(
    cache: &LoaderVersionCache,
    runtime: &'static str,
) -> Result<LoaderVersions> {
    if let Some(v) = read_cache(cache, runtime) {
        return Ok(v);
    }
    let url = if runtime == "neoforge" {
        NEOFORGE_URL
    } else {
        FORGE_URL
    };
    match fetch_and_parse(url, runtime).await {
        Ok(parsed) => {
            write_cache(cache, runtime, &parsed);
            Ok(parsed)
        }
        Err(e) => {
            if let Some(stale) = stale_cache(cache, runtime) {
                tracing::warn!(runtime, error = %e, "loader cache fetch failed; using stale");
                return Ok(stale);
            }
            Err(e)
        }
    }
}

fn read_cache(cache: &LoaderVersionCache, key: &str) -> Option<LoaderVersions> {
    let g = cache.lock().ok()?;
    let (v, ts) = g.get(key)?;
    if ts.elapsed() < CACHE_TTL {
        Some(v.clone())
    } else {
        None
    }
}

fn write_cache(cache: &LoaderVersionCache, key: &'static str, v: &LoaderVersions) {
    if let Ok(mut g) = cache.lock() {
        g.insert(key, (v.clone(), Instant::now()));
    }
}

/// Handler for `GET /api/runtimes/{runtime}/versions`.
///
/// # Errors
///
/// - 404 `unknown_runtime` when `runtime` is anything other than `forge` or
///   `neoforge` (Fabric covers every MC release; no listing is needed).
/// - 503 `loader_versions_unreachable` if the upstream maven is down AND the
///   cache is empty. A populated-but-stale cache is served as a graceful
///   fallback so a transient outage doesn't break the create form.
#[utoipa::path(
    get,
    path = "/api/runtimes/{runtime}/versions",
    params(
        ("runtime" = String, Path, description = "Loader runtime: forge | neoforge")
    ),
    responses(
        (status = 200, description = "Loader versions grouped by Minecraft version", body = LoaderVersions),
        (status = 404, description = "Unknown runtime"),
        (status = 503, description = "Upstream Maven unavailable and cache empty")
    ),
    tag = "runtimes"
)]
pub async fn handle_versions(
    Path(runtime): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<LoaderVersions>, AppError> {
    let key: &'static str = match runtime.as_str() {
        "neoforge" => "neoforge",
        "forge" => "forge",
        _ => {
            return Err(AppError::NotFound);
        }
    };

    if let Some(v) = read_cache(&state.loader_version_cache, key) {
        return Ok(Json(v));
    }

    let url = if key == "neoforge" {
        NEOFORGE_URL
    } else {
        FORGE_URL
    };
    match fetch_and_parse(url, key).await {
        Ok(parsed) => {
            write_cache(&state.loader_version_cache, key, &parsed);
            Ok(Json(parsed))
        }
        Err(e) => {
            tracing::warn!(runtime = key, error = %e, "loader-versions upstream failed");
            // Best-effort: serve stale cache if any (the freshness check above
            // returned None because the entry was either missing or expired —
            // serving expired beats failing the create form entirely).
            if let Some(stale) = stale_cache(&state.loader_version_cache, key) {
                return Ok(Json(stale));
            }
            Err(AppError::Internal(anyhow::anyhow!(
                "loader_versions_unreachable: {e}"
            )))
        }
    }
}

async fn fetch_and_parse(url: &str, runtime: &str) -> Result<LoaderVersions> {
    static HTTP: OnceLock<reqwest::Client> = OnceLock::new();
    let xml = HTTP
        .get_or_init(reqwest::Client::new)
        .get(url)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    if runtime == "neoforge" {
        parse_neoforge(&xml)
    } else {
        parse_forge(&xml)
    }
}

fn stale_cache(cache: &LoaderVersionCache, key: &str) -> Option<LoaderVersions> {
    let g = cache.lock().ok()?;
    g.get(key).map(|(v, _)| v.clone())
}

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

fn read_versions(xml: &str) -> Result<Vec<String>> {
    let m: MavenMetadata = quick_xml::de::from_str(xml)?;
    Ok(m.versioning.versions.version)
}

/// Parses a `NeoForge` `maven-metadata.xml` and groups loader versions by
/// the MC version they target. Beta entries (`*-beta` suffix) are dropped.
///
/// `NeoForge` versions follow `<a>.<b>.<c>` where `1.<a>.<b>` is the MC
/// version (e.g. `21.4.81` ⇒ `1.21.4`).
///
/// # Errors
///
/// Returns the underlying `quick_xml` error if `xml` is malformed.
pub fn parse_neoforge(xml: &str) -> Result<LoaderVersions> {
    let raws = read_versions(xml)?;
    let mut by_mc: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for raw in raws {
        if raw.contains("-beta") {
            continue;
        }
        let parts: Vec<&str> = raw.splitn(3, '.').collect();
        if parts.len() < 2 || parts[0].parse::<u32>().is_err() || parts[1].parse::<u32>().is_err() {
            continue;
        }
        let mc = format!("1.{}.{}", parts[0], parts[1]);
        by_mc.entry(mc).or_default().push(raw);
    }
    let mut mc_versions: Vec<String> = by_mc.keys().cloned().collect();
    sort_mc_desc(&mut mc_versions);
    for v in by_mc.values_mut() {
        sort_loader_versions_desc(v);
    }
    Ok(LoaderVersions { mc_versions, by_mc })
}

/// Parses a Forge `maven-metadata.xml` and groups loader versions by the MC
/// version they target.
///
/// Forge versions look like `<mc>-<loader>` (e.g. `1.21.4-54.1.0`); the full
/// string is what `FORGE_VERSION` consumes downstream, so we keep the raw
/// `<version>` text.
///
/// # Errors
///
/// Returns the underlying `quick_xml` error if `xml` is malformed.
pub fn parse_forge(xml: &str) -> Result<LoaderVersions> {
    let raws = read_versions(xml)?;
    let mut by_mc: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for raw in raws {
        let Some((mc, _loader)) = raw.split_once('-') else {
            continue;
        };
        by_mc.entry(mc.to_owned()).or_default().push(raw);
    }
    let mut mc_versions: Vec<String> = by_mc.keys().cloned().collect();
    sort_mc_desc(&mut mc_versions);
    for v in by_mc.values_mut() {
        sort_loader_versions_desc(v);
    }
    Ok(LoaderVersions { mc_versions, by_mc })
}

fn sort_mc_desc(v: &mut [String]) {
    v.sort_by(|a, b| {
        let pa: Vec<u32> = a.split('.').filter_map(|s| s.parse().ok()).collect();
        let pb: Vec<u32> = b.split('.').filter_map(|s| s.parse().ok()).collect();
        pb.cmp(&pa)
    });
}

/// Sorts loader versions newest-first, comparing numeric segments
/// numerically — plain string sort puts `54.0.9` above `54.0.10`.
/// Handles both `NeoForge` (`21.4.81`) and Forge (`1.21.4-54.1.0`) shapes
/// by splitting on `.` and `-`; ties fall back to string order.
fn sort_loader_versions_desc(v: &mut [String]) {
    fn key(s: &str) -> Vec<u64> {
        s.split(['.', '-'])
            .map(|seg| seg.parse().unwrap_or(0))
            .collect()
    }
    v.sort_by(|a, b| key(b).cmp(&key(a)).then_with(|| b.cmp(a)));
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEOFORGE_FIXTURE: &str = include_str!("../../tests/fixtures/neoforge_maven_metadata.xml");
    const FORGE_FIXTURE: &str = include_str!("../../tests/fixtures/forge_maven_metadata.xml");

    #[test]
    fn parse_neoforge_groups_by_mc_version() {
        let v = parse_neoforge(NEOFORGE_FIXTURE).expect("parse");
        assert_eq!(
            v.mc_versions,
            vec![
                "1.21.4".to_owned(),
                "1.21.1".to_owned(),
                "1.20.6".to_owned()
            ]
        );
        assert_eq!(
            v.by_mc.get("1.21.4").unwrap(),
            &vec!["21.4.81".to_owned(), "21.4.80".to_owned()]
        );
    }

    #[test]
    fn parse_neoforge_skips_beta_versions() {
        let v = parse_neoforge(NEOFORGE_FIXTURE).expect("parse");
        let combined: Vec<&String> = v.by_mc.values().flatten().collect();
        assert!(combined.iter().all(|s| !s.contains("-beta")));
    }

    #[test]
    fn parse_forge_groups_by_mc_prefix() {
        let v = parse_forge(FORGE_FIXTURE).expect("parse");
        assert_eq!(
            v.mc_versions,
            vec![
                "1.21.4".to_owned(),
                "1.21.1".to_owned(),
                "1.20.1".to_owned()
            ]
        );
        assert_eq!(
            v.by_mc.get("1.21.4").unwrap(),
            &vec!["1.21.4-54.1.0".to_owned(), "1.21.4-54.0.50".to_owned()]
        );
    }

    #[test]
    fn loader_sort_compares_segments_numerically() {
        let mut neo = vec!["21.1.9".to_owned(), "21.1.100".to_owned()];
        sort_loader_versions_desc(&mut neo);
        assert_eq!(neo, vec!["21.1.100".to_owned(), "21.1.9".to_owned()]);

        let mut forge = vec!["1.21.4-54.0.9".to_owned(), "1.21.4-54.0.10".to_owned()];
        sort_loader_versions_desc(&mut forge);
        assert_eq!(
            forge,
            vec!["1.21.4-54.0.10".to_owned(), "1.21.4-54.0.9".to_owned()]
        );
    }

    #[test]
    fn parse_neoforge_skips_non_numeric_versions() {
        let xml = r"<metadata><versioning><versions>
            <version>21.4.81</version>
            <version>junk.data.123</version>
        </versions></versioning></metadata>";
        let v = parse_neoforge(xml).expect("parse");
        assert_eq!(v.mc_versions, vec!["1.21.4".to_owned()]);
    }
}
