//! Application error type and HTTP mapping (spec §2.5).
//!
//! Every API error returns the same shape:
//! `{ "error": "<message>", "code": "<kebab-case>" }`.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use thiserror::Error;
use tracing::Level;
use tracing::event;

/// All error variants the API handlers return in M1.
///
/// `db_unavailable` is included for symmetry — no M1 handler hits the DB
/// path, but routing the error here means `?` in M2 just works.
#[derive(Debug, Error)]
pub enum AppError {
    /// Failure when calling the Kubernetes API.
    #[error("kubernetes API unavailable: {0}")]
    KubeUnavailable(#[from] kube::Error),

    /// Failure when calling `SQLite` (M2+).
    #[error("database unavailable: {0}")]
    DbUnavailable(#[from] sqlx::Error),

    /// Anything else worth a 500 — wrap with `anyhow` for free `?` from
    /// arbitrary error types.
    #[error("internal error: {0:#}")]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    /// Returns the wire-protocol code (spec §2.5) for this error.
    fn code(&self) -> &'static str {
        match self {
            Self::KubeUnavailable(_) => "k8s_unavailable",
            Self::DbUnavailable(_) => "db_unavailable",
            Self::Internal(_) => "internal",
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // Always 500 in M1 — there's no validation surface yet, so every
        // variant is an upstream/runtime failure.
        let status = StatusCode::INTERNAL_SERVER_ERROR;
        let code = self.code();
        let message = self.to_string();

        event!(
            name: "anvil.request.error",
            Level::ERROR,
            error.code = code,
            error.message = %message,
            "request failed: {{error.code}}",
        );

        (status, Json(json!({ "error": message, "code": code }))).into_response()
    }
}
