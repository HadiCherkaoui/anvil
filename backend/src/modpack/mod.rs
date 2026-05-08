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

pub mod backups;
pub mod cf_client;
pub mod curseforge;
pub mod dep_resolver;
pub mod deps;
pub mod guard;
pub mod jobs;
pub mod memory;
pub mod modded;
pub mod modrinth;
pub mod mods_apply;
pub mod mr_client;
pub mod orchestrator;
pub mod paper;
pub mod poller;
pub mod vanilla;
pub mod version_change;

pub use cf_client::CurseForgeClient;
pub use curseforge::{Channel, CurseForgeServerPack};
pub use modded::{ModdedRuntime, Runtime as ModdedRuntimeKind};
pub use modrinth::ModrinthServerPack;
pub use mr_client::ModrinthClient;
pub use paper::PaperServerProvider;
pub use vanilla::VanillaProvider;

/// Context the per-server `StatefulSet` builder hands to the provider so
/// `extra_env` can reference the right RCON Secret + memory budget.
#[derive(Debug)]
pub struct ProviderContext<'a> {
    /// Server UUID (drives the `mc-<id>-rcon` Secret reference).
    pub server_id: &'a str,
    /// Max heap budget in MiB (drives `MAX_MEMORY=4096M`; init is derived).
    pub memory_mi: i64,
}

/// Cached information about a modpack version.
///
/// Returned from [`ModpackProvider::latest`] and used by both the poller
/// (to write into `modpack_versions`) and the update orchestrator (to feed
/// the swap Job). The `id` is opaque — `CurseForge` stores its `u32` file
/// id as a decimal string; Modrinth stores its 8-char base62 version id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    /// Opaque upstream version id.
    pub id: String,
    /// Human-readable file/version name (e.g. `"All The Mods 11 - 4.4 - Server Pack"`).
    pub name: String,
    /// HTTPS URL the swap Job downloads. May expire — refresh per update.
    pub download_url: String,
}

/// HTTP context handed to provider methods so they can reach the right upstream.
///
/// `cf` is `None` when `CF_API_KEY` is unset — providers that need it must
/// surface a clear error. `mr` is always available (Modrinth is API-key-free).
#[derive(Debug)]
pub struct ModpackHttp<'a> {
    pub cf: Option<&'a CurseForgeClient>,
    pub mr: &'a ModrinthClient,
}

/// One provider per server source. Vanilla + `CurseForge` in M5; Modrinth later.
#[async_trait::async_trait]
pub trait ModpackProvider: Send + Sync + std::fmt::Debug {
    /// Discriminator persisted as `servers.source_kind` (`"vanilla"` | `"curseforge"`).
    fn kind(&self) -> &'static str;

    /// Opaque upstream project id when the provider has one (`CurseForge`,
    /// Modrinth). Returns `None` for providers without — vanilla, modded, paper.
    fn project_id(&self) -> Option<String> {
        None
    }

    /// Container command override. `None` lets the image's default entrypoint run.
    fn launch_command(&self) -> Option<Vec<String>>;

    /// Provider-specific env vars layered onto the container.
    fn extra_env(&self, ctx: &ProviderContext<'_>) -> Vec<EnvVar>;

    /// How long the orchestrator waits for `Done (` in pod logs before declaring boot failure.
    fn boot_timeout(&self) -> Duration;

    /// Resolves the latest upstream version. Vanilla / modded / paper return `Ok(None)`.
    ///
    /// # Errors
    ///
    /// Returns an error if the upstream call fails or rate-limits.
    async fn latest(&self, http: &ModpackHttp<'_>) -> Result<Option<VersionInfo>>;

    /// Returns a fresh download URL for the given version. Called from the orchestrator
    /// just before spawning the swap Job (URLs may be presigned and short-lived).
    ///
    /// # Errors
    ///
    /// Returns an error if the upstream call fails.
    ///
    /// # Panics
    ///
    /// Providers without an upstream (vanilla / modded / paper) panic — the
    /// orchestrator routes by [`ModpackProvider::kind`] and must never call
    /// this on them.
    async fn fetch_url(&self, http: &ModpackHttp<'_>, version: &VersionInfo) -> Result<String>;
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
        "modrinth" => {
            let cfg: modrinth::Config = serde_json::from_str(source_config)
                .map_err(|e| anyhow::anyhow!("source_config not valid Modrinth JSON: {e}"))?;
            Ok(Box::new(ModrinthServerPack::new(cfg)))
        }
        "modded" => {
            let cfg: modded::Config = serde_json::from_str(source_config)
                .map_err(|e| anyhow::anyhow!("source_config not valid modded JSON: {e}"))?;
            Ok(Box::new(ModdedRuntime::new(cfg)))
        }
        "paper" => {
            let cfg: paper::Config = serde_json::from_str(source_config)
                .map_err(|e| anyhow::anyhow!("source_config not valid paper JSON: {e}"))?;
            Ok(Box::new(PaperServerProvider::new(cfg)))
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

    #[test]
    fn from_db_returns_modrinth() {
        let cfg = r#"{
            "project_id": "AANobbMI",
            "channel": "release",
            "version_skip": [],
            "current_version_id": "",
            "current_version_name": "",
            "auto_update_mode": "notify"
        }"#;
        let p = from_db("modrinth", cfg).expect("modrinth provider");
        assert_eq!(p.kind(), "modrinth");
    }

    #[test]
    fn from_db_returns_modded() {
        let cfg = r#"{"runtime": "fabric", "mc_version": "1.21.1"}"#;
        let p = from_db("modded", cfg).expect("modded provider");
        assert_eq!(p.kind(), "modded");
    }

    #[test]
    fn from_db_returns_paper() {
        let cfg = r#"{"mc_version": "1.21.1"}"#;
        let p = from_db("paper", cfg).expect("paper provider");
        assert_eq!(p.kind(), "paper");
    }
}
