//! Anvil — k8s-native Minecraft server panel.
//!
//! This crate exposes both an `anvil` binary and a small library surface
//! used by integration tests. Only items that need to be exercised from
//! `tests/*.rs` are public.

use std::fmt;

use kube::Client;
use sqlx::SqlitePool;

use crate::routes::cluster::CapabilitiesCache;

pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod k8s;
pub mod k8s_builders;
pub mod k8s_status;
pub mod routes;
#[cfg(any(feature = "serve-dir", feature = "embed"))]
pub mod static_serve;
pub mod validation;
pub mod ws;

pub use routes::{router, stateless_router};

/// State shared across handlers.
///
/// Cheap to clone — `Client`, `SqlitePool`, and `String` all wrap
/// reference-counted internals — so axum's `State` extractor is fine to
/// use everywhere.
#[derive(Clone)]
pub struct AppState {
    /// Live Kubernetes client (in-cluster SA token *or* local kubeconfig).
    pub kube: Client,
    /// Connection pool for the panel's `SQLite` database.
    pub pool: SqlitePool,
    /// Namespace where managed Minecraft resources live.
    pub mc_namespace: String,
    /// Default `StorageClass` for managed-server PVCs.
    pub mc_storage_class: String,
    /// Default Service type for managed servers.
    pub mc_svc_type: String,
    /// External hostname/IP of any cluster node (used to display `NodePort` addresses).
    pub node_host: String,
    /// Whether the cluster has a `LoadBalancer` provider.
    pub loadbalancer_supported: bool,
    /// 5-minute cache for `GET /api/cluster/capabilities`.
    pub capabilities_cache: CapabilitiesCache,
}

// `kube::Client` doesn't impl `Debug`, so the derive on `AppState` would
// fail. Hand-rolling the impl keeps the `missing_debug_implementations`
// lint happy while still hiding the client's internals.
impl fmt::Debug for AppState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppState")
            .field("kube", &"<kube::Client>")
            .field("pool", &"<SqlitePool>")
            .field("mc_namespace", &self.mc_namespace)
            .field("mc_storage_class", &self.mc_storage_class)
            .field("mc_svc_type", &self.mc_svc_type)
            .field("node_host", &self.node_host)
            .field("loadbalancer_supported", &self.loadbalancer_supported)
            .field("capabilities_cache", &"<Mutex<...>>")
            .finish()
    }
}
