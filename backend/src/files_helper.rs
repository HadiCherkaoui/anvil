//! Helper-Pod lifecycle (`mc-{id}-files`) and the generic `pods/exec`
//! primitives for file ops. Pure plumbing — handler logic lives in
//! `routes/servers/files.rs`.

use std::time::Duration;

use anyhow::anyhow;
use bytes::Bytes;
use futures_util::{Stream, StreamExt as _};
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, AttachParams, DeleteParams, PostParams, PropagationPolicy};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::AppState;
use crate::error::AppError;
use crate::k8s::ServerStatus;
use crate::k8s_status::derive_status;

/// Result of a non-streaming exec invocation.
#[derive(Debug)]
pub struct PodExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

/// 5-second cap for capture-shape execs (list, stat, mkdir).
/// Streaming variants use a longer idle-read timeout instead.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);

/// 5-minute cap for long-running capture execs (`rm -rf`, large rename
/// across directories). Used by [`pod_exec_capture_long`].
const CAPTURE_LONG_TIMEOUT: Duration = Duration::from_mins(5);

/// Runs `cmd` in `pod_name`, capturing stdout / stderr / exit code.
/// 5-second end-to-end timeout.
///
/// For potentially long-running operations (`rm -rf` of a large world,
/// rename across directories) use [`pod_exec_capture_long`].
///
/// # Errors
///
/// `AppError::Internal` on kube transport failure, timeout, or stream
/// read failure.
pub async fn pod_exec_capture(
    state: &AppState,
    namespace: &str,
    pod_name: &str,
    cmd: &[&str],
) -> Result<PodExecResult, AppError> {
    pod_exec_capture_with_timeout(state, namespace, pod_name, cmd, CAPTURE_TIMEOUT).await
}

/// Long-form variant of [`pod_exec_capture`] with a 5-minute end-to-end
/// timeout. Use for `rm -rf` of large trees and cross-directory renames
/// where the 5-second cap would falsely abort a still-progressing
/// operation. Stdout / stderr drain concurrently.
///
/// # Errors
///
/// `AppError::Internal` on kube transport failure, timeout, or stream
/// read failure.
pub async fn pod_exec_capture_long(
    state: &AppState,
    namespace: &str,
    pod_name: &str,
    cmd: &[&str],
) -> Result<PodExecResult, AppError> {
    pod_exec_capture_with_timeout(state, namespace, pod_name, cmd, CAPTURE_LONG_TIMEOUT).await
}

async fn pod_exec_capture_with_timeout(
    state: &AppState,
    namespace: &str,
    pod_name: &str,
    cmd: &[&str],
    cap: Duration,
) -> Result<PodExecResult, AppError> {
    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), namespace);

    let attach = AttachParams::default()
        .stdin(false)
        .stdout(true)
        .stderr(true);

    let fut = async {
        let mut process = pods
            .exec(pod_name, cmd.iter().copied(), &attach)
            .await
            .map_err(|e| anyhow!(e))?;
        let stdout_reader = process.stdout();
        let stderr_reader = process.stderr();

        // Drain concurrently — sequential drain deadlocks when stderr
        // fills its pipe buffer before stdout reaches EOF.
        let drain_stdout = async {
            let mut buf = Vec::new();
            if let Some(mut s) = stdout_reader {
                tokio::io::copy(&mut s, &mut buf)
                    .await
                    .map_err(|e| anyhow!(e))?;
            }
            anyhow::Ok(buf)
        };
        let drain_stderr = async {
            let mut buf = Vec::new();
            if let Some(mut e) = stderr_reader {
                tokio::io::copy(&mut e, &mut buf)
                    .await
                    .map_err(|err| anyhow!(err))?;
            }
            anyhow::Ok(buf)
        };
        let (stdout_buf, stderr_buf) = tokio::try_join!(drain_stdout, drain_stderr)?;

        let status = process.take_status();
        let exit_code = match status {
            Some(fut) => match fut.await {
                Some(s) => parse_exit_code(s.status.as_deref(), s.message.as_deref()),
                None => None,
            },
            None => None,
        };

        anyhow::Ok(PodExecResult {
            stdout: String::from_utf8_lossy(&stdout_buf).into_owned(),
            stderr: String::from_utf8_lossy(&stderr_buf).into_owned(),
            exit_code,
        })
    };

    match tokio::time::timeout(cap, fut).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => Err(AppError::Internal(err)),
        Err(_) => Err(AppError::Internal(anyhow!(
            "pod_exec_capture timed out after {} seconds",
            cap.as_secs()
        ))),
    }
}

/// Maps a k8s exec termination status into a numeric exit code. Status
/// `"Success"` is exit 0; otherwise we look at the status `message` for
/// `command terminated with exit code N`.
fn parse_exit_code(status: Option<&str>, message: Option<&str>) -> Option<i32> {
    if let Some("Success") = status {
        return Some(0);
    }
    // The k8s exec status message looks like:
    // "command terminated with exit code 1"
    let msg = message?;
    let idx = msg.rfind(' ')?;
    msg[idx + 1..].parse::<i32>().ok()
}

/// 60-second idle-read timeout for streaming variants. The total
/// duration is unbounded — anvil keeps the connection open as long as
/// data flows.
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_mins(1);

/// Streams a request body into the named pod's exec stdin. Aborts
/// mid-stream and returns `payload_too_large` if the cap is exceeded.
/// Returns the byte count written on success.
///
/// # Errors
///
/// `BadRequest("payload_too_large")` on cap breach; `Internal` on
/// transport / IO errors.
pub async fn pod_exec_stream_in<S>(
    state: &AppState,
    namespace: &str,
    pod_name: &str,
    cmd: &[&str],
    mut body: S,
    cap_bytes: u64,
) -> Result<u64, AppError>
where
    S: Stream<Item = Result<Bytes, axum::Error>> + Send + Unpin + 'static,
{
    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), namespace);
    let attach = AttachParams::default()
        .stdin(true)
        .stdout(false)
        .stderr(true);

    let mut process = pods
        .exec(pod_name, cmd.iter().copied(), &attach)
        .await
        .map_err(|e| AppError::Internal(anyhow!(e)))?;

    let mut stdin = process
        .stdin()
        .ok_or_else(|| AppError::Internal(anyhow!("exec stdin unavailable")))?;

    let mut total: u64 = 0;
    while let Some(chunk) = body.next().await {
        let bytes = chunk.map_err(|e| AppError::Internal(anyhow!(e)))?;
        let new_total = total.saturating_add(bytes.len() as u64);
        if new_total > cap_bytes {
            // Best-effort: close stdin to abort the remote command.
            drop(stdin);
            return Err(AppError::BadRequest {
                code: "payload_too_large",
                message: format!("upload exceeded {cap_bytes} bytes"),
            });
        }
        stdin
            .write_all(&bytes)
            .await
            .map_err(|e| AppError::Internal(anyhow!(e)))?;
        total = new_total;
    }

    drop(stdin); // signal EOF to the remote command

    // Drain stderr and consume the termination status concurrently.
    // Sequential reads race: take_status() can resolve before stderr
    // EOF on a fast failure, dropping diagnostics; or stderr-first can
    // deadlock if the status channel closes first on some kube versions.
    let stderr_reader = process.stderr();
    let status_fut = process.take_status();

    let drain_stderr = async {
        let mut buf = Vec::new();
        if let Some(mut e) = stderr_reader {
            let _ =
                tokio::time::timeout(STREAM_IDLE_TIMEOUT, tokio::io::copy(&mut e, &mut buf)).await;
        }
        buf
    };
    let read_status = async {
        match status_fut {
            Some(fut) => match tokio::time::timeout(STREAM_IDLE_TIMEOUT, fut).await {
                Ok(Some(s)) => parse_exit_code(s.status.as_deref(), s.message.as_deref()),
                _ => None,
            },
            None => None,
        }
    };
    let (stderr_buf, exit_code) = tokio::join!(drain_stderr, read_status);

    if exit_code != Some(0) {
        let stderr_str = String::from_utf8_lossy(&stderr_buf);
        return Err(AppError::Internal(anyhow!(
            "remote command failed: exit={:?}, stderr={}",
            exit_code,
            stderr_str.trim(),
        )));
    }

    Ok(total)
}

/// Returns an owned async stream of stdout bytes from the named pod's
/// exec. Caller pipes it into `axum::body::Body::from_stream`. Idle-read
/// timeout per chunk: 60 seconds (the connection terminates if the
/// remote `cat` blocks for that long).
///
/// # Errors
///
/// `Internal` on attach failure; the returned stream surfaces per-chunk
/// errors as [`std::io::Error`].
pub async fn pod_exec_stream_out(
    state: &AppState,
    namespace: &str,
    pod_name: &str,
    cmd: &[&str],
) -> Result<impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static, AppError> {
    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), namespace);
    let attach = AttachParams::default()
        .stdin(false)
        .stdout(true)
        .stderr(false);

    let mut process = pods
        .exec(pod_name, cmd.iter().copied(), &attach)
        .await
        .map_err(|e| AppError::Internal(anyhow!(e)))?;

    let stdout = process
        .stdout()
        .ok_or_else(|| AppError::Internal(anyhow!("exec stdout unavailable")))?;

    // Hold the process alive for the duration of the stream. The try_stream
    // owns `process` so it isn't dropped (which would close the channel)
    // until the consumer finishes.
    Ok(async_stream::try_stream! {
        let _process_guard = process; // keep process alive
        let mut reader = stdout;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = tokio::time::timeout(STREAM_IDLE_TIMEOUT, reader.read(&mut buf))
                .await
                .map_err(|_| std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "exec stdout idle timeout",
                ))??;
            if n == 0 { break; }
            yield Bytes::copy_from_slice(&buf[..n]);
        }
    })
}

/// Best-effort delete of the files-helper Pod. 404 is treated as
/// success. Waits up to 30 s for the Pod to be fully gone before
/// returning.
///
/// # Errors
///
/// `Internal` on transport failure or wait timeout.
pub async fn tear_down_helper(state: &AppState, server_id: &str) -> Result<(), AppError> {
    let pod_name = format!("mc-{server_id}-files");
    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), &state.mc_namespace);

    let dp = DeleteParams {
        propagation_policy: Some(PropagationPolicy::Foreground),
        grace_period_seconds: Some(15),
        ..DeleteParams::default()
    };
    match pods.delete(&pod_name, &dp).await {
        Ok(_) => {}
        Err(kube::Error::Api(e)) if e.code == 404 => {}
        Err(e) => return Err(AppError::Internal(anyhow!(e))),
    }

    crate::modpack::orchestrator::wait_pod_gone(
        &state.kube,
        &state.mc_namespace,
        &pod_name,
        Duration::from_secs(30),
    )
    .await
    .map_err(AppError::Internal)?;

    Ok(())
}

/// Lazy-creates the files-helper Pod and waits for it to be Running.
/// On `409 AlreadyExists` we treat the pre-existing Pod as ours and
/// proceed to wait. On a "pvc not bound / not found" error from the
/// create call we surface `Conflict("pvc_not_initialized")` so the
/// frontend can show the "start the server once" gate copy.
///
/// # Errors
///
/// `Conflict("pvc_not_initialized")` if the data PVC is missing;
/// `NotFound` if the parent `StatefulSet` is missing;
/// `Internal` on transport failure or wait timeout.
pub async fn ensure_helper(state: &AppState, server_id: &str) -> Result<(), AppError> {
    let pod_name = format!("mc-{server_id}-files");
    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), &state.mc_namespace);

    // Fast path: helper already exists. If it's healthy / starting,
    // wait for Running. If it's wedged (Failed phase, or Pending with
    // a terminal waiting reason like ImagePullBackOff), delete and
    // recreate — otherwise the caller would block on a pod that will
    // never become Ready.
    if let Some(existing) = pods
        .get_opt(&pod_name)
        .await
        .map_err(|e| AppError::Internal(anyhow!(e)))?
    {
        if helper_pod_broken(&existing) {
            tear_down_helper(state, server_id).await?;
        } else {
            return crate::modpack::orchestrator::wait_pod_running(
                &state.kube,
                &state.mc_namespace,
                &pod_name,
                Duration::from_secs(30),
            )
            .await
            .map_err(AppError::Internal);
        }
    }

    // Fetch the STS to use as ownerReference target — when the user
    // deletes the server, k8s GCs the helper Pod with it.
    let stsets: Api<StatefulSet> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let sts = stsets
        .get_opt(&format!("mc-{server_id}"))
        .await
        .map_err(|e| AppError::Internal(anyhow!(e)))?
        .ok_or(AppError::NotFound)?;
    let sts_uid = sts.metadata.uid.as_deref();

    let pod = crate::k8s_builders::build_files_helper_pod(
        server_id,
        &state.mc_namespace,
        &state.mc_alpine_image,
        sts_uid,
    );

    match pods.create(&PostParams::default(), &pod).await {
        Ok(_) => {}
        Err(kube::Error::Api(e)) if e.code == 409 => {
            // Race: another request created it. Treat as success.
        }
        Err(kube::Error::Api(e)) if pvc_not_found(&e.message) => {
            return Err(AppError::Conflict {
                code: "pvc_not_initialized",
                message: format!(
                    "data PVC for server {server_id} does not exist — start the server once to initialize storage"
                ),
            });
        }
        Err(e) => return Err(AppError::Internal(anyhow!(e))),
    }

    crate::modpack::orchestrator::wait_pod_running(
        &state.kube,
        &state.mc_namespace,
        &pod_name,
        Duration::from_secs(30),
    )
    .await
    .map_err(AppError::Internal)
}

/// Container `waiting` reasons that mean the helper Pod will never
/// reach Running on its own. Mirrors `k8s_status::ERROR_REASONS`.
const HELPER_ERROR_REASONS: &[&str] = &[
    "CrashLoopBackOff",
    "ImagePullBackOff",
    "ErrImagePull",
    "CreateContainerConfigError",
    "RunContainerError",
];

/// Returns `true` when the helper Pod is in a state that requires
/// delete+recreate: terminal phase or a container stuck on a known-bad
/// waiting reason.
fn helper_pod_broken(pod: &Pod) -> bool {
    let Some(status) = pod.status.as_ref() else {
        return false;
    };
    if let Some(phase) = status.phase.as_deref()
        && (phase == "Failed" || phase == "Unknown")
    {
        return true;
    }
    let Some(statuses) = status.container_statuses.as_ref() else {
        return false;
    };
    statuses.iter().any(|cs| {
        cs.state
            .as_ref()
            .and_then(|st| st.waiting.as_ref())
            .and_then(|w| w.reason.as_deref())
            .is_some_and(|r| HELPER_ERROR_REASONS.contains(&r))
    })
}

fn pvc_not_found(message: &str) -> bool {
    let lc = message.to_ascii_lowercase();
    lc.contains("persistentvolumeclaim")
        && (lc.contains("not found") || lc.contains("does not exist"))
}

/// Returns the pod name to exec into based on the server's current
/// status. Lazy-creates the helper Pod when the server is stopped.
///
/// # Errors
///
/// `Conflict("pvc_not_initialized")` if the server has never started
/// and the data PVC therefore doesn't exist; `NotFound` if the
/// `StatefulSet` itself is missing; `Internal` on transport failure or
/// helper-Pod wait timeout.
pub async fn target_pod_for_files(state: &AppState, server_id: &str) -> Result<String, AppError> {
    let stsets: Api<StatefulSet> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), &state.mc_namespace);

    let sts_name = format!("mc-{server_id}");
    let mc_pod = format!("mc-{server_id}-0");

    let sts = stsets
        .get_opt(&sts_name)
        .await
        .map_err(|e| AppError::Internal(anyhow!(e)))?
        .ok_or(AppError::NotFound)?;

    let pod_opt = pods
        .get_opt(&mc_pod)
        .await
        .map_err(|e| AppError::Internal(anyhow!(e)))?;

    let replicas = sts.spec.as_ref().and_then(|s| s.replicas).unwrap_or(0);
    let ready = sts
        .status
        .as_ref()
        .and_then(|s| s.ready_replicas)
        .unwrap_or(0);
    let status = derive_status(replicas, ready, pod_opt.as_ref());

    match status {
        ServerStatus::Running => Ok(mc_pod),
        ServerStatus::Stopped => {
            ensure_helper(state, server_id).await?;
            Ok(format!("mc-{server_id}-files"))
        }
        ServerStatus::Starting | ServerStatus::Stopping => Err(AppError::Conflict {
            code: "server_transitioning",
            message: "server is starting or stopping; retry shortly".to_owned(),
        }),
        ServerStatus::Error => Err(AppError::Conflict {
            code: "server_error",
            message: "server is in error state; resolve before browsing files".to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_exit_code_success() {
        assert_eq!(parse_exit_code(Some("Success"), None), Some(0));
    }

    #[test]
    fn parse_exit_code_from_message() {
        assert_eq!(
            parse_exit_code(Some("Failure"), Some("command terminated with exit code 1")),
            Some(1)
        );
        assert_eq!(
            parse_exit_code(
                Some("Failure"),
                Some("command terminated with exit code 127")
            ),
            Some(127)
        );
    }

    #[test]
    fn parse_exit_code_returns_none_on_garbage() {
        assert_eq!(parse_exit_code(Some("Failure"), None), None);
        assert_eq!(parse_exit_code(Some("Failure"), Some("nonsense")), None);
    }

    #[test]
    fn capture_timeout_constant() {
        assert_eq!(CAPTURE_TIMEOUT.as_secs(), 5);
    }

    #[test]
    fn stream_timeout_constant() {
        assert_eq!(STREAM_IDLE_TIMEOUT.as_secs(), 60);
    }

    #[test]
    fn pvc_not_found_detects_typical_kube_messages() {
        assert!(pvc_not_found(
            "persistentvolumeclaim \"data-mc-x-0\" not found"
        ));
        assert!(pvc_not_found("persistentvolumeclaim does not exist"));
        // case-insensitive
        assert!(pvc_not_found("PersistentVolumeClaim not found"));
    }

    #[test]
    fn pvc_not_found_ignores_unrelated() {
        assert!(!pvc_not_found("pod \"mc-x-files\" not found"));
        assert!(!pvc_not_found("internal server error"));
    }
}
