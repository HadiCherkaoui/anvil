//! `GET /api/servers/{id}/metrics` — live CPU + memory from `metrics-server`.
//!
//! Reads `metrics.k8s.io/v1beta1` directly via [`kube::Client::request`]. When
//! the API isn't installed (cluster has no metrics-server) or the pod hasn't
//! been scraped yet, both fields come back `null` rather than 404 — the
//! frontend can render a hyphen and keep going.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::Request;
use serde::Serialize;

use crate::AppState;
use crate::error::AppError;
use crate::routes::servers::get::fetch_server_row;

/// Body of `GET /api/servers/{id}/metrics`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ServerMetrics {
    /// Sum of container CPU usage, expressed in millicores. `None` when the
    /// metrics API is unreachable or has no data for this pod yet.
    pub cpu_millicores: Option<u64>,
    /// Sum of container memory usage, expressed in MiB. `None` when the
    /// metrics API is unreachable or has no data for this pod yet.
    pub memory_mi: Option<u64>,
}

/// Handler for `GET /api/servers/{id}/metrics`.
///
/// # Errors
///
/// Returns `AppError::NotFound` for unknown servers, `AppError::Internal`
/// if the metrics request fails for a reason other than the API not being
/// installed or the pod not being scraped yet.
#[utoipa::path(
    get,
    path = "/api/servers/{id}/metrics",
    params(("id" = String, Path, description = "server UUID")),
    responses(
        (status = 200, description = "Live CPU and memory usage", body = ServerMetrics),
        (status = 404, description = "Server not found"),
        (status = 500, description = "Metrics request failed")
    ),
    tag = "metrics"
)]
pub async fn handle(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<ServerMetrics>, AppError> {
    fetch_server_row(&state.pool, &id).await?;
    let pod_name = format!("mc-{id}-0");
    let path = format!(
        "/apis/metrics.k8s.io/v1beta1/namespaces/{}/pods/{}",
        state.mc_namespace, pod_name
    );
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .body(Vec::new())
        .map_err(|e| AppError::Internal(anyhow::anyhow!(e)))?;

    let raw: serde_json::Value = match state.kube.request(req).await {
        Ok(v) => v,
        // Metrics API not installed or no scrape yet: return both fields null.
        Err(kube::Error::Api(err)) if err.code == 404 || err.code == 503 => {
            return Ok(Json(ServerMetrics {
                cpu_millicores: None,
                memory_mi: None,
            }));
        }
        Err(e) => return Err(AppError::Internal(anyhow::anyhow!(e))),
    };

    let metrics = aggregate_pod_metrics(&raw);
    Ok(Json(metrics))
}

/// Sums per-container `usage` blocks into a single [`ServerMetrics`].
///
/// The metrics-server response shape is:
/// ```json
/// { "containers": [{ "usage": { "cpu": "1500m", "memory": "8192Mi" } }, …] }
/// ```
fn aggregate_pod_metrics(raw: &serde_json::Value) -> ServerMetrics {
    let Some(containers) = raw.get("containers").and_then(|v| v.as_array()) else {
        return ServerMetrics {
            cpu_millicores: None,
            memory_mi: None,
        };
    };

    let mut cpu_n: u128 = 0;
    let mut mem_b: u128 = 0;
    for c in containers {
        let Some(usage) = c.get("usage") else {
            continue;
        };
        if let Some(s) = usage.get("cpu").and_then(|v| v.as_str()) {
            cpu_n = cpu_n.saturating_add(parse_cpu_nanocores(s));
        }
        if let Some(s) = usage.get("memory").and_then(|v| v.as_str()) {
            mem_b = mem_b.saturating_add(parse_memory_bytes(s));
        }
    }

    let cpu_millicores = u64::try_from(cpu_n / 1_000_000).ok();
    let memory_mi = u64::try_from(mem_b / (1024 * 1024)).ok();
    ServerMetrics {
        cpu_millicores,
        memory_mi,
    }
}

/// Parses a Kubernetes CPU quantity into nanocores.
///
/// Common shapes: `"123n"`, `"123u"`, `"123m"`, `"1.5"` (whole cores).
fn parse_cpu_nanocores(s: &str) -> u128 {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return 0;
    }
    if let Some(stripped) = trimmed.strip_suffix('n') {
        return stripped.parse::<u128>().unwrap_or(0);
    }
    if let Some(stripped) = trimmed.strip_suffix('u') {
        return stripped.parse::<u128>().unwrap_or(0).saturating_mul(1_000);
    }
    if let Some(stripped) = trimmed.strip_suffix('m') {
        return stripped
            .parse::<u128>()
            .unwrap_or(0)
            .saturating_mul(1_000_000);
    }
    if let Ok(cores) = trimmed.parse::<f64>()
        && cores >= 0.0
        && cores.is_finite()
    {
        // Convert via u64 to keep clippy's truncation/sign-loss lints quiet
        // while still covering the realistic `0 ≤ cores ≤ 1024` range.
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "guarded by finite + non-negative checks above; range fits u64"
        )]
        let nanos = (cores * 1_000_000_000.0) as u64;
        return u128::from(nanos);
    }
    0
}

/// Parses a Kubernetes memory quantity into bytes.
///
/// Handles binary (`Ki`/`Mi`/`Gi`/`Ti`) and decimal (`K`/`M`/`G`/`T`) suffixes
/// plus a bare-number fallback (already bytes).
fn parse_memory_bytes(s: &str) -> u128 {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return 0;
    }
    let (val, mult): (&str, u128) = if let Some(v) = trimmed.strip_suffix("Ki") {
        (v, 1024)
    } else if let Some(v) = trimmed.strip_suffix("Mi") {
        (v, 1024 * 1024)
    } else if let Some(v) = trimmed.strip_suffix("Gi") {
        (v, 1024 * 1024 * 1024)
    } else if let Some(v) = trimmed.strip_suffix("Ti") {
        (v, 1024_u128 * 1024 * 1024 * 1024)
    } else if let Some(v) = trimmed.strip_suffix('K') {
        (v, 1_000)
    } else if let Some(v) = trimmed.strip_suffix('M') {
        (v, 1_000_000)
    } else if let Some(v) = trimmed.strip_suffix('G') {
        (v, 1_000_000_000)
    } else if let Some(v) = trimmed.strip_suffix('T') {
        (v, 1_000_000_000_000)
    } else {
        (trimmed, 1)
    };
    val.parse::<u128>().unwrap_or(0).saturating_mul(mult)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cpu_parser_handles_nano_micro_milli_and_decimal() {
        assert_eq!(parse_cpu_nanocores("12345n"), 12_345);
        assert_eq!(parse_cpu_nanocores("250u"), 250_000);
        assert_eq!(parse_cpu_nanocores("1500m"), 1_500_000_000);
        assert_eq!(parse_cpu_nanocores("1.5"), 1_500_000_000);
        assert_eq!(parse_cpu_nanocores(""), 0);
    }

    #[test]
    fn memory_parser_handles_binary_decimal_and_bare() {
        assert_eq!(parse_memory_bytes("1024Ki"), 1_048_576);
        assert_eq!(parse_memory_bytes("8Mi"), 8 * 1024 * 1024);
        assert_eq!(parse_memory_bytes("4Gi"), 4_u128 * 1024 * 1024 * 1024);
        assert_eq!(parse_memory_bytes("1000M"), 1_000_000_000);
        assert_eq!(parse_memory_bytes("1000000000"), 1_000_000_000);
        assert_eq!(parse_memory_bytes(""), 0);
    }

    #[test]
    fn aggregate_sums_containers_and_converts_units() {
        let raw = json!({
            "containers": [
                { "usage": { "cpu": "500m", "memory": "1024Mi" } },
                { "usage": { "cpu": "250m", "memory": "512Mi" } }
            ]
        });
        let m = aggregate_pod_metrics(&raw);
        assert_eq!(m.cpu_millicores, Some(750));
        assert_eq!(m.memory_mi, Some(1_536));
    }

    #[test]
    fn aggregate_returns_none_when_containers_missing() {
        let raw = json!({});
        let m = aggregate_pod_metrics(&raw);
        assert_eq!(m.cpu_millicores, None);
        assert_eq!(m.memory_mi, None);
    }
}
