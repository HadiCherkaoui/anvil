//! `GET /api/servers/:id/logs` — last 200 lines of pod logs.
//!
//! NOT a streaming endpoint — the spec says snapshot only. The frontend
//! has a refresh button. Streaming via WebSocket lands in M3.

use axum::extract::{Path, State};
use axum::Json;
use k8s_openapi::api::core::v1::Pod;
use kube::api::LogParams;
use kube::Api;
use serde::Serialize;

use crate::error::AppError;
use crate::routes::servers::get::fetch_server_row;
use crate::AppState;

/// Number of trailing log lines returned per request.
const LOG_TAIL_LINES: i64 = 200;

/// Body for `GET /api/servers/:id/logs`.
#[derive(Debug, Serialize)]
pub struct LogsBody {
    pub lines: Vec<String>,
}

/// Handler.
///
/// # Errors
///
/// - 404 if the server does not exist.
/// - 500 on kube failures other than the pod simply not existing yet.
pub async fn handle(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<LogsBody>, AppError> {
    let _row = fetch_server_row(&state.pool, &id).await?;
    let pod_name = format!("mc-{id}-0");
    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), &state.mc_namespace);

    let params = LogParams {
        tail_lines: Some(LOG_TAIL_LINES),
        ..LogParams::default()
    };
    let body = match pods.logs(&pod_name, &params).await {
        Ok(s) => s,
        // Pod absent (server stopped) or container not yet running:
        // both are normal, return an empty array.
        Err(kube::Error::Api(err)) if err.code == 404 || err.code == 400 => String::new(),
        Err(other) => return Err(AppError::KubeUnavailable(other)),
    };

    let lines = body.lines().map(str::to_owned).collect::<Vec<String>>();
    Ok(Json(LogsBody { lines }))
}
