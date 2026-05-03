//! `CurseForge` `ServerFiles` provider.
//!
//! Drives a generic `eclipse-temurin:21` pod whose entrypoint runs the
//! pack's bundled `startserver.sh`; that script handles `NeoForge` install
//! and version bumps on its own. The provider's job is to pick the
//! correct `ServerFiles` file from the `CurseForge` API and hand a download
//! URL to the swap Job.

use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use k8s_openapi::api::core::v1::EnvVar;
use serde::{Deserialize, Serialize};

use super::cf_client::CfFile;
use super::{ModpackHttp, ModpackProvider, ProviderContext, VersionInfo};

/// Container image used for CurseForge-driven servers.
///
/// The pack's own `startserver.sh` handles `NeoForge` / Forge install and
/// version transitions, so we just need a JRE 21 with `bash` available.
const CF_IMAGE: &str = "eclipse-temurin:21-jdk";

/// Container command for `CurseForge` servers — the pack's bundled launcher.
const CF_LAUNCH_CMD: &[&str] = &["bash", "startserver.sh"];

/// How long the orchestrator waits for `Done (` after starting a CF server.
///
/// ATM-11 first boot downloads `NeoForge` (~70 MB) and runs the installer,
/// then unpacks ~400 mods. 15min is comfortably above the observed P99 on
/// a warm cache; first-boot can take longer and operators should override
/// per-server in a future revision if they hit the ceiling.
const CF_BOOT_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Release channel filter applied when picking the latest server-pack file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Release,
    Beta,
    Alpha,
}

impl Channel {
    /// Returns the upstream `releaseType` numeric code matching this channel.
    #[must_use]
    pub fn release_type(self) -> u8 {
        match self {
            Self::Release => 1,
            Self::Beta => 2,
            Self::Alpha => 3,
        }
    }

    /// Returns `true` if the supplied upstream `releaseType` is acceptable
    /// when this channel is selected. Beta accepts Beta + Release; Alpha accepts
    /// all three; Release is strict.
    #[must_use]
    pub fn accepts(self, release_type: u8) -> bool {
        match self {
            Self::Release => release_type == 1,
            Self::Beta => release_type <= 2,
            Self::Alpha => release_type <= 3,
        }
    }
}

/// Persisted `CurseForge` config (lives in `servers.source_config`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub project_id: u32,
    pub channel: Channel,
    /// Version names (or numeric ids as strings) the user has chosen to skip.
    #[serde(default)]
    pub version_skip: Vec<String>,
    /// When set, the orchestrator targets exactly this file id and bypasses
    /// the latest-version logic.
    #[serde(default)]
    pub force_version: Option<String>,
    /// File id currently deployed.
    pub current_version_id: u32,
    /// Display name of the currently deployed file.
    pub current_version_name: String,
    /// Auto-update behaviour for this server.
    #[serde(default)]
    pub auto_update_mode: AutoUpdateMode,
}

/// What the poller does when it detects a new version is available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoUpdateMode {
    /// Never write `update_available`; treat the server as version-pinned.
    Never,
    /// Write `update_available`; the user clicks Update to apply. (default)
    #[default]
    Notify,
    /// Auto-fire the update orchestrator on detection.
    Apply,
}

/// CurseForge-backed server provider.
#[derive(Debug, Clone)]
pub struct CurseForgeServerPack {
    config: Config,
}

impl CurseForgeServerPack {
    /// Wraps a persisted [`Config`].
    #[must_use]
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// Borrows the underlying config (used by the create handler / settings PATCH).
    #[must_use]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Picks the newest server-pack file matching the channel and not in the
    /// skip list. Returns `None` if no candidates qualify.
    #[must_use]
    pub fn pick_latest(&self, files: &[CfFile]) -> Option<VersionInfo> {
        let mut candidates: Vec<&CfFile> = files
            .iter()
            .filter(|f| f.is_server_pack)
            .filter(|f| f.download_url.is_some())
            .filter(|f| self.config.channel.accepts(f.release_type))
            .filter(|f| {
                let id_str = f.id.to_string();
                !self
                    .config
                    .version_skip
                    .iter()
                    .any(|s| s == &id_str || s == &f.display_name)
            })
            .collect();
        // Newest first by `fileDate` (ISO 8601 sorts lexically).
        candidates.sort_by(|a, b| b.file_date.cmp(&a.file_date));
        candidates.first().map(|f| VersionInfo {
            id: f.id.to_string(),
            name: f.display_name.clone(),
            download_url: f.download_url.clone().unwrap_or_default(),
        })
    }
}

#[async_trait::async_trait]
impl ModpackProvider for CurseForgeServerPack {
    fn kind(&self) -> &'static str {
        "curseforge"
    }

    fn project_id(&self) -> Option<String> {
        Some(self.config.project_id.to_string())
    }

    fn pod_image(&self) -> &str {
        CF_IMAGE
    }

    fn launch_command(&self) -> Option<Vec<String>> {
        Some(CF_LAUNCH_CMD.iter().map(|s| (*s).to_owned()).collect())
    }

    fn extra_env(&self, ctx: &ProviderContext<'_>) -> Vec<EnvVar> {
        // `JAVA_TOOL_OPTIONS` propagates to the JVM the pack's startserver.sh
        // launches without us having to touch the script. ATM's user_jvm_args.txt
        // is preserved on update, so per-server tuning persists; this just sets
        // the panel's memory ceiling on top.
        vec![EnvVar {
            name: "JAVA_TOOL_OPTIONS".to_owned(),
            value: Some(format!("-Xmx{}m -Xms{}m", ctx.memory_mi, ctx.memory_mi / 2)),
            value_from: None,
        }]
    }

    fn boot_timeout(&self) -> Duration {
        CF_BOOT_TIMEOUT
    }

    async fn latest(&self, http: &ModpackHttp<'_>) -> Result<Option<VersionInfo>> {
        let cf = http
            .cf
            .ok_or_else(|| anyhow!("CurseForge client unavailable"))?;
        let files = cf.list_files(self.config.project_id).await?;
        Ok(self.pick_latest(&files))
    }

    async fn fetch_url(&self, http: &ModpackHttp<'_>, version: &VersionInfo) -> Result<String> {
        let cf = http
            .cf
            .ok_or_else(|| anyhow!("CurseForge client unavailable"))?;
        let id_u32: u32 = version
            .id
            .parse()
            .with_context(|| format!("CF version id {:?} not numeric", version.id))?;
        // The cached download_url may be stale; re-fetch the file list and
        // pick the matching id so the swap Job gets a fresh URL.
        let files = cf.list_files(self.config.project_id).await?;
        let f = files
            .iter()
            .find(|f| f.id == id_u32)
            .ok_or_else(|| anyhow!("file id {} not found in project files", version.id))?;
        f.download_url.clone().ok_or_else(|| {
            anyhow!(
                "file {} has no download_url (project disabled API distribution)",
                version.id
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cf_file(id: u32, name: &str, server: bool, rtype: u8, date: &str) -> CfFile {
        CfFile {
            id,
            display_name: name.to_owned(),
            release_type: rtype,
            is_server_pack: server,
            download_url: Some(format!("https://example.com/{id}.zip")),
            file_date: date.to_owned(),
        }
    }

    fn pack(channel: Channel, skip: Vec<String>) -> CurseForgeServerPack {
        CurseForgeServerPack::new(Config {
            project_id: 1_148_445,
            channel,
            version_skip: skip,
            force_version: None,
            current_version_id: 0,
            current_version_name: String::new(),
            auto_update_mode: AutoUpdateMode::Notify,
        })
    }

    #[test]
    fn channel_release_rejects_beta() {
        assert!(!Channel::Release.accepts(2));
        assert!(Channel::Release.accepts(1));
    }

    #[test]
    fn channel_beta_accepts_release_and_beta() {
        assert!(Channel::Beta.accepts(1));
        assert!(Channel::Beta.accepts(2));
        assert!(!Channel::Beta.accepts(3));
    }

    #[test]
    fn pick_latest_skips_client_files() {
        let p = pack(Channel::Release, vec![]);
        let files = vec![
            cf_file(1, "client", false, 1, "2026-01-01T00:00:00Z"),
            cf_file(2, "server", true, 1, "2026-01-02T00:00:00Z"),
        ];
        assert_eq!(p.pick_latest(&files).unwrap().id, "2");
    }

    #[test]
    fn pick_latest_picks_newest_by_date() {
        let p = pack(Channel::Release, vec![]);
        let files = vec![
            cf_file(1, "old", true, 1, "2026-01-01T00:00:00Z"),
            cf_file(2, "new", true, 1, "2026-02-01T00:00:00Z"),
        ];
        assert_eq!(p.pick_latest(&files).unwrap().id, "2");
    }

    #[test]
    fn pick_latest_honours_skip_list_by_id() {
        let p = pack(Channel::Release, vec!["2".to_owned()]);
        let files = vec![
            cf_file(1, "old", true, 1, "2026-01-01T00:00:00Z"),
            cf_file(2, "new", true, 1, "2026-02-01T00:00:00Z"),
        ];
        assert_eq!(p.pick_latest(&files).unwrap().id, "1");
    }

    #[test]
    fn pick_latest_honours_skip_list_by_name() {
        let p = pack(Channel::Release, vec!["new".to_owned()]);
        let files = vec![
            cf_file(1, "old", true, 1, "2026-01-01T00:00:00Z"),
            cf_file(2, "new", true, 1, "2026-02-01T00:00:00Z"),
        ];
        assert_eq!(p.pick_latest(&files).unwrap().id, "1");
    }

    #[test]
    fn pick_latest_filters_by_channel() {
        let p = pack(Channel::Release, vec![]);
        let files = vec![
            cf_file(1, "beta", true, 2, "2026-02-01T00:00:00Z"),
            cf_file(2, "release", true, 1, "2026-01-01T00:00:00Z"),
        ];
        assert_eq!(p.pick_latest(&files).unwrap().id, "2");
    }

    #[test]
    fn pick_latest_returns_none_when_no_candidates() {
        let p = pack(Channel::Release, vec![]);
        let files = vec![cf_file(1, "client", false, 1, "2026-01-01T00:00:00Z")];
        assert!(p.pick_latest(&files).is_none());
    }
}
