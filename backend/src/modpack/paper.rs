//! Paper provider — configured itzg image with `TYPE=PAPER`.
//!
//! Paper consumes Bukkit-API plugins, not Forge mods. The plugin list is
//! persisted in `source_config` and synced to `/data/plugins/` via the
//! shared `mod-sync` Job (parameterised by target dir) the same way modded
//! mods are.

use std::time::Duration;

use anyhow::{Result, anyhow};
use k8s_openapi::api::core::v1::EnvVar;
use serde::{Deserialize, Serialize};

use super::memory::build_memory_env;
use super::modded::ModEntry;
use super::vanilla::{env_kv, env_secret};
use super::{ModpackHttp, ModpackProvider, ProviderContext, VersionInfo};

const PAPER_BOOT_TIMEOUT: Duration = Duration::from_mins(5);

/// Persisted Paper config (lives in `servers.source_config`).
///
/// `pending_plugins` holds the full desired plugin list staged for the next
/// apply. Empty means "no pending changes". On a successful apply the FSM
/// moves it into `plugins` and resets it to empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub mc_version: String,
    /// Optional Paper build pin — `None` lets itzg pick the latest stable build.
    #[serde(default)]
    pub paper_build: Option<String>,
    /// Currently installed plugins (committed after the last apply).
    #[serde(default)]
    pub plugins: Vec<ModEntry>,
    /// Desired plugin list awaiting the next apply. Empty = no draft.
    #[serde(default)]
    pub pending_plugins: Vec<ModEntry>,
    /// Per-plugin auto-update mode — mirrors the modded/modpack
    /// equivalents. Same semantics as
    /// [`crate::modpack::modded::AutoUpdateMode`].
    #[serde(default)]
    pub auto_update_mode: super::modded::AutoUpdateMode,
}

#[derive(Debug, Clone)]
pub struct PaperServerProvider {
    config: Config,
}

impl PaperServerProvider {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }
}

#[async_trait::async_trait]
impl ModpackProvider for PaperServerProvider {
    fn kind(&self) -> &'static str {
        "paper"
    }

    fn launch_command(&self) -> Option<Vec<String>> {
        None
    }

    fn extra_env(&self, ctx: &ProviderContext<'_>) -> Vec<EnvVar> {
        let mut env = vec![
            env_kv("EULA", "TRUE"),
            env_kv("TYPE", "PAPER"),
            env_kv("VERSION", &self.config.mc_version),
        ];
        env.extend(build_memory_env(ctx.memory_mi));
        env.extend([
            env_kv("ENABLE_RCON", "true"),
            env_secret(
                "RCON_PASSWORD",
                &format!("mc-{}-rcon", ctx.server_id),
                "password",
            ),
        ]);
        if let Some(build) = self.config.paper_build.as_deref() {
            env.push(env_kv("PAPER_BUILD", build));
        }
        env
    }

    fn boot_timeout(&self) -> Duration {
        PAPER_BOOT_TIMEOUT
    }

    async fn latest(&self, _http: &ModpackHttp<'_>) -> Result<Option<VersionInfo>> {
        Ok(None)
    }

    async fn fetch_url(&self, _http: &ModpackHttp<'_>, _v: &VersionInfo) -> Result<String> {
        Err(anyhow!("paper has no pack-level upstream"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(paper_build: Option<String>) -> Config {
        Config {
            mc_version: "1.21.1".to_owned(),
            paper_build,
            plugins: Vec::new(),
            pending_plugins: Vec::new(),
            auto_update_mode: super::super::modded::AutoUpdateMode::default(),
        }
    }

    #[test]
    fn provider_kind_is_paper() {
        let p = PaperServerProvider::new(cfg(None));
        assert_eq!(p.kind(), "paper");
    }

    #[test]
    fn extra_env_omits_paper_build_when_none() {
        let p = PaperServerProvider::new(cfg(None));
        let env = p.extra_env(&ProviderContext {
            server_id: "abc",
            memory_mi: 4096,
        });
        assert!(env.iter().all(|e| e.name != "PAPER_BUILD"));
    }

    #[test]
    fn extra_env_includes_paper_build_when_set() {
        let p = PaperServerProvider::new(cfg(Some("123".to_owned())));
        let env = p.extra_env(&ProviderContext {
            server_id: "abc",
            memory_mi: 4096,
        });
        let pb = env.iter().find(|e| e.name == "PAPER_BUILD").unwrap();
        assert_eq!(pb.value.as_deref(), Some("123"));
    }

    #[test]
    fn config_round_trips_with_plugin_fields() {
        let body = r#"{
            "mc_version": "1.21.1",
            "plugins": [{
                "provider": "modrinth",
                "project_id": "abc",
                "project_slug": "luckperms",
                "project_name": "LuckPerms",
                "version_id": "v1",
                "version_name": "5.4",
                "filename": "LuckPerms-Bukkit-5.4.jar",
                "download_url": "https://example/luckperms.jar"
            }],
            "pending_plugins": []
        }"#;
        let c: Config = serde_json::from_str(body).expect("parses");
        assert_eq!(c.plugins.len(), 1);
        assert!(c.pending_plugins.is_empty());
        assert_eq!(c.plugins[0].filename, "LuckPerms-Bukkit-5.4.jar");
    }

    #[test]
    fn config_defaults_plugin_fields_when_absent() {
        let body = r#"{"mc_version": "1.21.1"}"#;
        let c: Config = serde_json::from_str(body).expect("parses");
        assert!(c.plugins.is_empty());
        assert!(c.pending_plugins.is_empty());
    }
}
