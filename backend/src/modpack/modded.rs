//! Modded runtime provider — Fabric / Forge / `NeoForge` with explicit modlist.
//!
//! Reuses `itzg/minecraft-server:java25` with `TYPE` switching. Mod jars are
//! NOT delivered via itzg's `MODS=` env — anvil's `mod-sync` Job is the sole
//! writer to `/data/mods`. This keeps anvil's modlist the unambiguous source
//! of truth.

use std::time::Duration;

use anyhow::{Result, anyhow};
use k8s_openapi::api::core::v1::EnvVar;
use serde::{Deserialize, Serialize};

use super::memory::build_memory_env;
use super::vanilla::{env_kv, env_secret};
use super::{ModpackHttp, ModpackProvider, ProviderContext, VersionInfo};

const MODDED_IMAGE: &str = "itzg/minecraft-server:java25";
const MODDED_BOOT_TIMEOUT: Duration = Duration::from_mins(10);

/// One installed mod (persisted in `source_config.mods`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModEntry {
    pub provider: String,
    pub project_id: String,
    pub project_slug: String,
    pub project_name: String,
    pub version_id: String,
    pub version_name: String,
    pub filename: String,
    pub download_url: String,
    #[serde(default)]
    pub sha512: Option<String>,
}

/// One pending change in the modlist draft.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PendingOp {
    Add {
        mod_entry: ModEntry,
    },
    Remove {
        filename: String,
    },
    Bump {
        filename: String,
        to_version_id: String,
        to_version_name: String,
        to_filename: String,
        to_download_url: String,
        #[serde(default)]
        to_sha512: Option<String>,
    },
}

/// Loader runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runtime {
    Fabric,
    Forge,
    NeoForge,
}

impl Runtime {
    /// itzg `TYPE=` env value for this loader.
    #[must_use]
    pub fn type_env(self) -> &'static str {
        match self {
            Self::Fabric => "FABRIC",
            Self::Forge => "FORGE",
            Self::NeoForge => "NEOFORGE",
        }
    }
}

/// Persisted modded config (lives in `servers.source_config`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub runtime: Runtime,
    pub mc_version: String,
    /// Forge / `NeoForge` loader version (e.g. `"1.21.4-54.1.0"` for Forge,
    /// `"21.4.81"` for `NeoForge`). `None` keeps itzg's default behaviour
    /// (`*_VERSION=LATEST`); existing rows decode with `None` via
    /// `#[serde(default)]`. Fabric ignores this field — itzg does not
    /// surface a version env there.
    #[serde(default)]
    pub loader_version: Option<String>,
    #[serde(default)]
    pub mods: Vec<ModEntry>,
    #[serde(default)]
    pub pending: Vec<PendingOp>,
}

#[derive(Debug, Clone)]
pub struct ModdedRuntime {
    config: Config,
}

impl ModdedRuntime {
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Returns the modlist that would result from applying every pending op
    /// in order. Used by the mod-sync FSM to compute the desired state.
    #[must_use]
    pub fn desired_mods(&self) -> Vec<ModEntry> {
        let mut out = self.config.mods.clone();
        for op in &self.config.pending {
            match op {
                PendingOp::Add { mod_entry } => {
                    out.retain(|m| m.filename != mod_entry.filename);
                    out.push(mod_entry.clone());
                }
                PendingOp::Remove { filename } => {
                    out.retain(|m| m.filename != *filename);
                }
                PendingOp::Bump {
                    filename,
                    to_version_id,
                    to_version_name,
                    to_filename,
                    to_download_url,
                    to_sha512,
                } => {
                    if let Some(idx) = out.iter().position(|m| m.filename == *filename) {
                        let m = &mut out[idx];
                        m.version_id.clone_from(to_version_id);
                        m.version_name.clone_from(to_version_name);
                        m.filename.clone_from(to_filename);
                        m.download_url.clone_from(to_download_url);
                        m.sha512.clone_from(to_sha512);
                    }
                }
            }
        }
        out
    }
}

#[async_trait::async_trait]
impl ModpackProvider for ModdedRuntime {
    fn kind(&self) -> &'static str {
        "modded"
    }

    fn pod_image(&self) -> &str {
        MODDED_IMAGE
    }

    fn launch_command(&self) -> Option<Vec<String>> {
        None
    }

    fn extra_env(&self, ctx: &ProviderContext<'_>) -> Vec<EnvVar> {
        let mut env = vec![
            env_kv("EULA", "TRUE"),
            env_kv("TYPE", self.config.runtime.type_env()),
            env_kv("VERSION", &self.config.mc_version),
        ];
        env.extend(build_memory_env(ctx.memory_mi));
        match self.config.runtime {
            Runtime::Fabric => {}
            Runtime::Forge => env.push(env_kv(
                "FORGE_VERSION",
                self.config.loader_version.as_deref().unwrap_or("LATEST"),
            )),
            Runtime::NeoForge => env.push(env_kv(
                "NEOFORGE_VERSION",
                self.config.loader_version.as_deref().unwrap_or("LATEST"),
            )),
        }
        env.extend([
            env_kv("ENABLE_RCON", "true"),
            env_secret(
                "RCON_PASSWORD",
                &format!("mc-{}-rcon", ctx.server_id),
                "password",
            ),
        ]);
        env
    }

    fn boot_timeout(&self) -> Duration {
        MODDED_BOOT_TIMEOUT
    }

    async fn latest(&self, _http: &ModpackHttp<'_>) -> Result<Option<VersionInfo>> {
        // mods are pinned; per-mod update polling is a follow-up.
        Ok(None)
    }

    async fn fetch_url(&self, _http: &ModpackHttp<'_>, _version: &VersionInfo) -> Result<String> {
        Err(anyhow!("modded runtime has no pack-level upstream"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> ModEntry {
        ModEntry {
            provider: "modrinth".to_owned(),
            project_id: format!("{name}-id"),
            project_slug: name.to_owned(),
            project_name: name.to_owned(),
            version_id: format!("{name}-v"),
            version_name: format!("{name}-1.0"),
            filename: format!("{name}.jar"),
            download_url: format!("https://example/{name}.jar"),
            sha512: None,
        }
    }

    fn cfg(mods: Vec<ModEntry>, pending: Vec<PendingOp>) -> Config {
        Config {
            runtime: Runtime::Fabric,
            mc_version: "1.21.1".to_owned(),
            loader_version: None,
            mods,
            pending,
        }
    }

    fn cfg_with(runtime: Runtime, loader: Option<&str>) -> Config {
        Config {
            runtime,
            mc_version: "1.21.4".to_owned(),
            loader_version: loader.map(String::from),
            mods: vec![],
            pending: vec![],
        }
    }

    fn ctx() -> ProviderContext<'static> {
        ProviderContext {
            server_id: "id",
            memory_mi: 4096,
        }
    }

    #[test]
    fn desired_with_no_pending_returns_current() {
        let m = ModdedRuntime::new(cfg(vec![entry("sodium"), entry("lithium")], vec![]));
        assert_eq!(m.desired_mods().len(), 2);
    }

    #[test]
    fn desired_applies_remove() {
        let m = ModdedRuntime::new(cfg(
            vec![entry("sodium"), entry("lithium")],
            vec![PendingOp::Remove {
                filename: "sodium.jar".to_owned(),
            }],
        ));
        let d = m.desired_mods();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].filename, "lithium.jar");
    }

    #[test]
    fn desired_applies_add() {
        let m = ModdedRuntime::new(cfg(
            vec![],
            vec![PendingOp::Add {
                mod_entry: entry("sodium"),
            }],
        ));
        let d = m.desired_mods();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].filename, "sodium.jar");
    }

    #[test]
    fn desired_applies_bump() {
        let m = ModdedRuntime::new(cfg(
            vec![entry("sodium")],
            vec![PendingOp::Bump {
                filename: "sodium.jar".to_owned(),
                to_version_id: "newv".to_owned(),
                to_version_name: "sodium-2.0".to_owned(),
                to_filename: "sodium-2.0.jar".to_owned(),
                to_download_url: "https://example/sodium-2.0.jar".to_owned(),
                to_sha512: None,
            }],
        ));
        let d = m.desired_mods();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].filename, "sodium-2.0.jar");
        assert_eq!(d[0].version_id, "newv");
    }

    #[test]
    fn extra_env_emits_type_for_runtime() {
        let m = ModdedRuntime::new(cfg(vec![], vec![]));
        let ctx = ProviderContext {
            server_id: "abc",
            memory_mi: 4096,
        };
        let env = m.extra_env(&ctx);
        let t = env.iter().find(|e| e.name == "TYPE").unwrap();
        assert_eq!(t.value.as_deref(), Some("FABRIC"));
        let v = env.iter().find(|e| e.name == "VERSION").unwrap();
        assert_eq!(v.value.as_deref(), Some("1.21.1"));
    }

    #[test]
    fn provider_kind_is_modded() {
        assert_eq!(ModdedRuntime::new(cfg(vec![], vec![])).kind(), "modded");
    }

    #[test]
    fn fabric_no_loader_env() {
        let r = ModdedRuntime::new(cfg_with(Runtime::Fabric, None));
        let env = r.extra_env(&ctx());
        assert!(
            env.iter()
                .all(|e| e.name != "FORGE_VERSION" && e.name != "NEOFORGE_VERSION")
        );
    }

    #[test]
    fn forge_emits_forge_version() {
        let r = ModdedRuntime::new(cfg_with(Runtime::Forge, Some("1.21.4-54.1.0")));
        let env = r.extra_env(&ctx());
        let v = env.iter().find(|e| e.name == "FORGE_VERSION").unwrap();
        assert_eq!(v.value.as_deref(), Some("1.21.4-54.1.0"));
    }

    #[test]
    fn neoforge_with_no_loader_falls_back_to_latest() {
        let r = ModdedRuntime::new(cfg_with(Runtime::NeoForge, None));
        let env = r.extra_env(&ctx());
        let v = env.iter().find(|e| e.name == "NEOFORGE_VERSION").unwrap();
        assert_eq!(v.value.as_deref(), Some("LATEST"));
    }

    #[test]
    fn neoforge_with_loader_uses_provided() {
        let r = ModdedRuntime::new(cfg_with(Runtime::NeoForge, Some("21.4.81")));
        let env = r.extra_env(&ctx());
        let v = env.iter().find(|e| e.name == "NEOFORGE_VERSION").unwrap();
        assert_eq!(v.value.as_deref(), Some("21.4.81"));
    }

    #[test]
    fn config_decodes_legacy_rows_without_loader_version() {
        let cfg: Config = serde_json::from_str(
            r#"{"runtime":"fabric","mc_version":"1.21.1","mods":[],"pending":[]}"#,
        )
        .expect("decode legacy");
        assert!(cfg.loader_version.is_none());
    }
}
