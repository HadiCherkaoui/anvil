//! Server management handlers.
//!
//! Submodules host one HTTP handler each (create / get / start / stop /
//! restart / delete / logs). The list handler stays in this `mod.rs`
//! until Task 13 rewrites it to JOIN SQLite metadata with live k8s
//! status; until then it returns the M1 shape patched with M2 columns.

pub mod create;
pub mod get;
pub mod restart;
pub mod start;
pub mod stop;

use axum::extract::State;
use axum::Json;
use k8s_openapi::api::apps::v1::StatefulSet;
use kube::api::ListParams;
use kube::Api;
use serde::Serialize;

use crate::error::AppError;
use crate::k8s::{
    ServerSummary, ANNOTATION_MC_VERSION, ANNOTATION_MEMORY_MI, LABEL_SERVER, MANAGED_BY_LABEL,
    MANAGED_BY_VALUE,
};
use crate::k8s_status::derive_status;
use crate::AppState;

/// Body of `GET /api/servers` (spec §2.2).
#[derive(Debug, Serialize)]
pub struct ServersBody {
    pub servers: Vec<ServerSummary>,
}

/// Handler for `GET /api/servers`.
///
/// # Errors
///
/// Returns [`AppError::KubeUnavailable`] if the cluster API call fails.
pub async fn list(State(state): State<AppState>) -> Result<Json<ServersBody>, AppError> {
    let api: Api<StatefulSet> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let params = ListParams::default().labels(&format!("{MANAGED_BY_LABEL}={MANAGED_BY_VALUE}"));

    let list = api.list(&params).await?;
    let servers = list
        .items
        .iter()
        .filter(|sts| sts.metadata.name.is_some())
        .map(to_summary)
        .collect();

    Ok(Json(ServersBody { servers }))
}

/// Maps a `StatefulSet` to a [`ServerSummary`]. Endpoint is always `None`
/// here; Task 13 enriches with the live `Service` lookup.
fn to_summary(sts: &StatefulSet) -> ServerSummary {
    let labels = sts.metadata.labels.as_ref();
    let annotations = sts.metadata.annotations.as_ref();
    let id = labels
        .and_then(|l| l.get(LABEL_SERVER))
        .cloned()
        .unwrap_or_default();
    let name = sts.metadata.name.clone().unwrap_or_default();

    let replicas = sts.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0);
    let ready = sts
        .status
        .as_ref()
        .and_then(|s| s.ready_replicas)
        .unwrap_or(0);
    // Without the Pod here, status collapses to Stopped/Running/Starting.
    let status = derive_status(replicas, ready, None);

    let mc_version = annotations
        .and_then(|a| a.get(ANNOTATION_MC_VERSION))
        .cloned()
        .unwrap_or_default();
    let memory_mi = annotations
        .and_then(|a| a.get(ANNOTATION_MEMORY_MI))
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(0);

    let created_at = sts
        .metadata
        .creation_timestamp
        .as_ref()
        .map(|t| t.0.as_second())
        .unwrap_or_default();

    ServerSummary {
        id,
        name,
        status,
        mc_version,
        memory_mi,
        exposure_mode: String::new(),
        endpoint: None,
        created_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::k8s::ServerStatus;
    use k8s_openapi::api::apps::v1::{StatefulSetSpec, StatefulSetStatus};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
    use std::collections::BTreeMap;

    fn sts_with(
        labels: BTreeMap<String, String>,
        annotations: BTreeMap<String, String>,
    ) -> StatefulSet {
        StatefulSet {
            metadata: ObjectMeta {
                name: Some("mc-abcd".to_owned()),
                labels: Some(labels),
                annotations: Some(annotations),
                ..ObjectMeta::default()
            },
            spec: Some(StatefulSetSpec {
                replicas: Some(0),
                ..StatefulSetSpec::default()
            }),
            status: Some(StatefulSetStatus::default()),
        }
    }

    #[test]
    fn to_summary_extracts_id_from_label() {
        let mut labels = BTreeMap::new();
        labels.insert(LABEL_SERVER.to_owned(), "abcd".to_owned());
        let summary = to_summary(&sts_with(labels, BTreeMap::new()));
        assert_eq!(summary.id, "abcd");
        assert_eq!(summary.name, "mc-abcd");
    }

    #[test]
    fn to_summary_reads_memory_mi_annotation() {
        let mut annotations = BTreeMap::new();
        annotations.insert(ANNOTATION_MEMORY_MI.to_owned(), "4096".to_owned());
        annotations.insert(ANNOTATION_MC_VERSION.to_owned(), "1.21.4".to_owned());
        let summary = to_summary(&sts_with(BTreeMap::new(), annotations));
        assert_eq!(summary.memory_mi, 4096);
        assert_eq!(summary.mc_version, "1.21.4");
        assert_eq!(summary.status, ServerStatus::Stopped);
    }
}
