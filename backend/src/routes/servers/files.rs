//! File-browser handlers for sub-project D.
//!
//! All four endpoints share the same shape: route → fetch server row →
//! pick target pod via `files_helper::target_pod_for_files` → validate
//! path(s) → exec the appropriate command → audit (mutating only) →
//! respond.

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::Pod;
use kube::Api;
use serde::Deserialize;
use serde_json::json;

use crate::AppState;
use crate::error::AppError;
use crate::files::{
    FileListResponse, LIST_SCRIPT, is_enotdir_sentinel, parse_list_output, parse_stat_size,
};
use crate::files_helper::{
    pod_exec_capture, pod_exec_stream_in, pod_exec_stream_out, target_pod_for_files,
    tear_down_helper,
};
use crate::routes::servers::create::insert_audit;
use crate::validation::validate_data_path_argv_only;

/// 100 MiB upload cap (sub-project D scope) as bytes.
pub const UPLOAD_CAP_BYTES: u64 = 100 * 1024 * 1024;
/// Same cap typed as `usize` for the `DefaultBodyLimit` layer.
pub const UPLOAD_CAP_USIZE: usize = 100 * 1024 * 1024;

/// Query parameter for `?path=…` shared by list, download, upload.
#[derive(Debug, Deserialize)]
pub struct PathQuery {
    #[serde(default)]
    pub path: Option<String>,
}

/// Maps a validated `/data`-relative path to the actual absolute path
/// inside the target pod.
fn data_path(path: &str) -> String {
    if path == "/" {
        "/data".to_owned()
    } else {
        format!("/data{path}")
    }
}

/// Classifies a non-zero exec result from a file operation. Maps known
/// stderr substrings to user-facing errors; logs unknown failures and
/// returns a generic `Internal` so we never echo raw stderr to clients.
fn classify_exec_failure(op: &'static str, stderr: &str) -> AppError {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("no such file or directory") || lower.contains("enoent") {
        AppError::NotFound
    } else if lower.contains("is a directory") {
        AppError::BadRequest {
            code: "invalid_target",
            message: "target is a directory".to_owned(),
        }
    } else if lower.contains("permission denied") {
        tracing::error!(op, stderr = %stderr.trim(), "file op permission denied");
        AppError::Internal(anyhow::anyhow!("file operation failed"))
    } else {
        tracing::error!(op, stderr = %stderr.trim(), "file op failed");
        AppError::Internal(anyhow::anyhow!("file operation failed"))
    }
}

/// `GET /api/servers/{id}/files?path=/`.
///
/// # Errors
///
/// `BadRequest` for path validation failures; `NotFound` if the path
/// does not exist or is not a directory; `Conflict` if the server's
/// data PVC has not been initialised.
pub async fn list(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
    Query(q): Query<PathQuery>,
) -> Result<Json<FileListResponse>, AppError> {
    let raw_path = q.path.unwrap_or_else(|| "/".to_owned());
    let path = validate_data_path_argv_only(&raw_path)?.to_owned();
    let target = data_path(&path);

    let pod_name = target_pod_for_files(&state, &server_id).await?;

    let result = pod_exec_capture(
        &state,
        &state.mc_namespace,
        &pod_name,
        &["sh", "-c", LIST_SCRIPT, "_", &target],
    )
    .await?;

    if is_enotdir_sentinel(&result.stdout) {
        return Err(AppError::NotFound);
    }
    if result.exit_code != Some(0) {
        return Err(classify_exec_failure("list", &result.stderr));
    }

    let entries = parse_list_output(&result.stdout);
    Ok(Json(FileListResponse { path, entries }))
}

/// `GET /api/servers/{id}/files/raw?path=/foo/bar`.
///
/// # Errors
///
/// `BadRequest` for path validation; `NotFound` if the file does not
/// exist; `Conflict` if the server's data PVC has not been initialised.
///
/// # Panics
///
/// Panics if the file basename contains bytes that cannot live in an
/// HTTP header value. This is impossible for paths that pass
/// [`validate_data_path_argv_only`], which restricts segments to printable ASCII
/// minus single-quote and backslash.
pub async fn download(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
    Query(q): Query<PathQuery>,
) -> Result<Response, AppError> {
    let raw_path = q.path.ok_or_else(|| AppError::BadRequest {
        code: "path_required",
        message: "path query parameter required".to_owned(),
    })?;
    let path = validate_data_path_argv_only(&raw_path)?.to_owned();
    if path == "/" {
        return Err(AppError::BadRequest {
            code: "path_is_root",
            message: "cannot download the root directory".to_owned(),
        });
    }
    let target = data_path(&path);
    let pod_name = target_pod_for_files(&state, &server_id).await?;

    // Pre-flight: stat for existence. Missing file → 404. We deliberately
    // don't set Content-Length from the stat size — the file may be
    // mutated between stat and the cat stream finishing, and a wrong
    // length aborts the transfer. axum/hyper falls back to chunked
    // transfer-encoding when the header is absent.
    let stat = pod_exec_capture(
        &state,
        &state.mc_namespace,
        &pod_name,
        &["stat", "-c", "%s", &target],
    )
    .await?;
    if stat.exit_code != Some(0) {
        return Err(AppError::NotFound);
    }
    if parse_stat_size(&stat.stdout).is_none() {
        return Err(AppError::Internal(anyhow::anyhow!(
            "stat returned non-numeric output: {}",
            stat.stdout
        )));
    }

    let basename = path.rsplit('/').next().unwrap_or("file").to_owned();
    let stream =
        pod_exec_stream_out(&state, &state.mc_namespace, &pod_name, &["cat", &target]).await?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{basename}\""))
            .expect("validated basename produces valid header value"),
    );

    Ok((headers, Body::from_stream(stream)).into_response())
}

/// `PUT /api/servers/{id}/files?path=/foo/bar`.
///
/// Body is the raw file contents. Cap of [`UPLOAD_CAP_BYTES`] applies.
///
/// # Errors
///
/// `BadRequest("payload_too_large")` over cap; `Conflict("parent_not_directory")`
/// if the parent is missing or a file; path-validation `BadRequest`s.
pub async fn upload(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
    Query(q): Query<PathQuery>,
    request: Request,
) -> Result<StatusCode, AppError> {
    let raw_path = q.path.ok_or_else(|| AppError::BadRequest {
        code: "path_required",
        message: "path query parameter required".to_owned(),
    })?;
    let path = validate_data_path_argv_only(&raw_path)?.to_owned();
    if path == "/" {
        return Err(AppError::BadRequest {
            code: "path_is_root",
            message: "cannot upload to the root directory".to_owned(),
        });
    }

    let target = data_path(&path);
    let pod_name = target_pod_for_files(&state, &server_id).await?;

    // Pre-flight: parent must exist and be a directory.
    let parent = match path.rsplit_once('/').map(|(p, _)| p) {
        Some("") | None => "/".to_owned(),
        Some(p) => p.to_owned(),
    };
    let parent_target = data_path(&parent);
    let parent_check = pod_exec_capture(
        &state,
        &state.mc_namespace,
        &pod_name,
        &["sh", "-c", "test -d \"$1\"", "_", &parent_target],
    )
    .await?;
    if parent_check.exit_code != Some(0) {
        return Err(AppError::Conflict {
            code: "parent_not_directory",
            message: format!("parent {parent} is not a directory"),
        });
    }

    let body_stream = request.into_body().into_data_stream();
    // Atomic write: stream into "$1.tmp" then rename. Caller can re-issue
    // the upload safely if the connection drops mid-stream.
    let upload_script = "cat > \"$1.tmp\" && mv \"$1.tmp\" \"$1\"";

    let bytes = pod_exec_stream_in(
        &state,
        &state.mc_namespace,
        &pod_name,
        &["sh", "-c", upload_script, "_", &target],
        body_stream,
        UPLOAD_CAP_BYTES,
    )
    .await?;

    insert_audit(
        &state.pool,
        &server_id,
        "files.upload",
        Some(json!({ "path": path, "bytes": bytes })),
        Utc::now().timestamp(),
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

/// Discriminated body for `POST /api/servers/{id}/files/action`.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum FileAction {
    Mkdir { path: String },
    Rename { from: String, to: String },
    Delete { path: String, recursive: bool },
}

/// `POST /api/servers/{id}/files/action` — one of mkdir / rename / delete.
///
/// # Errors
///
/// Path-validation `BadRequest`s; `BadRequest("recursive_required")`
/// when deleting a directory without `recursive=true`; `Internal` on
/// unexpected exec failure.
pub async fn action(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
    Json(body): Json<FileAction>,
) -> Result<StatusCode, AppError> {
    let pod_name = target_pod_for_files(&state, &server_id).await?;
    let now = Utc::now().timestamp();

    match body {
        FileAction::Mkdir { path } => action_mkdir(&state, &server_id, &pod_name, &path, now).await,
        FileAction::Rename { from, to } => {
            action_rename(&state, &server_id, &pod_name, &from, &to, now).await
        }
        FileAction::Delete { path, recursive } => {
            action_delete(&state, &server_id, &pod_name, &path, recursive, now).await
        }
    }
}

async fn action_mkdir(
    state: &AppState,
    server_id: &str,
    pod_name: &str,
    path: &str,
    now: i64,
) -> Result<StatusCode, AppError> {
    let p = validate_data_path_argv_only(path)?.to_owned();
    if p == "/" {
        return Err(AppError::BadRequest {
            code: "path_is_root",
            message: "cannot mkdir the root directory".to_owned(),
        });
    }
    let target = data_path(&p);
    let r = pod_exec_capture(
        state,
        &state.mc_namespace,
        pod_name,
        &["mkdir", "-p", &target],
    )
    .await?;
    if r.exit_code != Some(0) {
        return Err(classify_exec_failure("mkdir", &r.stderr));
    }
    insert_audit(
        &state.pool,
        server_id,
        "files.mkdir",
        Some(json!({ "path": p })),
        now,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn action_rename(
    state: &AppState,
    server_id: &str,
    pod_name: &str,
    from: &str,
    to: &str,
    now: i64,
) -> Result<StatusCode, AppError> {
    let from_p = validate_data_path_argv_only(from)?.to_owned();
    let to_p = validate_data_path_argv_only(to)?.to_owned();
    if from_p == "/" || to_p == "/" {
        return Err(AppError::BadRequest {
            code: "path_is_root",
            message: "cannot rename involving the root directory".to_owned(),
        });
    }
    let from_t = data_path(&from_p);
    let to_t = data_path(&to_p);
    let r = pod_exec_capture(
        state,
        &state.mc_namespace,
        pod_name,
        &["mv", &from_t, &to_t],
    )
    .await?;
    if r.exit_code != Some(0) {
        return Err(classify_exec_failure("rename", &r.stderr));
    }
    insert_audit(
        &state.pool,
        server_id,
        "files.rename",
        Some(json!({ "from": from_p, "to": to_p })),
        now,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn action_delete(
    state: &AppState,
    server_id: &str,
    pod_name: &str,
    path: &str,
    recursive: bool,
    now: i64,
) -> Result<StatusCode, AppError> {
    let p = validate_data_path_argv_only(path)?.to_owned();
    if p == "/" {
        return Err(AppError::BadRequest {
            code: "path_is_root",
            message: "cannot delete the root directory".to_owned(),
        });
    }
    let target = data_path(&p);
    let cmd: Vec<&str> = if recursive {
        vec!["rm", "-rf", &target]
    } else {
        vec!["rm", &target]
    };
    let r = pod_exec_capture(state, &state.mc_namespace, pod_name, &cmd).await?;
    if r.exit_code != Some(0) {
        let stderr_lower = r.stderr.to_ascii_lowercase();
        if !recursive && stderr_lower.contains("is a directory") {
            return Err(AppError::BadRequest {
                code: "recursive_required",
                message: "target is a directory; pass recursive=true".to_owned(),
            });
        }
        return Err(classify_exec_failure("delete", &r.stderr));
    }
    insert_audit(
        &state.pool,
        server_id,
        "files.delete",
        Some(json!({ "path": p, "recursive": recursive })),
        now,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/servers/{id}/files/helper` — manual file-helper teardown.
///
/// The helper Pod (`mc-{id}-files`) auto-tears-down on server start; this
/// endpoint exists for the case where the user is done browsing files on a
/// stopped server and wants the helper gone before they (eventually) start.
///
/// # Errors
///
/// - 404 — server row missing.
/// - 409 `helper_unsafe_to_kill` — the `StatefulSet` has `replicas > 0`,
///   meaning the MC server is (or is about to be) running and the helper
///   may have files mid-write.
/// - 500 — kube transport / wait timeout.
pub async fn kill_helper(
    State(state): State<AppState>,
    Path(server_id): Path<String>,
) -> Result<Response, AppError> {
    let ss_api: Api<StatefulSet> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let ss = ss_api
        .get_opt(&format!("mc-{server_id}"))
        .await?
        .ok_or(AppError::NotFound)?;
    let replicas = ss.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0);
    if replicas > 0 {
        return Err(AppError::Conflict {
            code: "helper_unsafe_to_kill",
            message: "stop the server first".to_owned(),
        });
    }

    let pod_api: Api<Pod> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let helper_name = format!("mc-{server_id}-files");
    let exists = pod_api.get_opt(&helper_name).await?.is_some();
    if !exists {
        return Ok((StatusCode::OK, Json(json!({ "already_gone": true }))).into_response());
    }
    tear_down_helper(&state, &server_id).await?;

    insert_audit(
        &state.pool,
        &server_id,
        "files.helper.kill",
        None,
        Utc::now().timestamp(),
    )
    .await?;

    Ok(StatusCode::NO_CONTENT.into_response())
}
