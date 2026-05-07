//! `GET /api/servers/{id}/logs/stream` — live tail of pod logs over WS.
//!
//! Per-connection task structure: split the WebSocket into a sender and
//! a receiver, spawn a small read task that signals client-close via a
//! `oneshot`, and run the main writer loop. The writer alternates
//! between waiting for a Running pod (with a 60 s cap), forwarding the
//! kube `log_stream` line-by-line, and re-attaching when the stream
//! ends (e.g. pod restart). Heartbeat WS Pings every 30 s keep the
//! connection alive and let us detect client-side disconnects on
//! networks that quietly drop idle TCP.
//!
//! Known behaviour: the 60 s pod-unavailability window resets on every
//! re-attach, so a pod stuck in `CrashLoopBackOff` will keep this WS
//! open indefinitely (each crash is a fresh attach + restart cycle).
//! For the homelab use case this is fine; the user closes the tab.

use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use bytes::Bytes;
use chrono::Utc;
use futures_util::AsyncBufReadExt as _;
use futures_util::TryStreamExt as _;
use futures_util::sink::SinkExt as _;
use futures_util::stream::{SplitSink, SplitStream, StreamExt as _};
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::Pod;
use kube::Api;
use kube::api::LogParams;
use tokio::sync::oneshot;
use tokio::time::{Instant, MissedTickBehavior, interval};

use crate::AppState;
use crate::error::AppError;
use crate::k8s::ServerStatus;
use crate::k8s_status::derive_status;
use crate::routes::servers::get::fetch_server_row;
use crate::ws::{EndReason, Frame};

/// WS Ping interval.
const HEARTBEAT: Duration = Duration::from_secs(30);
/// Maximum time we wait for a pod to be Running before sending End.
const POD_WAIT_TIMEOUT: Duration = Duration::from_mins(1);
/// Sleep between pod-status polls while waiting for Running.
const POD_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// Number of trailing log lines kube sends as historical context
/// before live-following. Sized to fill the frontend's bounded buffer
/// so anything beyond would be trimmed in the UI anyway.
const HISTORY_LINES: i64 = 2000;

/// Handler for `GET /api/servers/{id}/logs/stream`.
///
/// The 404 check happens BEFORE upgrade so an unknown server returns a
/// proper HTTP error rather than an opened-then-closed WebSocket.
///
/// # Errors
///
/// - 404 if the server is not in the panel database.
pub async fn handle(
    Path(id): Path<String>,
    State(state): State<AppState>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, AppError> {
    let _row = fetch_server_row(&state.pool, &id).await?;
    Ok(upgrade.on_upgrade(move |socket| run(socket, state, id)))
}

/// Top-level connection task. Splits the socket, spawns a reader to
/// detect client-close, and drives the writer.
async fn run(socket: WebSocket, state: AppState, id: String) {
    let (sender, receiver) = socket.split();
    let (close_tx, close_rx) = oneshot::channel::<()>();

    let read_task = tokio::spawn(watch_close(receiver, close_tx));
    write_loop(sender, state, id, close_rx).await;

    // The reader is either already done (client closed) or stuck
    // waiting on a dropped connection. Aborting it releases the task.
    read_task.abort();
}

/// Reads the inbound side of the socket only to detect Close frames or
/// transport errors. When it sees one, signals via `close_tx`.
async fn watch_close(mut receiver: SplitStream<WebSocket>, close_tx: oneshot::Sender<()>) {
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(Message::Close(_)) | Err(_) => break,
            _ => {} // Pings/Pongs/Text from client are ignored
        }
    }
    let _ = close_tx.send(());
}

/// Drives the heartbeat + log-forwarding loop until the client closes,
/// the pod stays unavailable for too long, or any send errors out.
async fn write_loop(
    mut sender: SplitSink<WebSocket, Message>,
    state: AppState,
    id: String,
    mut close_rx: oneshot::Receiver<()>,
) {
    let resource_name = format!("mc-{id}");
    let pod_name = format!("{resource_name}-0");
    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let stsets: Api<StatefulSet> = Api::namespaced(state.kube.clone(), &state.mc_namespace);

    let mut hb = interval(HEARTBEAT);
    hb.set_missed_tick_behavior(MissedTickBehavior::Skip);
    // Consume the immediate first tick — we want the FIRST heartbeat to
    // happen 30 s after connect, not at t=0.
    hb.tick().await;

    'outer: loop {
        // Wait for Running. Heartbeats and close-watch run during the wait.
        match wait_for_running(
            &pods,
            &stsets,
            &resource_name,
            &pod_name,
            &mut sender,
            &mut hb,
            &mut close_rx,
        )
        .await
        {
            WaitOutcome::Running => {}
            WaitOutcome::ClientClosed => return,
            WaitOutcome::Timeout => {
                send_end(&mut sender, EndReason::PodUnavailable).await;
                return;
            }
        }

        // Announce the new attachment.
        if send_frame(
            &mut sender,
            Frame::Hello {
                pod: pod_name.clone(),
                attached_at: Utc::now(),
            },
        )
        .await
        .is_err()
        {
            return;
        }

        // Open the kube log stream. `tail_lines` makes kube replay
        // the last N lines as history before following — so opening
        // the console shows recent context, not just lines emitted
        // after attach.
        let log_params = LogParams {
            follow: true,
            tail_lines: Some(HISTORY_LINES),
            ..LogParams::default()
        };
        // Pod can die between the status check above and the log open;
        // any failure means "go back to waiting." Bound the open with a
        // 60s timeout + close-watch so a stalled kube API doesn't freeze
        // the WS task without heartbeats.
        let log_stream = tokio::select! {
            biased;
            _ = &mut close_rx => return,
            res = tokio::time::timeout(Duration::from_secs(60), pods.log_stream(&pod_name, &log_params)) => {
                match res {
                    Ok(Ok(s)) => s,
                    Ok(Err(_)) => continue 'outer,
                    Err(_) => continue 'outer,
                }
            }
        };
        let mut lines = log_stream.lines();

        // Forward lines, heartbeat, and watch close.
        loop {
            tokio::select! {
                biased;
                _ = &mut close_rx => return,
                _ = hb.tick() => {
                    if sender.send(Message::Ping(Bytes::new())).await.is_err() {
                        return;
                    }
                }
                // Not fully cancel-safe: if hb.tick wins while a line is
                // partially buffered inside the Lines adaptor, that
                // partial line is dropped. Acceptable for a live tail.
                next = lines.try_next() => {
                    match next {
                        Ok(Some(line)) => {
                            if send_frame(&mut sender, Frame::Log { line }).await.is_err() {
                                return;
                            }
                        }
                        // EOF or read error: re-attach.
                        Ok(None) | Err(_) => continue 'outer,
                    }
                }
            }
        }
    }
}

/// Outcome of [`wait_for_running`].
enum WaitOutcome {
    /// Pod transitioned to Running.
    Running,
    /// Client closed the WS while we were waiting.
    ClientClosed,
    /// Hit [`POD_WAIT_TIMEOUT`] without ever seeing Running.
    Timeout,
}

/// Polls the pod status every [`POD_POLL_INTERVAL`] up to
/// [`POD_WAIT_TIMEOUT`]. Heartbeats keep ticking; client-close aborts.
async fn wait_for_running(
    pods: &Api<Pod>,
    stsets: &Api<StatefulSet>,
    resource_name: &str,
    pod_name: &str,
    sender: &mut SplitSink<WebSocket, Message>,
    hb: &mut tokio::time::Interval,
    close_rx: &mut oneshot::Receiver<()>,
) -> WaitOutcome {
    let deadline = Instant::now() + POD_WAIT_TIMEOUT;
    loop {
        if let Ok(true) = check_running(pods, stsets, resource_name, pod_name).await {
            return WaitOutcome::Running;
        }
        if Instant::now() >= deadline {
            return WaitOutcome::Timeout;
        }
        tokio::select! {
            biased;
            _ = &mut *close_rx => return WaitOutcome::ClientClosed,
            _ = hb.tick() => {
                if sender.send(Message::Ping(Bytes::new())).await.is_err() {
                    return WaitOutcome::ClientClosed;
                }
            }
            () = tokio::time::sleep(POD_POLL_INTERVAL) => {}
        }
    }
}

/// Reads the `StatefulSet` + Pod and returns whether the pod is Running.
/// Errors are treated as "not running yet, keep polling."
///
/// Both reads are issued concurrently — on a busy kube API the
/// difference between sequential and parallel is the time of one extra
/// round-trip per poll, which directly bounds how long the WS sits in
/// "connecting" before sending Hello.
async fn check_running(
    pods: &Api<Pod>,
    stsets: &Api<StatefulSet>,
    resource_name: &str,
    pod_name: &str,
) -> Result<bool, kube::Error> {
    let (sts_res, pod_res) = tokio::join!(stsets.get_opt(resource_name), pods.get_opt(pod_name),);
    let sts = sts_res?;
    let pod = pod_res?;
    let (replicas, ready) = sts.as_ref().map_or((0, 0), |s| {
        let r = s.spec.as_ref().and_then(|sp| sp.replicas).unwrap_or(0);
        let ready = s
            .status
            .as_ref()
            .and_then(|st| st.ready_replicas)
            .unwrap_or(0);
        (r, ready)
    });
    Ok(derive_status(replicas, ready, pod.as_ref()) == ServerStatus::Running)
}

/// Sends a frame, returning Err on transport failure so the caller can
/// short-circuit out of the loop.
async fn send_frame(
    sender: &mut SplitSink<WebSocket, Message>,
    frame: Frame,
) -> Result<(), axum::Error> {
    sender.send(frame.into_message()).await
}

/// Best-effort: sends End{reason} then a Close frame. Errors are
/// swallowed because we are about to drop the socket anyway.
async fn send_end(sender: &mut SplitSink<WebSocket, Message>, reason: EndReason) {
    let _ = send_frame(sender, Frame::End { reason }).await;
    let _ = sender
        .send(Message::Close(Some(CloseFrame {
            code: 1000, // RFC 6455 Normal Closure
            reason: Utf8Bytes::from(""),
        })))
        .await;
}
