//! `GET /api/runtimes/{runtime}/versions` — Forge / `NeoForge` loader versions.
//!
//! Anvil scrapes the upstream Maven `maven-metadata.xml` for each runtime,
//! groups versions by their corresponding Minecraft version, and serves
//! the result with a 1-hour in-memory cache. Frontend uses the result to
//! render cascading MC ↔ loader pickers in the create form so users only
//! pick combinations that actually exist (`NeoForge` skips MC versions, so
//! the previous LATEST-everywhere flow installed-failed silently).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;

/// Parsed loader-version listing returned by the endpoint.
#[derive(Debug, Clone, Serialize)]
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
