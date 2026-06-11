//! `GET /api/health` — liveness probe with build version.

use axum::Json;
use serde::Serialize;

/// Build version reported by the health endpoint.
///
/// Pulled from Cargo at compile time so it always matches the binary.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Health response body.
///
/// Shape: `{ "ok": true, "version": "0.1.0" }`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct HealthBody {
    pub ok: bool,
    pub version: &'static str,
}

/// Handler for `GET /api/health`.
///
/// Always returns 200; the panel is "healthy" iff the process is up — k8s
/// liveness probes care only that the binary is running.
#[utoipa::path(
    get,
    path = "/api/health",
    responses(
        (status = 200, description = "Panel is up", body = HealthBody)
    ),
    tag = "health"
)]
pub async fn get() -> Json<HealthBody> {
    Json(HealthBody {
        ok: true,
        version: VERSION,
    })
}
