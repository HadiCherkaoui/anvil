//! `GET /api/servers` — list managed Minecraft `StatefulSet`s.
//!
//! Filters by the `app.anvil.io/managed-by=anvil` label so unrelated
//! `StatefulSet`s in the same namespace are ignored. M1 returns an empty
//! list in normal cluster state — the create handler that populates the
//! namespace lives in M2.

use axum::Json;
use axum::extract::State;
use k8s_openapi::api::apps::v1::StatefulSet;
use kube::Api;
use kube::api::ListParams;
use serde::Serialize;

use crate::AppState;
use crate::error::AppError;
use crate::k8s::{MANAGED_BY_LABEL, MANAGED_BY_VALUE, ServerSummary, to_server_summary};

/// Body of `GET /api/servers` (spec §2.2).
#[derive(Debug, Serialize)]
pub struct ServersBody {
    pub servers: Vec<ServerSummary>,
}

/// Handler for `GET /api/servers`.
///
/// # Errors
///
/// Returns `AppError::KubeUnavailable` if the cluster API call fails.
pub async fn list(State(state): State<AppState>) -> Result<Json<ServersBody>, AppError> {
    let api: Api<StatefulSet> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let params = ListParams::default().labels(&format!("{MANAGED_BY_LABEL}={MANAGED_BY_VALUE}"));

    let list = api.list(&params).await?;
    // `metadata.name` is always populated by the k8s API for listed objects,
    // but the field is `Option<String>` in the OpenAPI schema. Skip any
    // pathological item rather than emit a row the UI can't act on.
    let servers = list
        .items
        .iter()
        .filter(|sts| sts.metadata.name.is_some())
        .map(to_server_summary)
        .collect();

    Ok(Json(ServersBody { servers }))
}
