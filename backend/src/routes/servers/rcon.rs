//! `POST /api/servers/{id}/rcon` — send one RCON command and return its
//! response.
//!
//! Per-request: open a TCP connection to the in-cluster headless Service
//! at `mc-<id>-0.mc-<id>-headless.<ns>.svc:25575`, authenticate with the
//! per-server password (read from the `mc-<id>-rcon` Secret), send the
//! command, read the response, close. No connection pool — RCON traffic
//! from the panel is rare and the open/close overhead is dwarfed by the
//! Minecraft server's command handling latency.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use chrono::Utc;
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::{Pod, Secret};
use kube::Api;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::AppState;
use crate::error::AppError;
use crate::k8s::ServerStatus;
use crate::k8s_status::{RCON_PORT, derive_status};
use crate::routes::servers::create::insert_audit;
use crate::routes::servers::get::fetch_server_row;

/// Maximum length of `cmd`, in bytes. Pre-validated before any k8s I/O
/// to short-circuit obviously-bogus requests.
const MAX_CMD_LEN: usize = 1024;

/// Single end-to-end timeout: DNS, TCP connect, RCON auth, send, receive.
/// 5 s is generous for an in-cluster connection — anything longer means
/// the server is wedged and the user should investigate, not retry.
const RCON_TIMEOUT: Duration = Duration::from_secs(5);

/// Request body for `POST /api/servers/{id}/rcon`.
#[derive(Debug, Deserialize)]
pub struct RconRequest {
    /// The command to send, as the user would type it in-game (without the
    /// leading `/`). Whitespace is trimmed.
    pub cmd: String,
}

/// Response body for `POST /api/servers/{id}/rcon`.
#[derive(Debug, Serialize)]
pub struct RconResponse {
    /// Server-side response. May be empty (e.g. `say` produces no output).
    pub output: String,
}

/// Validates `cmd` and returns a trimmed view.
///
/// # Errors
///
/// - [`AppError::BadRequest`] with code `cmd_empty` if `cmd` is empty
///   after trimming.
/// - [`AppError::BadRequest`] with code `cmd_too_long` if the trimmed
///   command exceeds [`MAX_CMD_LEN`] bytes.
fn validate_cmd(cmd: &str) -> Result<&str, AppError> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return Err(AppError::BadRequest {
            code: "cmd_empty",
            message: "cmd must not be empty".to_owned(),
        });
    }
    if trimmed.len() > MAX_CMD_LEN {
        return Err(AppError::BadRequest {
            code: "cmd_too_long",
            message: format!("cmd must be <= {MAX_CMD_LEN} bytes"),
        });
    }
    Ok(trimmed)
}

/// Handler for `POST /api/servers/{id}/rcon`.
///
/// # Errors
///
/// - 400 `cmd_empty` / `cmd_too_long` on a malformed body.
/// - 404 if the server is not in the panel database.
/// - 409 `server_not_running` if the `StatefulSet` is scaled down or the
///   pod is not in `Running` state.
/// - 500 on any other failure (k8s, DB, auth, I/O, timeout). The RCON
///   password is never echoed in the error message.
pub async fn handle(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(request): Json<RconRequest>,
) -> Result<Json<RconResponse>, AppError> {
    let cmd = validate_cmd(&request.cmd)?.to_owned();

    let _row = fetch_server_row(&state.pool, &id).await?;

    let resource_name = format!("mc-{id}");
    let pod_name = format!("{resource_name}-0");
    let secret_name = format!("{resource_name}-rcon");
    let headless_dns = format!(
        "{pod_name}.{resource_name}-headless.{ns}.svc:{port}",
        ns = state.mc_namespace,
        port = RCON_PORT,
    );

    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let stsets: Api<StatefulSet> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let secrets: Api<Secret> = Api::namespaced(state.kube.clone(), &state.mc_namespace);

    // Status gate: only Running servers accept RCON. Mirrors the
    // derive_status truth table used by the list/detail handlers so
    // status semantics stay in one place.
    let (replicas, ready) = stsets.get_opt(&resource_name).await?.map_or((0, 0), |s| {
        let r = s.spec.as_ref().and_then(|sp| sp.replicas).unwrap_or(0);
        let ready = s
            .status
            .as_ref()
            .and_then(|st| st.ready_replicas)
            .unwrap_or(0);
        (r, ready)
    });
    let pod = pods.get_opt(&pod_name).await?;
    if derive_status(replicas, ready, pod.as_ref()) != ServerStatus::Running {
        return Err(AppError::Conflict {
            code: "server_not_running",
            message: "server is not running".to_owned(),
        });
    }

    // Read the RCON password. Surface a clear internal error if the
    // Secret is missing or malformed — this should not happen unless
    // someone deleted it out-of-band.
    let secret = secrets.get(&secret_name).await?;
    let password = secret
        .data
        .as_ref()
        .and_then(|d| d.get("password"))
        .map(|bs| bs.0.clone())
        .ok_or_else(|| {
            AppError::Internal(anyhow::anyhow!(
                "rcon secret {secret_name} missing 'password' key"
            ))
        })?;
    let password = String::from_utf8(password).map_err(|_| {
        AppError::Internal(anyhow::anyhow!(
            "rcon secret {secret_name} 'password' is not UTF-8"
        ))
    })?;

    // Connect, send, receive, close — all under one timeout.
    let output = timeout(RCON_TIMEOUT, async {
        let mut conn = <rcon::Connection<TcpStream>>::connect(&headless_dns, &password).await?;
        conn.cmd(&cmd).await
    })
    .await
    .map_err(|_| AppError::Internal(anyhow::anyhow!("rcon timed out after {RCON_TIMEOUT:?}")))?
    .map_err(map_rcon_error)?;

    let now = Utc::now().timestamp();
    insert_audit(&state.pool, &id, "rcon", Some(json!({ "cmd": cmd })), now).await?;

    Ok(Json(RconResponse { output }))
}

/// Translates a [`rcon::Error`] into an [`AppError`] without leaking the
/// password — the handler only ever passes it via the connect call, so
/// the error messages here are safe to surface.
fn map_rcon_error(err: rcon::Error) -> AppError {
    match err {
        rcon::Error::Auth => AppError::Internal(anyhow::anyhow!("rcon auth failed")),
        rcon::Error::CommandTooLong => AppError::BadRequest {
            code: "cmd_too_long",
            message: "command exceeded server-side max payload (1413 B)".to_owned(),
        },
        rcon::Error::Io(io) => AppError::Internal(anyhow::anyhow!("rcon io: {io}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_cmd_rejects_empty() {
        match validate_cmd("") {
            Err(AppError::BadRequest { code, .. }) => assert_eq!(code, "cmd_empty"),
            other => panic!("expected BadRequest cmd_empty, got: {other:?}"),
        }
    }

    #[test]
    fn validate_cmd_rejects_whitespace_only() {
        match validate_cmd("   \t\n  ") {
            Err(AppError::BadRequest { code, .. }) => assert_eq!(code, "cmd_empty"),
            other => panic!("expected BadRequest cmd_empty, got: {other:?}"),
        }
    }

    #[test]
    fn validate_cmd_rejects_overlong() {
        let cmd = "x".repeat(MAX_CMD_LEN + 1);
        match validate_cmd(&cmd) {
            Err(AppError::BadRequest { code, .. }) => assert_eq!(code, "cmd_too_long"),
            other => panic!("expected BadRequest cmd_too_long, got: {other:?}"),
        }
    }

    #[test]
    fn validate_cmd_accepts_max_length() {
        let cmd = "x".repeat(MAX_CMD_LEN);
        assert_eq!(validate_cmd(&cmd).unwrap(), cmd);
    }

    #[test]
    fn validate_cmd_trims_outer_whitespace() {
        assert_eq!(validate_cmd("  say hi  ").unwrap(), "say hi");
    }

    #[test]
    fn map_rcon_error_auth_does_not_leak_details() {
        let mapped = map_rcon_error(rcon::Error::Auth);
        let rendered = format!("{mapped}");
        assert!(rendered.contains("auth failed"));
        // Defensive: ensure no internal sleeve like "password" leaked.
        assert!(!rendered.to_lowercase().contains("password"));
    }

    #[test]
    fn map_rcon_error_command_too_long_is_400() {
        match map_rcon_error(rcon::Error::CommandTooLong) {
            AppError::BadRequest { code, .. } => assert_eq!(code, "cmd_too_long"),
            other => panic!("expected BadRequest cmd_too_long, got: {other:?}"),
        }
    }
}
