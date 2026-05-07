//! `DELETE /api/servers/:id` — tear down a managed server in the order
//! `StatefulSet` → wait for pod gone → PVC → public Service → headless
//! Service → Secret → `SQLite` row.
//!
//! Wrong order leaks resources; the documented order ensures the PVC
//! attachment is released before its delete attempt and the `SQLite` row
//! is the last to go so a partially-failed teardown is replayable
//! (second `DELETE` retries the k8s steps; third returns 404).

use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::Utc;
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::{PersistentVolumeClaim, Pod, Secret, Service};
use kube::Api;
use kube::api::DeleteParams;

use crate::AppState;
use crate::error::AppError;
use crate::files_helper::tear_down_helper;
use crate::routes::servers::create::insert_audit;
use crate::routes::servers::get::fetch_server_row;

/// Maximum time to wait for the pod to disappear before bailing.
const POD_TERMINATE_TIMEOUT: Duration = Duration::from_mins(2);
/// Poll interval while waiting for the pod.
const POD_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Handler for `DELETE /api/servers/:id`.
///
/// # Errors
///
/// - 404 if the server does not exist.
/// - 409 `must_be_stopped` if the `StatefulSet` still has replicas >= 1.
/// - 500 on k8s or DB failure (other than the tolerated 404s).
pub async fn handle(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<StatusCode, AppError> {
    let _row = fetch_server_row(&state.pool, &id).await?;
    let resource_name = format!("mc-{id}");
    let pod_name = format!("{resource_name}-0");
    let pvc_name = format!("data-{resource_name}-0");
    let headless_name = format!("{resource_name}-headless");
    let secret_name = format!("{resource_name}-rcon");

    let stsets: Api<StatefulSet> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let pvcs: Api<PersistentVolumeClaim> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let services: Api<Service> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let secrets: Api<Secret> = Api::namespaced(state.kube.clone(), &state.mc_namespace);

    // Refuse to delete a running server. Tolerate the StatefulSet
    // already being gone — the SQLite row may have outlived k8s
    // (partial create failure or earlier kubectl delete).
    if let Some(sts) = stsets.get_opt(&resource_name).await? {
        let replicas = sts.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0);
        if replicas >= 1 {
            return Err(AppError::Conflict {
                code: "must_be_stopped",
                message: "stop the server before deleting".to_owned(),
            });
        }
    }

    // 0. Sub-project D: best-effort tear-down of the files-helper Pod
    //    so we don't leak it when the server is deleted while stopped.
    let _ = tear_down_helper(&state, &id).await;

    // 1. StatefulSet
    delete_tolerate_404(
        stsets
            .delete(&resource_name, &DeleteParams::default())
            .await,
    )?;

    // 2. Wait for the pod to be gone.
    let deadline = tokio::time::Instant::now() + POD_TERMINATE_TIMEOUT;
    loop {
        let still = pods.get_opt(&pod_name).await?.is_some();
        if !still {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(AppError::Internal(anyhow::anyhow!(
                "pod {pod_name} did not terminate within {POD_TERMINATE_TIMEOUT:?}"
            )));
        }
        tokio::time::sleep(POD_POLL_INTERVAL).await;
    }

    // 3. PVC
    delete_tolerate_404(pvcs.delete(&pvc_name, &DeleteParams::default()).await)?;

    // 4. Public Service
    delete_tolerate_404(
        services
            .delete(&resource_name, &DeleteParams::default())
            .await,
    )?;

    // 5. Headless Service (M3 — added alongside the public Service for
    //    in-cluster RCON DNS).
    delete_tolerate_404(
        services
            .delete(&headless_name, &DeleteParams::default())
            .await,
    )?;

    // 6. Secret
    delete_tolerate_404(secrets.delete(&secret_name, &DeleteParams::default()).await)?;

    // 7. Spec 5: schedule a fire-and-forget Job that wipes /snap/mc-{id}/manual
    //    on the snapshots PVC so manual backup tarballs don't orphan when the
    //    server goes away. The SQLite rows in `backups` are removed by the FK
    //    CASCADE on the next step. We don't wait — if the Job fails, the
    //    leftover dir is small (just tarballs) and gets cleaned up later.
    let snapshots_pvc = state.snapshots_pvc.as_ref();
    let cleanup_job = crate::modpack::backups::build_dir_cleanup_job(
        &id,
        &state.mc_namespace,
        snapshots_pvc.as_str(),
    );
    if let Err(e) =
        crate::modpack::orchestrator::spawn_job(&state.kube, &state.mc_namespace, &cleanup_job)
            .await
    {
        tracing::warn!(?e, server.id = %id, "backup dir cleanup Job failed to spawn");
    }

    // 8. Audit + SQLite row. Audit goes first: if it fails we return 500
    //    without deleting the row, so a retry replays the k8s steps
    //    (each 404-tolerant) and lands here again. The schema's
    //    `audit_log` row references the server_id by string — the row
    //    is allowed to outlive the server.
    let now = Utc::now().timestamp();
    insert_audit(&state.pool, &id, "deleted", None, now).await?;
    if let Err(e) = insert_audit(&state.pool, &id, "backup_dir_cleanup_scheduled", None, now).await
    {
        tracing::error!(error = ?e, "audit insert failed");
    }
    sqlx::query("DELETE FROM servers WHERE id = ?")
        .bind(&id)
        .execute(&state.pool)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// Maps a kube delete result so a 404 is treated as success.
fn delete_tolerate_404<T>(result: Result<T, kube::Error>) -> Result<(), AppError> {
    match result {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(err)) if err.code == 404 => Ok(()),
        Err(other) => Err(AppError::KubeUnavailable(other)),
    }
}
