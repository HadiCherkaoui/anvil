//! Runtime configuration read from env vars at startup.
//!
//! M2 adds four knobs surfacing the cluster contract from `mcDefaults` in
//! the Helm chart: the storage class for managed PVCs, the default Service
//! type, the external host used to display NodePort addresses, and the
//! LoadBalancer-supported flag the create handler honors.

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

/// Default value for [`Config::mc_svc_type`].
const DEFAULT_MC_SVC_TYPE: &str = "LoadBalancer";

/// Default value for [`Config::node_host`] (empty = no NodePort host configured).
const DEFAULT_NODE_HOST: &str = "";

/// Default value for [`Config::loadbalancer_supported`] expressed as a string,
/// since env vars are stringly typed.
const DEFAULT_LB_SUPPORTED: &str = "true";

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
    /// Default `StorageClass` for managed-server PVCs (chart `mcDefaults.storageClassName`).
    /// Required — the chart enforces non-empty at render time.
    pub mc_storage_class: String,
    /// Default Service type for managed servers (chart `mcDefaults.serviceType`).
    pub mc_svc_type: String,
    /// External hostname/IP of any cluster node, used when displaying NodePort addresses.
    pub node_host: String,
    /// Whether the cluster has a LoadBalancer provider. When false, requests for
    /// `exposure_mode=loadbalancer` are rejected with 502.
    pub loadbalancer_supported: bool,
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

        Ok(Self {
            bind_addr,
            database_url,
            mc_namespace,
            log_level,
            mc_storage_class,
            mc_svc_type,
            node_host,
            loadbalancer_supported,
        })
    }
}
