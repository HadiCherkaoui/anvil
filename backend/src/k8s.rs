//! Kubernetes client glue.
//!
//! Wraps `kube::Client` construction and the pure mapping from `StatefulSet`
//! to `ServerSummary`. Live status (per spec §2.4) is derived from the spec's
//! `replicas` and the status's `readyReplicas`; M1 does not inspect Pods, so
//! the `stopping` and `error` states will land in M2 alongside the lifecycle
//! handlers that can observe a Pod's phase.

use anyhow::{Context as _, Result};
use k8s_openapi::api::apps::v1::StatefulSet;
use kube::Client;
use serde::Serialize;

/// Label that identifies `StatefulSet`s managed by anvil.
pub const MANAGED_BY_LABEL: &str = "app.anvil.io/managed-by";

/// Value paired with [`MANAGED_BY_LABEL`].
pub const MANAGED_BY_VALUE: &str = "anvil";

/// Annotation key for the snapshotted Minecraft version. Read-only in M1.
pub const ANNOTATION_MC_VERSION: &str = "app.anvil.io/mc-version";

/// Annotation key for the memory budget (MiB). Read-only in M1.
pub const ANNOTATION_MEMORY_MB: &str = "app.anvil.io/memory-mb";

/// Live runtime status of a managed Minecraft server (spec §2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerStatus {
    Running,
    Stopped,
    Starting,
    Stopping,
    Error,
}

/// Connection endpoint for a managed Minecraft server.
///
/// `None` until M2 wires the `Service` LB-IP lookup.
#[derive(Debug, Clone, Serialize)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

/// One entry in the response of `GET /api/servers` (spec §2.2).
#[derive(Debug, Clone, Serialize)]
pub struct ServerSummary {
    pub name: String,
    pub status: ServerStatus,
    pub mc_version: String,
    pub memory_mb: u32,
    pub endpoint: Option<Endpoint>,
    pub created_at: String,
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

/// Derives live status from `StatefulSet`'s spec.replicas and
/// status.readyReplicas (spec §2.4 — Pod-phase refinement is M2).
#[must_use]
pub fn derive_status(replicas: i32, ready_replicas: i32) -> ServerStatus {
    match (replicas, ready_replicas) {
        (r, _) if r <= 0 => ServerStatus::Stopped,
        (_, ready) if ready >= 1 => ServerStatus::Running,
        _ => ServerStatus::Starting,
    }
}

/// Maps a `StatefulSet` retrieved with the managed-by label into the
/// summary shape the API returns.
///
/// Missing labels and annotations are tolerated — they default to the empty
/// string and zero, respectively. M1 returns an empty list in normal cluster
/// state, so this function is exercised mostly by unit tests.
#[must_use]
pub fn to_server_summary(sts: &StatefulSet) -> ServerSummary {
    let name = sts.metadata.name.clone().unwrap_or_default();

    let replicas = sts.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0);
    let ready = sts
        .status
        .as_ref()
        .and_then(|s| s.ready_replicas)
        .unwrap_or(0);
    let status = derive_status(replicas, ready);

    let annotations = sts.metadata.annotations.as_ref();
    let mc_version = annotations
        .and_then(|a| a.get(ANNOTATION_MC_VERSION))
        .cloned()
        .unwrap_or_default();
    let memory_mb = annotations
        .and_then(|a| a.get(ANNOTATION_MEMORY_MB))
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);

    // `k8s_openapi::Time` wraps `jiff::Timestamp`; jiff's Display impl is
    // already RFC 3339, so `to_string()` is the right call here.
    let created_at = sts
        .metadata
        .creation_timestamp
        .as_ref()
        .map(|t| t.0.to_string())
        .unwrap_or_default();

    ServerSummary {
        name,
        status,
        mc_version,
        memory_mb,
        endpoint: None,
        created_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::apps::v1::{StatefulSetSpec, StatefulSetStatus};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::BTreeMap;

    #[test]
    fn derive_status_replicas_zero_is_stopped() {
        assert_eq!(derive_status(0, 0), ServerStatus::Stopped);
    }

    #[test]
    fn derive_status_replicas_one_ready_one_is_running() {
        assert_eq!(derive_status(1, 1), ServerStatus::Running);
    }

    #[test]
    fn derive_status_replicas_one_ready_zero_is_starting() {
        assert_eq!(derive_status(1, 0), ServerStatus::Starting);
    }

    #[test]
    fn derive_status_negative_replicas_treated_as_stopped() {
        // Defensive — k8s never sends this, but guard against logic bugs.
        assert_eq!(derive_status(-1, 0), ServerStatus::Stopped);
    }

    #[test]
    fn to_server_summary_extracts_name_and_status() {
        let sts = StatefulSet {
            metadata: ObjectMeta {
                name: Some("smp".to_owned()),
                ..ObjectMeta::default()
            },
            spec: Some(StatefulSetSpec {
                replicas: Some(1),
                ..StatefulSetSpec::default()
            }),
            status: Some(StatefulSetStatus {
                ready_replicas: Some(1),
                ..StatefulSetStatus::default()
            }),
        };
        let summary = to_server_summary(&sts);
        assert_eq!(summary.name, "smp");
        assert_eq!(summary.status, ServerStatus::Running);
        assert_eq!(summary.memory_mb, 0);
        assert_eq!(summary.mc_version, "");
    }

    #[test]
    fn to_server_summary_reads_annotations() {
        let mut annotations = BTreeMap::new();
        annotations.insert(ANNOTATION_MC_VERSION.to_owned(), "1.21.4".to_owned());
        annotations.insert(ANNOTATION_MEMORY_MB.to_owned(), "4096".to_owned());

        let sts = StatefulSet {
            metadata: ObjectMeta {
                name: Some("smp".to_owned()),
                annotations: Some(annotations),
                ..ObjectMeta::default()
            },
            spec: Some(StatefulSetSpec {
                replicas: Some(0),
                ..StatefulSetSpec::default()
            }),
            status: None,
        };
        let summary = to_server_summary(&sts);
        assert_eq!(summary.mc_version, "1.21.4");
        assert_eq!(summary.memory_mb, 4096);
        assert_eq!(summary.status, ServerStatus::Stopped);
    }

    #[test]
    fn to_server_summary_missing_metadata_yields_defaults() {
        let sts = StatefulSet::default();
        let summary = to_server_summary(&sts);
        assert!(summary.name.is_empty());
        assert_eq!(summary.status, ServerStatus::Stopped);
        assert_eq!(summary.memory_mb, 0);
        assert!(summary.endpoint.is_none());
    }
}
