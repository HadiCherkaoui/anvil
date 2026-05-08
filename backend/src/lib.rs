//! Anvil — k8s-native Minecraft server panel.
//!
//! This crate exposes both an `anvil` binary and a small library surface
//! used by integration tests. Only items that need to be exercised from
//! `tests/*.rs` are public.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::FromRef;
use axum_extra::extract::cookie::Key;
use kube::Client;
use sqlx::SqlitePool;
use tokio::sync::{Mutex as AsyncMutex, watch};

use crate::auth::OidcState;
use crate::modpack::{CurseForgeClient, ModrinthClient, orchestrator::UpdatePhase};
use crate::routes::cluster::CapabilitiesCache;
use crate::routes::mc_versions::McVersionsCache;
use crate::routes::papermc::PaperVersionsCache;
use crate::routes::runtimes::LoaderVersionCache;

pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod files;
pub mod files_helper;
pub mod k8s;
pub mod k8s_builders;
pub mod k8s_patches;
pub mod k8s_status;
pub mod modpack;
pub mod players;
pub mod routes;
pub mod server_properties;
#[cfg(any(feature = "serve-dir", feature = "embed"))]
pub mod static_serve;
pub mod validation;
pub mod ws;

pub use routes::{router, stateless_router};

/// State shared across handlers.
///
/// Cheap to clone — `Client`, `SqlitePool`, `String`, and `Arc` all wrap
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
    /// 24-hour cache for `GET /api/cluster/mc-versions`.
    pub mc_versions_cache: McVersionsCache,
    /// 1-hour cache for `GET /api/runtimes/{runtime}/versions` (Forge / `NeoForge`).
    pub loader_version_cache: LoaderVersionCache,
    /// 1-hour cache for `GET /api/papermc/versions` (Paper MC versions).
    pub papermc_cache: PaperVersionsCache,
    /// HMAC secret for the session JWT.
    pub session_key: Vec<u8>,
    /// AES-GCM key (derived from `session_key`) for the encrypted OIDC-state cookie.
    pub cookie_key: Key,
    /// Authentik subjects allowed to use the panel. Empty = any authenticated user.
    pub allowed_subs: Vec<String>,
    /// OIDC client + cached provider metadata.
    pub oidc: Arc<OidcState>,
    /// `CurseForge` HTTP client. `None` when `CF_API_KEY` is unset — CF
    /// modpack support is then disabled, but Modrinth still works.
    pub cf_client: Option<Arc<CurseForgeClient>>,
    /// Modrinth HTTP client. Always present (no API key required).
    pub mr_client: Arc<ModrinthClient>,
    /// Name of the shared snapshots PVC mounted by backup/swap/sync Jobs.
    pub snapshots_pvc: Arc<String>,
    /// How often the modpack poller refreshes `modpack_versions`.
    pub modpack_poll_interval: Duration,
    /// Server ids with an update orchestrator currently running.
    pub update_locks: Arc<std::sync::Mutex<HashSet<String>>>,
    /// Live `watch::Receiver` per running update — fed to the update WS.
    pub update_phase_buses: Arc<std::sync::Mutex<HashMap<String, watch::Receiver<UpdatePhase>>>>,
    /// Last error string captured by a failed update / backup / restore /
    /// version-change FSM, keyed by `server_id`. Cleared on the next
    /// `UpdateGuard::try_acquire` for that server. Read by the update WS
    /// when emitting a `done{result: failed*}` frame so the UI can show
    /// the operator a real failure reason.
    pub update_errors: Arc<std::sync::Mutex<HashMap<String, String>>>,
    /// Last terminal phase for each server, kept for ~`RECENT_TERMINAL_TTL`
    /// after the FSM completes. Lets the update / apply WS surface a
    /// done-frame even when a fast FSM finished before the client
    /// connected (the "remove + apply on a stopped server completes in
    /// 200ms" case). Without this the client sees `no-apply-in-progress`
    /// and is left wondering whether anything happened.
    pub update_terminals: Arc<std::sync::Mutex<HashMap<String, (UpdatePhase, std::time::Instant)>>>,
    /// Serializes backup + swap + restore Jobs panel-wide so one Job at a
    /// time mounts the shared snapshots PVC (RWO on single-node ZFS).
    pub snapshot_pvc_lock: Arc<AsyncMutex<()>>,
    /// Alpine image shared by the per-server file-browser helper Pod
    /// (sub-project D) and the mod-sync Job (M5).
    pub mc_alpine_image: String,
    /// IANA timezone applied as `TZ` env var on every managed MC pod.
    pub mc_timezone: String,
    /// Container image for managed MC `StatefulSet`s across all providers.
    pub mc_itzg_image: String,
    /// Image used by backup / restore / snapshot-cleanup Jobs.
    pub mc_busybox_image: String,
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
            .field("mc_versions_cache", &"<Mutex<...>>")
            .field("loader_version_cache", &"<Mutex<...>>")
            .field("papermc_cache", &"<Mutex<...>>")
            .field("session_key", &"<redacted>")
            .field("cookie_key", &"<redacted>")
            .field("allowed_subs", &self.allowed_subs)
            .field("oidc", &self.oidc)
            .field("cf_client", &self.cf_client.as_ref().map(|_| "<cf>"))
            .field("mr_client", &"<mr>")
            .field("snapshots_pvc", &self.snapshots_pvc)
            .field("modpack_poll_interval", &self.modpack_poll_interval)
            .field("update_locks", &"<lock>")
            .field("update_phase_buses", &"<map>")
            .field("update_errors", &"<map>")
            .field("update_terminals", &"<map>")
            .field("snapshot_pvc_lock", &"<lock>")
            .field("mc_alpine_image", &self.mc_alpine_image)
            .field("mc_timezone", &self.mc_timezone)
            .field("mc_itzg_image", &self.mc_itzg_image)
            .field("mc_busybox_image", &self.mc_busybox_image)
            .finish()
    }
}

// Lets `axum_extra::extract::cookie::PrivateCookieJar` find its key from `AppState`.
impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Self {
        state.cookie_key.clone()
    }
}
