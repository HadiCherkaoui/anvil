//! Modrinth `.mrpack` provider.
//!
//! Reuses `itzg/minecraft-server:java25` with `TYPE=AUTO_MODRINTH` —
//! itzg's launcher handles `.mrpack` unzip + loader install. The provider
//! picks the newest version matching the channel filter and skip list.

use std::time::Duration;

use anyhow::{Result, anyhow};
use k8s_openapi::api::core::v1::EnvVar;
use serde::{Deserialize, Serialize};

use super::curseforge::{AutoUpdateMode, Channel};
use super::mr_client::MrVersion;
use super::vanilla::{IDLE_GC_OPTS, env_kv, env_secret, init_memory_mi};
use super::{ModpackHttp, ModpackProvider, ProviderContext, VersionInfo};

const MR_IMAGE: &str = "itzg/minecraft-server:java25";
const MR_BOOT_TIMEOUT: Duration = Duration::from_mins(15);

/// Persisted Modrinth modpack config (lives in `servers.source_config`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Modrinth project id (8-char base62) or slug.
    pub project_id: String,
    pub channel: Channel,
    pub version_skip: Vec<String>,
    pub force_version: Option<String>,
    pub current_version_id: String,
    pub current_version_name: String,
    pub auto_update_mode: AutoUpdateMode,
}

/// Modrinth modpack provider.
#[derive(Debug, Clone)]
pub struct ModrinthServerPack {
    config: Config,
}

impl ModrinthServerPack {
    /// Wraps a persisted [`Config`].
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// Borrows the underlying config (used by the create handler).
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Picks the newest version matching the channel + skip list, with a
    /// primary file present.
    fn pick_latest(&self, versions: &[MrVersion]) -> Option<VersionInfo> {
        let mut candidates: Vec<&MrVersion> = versions
            .iter()
            .filter(|v| {
                matches!(
                    (self.config.channel, v.version_type.as_str()),
                    (Channel::Release, "release")
                        | (Channel::Beta, "release" | "beta")
                        | (Channel::Alpha, _)
                )
            })
            .filter(|v| {
                !self
                    .config
                    .version_skip
                    .iter()
                    .any(|s| s == &v.id || s == &v.name)
            })
            .filter(|v| v.files.iter().any(|f| f.primary))
            .collect();
        candidates.sort_by(|a, b| b.date_published.cmp(&a.date_published));
        candidates.first().map(|v| {
            let primary = v
                .files
                .iter()
                .find(|f| f.primary)
                .expect("filter above ensured a primary file");
            VersionInfo {
                id: v.id.clone(),
                name: v.name.clone(),
                download_url: primary.url.clone(),
            }
        })
    }
}

#[async_trait::async_trait]
impl ModpackProvider for ModrinthServerPack {
    fn kind(&self) -> &'static str {
        "modrinth"
    }

    fn project_id(&self) -> Option<String> {
        Some(self.config.project_id.clone())
    }

    fn pod_image(&self) -> &str {
        MR_IMAGE
    }

    fn launch_command(&self) -> Option<Vec<String>> {
        None
    }

    fn extra_env(&self, ctx: &ProviderContext<'_>) -> Vec<EnvVar> {
        // MODRINTH_VERSION pins the deployed version so the orchestrator
        // can bump it via env patch on update — itzg's mc-image-helper
        // compares the env var to its stored install marker and reinstalls
        // when they differ.
        vec![
            env_kv("EULA", "TRUE"),
            env_kv("TYPE", "AUTO_MODRINTH"),
            env_kv("MODRINTH_PROJECT", &self.config.project_id),
            env_kv("MODRINTH_VERSION", &self.config.current_version_id),
            env_kv("MODRINTH_DOWNLOAD_DEPENDENCIES", "required"),
            env_kv(
                "INIT_MEMORY",
                &format!("{}M", init_memory_mi(ctx.memory_mi)),
            ),
            env_kv("MAX_MEMORY", &format!("{}M", ctx.memory_mi)),
            env_kv("JVM_XX_OPTS", IDLE_GC_OPTS),
            env_kv("ENABLE_RCON", "true"),
            env_secret(
                "RCON_PASSWORD",
                &format!("mc-{}-rcon", ctx.server_id),
                "password",
            ),
        ]
    }

    fn boot_timeout(&self) -> Duration {
        MR_BOOT_TIMEOUT
    }

    async fn latest(&self, http: &ModpackHttp<'_>) -> Result<Option<VersionInfo>> {
        let versions = http.mr.list_versions(&self.config.project_id).await?;
        Ok(self.pick_latest(&versions))
    }

    async fn fetch_url(&self, http: &ModpackHttp<'_>, version: &VersionInfo) -> Result<String> {
        let v = http.mr.version(&version.id).await?;
        let primary = v
            .files
            .iter()
            .find(|f| f.primary)
            .ok_or_else(|| anyhow!("Modrinth version {} has no primary file", version.id))?;
        Ok(primary.url.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::super::mr_client::{MrFile, MrHashes};
    use super::*;

    fn mr_v(id: &str, name: &str, vtype: &str, date: &str) -> MrVersion {
        MrVersion {
            id: id.to_owned(),
            project_id: "p".to_owned(),
            name: name.to_owned(),
            version_number: name.to_owned(),
            version_type: vtype.to_owned(),
            loaders: vec!["fabric".to_owned()],
            game_versions: vec!["1.21.1".to_owned()],
            date_published: date.to_owned(),
            files: vec![MrFile {
                url: format!("https://example/{id}.mrpack"),
                filename: format!("{id}.mrpack"),
                primary: true,
                hashes: MrHashes::default(),
            }],
        }
    }

    fn pack(channel: Channel, skip: Vec<String>) -> ModrinthServerPack {
        ModrinthServerPack::new(Config {
            project_id: "AANobbMI".to_owned(),
            channel,
            version_skip: skip,
            force_version: None,
            current_version_id: String::new(),
            current_version_name: String::new(),
            auto_update_mode: AutoUpdateMode::Notify,
        })
    }

    #[test]
    fn pick_latest_picks_newest_release() {
        let p = pack(Channel::Release, vec![]);
        let vs = vec![
            mr_v("a", "old", "release", "2026-01-01T00:00:00Z"),
            mr_v("b", "new", "release", "2026-02-01T00:00:00Z"),
        ];
        assert_eq!(p.pick_latest(&vs).unwrap().id, "b");
    }

    #[test]
    fn pick_latest_release_rejects_beta() {
        let p = pack(Channel::Release, vec![]);
        let vs = vec![mr_v("a", "beta-only", "beta", "2026-01-01T00:00:00Z")];
        assert!(p.pick_latest(&vs).is_none());
    }

    #[test]
    fn pick_latest_beta_accepts_release_and_beta() {
        let p = pack(Channel::Beta, vec![]);
        let vs = vec![
            mr_v("a", "rel", "release", "2026-01-01T00:00:00Z"),
            mr_v("b", "beta", "beta", "2026-02-01T00:00:00Z"),
        ];
        assert_eq!(p.pick_latest(&vs).unwrap().id, "b");
    }

    #[test]
    fn pick_latest_honours_skip_list_by_id() {
        let p = pack(Channel::Release, vec!["b".to_owned()]);
        let vs = vec![
            mr_v("a", "old", "release", "2026-01-01T00:00:00Z"),
            mr_v("b", "new", "release", "2026-02-01T00:00:00Z"),
        ];
        assert_eq!(p.pick_latest(&vs).unwrap().id, "a");
    }

    #[test]
    fn provider_kind_is_modrinth() {
        assert_eq!(pack(Channel::Release, vec![]).kind(), "modrinth");
    }

    #[test]
    fn provider_extra_env_contains_modrinth_project() {
        let p = pack(Channel::Release, vec![]);
        let ctx = ProviderContext {
            server_id: "abc",
            memory_mi: 4096,
        };
        let env = p.extra_env(&ctx);
        let project = env.iter().find(|e| e.name == "MODRINTH_PROJECT").unwrap();
        assert_eq!(project.value.as_deref(), Some("AANobbMI"));
        let t = env.iter().find(|e| e.name == "TYPE").unwrap();
        assert_eq!(t.value.as_deref(), Some("AUTO_MODRINTH"));
    }
}
