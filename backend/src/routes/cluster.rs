// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

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
pub const CAPABILITIES_TTL: Duration = Duration::from_mins(5);

/// Cluster capabilities response shape.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ClusterCapabilities {
    /// Whether the create handler will accept `exposure_mode=loadbalancer`.
    pub loadbalancer: bool,
    /// `NodePort` is always available on a working k8s cluster.
    pub nodeport: bool,
    /// `ClusterIP` is always available.
    pub clusterip: bool,
    /// Names of all `StorageClass`es discovered on the cluster.
    pub available_storage_classes: Vec<String>,
    /// Names of `StorageClass`es with `allowVolumeExpansion: true`. Subset of
    /// [`Self::available_storage_classes`]. The frontend gates the storage
    /// resize control on the server's SC being in this list.
    pub expandable_storage_classes: Vec<String>,
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
#[utoipa::path(
    get,
    path = "/api/cluster/capabilities",
    responses(
        (status = 200, description = "Cluster capabilities", body = ClusterCapabilities),
        (status = 503, description = "Kubernetes API unavailable")
    ),
    tag = "cluster"
)]
pub async fn handle(State(state): State<AppState>) -> Result<Json<ClusterCapabilities>, AppError> {
    Ok(Json(current_caps(&state).await?))
}

/// Returns the current [`ClusterCapabilities`], serving from the in-memory
/// cache when fresh and otherwise listing `StorageClass`es from the cluster.
///
/// Used both by [`handle`] and by handlers that need to know whether the
/// server's SC is in `expandable_storage_classes` (e.g. the PVC resize
/// endpoint).
///
/// # Errors
///
/// Returns [`AppError::KubeUnavailable`] if listing `StorageClass`es fails
/// on a cache miss.
pub async fn current_caps(state: &AppState) -> Result<ClusterCapabilities, AppError> {
    if let Some(cached) = read_cache(&state.capabilities_cache) {
        return Ok(cached);
    }
    let storage_classes: Api<StorageClass> = Api::all(state.kube.clone());
    let lp = ListParams::default();
    let list = storage_classes.list(&lp).await?;
    let caps = compute_caps_from_scs(&list.items, state.loadbalancer_supported);
    write_cache(&state.capabilities_cache, &caps);
    Ok(caps)
}

/// Reduces a `StorageClass` list to the panel's [`ClusterCapabilities`] view.
///
/// Pure function — extracted from [`handle`] so the `expandable_storage_classes`
/// derivation can be unit-tested without driving a kube client.
#[must_use]
pub fn compute_caps_from_scs(scs: &[StorageClass], loadbalancer: bool) -> ClusterCapabilities {
    let mut classes: Vec<String> = Vec::new();
    let mut expandable: Vec<String> = Vec::new();
    let mut default: Option<String> = None;
    for sc in scs {
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
        if sc.allow_volume_expansion.unwrap_or(false) {
            expandable.push(name.clone());
        }
        classes.push(name);
    }
    classes.sort();
    expandable.sort();
    ClusterCapabilities {
        loadbalancer,
        // The cluster always has NodePort and ClusterIP available — these
        // do not depend on an external provider.
        nodeport: true,
        clusterip: true,
        available_storage_classes: classes,
        expandable_storage_classes: expandable,
        default_storage_class: default,
    }
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
    use kube::core::ObjectMeta;
    use std::collections::BTreeMap;

    fn sc(name: &str, allow_expand: bool, default: bool) -> StorageClass {
        let annotations = if default {
            let mut a = BTreeMap::new();
            a.insert(
                "storageclass.kubernetes.io/is-default-class".to_owned(),
                "true".to_owned(),
            );
            Some(a)
        } else {
            None
        };
        StorageClass {
            metadata: ObjectMeta {
                name: Some(name.to_owned()),
                annotations,
                ..ObjectMeta::default()
            },
            allow_volume_expansion: Some(allow_expand),
            ..StorageClass::default()
        }
    }

    #[test]
    fn capabilities_compute_expandable_set() {
        let scs = vec![
            sc("tank", true, true),
            sc("openebs-hostpath", false, false),
            sc("fast", true, false),
        ];
        let caps = compute_caps_from_scs(&scs, true);

        assert_eq!(
            caps.expandable_storage_classes,
            vec!["fast".to_owned(), "tank".to_owned()],
        );
        assert_eq!(
            caps.available_storage_classes,
            vec![
                "fast".to_owned(),
                "openebs-hostpath".to_owned(),
                "tank".to_owned(),
            ],
        );
        assert_eq!(caps.default_storage_class, Some("tank".to_owned()));
        assert!(caps.loadbalancer);
        assert!(caps.nodeport);
        assert!(caps.clusterip);
    }

    #[test]
    fn capabilities_no_expandable_when_all_disallow() {
        let scs = vec![sc("a", false, false), sc("b", false, false)];
        let caps = compute_caps_from_scs(&scs, false);
        assert!(caps.expandable_storage_classes.is_empty());
        assert!(!caps.loadbalancer);
        assert_eq!(caps.default_storage_class, None);
    }

    #[test]
    fn capabilities_skip_storage_class_without_name() {
        let mut nameless = StorageClass::default();
        nameless.metadata.name = None;
        nameless.allow_volume_expansion = Some(true);
        let scs = vec![nameless, sc("named", true, false)];
        let caps = compute_caps_from_scs(&scs, true);
        assert_eq!(caps.expandable_storage_classes, vec!["named".to_owned()],);
    }
}
