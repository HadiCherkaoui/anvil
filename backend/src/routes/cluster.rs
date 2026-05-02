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
use k8s_openapi::api::storage::v1::StorageClass;
use kube::Api;
use kube::api::ListParams;
use serde::Serialize;

use crate::AppState;
use crate::error::AppError;

/// How long to cache the `StorageClass` list before re-querying.
pub const CAPABILITIES_TTL: Duration = Duration::from_secs(5 * 60);

/// Cluster capabilities response shape.
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
    let list = storage_classes.list(&ListParams::default()).await?;
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

    let caps = ClusterCapabilities {
        loadbalancer: state.loadbalancer_supported,
        // The cluster always has NodePort and ClusterIP available — these
        // do not depend on an external provider.
        nodeport: true,
        clusterip: true,
        available_storage_classes: classes,
        default_storage_class: default,
    };
    write_cache(&state.capabilities_cache, &caps);
    Ok(Json(caps))
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
