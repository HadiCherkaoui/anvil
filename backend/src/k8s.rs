//! Kubernetes client glue and label/annotation constants.
//!
//! Wraps `kube::Client` construction and defines the shape types
//! (`ServerStatus`, `Endpoint`, `ServerSummary`) shared across handlers.
//! Status and endpoint derivation live in [`crate::k8s_status`].

use anyhow::{Context as _, Result};
use kube::Client;
use serde::Serialize;

/// Label that identifies resources managed by anvil.
pub const MANAGED_BY_LABEL: &str = "app.anvil.io/managed-by";

/// Value paired with [`MANAGED_BY_LABEL`].
pub const MANAGED_BY_VALUE: &str = "anvil";

/// Label whose value is the server's UUID `id`. Used to look up the
/// matching `StatefulSet`/`Service`/`Pod` for a given server.
pub const LABEL_SERVER: &str = "app.anvil.io/server";

/// Annotation key for the snapshotted Minecraft version.
pub const ANNOTATION_MC_VERSION: &str = "app.anvil.io/mc-version";

/// Annotation key for the memory budget (MiB).
///
/// Renamed from `memory-mb` in M1 — the value was always MiB despite the
/// name. M1 had no production data so the rename is safe.
pub const ANNOTATION_MEMORY_MI: &str = "app.anvil.io/memory-mi";

/// Annotation key for the user-facing server name (the `servers.name`
/// column in `SQLite`). Stable across renames is not required in M2.
pub const ANNOTATION_SERVER_NAME: &str = "app.anvil.io/server-name";

/// Annotation key for the unix-second creation timestamp.
pub const ANNOTATION_CREATED_AT: &str = "app.anvil.io/created-at";

/// Live runtime status of a managed Minecraft server (spec §2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServerStatus {
    /// Pod is running and ready.
    Running,
    /// `replicas: 0` and no pod present.
    Stopped,
    /// `replicas: 1` but pod not yet ready.
    Starting,
    /// `replicas: 0` but pod still terminating.
    Stopping,
    /// `replicas: 1`, pod present, container in `CrashLoopBackOff` /
    /// `OOMKilled` / similar terminal failure.
    Error,
}

/// Connection endpoint for a managed Minecraft server.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Endpoint {
    /// Hostname or IP the client connects to.
    pub host: String,
    /// TCP port — `25565` for LB/ClusterIP, the assigned `NodePort` otherwise.
    pub port: u16,
}

/// One entry in the response of `GET /api/servers` (spec §2.2).
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ServerSummary {
    /// Server UUID (URL identifier).
    pub id: String,
    /// User-facing server name.
    pub name: String,
    /// Live runtime status.
    pub status: ServerStatus,
    /// Snapshotted Minecraft version.
    pub mc_version: String,
    /// Configured memory budget (MiB).
    pub memory_mi: i64,
    /// Service exposure mode (`loadbalancer` | `nodeport` | `clusterip`).
    pub exposure_mode: String,
    /// Resolved connection endpoint, or `None` if the address has not been
    /// assigned yet (e.g. LB IP pending).
    pub endpoint: Option<Endpoint>,
    /// Unix-second creation timestamp.
    pub created_at: i64,
    /// Provider discriminator (`vanilla` | `curseforge`).
    pub source_kind: String,
    /// `true` when a newer modpack version is cached for this server.
    pub update_available: bool,
    /// Display name of the latest cached upstream version, when any.
    pub latest_version_name: Option<String>,
    /// `true` while the orchestrator is mid-update for this server.
    pub update_in_progress: bool,
}

/// Builds an in-cluster client when one is available, otherwise falls back
/// to the local kubeconfig.
///
/// `kube::Client::try_default()` already encodes both paths, so this is just
/// a thin wrapper that adds an error context.
///
/// # Errors
///
/// Returns an error if neither a Service Account token (in-cluster) nor a
/// usable kubeconfig is found.
pub async fn try_default_client() -> Result<Client> {
    Client::try_default()
        .await
        .context("constructing kube::Client (try in-cluster, then ~/.kube/config)")
}
