//! Paper provider — `itzg/minecraft-server` with `TYPE=PAPER`.
//!
//! Paper consumes Bukkit-API plugins, not Forge mods — anvil ships Paper as
//! a runtime in B (servers boot fine) but the Mods tab shows a deferred-
//! plugin placeholder. Plugin browsing arrives in a later sub-project.

use std::time::Duration;

use anyhow::{Result, anyhow};
use k8s_openapi::api::core::v1::EnvVar;
use serde::{Deserialize, Serialize};

use super::vanilla::{env_kv, env_secret};
use super::{ModpackHttp, ModpackProvider, ProviderContext, VersionInfo};

const PAPER_IMAGE: &str = "itzg/minecraft-server:java21";
const PAPER_BOOT_TIMEOUT: Duration = Duration::from_mins(5);

/// Persisted Paper config (lives in `servers.source_config`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub mc_version: String,
    /// Optional Paper build pin — `None` lets itzg pick the latest stable build.
    #[serde(default)]
    pub paper_build: Option<String>,
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

    fn pod_image(&self) -> &str {
        PAPER_IMAGE
    }

    fn launch_command(&self) -> Option<Vec<String>> {
        None
    }

    fn extra_env(&self, ctx: &ProviderContext<'_>) -> Vec<EnvVar> {
        let mut env = vec![
            env_kv("EULA", "TRUE"),
            env_kv("TYPE", "PAPER"),
            env_kv("VERSION", &self.config.mc_version),
            env_kv("MEMORY", &format!("{}M", ctx.memory_mi)),
            env_kv("ENABLE_RCON", "true"),
            env_secret(
                "RCON_PASSWORD",
                &format!("mc-{}-rcon", ctx.server_id),
                "password",
            ),
        ];
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

    #[test]
    fn provider_kind_is_paper() {
        let p = PaperServerProvider::new(Config {
            mc_version: "1.21.1".to_owned(),
            paper_build: None,
        });
        assert_eq!(p.kind(), "paper");
    }

    #[test]
    fn extra_env_omits_paper_build_when_none() {
        let p = PaperServerProvider::new(Config {
            mc_version: "1.21.1".to_owned(),
            paper_build: None,
        });
        let env = p.extra_env(&ProviderContext {
            server_id: "abc",
            memory_mi: 4096,
        });
        assert!(env.iter().all(|e| e.name != "PAPER_BUILD"));
    }

    #[test]
    fn extra_env_includes_paper_build_when_set() {
        let p = PaperServerProvider::new(Config {
            mc_version: "1.21.1".to_owned(),
            paper_build: Some("123".to_owned()),
        });
        let env = p.extra_env(&ProviderContext {
            server_id: "abc",
            memory_mi: 4096,
        });
        let pb = env.iter().find(|e| e.name == "PAPER_BUILD").unwrap();
        assert_eq!(pb.value.as_deref(), Some("123"));
    }
}
