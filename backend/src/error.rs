//! Application error type and HTTP mapping (spec §2.5).
//!
//! Every API error returns the same shape:
//! `{ "error": "<message>", "code": "<kebab-case>" }`.
//!
//! Status mapping:
//! - `BadRequest`    → 400 (validation failure)
//! - `NotFound`      → 404
//! - `Conflict`      → 409 (state precondition: name taken, must be stopped, …)
//! - `LbUnavailable` → 502 (cluster does not support LoadBalancer)
//! - everything else → 500

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use thiserror::Error;
use tracing::event;
use tracing::Level;

/// All error variants the API handlers return.
#[derive(Debug, Error)]
pub enum AppError {
    /// Failure when calling the Kubernetes API.
    #[error("kubernetes API unavailable: {0}")]
    KubeUnavailable(#[from] kube::Error),

    /// Failure when calling `SQLite`.
    #[error("database unavailable: {0}")]
    DbUnavailable(#[from] sqlx::Error),

    /// Anything else worth a 500 — wrap with `anyhow` for free `?` from
    /// arbitrary error types.
    #[error("internal error: {0:#}")]
    Internal(#[from] anyhow::Error),

    /// Resource does not exist.
    #[error("not found")]
    NotFound,

    /// State precondition failure (name taken, must be stopped, etc.).
    #[error("{message}")]
    Conflict {
        /// Stable kebab-case code surfaced to the client.
        code: &'static str,
        /// Human-readable description of the conflict.
        message: String,
    },

    /// Validation failure on the request body or path.
    #[error("{message}")]
    BadRequest {
        /// Stable kebab-case code surfaced to the client.
        code: &'static str,
        /// Human-readable description of the validation failure.
        message: String,
    },

    /// LoadBalancer requested but the cluster cannot provide one.
    #[error("LoadBalancer is not supported on this cluster")]
    LbUnavailable,
}

impl AppError {
    /// Returns the wire-protocol code (spec §2.5) for this error.
    fn code(&self) -> &'static str {
        match self {
            Self::KubeUnavailable(_) => "k8s_unavailable",
            Self::DbUnavailable(_) => "db_unavailable",
            Self::Internal(_) => "internal",
            Self::NotFound => "not_found",
            Self::Conflict { code, .. } | Self::BadRequest { code, .. } => code,
            Self::LbUnavailable => "lb_unavailable",
        }
    }

    /// Returns the HTTP status code for this error.
    fn status(&self) -> StatusCode {
        match self {
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::BadRequest { .. } => StatusCode::BAD_REQUEST,
            Self::Conflict { .. } => StatusCode::CONFLICT,
            Self::LbUnavailable => StatusCode::BAD_GATEWAY,
            Self::KubeUnavailable(_) | Self::DbUnavailable(_) | Self::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        let code = self.code();
        let message = self.to_string();

        // 4xx are user errors logged at WARN; 5xx (and 502) are operator
        // incidents at ERROR.
        if status.is_client_error() {
            event!(
                name: "anvil.request.error",
                Level::WARN,
                http.status = status.as_u16(),
                error.code = code,
                error.message = %message,
                "request failed: {{error.code}}",
            );
        } else {
            event!(
                name: "anvil.request.error",
                Level::ERROR,
                http.status = status.as_u16(),
                error.code = code,
                error.message = %message,
                "request failed: {{error.code}}",
            );
        }

        (status, Json(json!({ "error": message, "code": code }))).into_response()
    }
}
