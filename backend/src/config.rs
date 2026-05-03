//! Runtime configuration read from env vars at startup.
//!
//! M2 adds four knobs surfacing the cluster contract from `mcDefaults` in
//! the Helm chart: the storage class for managed PVCs, the default Service
//! type, the external host used to display `NodePort` addresses, and the
//! LoadBalancer-supported flag the create handler honors.

use std::env;
use std::net::SocketAddr;

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;

/// Default value for [`Config::bind_addr`].
const DEFAULT_BIND_ADDR: &str = "0.0.0.0:8080";

/// Default value for [`Config::database_url`].
///
/// Relative path so `cargo run` from `backend/` writes alongside the source
/// tree. The Helm chart overrides this to `sqlite:///var/lib/anvil/anvil.db`.
const DEFAULT_DATABASE_URL: &str = "sqlite://./anvil.db?mode=rwc";

/// Default value for [`Config::mc_namespace`].
const DEFAULT_MC_NAMESPACE: &str = "mc";

/// Default value for [`Config::log_level`].
const DEFAULT_LOG_LEVEL: &str = "info";

/// Default value for [`Config::mc_svc_type`].
const DEFAULT_MC_SVC_TYPE: &str = "LoadBalancer";

/// Default value for [`Config::node_host`] (empty = no `NodePort` host configured).
const DEFAULT_NODE_HOST: &str = "";

/// Default value for [`Config::loadbalancer_supported`] expressed as a string,
/// since env vars are stringly typed.
const DEFAULT_LB_SUPPORTED: &str = "true";

/// Default value for [`Config::modpack_poll_interval_minutes`].
const DEFAULT_MODPACK_POLL_MINUTES: &str = "60";

/// Resolved process configuration.
#[derive(Clone)]
pub struct Config {
    /// Address the HTTP server binds to.
    pub bind_addr: SocketAddr,
    /// `sqlx`-compatible URL for the panel's `SQLite` database.
    pub database_url: String,
    /// Namespace where managed Minecraft resources live.
    pub mc_namespace: String,
    /// `tracing` filter directive (e.g. `info`, `info,kube=warn`).
    pub log_level: String,
    /// Default `StorageClass` for managed-server PVCs (chart `mcDefaults.storageClassName`).
    /// Required — the chart enforces non-empty at render time.
    pub mc_storage_class: String,
    /// Default Service type for managed servers (chart `mcDefaults.serviceType`).
    pub mc_svc_type: String,
    /// External hostname/IP of any cluster node, used when displaying `NodePort` addresses.
    pub node_host: String,
    /// Whether the cluster has a `LoadBalancer` provider. When false, requests for
    /// `exposure_mode=loadbalancer` are rejected with 502.
    pub loadbalancer_supported: bool,
    /// OIDC issuer URL — Authentik's `application/o/<slug>/`.
    pub oidc_issuer_url: String,
    /// OIDC client ID issued by Authentik for the Anvil application.
    pub oidc_client_id: String,
    /// OIDC client secret. Loaded from `ANVIL_OIDC_CLIENT_SECRET_FILE` (k8s mount)
    /// when set, falling back to `ANVIL_OIDC_CLIENT_SECRET`.
    pub oidc_client_secret: String,
    /// Public callback URL Authentik redirects to after login.
    pub oidc_redirect_url: String,
    /// HMAC key for the session JWT and the encrypted OIDC-state cookie.
    /// Base64-decoded; ≥32 bytes required.
    pub session_key: Vec<u8>,
    /// Allowlist of Authentik subject UUIDs. Empty = any authenticated user.
    pub allowed_subs: Vec<String>,
    /// `CurseForge` API key (M5). When unset / empty, modpack support is disabled
    /// and the New Server modal hides the `CurseForge` option.
    pub cf_api_key: Option<String>,
    /// Name of the PVC mounted by backup/swap/sync Jobs. Required — Modrinth
    /// (always-on) needs the snapshots PVC for the mod-sync FSM. The chart's
    /// modpack.snapshotsPvc value gates this.
    pub modpack_snapshots_pvc: String,
    /// Hourly poll interval for `modpack_versions` updates.
    pub modpack_poll_interval_minutes: u64,
}

// `Vec<u8>` for `session_key` would print the raw HMAC key in `Debug`; hand-roll
// to redact it.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("bind_addr", &self.bind_addr)
            .field("database_url", &self.database_url)
            .field("mc_namespace", &self.mc_namespace)
            .field("log_level", &self.log_level)
            .field("mc_storage_class", &self.mc_storage_class)
            .field("mc_svc_type", &self.mc_svc_type)
            .field("node_host", &self.node_host)
            .field("loadbalancer_supported", &self.loadbalancer_supported)
            .field("oidc_issuer_url", &self.oidc_issuer_url)
            .field("oidc_client_id", &self.oidc_client_id)
            .field("oidc_client_secret", &"<redacted>")
            .field("oidc_redirect_url", &self.oidc_redirect_url)
            .field("session_key", &"<redacted>")
            .field("allowed_subs", &self.allowed_subs)
            .field(
                "cf_api_key",
                &self.cf_api_key.as_ref().map(|_| "<redacted>"),
            )
            .field("modpack_snapshots_pvc", &self.modpack_snapshots_pvc)
            .field(
                "modpack_poll_interval_minutes",
                &self.modpack_poll_interval_minutes,
            )
            .finish()
    }
}

impl Config {
    /// Reads configuration from `ANVIL_*` environment variables.
    ///
    /// # Errors
    ///
    /// Returns an error if `ANVIL_BIND_ADDR` is set but does not parse as a
    /// `SocketAddr`, if `ANVIL_MC_STORAGE_CLASS` is unset (the chart enforces
    /// this; the binary refuses to start without it), or if `ANVIL_LB_SUPPORTED`
    /// does not parse as a `bool`.
    pub fn from_env() -> Result<Self> {
        let bind_addr_str =
            env::var("ANVIL_BIND_ADDR").unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_owned());
        let bind_addr: SocketAddr = bind_addr_str
            .parse()
            .with_context(|| format!("ANVIL_BIND_ADDR={bind_addr_str:?} is not a SocketAddr"))?;

        let database_url =
            env::var("ANVIL_DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_owned());
        let mc_namespace =
            env::var("ANVIL_MC_NAMESPACE").unwrap_or_else(|_| DEFAULT_MC_NAMESPACE.to_owned());
        let log_level =
            env::var("ANVIL_LOG_LEVEL").unwrap_or_else(|_| DEFAULT_LOG_LEVEL.to_owned());

        let mc_storage_class = env::var("ANVIL_MC_STORAGE_CLASS").context(
            "ANVIL_MC_STORAGE_CLASS must be set (helm chart mcDefaults.storageClassName)",
        )?;
        let mc_svc_type =
            env::var("ANVIL_MC_SVC_TYPE").unwrap_or_else(|_| DEFAULT_MC_SVC_TYPE.to_owned());
        let node_host =
            env::var("ANVIL_NODE_HOST").unwrap_or_else(|_| DEFAULT_NODE_HOST.to_owned());

        let lb_supported_str =
            env::var("ANVIL_LB_SUPPORTED").unwrap_or_else(|_| DEFAULT_LB_SUPPORTED.to_owned());
        let loadbalancer_supported: bool = lb_supported_str
            .parse()
            .with_context(|| format!("ANVIL_LB_SUPPORTED={lb_supported_str:?} is not a boolean"))?;

        let oidc_issuer_url =
            env::var("ANVIL_OIDC_ISSUER_URL").context("ANVIL_OIDC_ISSUER_URL must be set")?;
        let oidc_client_id =
            env::var("ANVIL_OIDC_CLIENT_ID").context("ANVIL_OIDC_CLIENT_ID must be set")?;
        let oidc_client_secret =
            read_secret("ANVIL_OIDC_CLIENT_SECRET_FILE", "ANVIL_OIDC_CLIENT_SECRET")?;
        let oidc_redirect_url =
            env::var("ANVIL_OIDC_REDIRECT_URL").context("ANVIL_OIDC_REDIRECT_URL must be set")?;
        let session_key_b64 = read_secret("ANVIL_SESSION_KEY_FILE", "ANVIL_SESSION_KEY")?;
        let session_key = base64::engine::general_purpose::STANDARD
            .decode(session_key_b64.trim())
            .context("ANVIL_SESSION_KEY must be standard base64")?;
        if session_key.len() < 32 {
            bail!(
                "ANVIL_SESSION_KEY is {} bytes after base64-decode; need at least 32",
                session_key.len()
            );
        }
        let allowed_subs = env::var("ANVIL_ALLOWED_SUBS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();

        // CF_API_KEY is optional (CF disabled if unset). The snapshots PVC is
        // mandatory because Modrinth (always-on) and modded servers both need
        // it for the mod-sync / pack-swap FSMs.
        let cf_api_key = optional_secret("CF_API_KEY_FILE", "CF_API_KEY")?;
        let modpack_snapshots_pvc = env::var("ANVIL_MODPACK_SNAPSHOTS_PVC").context(
            "ANVIL_MODPACK_SNAPSHOTS_PVC must be set — modpack/modded updates need a snapshots PVC",
        )?;
        if modpack_snapshots_pvc.is_empty() {
            bail!("ANVIL_MODPACK_SNAPSHOTS_PVC must not be empty");
        }
        let modpack_poll_interval_minutes_str = env::var("ANVIL_MODPACK_POLL_MINUTES")
            .unwrap_or_else(|_| DEFAULT_MODPACK_POLL_MINUTES.to_owned());
        let modpack_poll_interval_minutes: u64 =
            modpack_poll_interval_minutes_str.parse().with_context(|| {
                format!(
                    "ANVIL_MODPACK_POLL_MINUTES={modpack_poll_interval_minutes_str:?} is not a u64"
                )
            })?;
        if modpack_poll_interval_minutes == 0 {
            bail!("ANVIL_MODPACK_POLL_MINUTES must be > 0");
        }

        Ok(Self {
            bind_addr,
            database_url,
            mc_namespace,
            log_level,
            mc_storage_class,
            mc_svc_type,
            node_host,
            loadbalancer_supported,
            oidc_issuer_url,
            oidc_client_id,
            oidc_client_secret,
            oidc_redirect_url,
            session_key,
            allowed_subs,
            cf_api_key,
            modpack_snapshots_pvc,
            modpack_poll_interval_minutes,
        })
    }
}

/// Like [`read_secret`] but returns `None` (not an error) when both vars are unset.
fn optional_secret(file_var: &str, value_var: &str) -> Result<Option<String>> {
    if let Ok(path) = env::var(file_var) {
        return std::fs::read_to_string(&path)
            .with_context(|| format!("{file_var}={path:?} could not be read"))
            .map(|s| Some(s.trim_end_matches(['\n', '\r']).to_owned()));
    }
    Ok(env::var(value_var).ok().filter(|s| !s.is_empty()))
}

/// Reads a secret value from a k8s-mounted file path or, failing that, an env var.
fn read_secret(file_var: &str, value_var: &str) -> Result<String> {
    if let Ok(path) = env::var(file_var) {
        return std::fs::read_to_string(&path)
            .with_context(|| format!("{file_var}={path:?} could not be read"))
            .map(|s| s.trim_end_matches(['\n', '\r']).to_owned());
    }
    env::var(value_var).with_context(|| format!("{value_var} (or {file_var}) must be set"))
}
