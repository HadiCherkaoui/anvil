//! Vanilla MC server provider (refactored from the M2 inline path).
//!
//! Drives the `itzg/minecraft-server` image which configures itself from
//! env vars (EULA, TYPE, VERSION, `INIT_MEMORY`, `MAX_MEMORY`, RCON). The
//! `extra_env` set mirrors what M2's `build_statefulset` hardcoded.

use std::time::Duration;

use anyhow::Result;
use k8s_openapi::api::core::v1::{EnvVar, EnvVarSource, SecretKeySelector};

use super::{ModpackHttp, ModpackProvider, ProviderContext, VersionInfo};

/// Container image used for managed vanilla Minecraft servers.
///
/// Pinned to the Java 25 tag — the JRE is forward-compatible with vanilla
/// jars compiled against earlier Javas, and modern modpacks (ATM-11 +
/// `NeoForge` 26.x) ship class files with version 69 (Java 25). Same image
/// across all providers means the cluster pulls one upstream tag.
const VANILLA_IMAGE: &str = "itzg/minecraft-server:java25";

/// How long we wait for the vanilla server to print `Done (` after boot.
///
/// Plain Vanilla starts in well under a minute; allow 5 to absorb a slow
/// pull or a cold ZFS PVC mount.
const VANILLA_BOOT_TIMEOUT: Duration = Duration::from_mins(5);

/// Vanilla MC server provider — itzg/minecraft-server with env-var config.
#[derive(Debug, Clone, Copy, Default)]
pub struct VanillaProvider {
    mc_version: VanillaVersion,
}

/// Minecraft version the vanilla provider asks itzg to download.
///
/// Currently a thin wrapper over a `&'static str` so that the M2 path which
/// validates against `KNOWN_MC_VERSIONS` continues to drive the env. The
/// provider stores this so `extra_env` can return the `VERSION` env var
/// without an extra parameter.
#[derive(Debug, Clone, Copy, Default)]
pub struct VanillaVersion(Option<&'static str>);

impl VanillaProvider {
    /// Creates a vanilla provider with no MC version set (used by `from_db`
    /// where the env vars are layered on at the create site that holds the
    /// validated version string).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds the EULA / TYPE / VERSION / memory env vars and the RCON pair.
    ///
    /// Pulled out so the create handler can call it directly when it has a
    /// validated `mc_version: &str` without needing to construct a typed
    /// version first. The trait-method form below assumes the version was
    /// already baked in.
    #[must_use]
    pub fn build_env(server_id: &str, mc_version: &str, memory_mi: i64) -> Vec<EnvVar> {
        vec![
            env_kv("EULA", "TRUE"),
            env_kv("TYPE", "VANILLA"),
            env_kv("VERSION", mc_version),
            env_kv("INIT_MEMORY", &format!("{}M", init_memory_mi(memory_mi))),
            env_kv("MAX_MEMORY", &format!("{memory_mi}M")),
            env_kv("JVM_XX_OPTS", IDLE_GC_OPTS),
            env_kv("ENABLE_RCON", "true"),
            env_secret("RCON_PASSWORD", &format!("mc-{server_id}-rcon"), "password"),
        ]
    }
}

/// Initial JVM heap size in MiB given a max budget.
///
/// itzg's image sets `-Xms` from `INIT_MEMORY` and `-Xmx` from `MAX_MEMORY`;
/// when `INIT_MEMORY` matches `MAX_MEMORY` the JVM commits the full heap up
/// front and never returns pages to the OS, which leaves idle pods sitting
/// at the configured ceiling. A quarter-of-max start (floor 1 GiB) lets the
/// heap commit lazily as mods load — paired with [`IDLE_GC_OPTS`] so the
/// heap also shrinks back during long idles.
pub(super) fn init_memory_mi(max_mi: i64) -> i64 {
    (max_mi / 4).max(1024)
}

/// JVM `-XX:` flags that let G1 release committed heap to the OS during
/// long idles. Without these, G1 only grows the heap toward `-Xmx`; with
/// them, every 30s of idle the JVM runs a concurrent collection that can
/// return unused regions to the OS, so an idle pod's RSS tracks live-set
/// rather than peak heap. JEP 346 — supported on Java 12+.
pub(super) const IDLE_GC_OPTS: &str =
    "-XX:+G1PeriodicGCInvokesConcurrent -XX:G1PeriodicGCInterval=30000";

#[async_trait::async_trait]
impl ModpackProvider for VanillaProvider {
    fn kind(&self) -> &'static str {
        "vanilla"
    }

    fn pod_image(&self) -> &str {
        VANILLA_IMAGE
    }

    fn launch_command(&self) -> Option<Vec<String>> {
        None
    }

    fn extra_env(&self, ctx: &ProviderContext<'_>) -> Vec<EnvVar> {
        let version = self.mc_version.0.unwrap_or("LATEST");
        Self::build_env(ctx.server_id, version, ctx.memory_mi)
    }

    fn boot_timeout(&self) -> Duration {
        VANILLA_BOOT_TIMEOUT
    }

    async fn latest(&self, _http: &ModpackHttp<'_>) -> Result<Option<VersionInfo>> {
        Ok(None)
    }

    async fn fetch_url(&self, _http: &ModpackHttp<'_>, _version: &VersionInfo) -> Result<String> {
        unreachable!("orchestrator must never call fetch_url on a vanilla provider")
    }
}

/// `EnvVar { name, value }` constructor.
pub(super) fn env_kv(name: &str, value: &str) -> EnvVar {
    EnvVar {
        name: name.to_owned(),
        value: Some(value.to_owned()),
        value_from: None,
    }
}

/// `EnvVar` sourced from a Secret key.
pub(super) fn env_secret(name: &str, secret_name: &str, key: &str) -> EnvVar {
    EnvVar {
        name: name.to_owned(),
        value: None,
        value_from: Some(EnvVarSource {
            secret_key_ref: Some(SecretKeySelector {
                name: secret_name.to_owned(),
                key: key.to_owned(),
                optional: Some(false),
            }),
            ..EnvVarSource::default()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_env_emits_expected_keys_in_order() {
        let env = VanillaProvider::build_env("abcd", "1.21.4", 4096);
        let names: Vec<_> = env.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "EULA",
                "TYPE",
                "VERSION",
                "INIT_MEMORY",
                "MAX_MEMORY",
                "JVM_XX_OPTS",
                "ENABLE_RCON",
                "RCON_PASSWORD"
            ]
        );
    }

    #[test]
    fn build_env_memory_appends_m_suffix() {
        let env = VanillaProvider::build_env("abcd", "1.21.4", 4096);
        let max = env.iter().find(|e| e.name == "MAX_MEMORY").unwrap();
        assert_eq!(max.value.as_deref(), Some("4096M"));
        let init = env.iter().find(|e| e.name == "INIT_MEMORY").unwrap();
        assert_eq!(init.value.as_deref(), Some("1024M"));
    }

    #[test]
    fn init_memory_mi_floors_at_one_gib() {
        assert_eq!(init_memory_mi(1024), 1024);
        assert_eq!(init_memory_mi(2048), 1024);
        assert_eq!(init_memory_mi(4096), 1024);
        assert_eq!(init_memory_mi(8192), 2048);
        assert_eq!(init_memory_mi(17408), 4352);
    }

    #[test]
    fn build_env_rcon_password_uses_per_server_secret() {
        let env = VanillaProvider::build_env("abcd", "1.21.4", 4096);
        let rcon = env.iter().find(|e| e.name == "RCON_PASSWORD").unwrap();
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
    fn provider_kind_is_vanilla() {
        let p = VanillaProvider::new();
        assert_eq!(p.kind(), "vanilla");
    }

    #[test]
    fn provider_pod_image_is_itzg_java25() {
        let p = VanillaProvider::new();
        assert_eq!(p.pod_image(), VANILLA_IMAGE);
    }

    #[test]
    fn provider_launch_command_is_none() {
        let p = VanillaProvider::new();
        assert!(p.launch_command().is_none());
    }
}
