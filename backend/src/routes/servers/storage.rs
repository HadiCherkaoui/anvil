//! `PATCH /api/servers/:id/storage` — grow-only PVC resize.
//!
//! ZFS-CSI supports online expansion when the PVC is mounted, otherwise
//! expands on next mount. The handler does not gate on server status; the
//! frontend sees the new size on the next detail fetch (PVC patches are
//! applied asynchronously by the CSI driver).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use k8s_openapi::api::core::v1::PersistentVolumeClaim;
use kube::Api;
use kube::api::{Patch, PatchParams};
use serde::{Deserialize, Serialize};
use serde_json::json;

use chrono::Utc;

use crate::AppState;
use crate::error::AppError;
use crate::routes::cluster::current_caps;
use crate::routes::servers::create::insert_audit;
use crate::validation::validate_storage_size_gi;

/// Request body for `PATCH /api/servers/:id/storage`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ResizeRequest {
    /// Target size in gibibytes. Must be strictly greater than the
    /// current size — shrinking is not supported.
    pub size_gi: u32,
}

/// Response body for `PATCH /api/servers/:id/storage`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ResizeResponse {
    /// The new requested size, echoed back. Filesystem expansion is async;
    /// the next detail fetch reflects the live PVC value.
    pub size_gi: u32,
}

/// Handler.
///
/// # Errors
///
/// - 400 `shrink_unsupported` — `size_gi <= current_size_gi`.
/// - 404 generic — server row missing.
/// - 409 `expansion_unsupported` — server's SC is not in
///   [`crate::routes::cluster::ClusterCapabilities::expandable_storage_classes`].
/// - 500 `internal` — PVC patch or DB update failed.
#[utoipa::path(
    patch,
    path = "/api/servers/{id}/storage",
    params(("id" = String, Path, description = "server UUID")),
    request_body = ResizeRequest,
    responses(
        (status = 200, description = "Storage resized", body = ResizeResponse),
        (status = 400, description = "Shrink not supported"),
        (status = 404, description = "Server not found"),
        (status = 409, description = "Storage class does not support expansion")
    ),
    tag = "servers"
)]
pub async fn handle(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(req): Json<ResizeRequest>,
) -> Result<(StatusCode, Json<ResizeResponse>), AppError> {
    validate_storage_size_gi(i64::from(req.size_gi))?;

    let row: Option<(i64, Option<String>)> =
        sqlx::query_as("SELECT storage_size_gi, storage_class FROM servers WHERE id = ?")
            .bind(&id)
            .fetch_optional(&state.pool)
            .await?;
    let (current_size_gi, sc) = row.ok_or(AppError::NotFound)?;
    let current = u32::try_from(current_size_gi).unwrap_or(0);
    let sc = sc.unwrap_or_else(|| state.mc_storage_class.clone());

    if req.size_gi <= current {
        return Err(AppError::BadRequest {
            code: "shrink_unsupported",
            message: "storage size can only grow".to_owned(),
        });
    }

    let caps = current_caps(&state).await?;
    if !caps.expandable_storage_classes.iter().any(|n| n == &sc) {
        return Err(AppError::Conflict {
            code: "expansion_unsupported",
            message: format!("storage class {sc} does not support volume expansion"),
        });
    }

    let pvc_api: Api<PersistentVolumeClaim> =
        Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let pvc_name = format!("data-mc-{id}-0");
    let patch = json!({
        "spec": { "resources": { "requests": { "storage": format!("{}Gi", req.size_gi) } } }
    });
    pvc_api
        .patch(
            &pvc_name,
            &PatchParams::default(),
            &Patch::Strategic(&patch),
        )
        .await
        .map_err(|e| AppError::Internal(anyhow::anyhow!("PVC patch failed: {e}")))?;

    sqlx::query("UPDATE servers SET storage_size_gi = ? WHERE id = ?")
        .bind(i64::from(req.size_gi))
        .bind(&id)
        .execute(&state.pool)
        .await?;

    insert_audit(
        &state.pool,
        &id,
        "storage_resized",
        Some(json!({ "from_gi": current, "to_gi": req.size_gi })),
        Utc::now().timestamp(),
    )
    .await?;

    Ok((
        StatusCode::OK,
        Json(ResizeResponse {
            size_gi: req.size_gi,
        }),
    ))
}
