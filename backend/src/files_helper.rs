//! Helper-Pod lifecycle (`mc-{id}-files`) and the generic `pods/exec`
//! primitives sub-project D uses for file ops. Pure plumbing — handler
//! logic lives in `routes/servers/files.rs`.

use std::time::Duration;

use anyhow::anyhow;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, AttachParams};

use crate::error::AppError;
use crate::AppState;

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
}
