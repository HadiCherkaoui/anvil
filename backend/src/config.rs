//! Runtime configuration read from env vars at startup.
//!
//! M1 only needs four knobs. Everything else (`StorageClass`, MC version
//! defaults, `Service` type) is M2 and lives in `mcDefaults` on the Helm
//! chart — those env vars get added when the create handler does.

use std::env;
use std::net::SocketAddr;

use anyhow::{Context as _, Result};

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

/// Resolved process configuration.
#[derive(Debug, Clone)]
pub struct Config {
    /// Address the HTTP server binds to.
    pub bind_addr: SocketAddr,
    /// `sqlx`-compatible URL for the panel's `SQLite` database.
    pub database_url: String,
    /// Namespace where managed Minecraft resources live.
    pub mc_namespace: String,
    /// `tracing` filter directive (e.g. `info`, `info,kube=warn`).
    pub log_level: String,
}

impl Config {
    /// Reads configuration from `ANVIL_*` environment variables.
    ///
    /// # Errors
    ///
    /// Returns an error if `ANVIL_BIND_ADDR` is set but does not parse as a
    /// `SocketAddr`. Other knobs accept any string.
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

        Ok(Self {
            bind_addr,
            database_url,
            mc_namespace,
            log_level,
        })
    }
}
