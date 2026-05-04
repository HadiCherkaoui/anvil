//! Helper-Pod lifecycle (`mc-{id}-files`) and the generic `pods/exec`
//! primitives sub-project D uses for file ops. Pure plumbing — handler
//! logic lives in `routes/servers/files.rs`.

use std::time::Duration;

use anyhow::anyhow;
use bytes::Bytes;
use futures_util::{Stream, StreamExt as _};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, AttachParams};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::AppState;
use crate::error::AppError;

/// Result of a non-streaming exec invocation.
#[derive(Debug)]
pub struct PodExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
}

/// 5-second cap for capture-shape execs (list, stat, mkdir, rename,
/// delete). Streaming variants use a longer idle-read timeout instead.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);

/// Runs `cmd` in `pod_name`, capturing stdout / stderr / exit code.
/// 5-second end-to-end timeout. Used for: list (`LIST_SCRIPT`), stat
/// pre-flights, mkdir, rename, single-file delete, recursive delete.
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
        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();

        if let Some(mut s) = process.stdout() {
            tokio::io::copy(&mut s, &mut stdout_buf)
                .await
                .map_err(|e| anyhow!(e))?;
        }
        if let Some(mut e) = process.stderr() {
            tokio::io::copy(&mut e, &mut stderr_buf)
                .await
                .map_err(|err| anyhow!(err))?;
        }

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

    match tokio::time::timeout(CAPTURE_TIMEOUT, fut).await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => Err(AppError::Internal(err)),
        Err(_) => Err(AppError::Internal(anyhow!(
            "pod_exec_capture timed out after {} seconds",
            CAPTURE_TIMEOUT.as_secs()
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
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

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

    // Drain stderr and check exit code under the idle-read timeout.
    let mut stderr_buf = Vec::new();
    if let Some(mut e) = process.stderr() {
        let _ = tokio::time::timeout(
            STREAM_IDLE_TIMEOUT,
            tokio::io::copy(&mut e, &mut stderr_buf),
        )
        .await;
    }

    let status = process.take_status();
    let exit_code = match status {
        Some(fut) => match tokio::time::timeout(STREAM_IDLE_TIMEOUT, fut).await {
            Ok(Some(s)) => parse_exit_code(s.status.as_deref(), s.message.as_deref()),
            _ => None,
        },
        None => None,
    };

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
}
