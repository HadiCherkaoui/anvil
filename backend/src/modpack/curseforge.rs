//! `CurseForge` modpack provider.
//!
//! Drives the configured itzg image with `TYPE=AUTO_CURSEFORGE`. itzg
//! shells out to its `mc-image-helper install-curseforge` tool, which
//! requires `--slug` (the modpack URL slug, e.g. `all-the-mods-11`) and
//! optionally `--file-id`. The file id MUST point at the modpack's CLIENT
//! file — itzg refuses server-pack files because they ship without the
//! `manifest.json` it needs to drive the install. mc-image-helper reads
//! the client manifest, downloads each listed mod via the CF API, installs
//! the mod loader, and writes `/data/.install-curseforge.env` (verified
//! in mc-image-helper's `CurseForgeInstaller.matchesPreviousInstall`) so
//! subsequent boots only reinstall when `CF_FILE_ID` differs.
//!
//! The provider's runtime job:
//!   1. At create / poll time, pick the newest CLIENT file matching the
//!      channel filter. Server packs are never consumed on this path, so
//!      a `serverPackFileId` link is NOT required — some projects (ATM-11
//!      0.2.0) upload their server files as unlinked "additional files",
//!      which the files API doesn't even list. Direct server-pack files
//!      (`isServerPack: true`) stay excluded — they have no manifest.
//!   2. Render the `CF_SLUG` + `CF_FILE_ID` env pair so itzg can resolve
//!      and install the pack. The slug is captured once at create-time
//!      from `CurseForgeApiClient::project` and never changes for a row.

use std::time::Duration;

use anyhow::{anyhow, Context as _, Result};
use k8s_openapi::api::core::v1::EnvVar;
use serde::{Deserialize, Deserializer, Serialize};

use super::cf_client::CfFile;
use super::memory::build_memory_env;
use super::vanilla::{env_kv, env_secret};
use super::{ModpackHttp, ModpackProvider, ProviderContext, VersionInfo};

/// Accepts either a JSON number or a numeric string for `current_version_id`.
///
/// A pre-fix orchestrator bug serialized this field as a JSON string after
/// every successful upgrade. The forward fix in `orchestrator.rs` now writes
/// a number, but rows persisted while the bug was live still carry a string.
/// This deserializer auto-heals those on read so they don't have to be
/// rewritten by hand. It can be removed once those rows are gone.
fn u32_or_numeric_string<'de, D>(de: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Either {
        Num(u32),
        Str(String),
    }
    match Either::deserialize(de)? {
        Either::Num(n) => Ok(n),
        Either::Str(s) => s.parse().map_err(serde::de::Error::custom),
    }
}

/// Name of the shared `Secret` carrying `CF_API_KEY`. The chart provisions
/// this in the managed-server namespace alongside the panel's own copy.
const CF_API_KEY_SECRET: &str = "cf-api-key";

/// Key inside [`CF_API_KEY_SECRET`] that holds the API key bytes.
const CF_API_KEY_SECRET_FIELD: &str = "CF_API_KEY";

/// How long the orchestrator waits for `Done (` after starting a CF server.
///
/// ATM-11 first boot downloads `NeoForge` (~70 MB) and runs the installer,
/// then unpacks ~400 mods. 15min is comfortably above the observed P99 on
/// a warm cache; first-boot can take longer and operators should override
/// per-server in a future revision if they hit the ceiling.
const CF_BOOT_TIMEOUT: Duration = Duration::from_mins(15);

/// Release channel filter applied when picking the latest client file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
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
    /// Modpack URL slug (e.g. `"all-the-mods-11"`). Resolved once at
    /// create-time via the project endpoint and shipped to itzg as
    /// `CF_SLUG` — `mc-image-helper install-curseforge` rejects calls
    /// without one (`requireNonNull(slug)` in `CurseForgeInstaller`).
    pub slug: String,
    pub channel: Channel,
    /// Version names (or numeric file ids as strings) the user has chosen
    /// to skip. Compared against the CLIENT file id stored in
    /// `current_version_id`, since that's what we surface in the UI.
    pub version_skip: Vec<String>,
    /// CLIENT file id currently deployed (the one with `manifest.json`).
    /// itzg unpacks the client zip, reads its manifest, and downloads each
    /// listed mod via the CF API — passing a server-pack id here would
    /// trip itzg's "do not select a server file" guard.
    #[serde(deserialize_with = "u32_or_numeric_string")]
    pub current_version_id: u32,
    /// Display name of the currently deployed CLIENT file.
    pub current_version_name: String,
    /// Auto-update behaviour for this server.
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

    /// Picks the CLIENT file id of the newest version matching this config.
    ///
    /// Returns `None` if nothing qualifies. itzg's `AUTO_CURSEFORGE` path
    /// requires a client file (the one with `manifest.json`) — projects
    /// that only ship direct `isServerPack: true` files have no manifest
    /// and are unsupported here.
    ///
    /// The candidates are non-server-pack files matching the channel
    /// filter and not in the skip list (matched against the file id or
    /// display name). A linked `serverPackFileId` is deliberately NOT
    /// required: mc-image-helper installs from the client manifest and
    /// never reads server packs, and some projects (ATM-11 0.2.0) ship
    /// their server files as unlinked "additional files".
    #[must_use]
    pub fn pick_latest_client_file_id(&self, files: &[CfFile]) -> Option<u32> {
        files
            .iter()
            .filter(|f| !f.is_server_pack)
            .filter(|f| self.config.channel.accepts(f.release_type))
            .filter(|f| {
                let id_str = f.id.to_string();
                !self
                    .config
                    .version_skip
                    .iter()
                    .any(|s| s == &id_str || s == &f.display_name)
            })
            .max_by(|a, b| a.file_date.cmp(&b.file_date))
            .map(|f| f.id)
    }

    /// Looks up the [`VersionInfo`] for `client_file_id` in the cached
    /// listing, falling back to a per-id GET if the user pinned a file
    /// that isn't on the recent pages.
    async fn resolve_client_file(
        &self,
        cf: &super::cf_client::CurseForgeClient,
        files: &[CfFile],
        client_file_id: u32,
    ) -> Result<VersionInfo> {
        if let Some(found) = files.iter().find(|f| f.id == client_file_id) {
            return Ok(VersionInfo {
                id: found.id.to_string(),
                name: found.display_name.clone(),
                download_url: found.download_url.clone().unwrap_or_default(),
            });
        }
        let f = cf.file(self.config.project_id, client_file_id).await?;
        Ok(VersionInfo {
            id: f.id.to_string(),
            name: f.display_name,
            download_url: f.download_url.unwrap_or_default(),
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

    fn launch_command(&self) -> Option<Vec<String>> {
        None
    }

    fn extra_env(&self, ctx: &ProviderContext<'_>) -> Vec<EnvVar> {
        // itzg's AUTO_CURSEFORGE: CF_SLUG identifies the modpack, CF_FILE_ID
        // pins the CLIENT file, CF_API_KEY (mounted from the per-namespace
        // Secret the chart provisions) authorises the API calls. itzg writes
        // /data/.install-curseforge.env after install; on subsequent boots
        // mc-image-helper compares the requested CF_FILE_ID to the persisted
        // one and reinstalls when they differ — the orchestrator just patches
        // this env var to apply an update.
        let mut env = vec![
            env_kv("EULA", "TRUE"),
            env_kv("TYPE", "AUTO_CURSEFORGE"),
            env_secret("CF_API_KEY", CF_API_KEY_SECRET, CF_API_KEY_SECRET_FIELD),
            env_kv("CF_SLUG", &self.config.slug),
            env_kv("CF_FILE_ID", &self.config.current_version_id.to_string()),
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
        env
    }

    fn boot_timeout(&self) -> Duration {
        CF_BOOT_TIMEOUT
    }

    async fn latest(&self, http: &ModpackHttp<'_>) -> Result<Option<VersionInfo>> {
        let cf = http
            .cf
            .ok_or_else(|| anyhow!("CurseForge client unavailable"))?;
        let files = cf.list_files(self.config.project_id).await?;
        let Some(client_id) = self.pick_latest_client_file_id(&files) else {
            return Ok(None);
        };
        Ok(Some(self.resolve_client_file(cf, &files, client_id).await?))
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
        // look for the matching id. Linked server-pack files don't appear in
        // the listing — fall through to a per-id lookup.
        let listed = cf.list_files(self.config.project_id).await?;
        if let Some(f) = listed.iter().find(|f| f.id == id_u32) {
            return f.download_url.clone().ok_or_else(|| {
                anyhow!(
                    "file {} has no download_url (project disabled API distribution)",
                    version.id
                )
            });
        }
        let f = cf.file(self.config.project_id, id_u32).await?;
        f.download_url.ok_or_else(|| {
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
            server_pack_file_id: None,
            download_url: Some(format!("https://example.com/{id}.zip")),
            file_date: date.to_owned(),
            file_name: format!("{id}.zip"),
            game_versions: Vec::new(),
        }
    }

    /// Builds a CLIENT-style file (no `isServerPack`, but with a linked
    /// server-pack id). This is the shape every modern modpack returns.
    fn cf_client_with_link(id: u32, name: &str, rtype: u8, date: &str, linked: u32) -> CfFile {
        let mut f = cf_file(id, name, false, rtype, date);
        f.server_pack_file_id = Some(linked);
        f
    }

    fn pack(channel: Channel, skip: Vec<String>) -> CurseForgeServerPack {
        CurseForgeServerPack::new(Config {
            project_id: 1_148_445,
            slug: "atm-11".to_owned(),
            channel,
            version_skip: skip,
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
    fn config_accepts_current_version_id_as_number() {
        let cfg: Config = serde_json::from_str(
            r#"{
                "project_id": 520914,
                "slug": "atm-11",
                "channel": "release",
                "version_skip": [],
                "current_version_id": 8066228,
                "current_version_name": "v1",
                "auto_update_mode": "notify"
            }"#,
        )
        .expect("number form");
        assert_eq!(cfg.current_version_id, 8_066_228);
    }

    // Defensive deserializer: rows persisted while Bug 1 was live carry a
    // stringified id. Reading those rows must succeed so the poller heals
    // them on the next tick instead of skipping the server forever.
    #[test]
    fn config_accepts_current_version_id_as_numeric_string() {
        let cfg: Config = serde_json::from_str(
            r#"{
                "project_id": 520914,
                "slug": "atm-11",
                "channel": "release",
                "version_skip": [],
                "current_version_id": "8066228",
                "current_version_name": "v1",
                "auto_update_mode": "notify"
            }"#,
        )
        .expect("string form");
        assert_eq!(cfg.current_version_id, 8_066_228);
    }

    #[test]
    fn config_rejects_current_version_id_as_non_numeric_string() {
        let err = serde_json::from_str::<Config>(
            r#"{
                "project_id": 520914,
                "slug": "atm-11",
                "channel": "release",
                "version_skip": [],
                "current_version_id": "not-a-number",
                "current_version_name": "v1",
                "auto_update_mode": "notify"
            }"#,
        )
        .expect_err("garbage must still error");
        assert!(err.to_string().contains("invalid digit"));
    }

    #[test]
    fn channel_beta_accepts_release_and_beta() {
        assert!(Channel::Beta.accepts(1));
        assert!(Channel::Beta.accepts(2));
        assert!(!Channel::Beta.accepts(3));
    }

    // Reproduces the ATM-11 shape: every file in the listing is a client
    // with a linked server-pack id; the picker returns the CLIENT id (the
    // one with manifest.json that itzg's mc-image-helper consumes).
    #[test]
    fn pick_latest_id_returns_client_id_with_linked_server_pack() {
        let p = pack(Channel::Release, vec![]);
        let files = vec![
            cf_client_with_link(1, "ATM-0.0.12", 1, "2026-04-01T00:00:00Z", 100),
            cf_client_with_link(2, "ATM-0.0.13", 1, "2026-05-01T00:00:00Z", 200),
        ];
        // Returns 2 (newest CLIENT), not 200 (its linked server pack).
        assert_eq!(p.pick_latest_client_file_id(&files), Some(2));
    }

    // Regression (ATM-11 0.2.0, 2026-07): authors sometimes upload the
    // server pack as an unlinked "additional file", so the newest client
    // file carries no `serverPackFileId`. itzg installs from the client
    // manifest and never reads server packs — the picker must not hide
    // such releases behind older, linked ones.
    #[test]
    fn pick_latest_id_accepts_clients_without_linked_server_pack() {
        let p = pack(Channel::Release, vec![]);
        let files = vec![
            cf_client_with_link(1, "ATM-0.1.2", 1, "2026-06-23T00:00:00Z", 100),
            cf_file(2, "ATM-0.2.0", false, 1, "2026-07-02T00:00:00Z"),
        ];
        assert_eq!(p.pick_latest_client_file_id(&files), Some(2));
    }

    #[test]
    fn pick_latest_id_skips_legacy_direct_server_pack_files() {
        // Direct server-pack files (isServerPack=true) lack manifest.json,
        // so itzg refuses them ("do not select a server file").
        let p = pack(Channel::Release, vec![]);
        let files = vec![cf_file(7, "ServerFiles", true, 1, "2026-01-02T00:00:00Z")];
        assert!(p.pick_latest_client_file_id(&files).is_none());
    }

    #[test]
    fn pick_latest_id_filters_clients_by_channel() {
        let p = pack(Channel::Release, vec![]);
        let files = vec![
            cf_client_with_link(1, "release", 1, "2026-01-01T00:00:00Z", 100),
            cf_client_with_link(2, "beta", 2, "2026-02-01T00:00:00Z", 200),
        ];
        assert_eq!(p.pick_latest_client_file_id(&files), Some(1));
    }

    #[test]
    fn pick_latest_id_skip_list_matches_client_id() {
        let p = pack(Channel::Release, vec!["2".to_owned()]);
        let files = vec![
            cf_client_with_link(1, "old", 1, "2026-01-01T00:00:00Z", 100),
            cf_client_with_link(2, "new", 1, "2026-02-01T00:00:00Z", 200),
        ];
        assert_eq!(p.pick_latest_client_file_id(&files), Some(1));
    }

    #[test]
    fn pick_latest_id_skip_list_matches_client_display_name() {
        let p = pack(Channel::Release, vec!["new".to_owned()]);
        let files = vec![
            cf_client_with_link(1, "old", 1, "2026-01-01T00:00:00Z", 100),
            cf_client_with_link(2, "new", 1, "2026-02-01T00:00:00Z", 200),
        ];
        assert_eq!(p.pick_latest_client_file_id(&files), Some(1));
    }

    #[test]
    fn pick_latest_id_picks_newest_by_date() {
        let p = pack(Channel::Release, vec![]);
        let files = vec![
            cf_client_with_link(1, "v1", 1, "2026-01-01T00:00:00Z", 100),
            cf_client_with_link(2, "v2", 1, "2026-03-01T00:00:00Z", 200),
            cf_client_with_link(3, "v1.5", 1, "2026-02-01T00:00:00Z", 150),
        ];
        assert_eq!(p.pick_latest_client_file_id(&files), Some(2));
    }

    fn pack_with_file(id: u32) -> CurseForgeServerPack {
        CurseForgeServerPack::new(Config {
            project_id: 1_148_445,
            slug: "atm-11".to_owned(),
            channel: Channel::Release,
            version_skip: Vec::new(),
            current_version_id: id,
            current_version_name: format!("file-{id}"),
            auto_update_mode: AutoUpdateMode::Notify,
        })
    }

    #[test]
    fn extra_env_runs_itzg_auto_curseforge() {
        let p = pack_with_file(7_777);
        let ctx = ProviderContext {
            server_id: "abcd",
            memory_mi: 8192,
        };
        let env = p.extra_env(&ctx);
        let by_name: std::collections::BTreeMap<&str, Option<&str>> = env
            .iter()
            .map(|e| (e.name.as_str(), e.value.as_deref()))
            .collect();
        assert_eq!(by_name.get("EULA").copied().flatten(), Some("TRUE"));
        assert_eq!(
            by_name.get("TYPE").copied().flatten(),
            Some("AUTO_CURSEFORGE"),
        );
        assert_eq!(by_name.get("CF_SLUG").copied().flatten(), Some("atm-11"));
        assert_eq!(by_name.get("CF_FILE_ID").copied().flatten(), Some("7777"));
        assert_eq!(by_name.get("MAX_MEMORY").copied().flatten(), Some("8192M"),);
        assert_eq!(by_name.get("INIT_MEMORY").copied().flatten(), Some("2048M"),);
        assert_eq!(by_name.get("ENABLE_RCON").copied().flatten(), Some("true"));
    }

    #[test]
    fn extra_env_pulls_cf_api_key_from_shared_secret() {
        let p = pack_with_file(7_777);
        let ctx = ProviderContext {
            server_id: "abcd",
            memory_mi: 4096,
        };
        let env = p.extra_env(&ctx);
        let cf = env
            .iter()
            .find(|e| e.name == "CF_API_KEY")
            .expect("api key");
        let sk = cf
            .value_from
            .as_ref()
            .unwrap()
            .secret_key_ref
            .as_ref()
            .unwrap();
        assert_eq!(sk.name, "cf-api-key");
        assert_eq!(sk.key, "CF_API_KEY");
    }

    #[test]
    fn extra_env_pulls_rcon_password_from_per_server_secret() {
        let p = pack_with_file(7_777);
        let ctx = ProviderContext {
            server_id: "abcd",
            memory_mi: 4096,
        };
        let env = p.extra_env(&ctx);
        let rcon = env
            .iter()
            .find(|e| e.name == "RCON_PASSWORD")
            .expect("rcon");
        let sk = rcon
            .value_from
            .as_ref()
            .unwrap()
            .secret_key_ref
            .as_ref()
            .unwrap();
        assert_eq!(sk.name, "mc-abcd-rcon");
        assert_eq!(sk.key, "password");
    }

    #[test]
    fn provider_launch_command_is_none() {
        let p = pack_with_file(7_777);
        assert!(p.launch_command().is_none());
    }
}
