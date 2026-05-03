//! Modpack provider abstraction.
//!
//! M5 introduces the second provider (`CurseForge`) alongside the original
//! vanilla path; the trait emerges here. Each provider knows how to render
//! the per-server pod image / launch command / env vars, and the
//! `CurseForge` variant additionally knows how to look up the latest
//! `ServerFiles` version and produce a download URL for the swap Job.
//!
//! Providers are reconstructed from the `SQLite` `source_kind` + `source_config`
//! columns at the call sites that need them (the create handler, the poller,
//! the update orchestrator).

use std::time::Duration;

use anyhow::Result;
use k8s_openapi::api::core::v1::EnvVar;
use serde::{Deserialize, Serialize};

pub mod cf_client;
pub mod curseforge;
pub mod guard;
pub mod jobs;
pub mod orchestrator;
pub mod poller;
pub mod vanilla;

pub use cf_client::CurseForgeClient;
pub use curseforge::{Channel, CurseForgeServerPack};
pub use vanilla::VanillaProvider;

/// Context the per-server `StatefulSet` builder hands to the provider so
/// `extra_env` can reference the right RCON Secret + memory budget.
#[derive(Debug)]
pub struct ProviderContext<'a> {
    /// Server UUID (drives the `mc-<id>-rcon` Secret reference).
    pub server_id: &'a str,
    /// Memory budget in MiB (drives `MEMORY=4096M` style env).
    pub memory_mi: i64,
}

/// Cached information about a `CurseForge` `ServerFiles` version.
///
/// Returned from [`ModpackProvider::latest`] and used by both the poller
/// (to write into `modpack_versions`) and the update orchestrator (to feed
/// the swap Job).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    /// `CurseForge` file id — opaque, used as the version identifier.
    pub id: u32,
    /// Human-readable file name (e.g. `"All The Mods 11 - 4.4 - Server Pack"`).
    pub name: String,
    /// HTTPS URL the swap Job downloads. May expire — refresh per update.
    pub download_url: String,
}

/// One provider per server source. Vanilla + `CurseForge` in M5; Modrinth later.
#[async_trait::async_trait]
pub trait ModpackProvider: Send + Sync + std::fmt::Debug {
    /// Discriminator persisted as `servers.source_kind` (`"vanilla"` | `"curseforge"`).
    fn kind(&self) -> &'static str;

    /// Numeric upstream project id when the provider has one (`CurseForge`,
    /// Modrinth eventually). Returns `None` for providers without — vanilla.
    fn project_id(&self) -> Option<u32> {
        None
    }

    /// Container image the per-server `StatefulSet` runs.
    fn pod_image(&self) -> &str;

    /// Container command override. `None` lets the image's default entrypoint run.
    fn launch_command(&self) -> Option<Vec<String>>;

    /// Provider-specific env vars layered onto the container.
    fn extra_env(&self, ctx: &ProviderContext<'_>) -> Vec<EnvVar>;

    /// How long the orchestrator waits for `Done (` in pod logs before declaring boot failure.
    fn boot_timeout(&self) -> Duration;

    /// Resolves the latest upstream version. Vanilla returns `Ok(None)`.
    ///
    /// # Errors
    ///
    /// Returns an error if the upstream call fails or rate-limits.
    async fn latest(&self, http: &CurseForgeClient) -> Result<Option<VersionInfo>>;

    /// Returns a fresh download URL for the given version. Called from the orchestrator
    /// just before spawning the swap Job (URLs may be presigned and short-lived).
    ///
    /// # Errors
    ///
    /// Returns an error if the upstream call fails.
    ///
    /// # Panics
    ///
    /// Vanilla panics — the orchestrator routes by [`ModpackProvider::kind`] and
    /// must never call this on a vanilla provider.
    async fn fetch_url(&self, http: &CurseForgeClient, version: &VersionInfo) -> Result<String>;
}

/// Boxed-trait alias used by handlers, the poller, and the orchestrator.
pub type Provider = Box<dyn ModpackProvider>;

/// Reconstructs a provider from the `source_kind` + `source_config` columns.
///
/// # Errors
///
/// Returns an error when `source_kind` is unknown or the `source_config` JSON
/// fails to deserialize for the matched provider.
pub fn from_db(source_kind: &str, source_config: &str) -> Result<Provider> {
    match source_kind {
        "vanilla" => Ok(Box::new(VanillaProvider::new())),
        "curseforge" => {
            let cfg: curseforge::Config = serde_json::from_str(source_config)
                .map_err(|e| anyhow::anyhow!("source_config not valid CurseForge JSON: {e}"))?;
            Ok(Box::new(CurseForgeServerPack::new(cfg)))
        }
        other => Err(anyhow::anyhow!("unknown source_kind {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_db_returns_vanilla() {
        let p = from_db("vanilla", "{}").expect("vanilla provider");
        assert_eq!(p.kind(), "vanilla");
    }

    #[test]
    fn from_db_rejects_unknown_kind() {
        let err = from_db("bogus", "{}").expect_err("must reject");
        assert!(err.to_string().contains("unknown source_kind"));
    }

    #[test]
    fn from_db_rejects_curseforge_with_bad_json() {
        let err = from_db("curseforge", "not json").expect_err("must reject");
        assert!(err.to_string().contains("source_config"));
    }
}
