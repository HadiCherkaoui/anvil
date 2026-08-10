// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `GET /api/papermc/versions` — supported Paper Minecraft versions.
//!
//! itzg's `TYPE=PAPER` rejects MC versions Paper doesn't ship for, so the
//! create form needs to know the Paper-supported subset of the Mojang
//! manifest. `PaperMC`'s Fill API (`fill.papermc.io/v3/projects/paper`)
//! lists every version with at least one Paper build. The result is
//! cached for 1 hour and falls back to a stale cache (then a hardcoded
//! baseline) when the upstream is briefly unreachable.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use axum::Json;
use axum::extract::State;
use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::AppState;
use crate::error::AppError;

/// In-memory cache slot held in [`crate::AppState`].
pub type PaperVersionsCache = Arc<Mutex<Option<(Vec<String>, Instant)>>>;

/// How long to keep a parsed listing before re-fetching.
const CACHE_TTL: Duration = Duration::from_hours(1);
/// Per-fetch HTTP timeout.
const FETCH_TIMEOUT: Duration = Duration::from_secs(8);
/// `PaperMC` Fill API project endpoint.
///
/// The old v2 API (`api.papermc.io/v2/projects/paper`) was sunset on
/// 2026-07-01 and now answers `410 Gone` — do not point this back at it.
const PROJECT_URL: &str = "https://fill.papermc.io/v3/projects/paper";
/// Polite User-Agent — `PaperMC` asks API consumers to identify themselves.
const PAPER_USER_AGENT: &str = concat!(
    "anvil/",
    env!("CARGO_PKG_VERSION"),
    " (https://gitlab.cherkaoui.ch/HadiCherkaoui/anvil)"
);

/// Returns a fresh, empty cache slot for use at startup.
#[must_use]
pub fn new_cache() -> PaperVersionsCache {
    Arc::new(Mutex::new(None))
}

#[derive(Deserialize)]
struct ProjectResponse {
    versions: VersionGroups,
}

/// Flattened version list from Fill's `versions` object.
///
/// Fill groups full versions under their minor line
/// (`"1.21": ["1.21.11", …]`) and orders both the groups and the versions
/// inside them newest-first. Serde's map types would destroy that order —
/// `BTreeMap` sorts `"1.10"` ahead of `"1.9"` and `"1.21"` ahead of
/// `"26.2"` — so entries are collected in document order instead.
struct VersionGroups(Vec<String>);

impl<'de> Deserialize<'de> for VersionGroups {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        struct GroupVisitor;

        impl<'de> Visitor<'de> for GroupVisitor {
            type Value = VersionGroups;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a map of minor version to full version list")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut out = Vec::new();
                while let Some((_minor, versions)) = map.next_entry::<String, Vec<String>>()? {
                    out.extend(versions);
                }
                Ok(VersionGroups(out))
            }
        }

        de.deserialize_map(GroupVisitor)
    }
}

/// Returns true for stable releases, false for pre-releases and RCs.
///
/// Fill lists `1.21.11-rc3` and `1.21.9-pre4` alongside stable builds, but
/// [`crate::validation::validate_mc_version`] only accepts ids from Mojang's
/// release channel, and no Mojang release id contains a `-`. Offering one in
/// the dropdown would earn a `mc_version_unknown` rejection at create time.
fn is_release(version: &str) -> bool {
    !version.contains('-')
}

/// Response body for `GET /api/papermc/versions`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PaperVersionsResponse {
    /// Paper-supported MC versions, newest first (Paper ships back to 1.7.10).
    pub versions: Vec<String>,
    /// `"papermc"` on cache hit / fresh fetch; `"fallback"` when the
    /// `PaperMC` API was unreachable AND no cache is available, in which
    /// case the response carries the hardcoded baseline.
    pub source: &'static str,
}

/// Hardcoded fallback for when both the upstream and the cache are unavailable.
/// Updated periodically; the UI labels these as "fallback" so the user knows.
const FALLBACK_VERSIONS: &[&str] = &[
    "26.2", "26.1.2", "1.21.11", "1.21.8", "1.21.4", "1.21.1", "1.20.6", "1.20.4", "1.20.1",
    "1.19.4", "1.18.2",
];

/// Parses the `PaperMC` project response into a newest-first version list.
///
/// Fill already returns groups and their contents newest-first, so document
/// order is preserved as-is. Every stable Paper-supported MC version is
/// included — Paper ships builds back to 1.7.10 — while pre-releases and
/// release candidates are dropped (see [`is_release`]).
///
/// # Errors
///
/// Returns the underlying [`serde_json::Error`] if the body shape is wrong.
pub fn parse_project(body: &str) -> Result<Vec<String>, serde_json::Error> {
    let p: ProjectResponse = serde_json::from_str(body)?;
    Ok(p.versions.0.into_iter().filter(|v| is_release(v)).collect())
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
        .header(reqwest::header::USER_AGENT, PAPER_USER_AGENT)
        .timeout(FETCH_TIMEOUT)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let versions = parse_project(&body)?;
    // An upstream shape change could parse cleanly into nothing; the client
    // requires a non-empty list, so degrade to the fallback instead of
    // caching emptiness for an hour.
    if versions.is_empty() {
        anyhow::bail!("PaperMC returned no stable versions");
    }
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
#[utoipa::path(
    get,
    path = "/api/papermc/versions",
    responses(
        (status = 200, description = "Paper-supported Minecraft versions", body = PaperVersionsResponse)
    ),
    tag = "papermc"
)]
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

    /// Trimmed real Fill response — grouped, newest-first, mixed stability.
    const FILL_BODY: &str = r#"{
        "project": {"id":"paper","name":"Paper"},
        "versions": {
            "26.2": ["26.2","26.2-rc-2"],
            "26.1": ["26.1.2","26.1.1"],
            "1.21": ["1.21.11","1.21.11-rc3","1.21.10"],
            "1.9":  ["1.9.4"],
            "1.8":  ["1.8.8"]
        }
    }"#;

    #[test]
    fn flattens_groups_in_document_order() {
        // Group order must survive parsing: a BTreeMap would sort "1.8"
        // and "1.9" ahead of "26.2" and hand the UI a backwards dropdown.
        let v = parse_project(FILL_BODY).expect("parse");
        assert_eq!(
            v,
            vec![
                "26.2", "26.1.2", "26.1.1", "1.21.11", "1.21.10", "1.9.4", "1.8.8"
            ]
        );
    }

    #[test]
    fn drops_prereleases_and_release_candidates() {
        // Mojang's release channel has no such ids, so validate_mc_version
        // would reject them with mc_version_unknown at create time.
        let v = parse_project(FILL_BODY).expect("parse");
        assert!(!v.iter().any(|s| s.contains('-')), "got {v:?}");
        assert!(v.contains(&"1.21.11".to_owned()));
    }

    #[test]
    fn returns_all_versions_no_cap() {
        // Every Paper-supported version must come through so legacy
        // versions (1.8.x, 1.12.x, …) remain selectable.
        let groups: Vec<String> = (0..80_usize)
            .map(|i| format!("\"g{i}\":[\"v{i}\"]"))
            .collect();
        let body = format!("{{\"versions\":{{{}}}}}", groups.join(","));
        let v = parse_project(&body).expect("parse");
        assert_eq!(v.len(), 80);
        assert_eq!(v[0], "v0");
        assert_eq!(v[79], "v79");
    }

    #[test]
    fn invalid_json_errors() {
        assert!(parse_project("not json").is_err());
    }

    #[test]
    fn rejects_legacy_v2_array_shape() {
        // The sunset v2 API returned a flat array; if anyone points
        // PROJECT_URL back at it, fail loudly rather than silently empty.
        assert!(parse_project(r#"{"versions":["1.20","1.21"]}"#).is_err());
    }

    #[test]
    fn fallback_versions_are_usable() {
        assert!(!FALLBACK_VERSIONS.is_empty());
        // The fallback feeds the same dropdown, so it is bound by the same
        // release-channel rule as parsed upstream data.
        assert!(FALLBACK_VERSIONS.iter().all(|v| is_release(v)));
    }
}
