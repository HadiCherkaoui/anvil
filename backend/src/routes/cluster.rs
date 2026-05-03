//! `GET /api/cluster/capabilities` — what the cluster supports.
//!
//! Hybrid source per ADR 0005: LB / `NodePort` / `ClusterIP` availability
//! comes from the helm-static `loadbalancer_supported` flag (always
//! true for `NodePort` and `ClusterIP`); the `StorageClass` list comes from
//! a runtime k8s API call cached in memory for 5 minutes.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use k8s_openapi::api::core::v1::Node;
use k8s_openapi::api::storage::v1::StorageClass;
use kube::Api;
use kube::api::ListParams;
use serde::Serialize;

use crate::AppState;
use crate::error::AppError;

/// How long to cache the `StorageClass` list before re-querying.
pub const CAPABILITIES_TTL: Duration = Duration::from_secs(5 * 60);

/// Cluster capabilities response shape.
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool gates an independent feature in the frontend; bitmask would be opaque"
)]
#[derive(Debug, Clone, Serialize)]
pub struct ClusterCapabilities {
    /// Whether the create handler will accept `exposure_mode=loadbalancer`.
    pub loadbalancer: bool,
    /// `NodePort` is always available on a working k8s cluster.
    pub nodeport: bool,
    /// `ClusterIP` is always available.
    pub clusterip: bool,
    /// Names of all `StorageClass`es discovered on the cluster.
    pub available_storage_classes: Vec<String>,
    /// Name of the `StorageClass` annotated `is-default-class=true`,
    /// if any.
    pub default_storage_class: Option<String>,
    /// Whether the backend has a `CurseForge` API key configured. The frontend
    /// hides the `CurseForge` option in the New Server modal when this is false.
    pub cf_api_key_present: bool,
    /// Sum of allocatable CPU across schedulable nodes, in fractional cores.
    /// Surfaces as a hint in the create-page Resources section.
    pub available_cpu_cores: f64,
}

/// In-memory cache slot held in `AppState`.
pub type CapabilitiesCache = Arc<Mutex<Option<(ClusterCapabilities, Instant)>>>;

/// Returns a fresh, empty cache slot for use at startup.
#[must_use]
pub fn new_cache() -> CapabilitiesCache {
    Arc::new(Mutex::new(None))
}

/// Handler for `GET /api/cluster/capabilities`.
///
/// # Errors
///
/// Returns [`AppError::KubeUnavailable`] if listing `StorageClass`es
/// fails. Cached responses never error.
pub async fn handle(State(state): State<AppState>) -> Result<Json<ClusterCapabilities>, AppError> {
    if let Some(cached) = read_cache(&state.capabilities_cache) {
        return Ok(Json(cached));
    }

    let storage_classes: Api<StorageClass> = Api::all(state.kube.clone());
    let nodes: Api<Node> = Api::all(state.kube.clone());
    let lp = ListParams::default();
    let (sc_res, node_res) = tokio::join!(storage_classes.list(&lp), nodes.list(&lp));
    let list = sc_res?;
    let mut classes: Vec<String> = Vec::new();
    let mut default: Option<String> = None;
    for sc in list.items {
        let Some(name) = sc.metadata.name.clone() else {
            continue;
        };
        let is_default = sc
            .metadata
            .annotations
            .as_ref()
            .and_then(|a| a.get("storageclass.kubernetes.io/is-default-class"))
            .map(String::as_str)
            == Some("true");
        if is_default {
            default = Some(name.clone());
        }
        classes.push(name);
    }
    classes.sort();

    // Node listing is best-effort: surfacing 0.0 cores is better than a 500
    // on the capabilities endpoint, which the create page polls.
    let available_cpu_cores = match node_res {
        Ok(node_list) => sum_allocatable_cpu_cores(&node_list.items),
        Err(e) => {
            tracing::warn!(error = %e, "capabilities: node list failed; cpu_cores=0.0");
            0.0
        }
    };

    let caps = ClusterCapabilities {
        loadbalancer: state.loadbalancer_supported,
        // The cluster always has NodePort and ClusterIP available — these
        // do not depend on an external provider.
        nodeport: true,
        clusterip: true,
        available_storage_classes: classes,
        default_storage_class: default,
        cf_api_key_present: state.cf_client.is_some(),
        available_cpu_cores,
    };
    write_cache(&state.capabilities_cache, &caps);
    Ok(Json(caps))
}

/// Parses a Kubernetes CPU `Quantity` string into millicores.
///
/// Accepts the two forms k8s emits: `"500m"` (already millicores) and
/// `"4"` / `"3.5"` (whole cores). Returns `None` for anything else.
fn parse_cpu_quantity(q: &str) -> Option<i64> {
    if let Some(n) = q.strip_suffix('m') {
        n.parse::<i64>().ok()
    } else {
        let f = q.parse::<f64>().ok()?;
        if !f.is_finite() {
            return None;
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Some((f * 1000.0) as i64)
    }
}

/// Sums allocatable CPU (cores) across schedulable nodes.
fn sum_allocatable_cpu_cores(nodes: &[Node]) -> f64 {
    let total_millicores: i64 = nodes
        .iter()
        .filter(|n| {
            n.spec
                .as_ref()
                .is_none_or(|s| s.unschedulable != Some(true))
        })
        .filter_map(|n| {
            n.status
                .as_ref()?
                .allocatable
                .as_ref()?
                .get("cpu")
                .and_then(|q| parse_cpu_quantity(&q.0))
        })
        .sum();
    #[allow(clippy::cast_precision_loss)]
    let cores = (total_millicores as f64) / 1000.0;
    cores
}

fn read_cache(cache: &CapabilitiesCache) -> Option<ClusterCapabilities> {
    let guard = cache.lock().expect("capabilities cache poisoned");
    guard.as_ref().and_then(|(caps, since)| {
        if since.elapsed() < CAPABILITIES_TTL {
            Some(caps.clone())
        } else {
            None
        }
    })
}

fn write_cache(cache: &CapabilitiesCache, caps: &ClusterCapabilities) {
    let mut guard = cache.lock().expect("capabilities cache poisoned");
    *guard = Some((caps.clone(), Instant::now()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_millicore_form() {
        assert_eq!(parse_cpu_quantity("500m"), Some(500));
        assert_eq!(parse_cpu_quantity("4000m"), Some(4000));
        assert_eq!(parse_cpu_quantity("16000m"), Some(16_000));
    }

    #[test]
    fn parses_core_form() {
        assert_eq!(parse_cpu_quantity("4"), Some(4000));
        assert_eq!(parse_cpu_quantity("3.5"), Some(3500));
        assert_eq!(parse_cpu_quantity("0.25"), Some(250));
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_cpu_quantity(""), None);
        assert_eq!(parse_cpu_quantity("nan"), None);
        assert_eq!(parse_cpu_quantity("xm"), None);
    }
}
