//! `/api/servers/:id/backups` — manual backup CRUD + restore (Spec 5).
//!
//! Create + restore spawn the matching FSM (announce → stop → tar → start →
//! verify) and return 202 with the backup id. List is a plain `SQLite`
//! query. Delete spawns a small `rm -f` Job and waits synchronously — the
//! tarball is at most a few GB and `rm` over a mounted PVC takes <1s.

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::AppError;
use crate::modpack::backups;
use crate::modpack::guard::UpdateGuard;

/// Body of `POST /api/servers/:id/backups`.
#[derive(Debug, Deserialize)]
pub struct CreateRequest {
    pub name: Option<String>,
}

/// 202 body of `POST /api/servers/:id/backups`.
#[derive(Debug, Serialize)]
pub struct CreateResponse {
    pub status: &'static str,
    pub backup_id: String,
}

/// One row of `GET /api/servers/:id/backups`.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct BackupListItem {
    pub id: String,
    pub name: Option<String>,
    pub created_at: i64,
    pub mc_version: String,
    pub size_bytes: Option<i64>,
}

/// 202 body shared by restore.
#[derive(Debug, Serialize)]
pub struct StartedResponse {
    pub status: &'static str,
}

/// Synchronous timeout for the delete Job. Manual archives are small —
/// `rm` over a mounted PVC takes <1s; the timeout absorbs scheduling
/// + image-pull on a cold node.
const DELETE_JOB_TIMEOUT: Duration = Duration::from_secs(60);

/// Handler: `POST /api/servers/:id/backups`.
///
/// # Errors
///
/// - 404 if the server does not exist.
/// - 400 `invalid_name` when `name` exceeds 64 chars or contains a newline.
/// - 409 `update_in_progress` if another update / apply / backup is running.
pub async fn create(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(req): Json<CreateRequest>,
) -> Result<(StatusCode, Json<CreateResponse>), AppError> {
    if let Some(n) = req.name.as_ref()
        && (n.len() > 64 || n.contains('\n'))
    {
        return Err(AppError::BadRequest {
            code: "invalid_name",
            message: "name too long or contains newline".to_owned(),
        });
    }
    server_must_exist(&state, &id).await?;

    let Some(guard) = UpdateGuard::try_acquire(
        &id,
        state.update_locks.clone(),
        state.update_phase_buses.clone(),
    ) else {
        return Err(AppError::Conflict {
            code: "update_in_progress",
            message: "another update or apply is running".to_owned(),
        });
    };

    let backup_id = backups::new_backup_id();
    let task_state = state.clone();
    let task_id = id.clone();
    let task_backup = backup_id.clone();
    let task_name = req.name.clone();
    tokio::spawn(async move {
        backups::run_backup(task_state, task_id, task_backup, task_name, guard).await;
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(CreateResponse {
            status: "started",
            backup_id,
        }),
    ))
}

/// Handler: `GET /api/servers/:id/backups`.
///
/// # Errors
///
/// Returns [`AppError::DbUnavailable`] if `SQLite` is unreachable.
pub async fn list(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Vec<BackupListItem>>, AppError> {
    let rows: Vec<BackupListItem> = sqlx::query_as(
        "SELECT id, name, created_at, mc_version, size_bytes
         FROM backups WHERE server_id = ? ORDER BY created_at DESC",
    )
    .bind(&id)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

/// Handler: `POST /api/servers/:id/backups/:backup_id/restore`.
///
/// # Errors
///
/// - 404 if either the server or the backup row is missing.
/// - 409 `update_in_progress` if another update / apply / backup is running.
pub async fn restore(
    Path((id, backup_id)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<StartedResponse>), AppError> {
    backup_must_exist(&state, &id, &backup_id).await?;

    let Some(guard) = UpdateGuard::try_acquire(
        &id,
        state.update_locks.clone(),
        state.update_phase_buses.clone(),
    ) else {
        return Err(AppError::Conflict {
            code: "update_in_progress",
            message: "another update or apply is running".to_owned(),
        });
    };
    let task_state = state.clone();
    let task_id = id.clone();
    let task_b = backup_id.clone();
    tokio::spawn(async move {
        backups::run_restore(task_state, task_id, task_b, guard).await;
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(StartedResponse { status: "started" }),
    ))
}

/// Handler: `DELETE /api/servers/:id/backups/:backup_id`.
///
/// Spawns a one-shot Job and waits for completion before deleting the row.
/// 204 on success.
///
/// # Errors
///
/// - 404 if the backup does not exist (idempotent: re-deleting returns 404).
pub async fn delete(
    Path((id, backup_id)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<StatusCode, AppError> {
    backup_must_exist(&state, &id, &backup_id).await?;

    let snapshots_pvc = state.snapshots_pvc.as_ref();
    let job =
        backups::build_delete_job(&id, &backup_id, &state.mc_namespace, snapshots_pvc.as_str());
    let job_name = job
        .metadata
        .name
        .clone()
        .ok_or_else(|| AppError::Internal(anyhow::anyhow!("delete Job missing name")))?;
    let _permit = state.snapshot_pvc_lock.lock().await;
    crate::modpack::orchestrator::spawn_job(&state.kube, &state.mc_namespace, &job).await?;
    crate::modpack::orchestrator::wait_job(
        &state.kube,
        &state.mc_namespace,
        &job_name,
        DELETE_JOB_TIMEOUT,
    )
    .await
    .map_err(AppError::Internal)?;
    sqlx::query("DELETE FROM backups WHERE id = ?")
        .bind(&backup_id)
        .execute(&state.pool)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn server_must_exist(state: &AppState, id: &str) -> Result<(), AppError> {
    let opt: Option<(String,)> = sqlx::query_as("SELECT id FROM servers WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.pool)
        .await?;
    if opt.is_none() {
        return Err(AppError::NotFound);
    }
    Ok(())
}

async fn backup_must_exist(state: &AppState, id: &str, backup_id: &str) -> Result<(), AppError> {
    let opt: Option<(String,)> =
        sqlx::query_as("SELECT id FROM backups WHERE id = ? AND server_id = ?")
            .bind(backup_id)
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    if opt.is_none() {
        return Err(AppError::NotFound);
    }
    Ok(())
}
