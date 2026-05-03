//! `GET /api/servers/{id}/update/stream` — live update progress over WS.
//!
//! Subscribes to the orchestrator's `watch::Receiver<UpdatePhase>` for the
//! server, emits a `hello` frame with the current phase, then forwards
//! every transition as `progress` until a terminal phase (`succeeded` /
//! `rolled-back` / `failed`) is reached, at which point it sends `done`
//! and closes. When no orchestrator is running for `:id`, immediately sends
//! `hello{phase:idle}` then `end{reason:no-update-in-progress}`.

use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use bytes::Bytes;
use futures_util::sink::SinkExt as _;
use futures_util::stream::{SplitSink, SplitStream, StreamExt as _};
use serde::Serialize;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tokio::time::{MissedTickBehavior, interval};

use crate::AppState;
use crate::error::AppError;
use crate::modpack::orchestrator::UpdatePhase;
use crate::routes::servers::get::fetch_server_row;

/// WS Ping interval — matches the logs stream so frontends share retry timing.
const HEARTBEAT: Duration = Duration::from_secs(30);

/// Wire-format frame for the update stream.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
enum Frame {
    Hello { phase: UpdatePhase },
    Progress { phase: UpdatePhase },
    Done { result: DoneResult },
    End { reason: &'static str },
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
enum DoneResult {
    Succeeded,
    FailedRolledBack,
    Failed,
}

impl Frame {
    fn into_message(self) -> Message {
        let payload =
            serde_json::to_string(&self).expect("Frame serialization is infallible for our types");
        Message::Text(Utf8Bytes::from(payload))
    }
}

/// Maps a phase to a terminal `DoneResult`, or `None` if the phase is in-flight.
fn terminal(phase: UpdatePhase) -> Option<DoneResult> {
    match phase {
        UpdatePhase::Succeeded => Some(DoneResult::Succeeded),
        UpdatePhase::RolledBack => Some(DoneResult::FailedRolledBack),
        UpdatePhase::Failed => Some(DoneResult::Failed),
        _ => None,
    }
}

/// Handler for `GET /api/servers/{id}/update/stream`.
///
/// # Errors
///
/// - 404 if the server is not in the panel database. Note: the WS itself
///   handles the "no update running" case in-band by sending a `no-update-in-progress`
///   `end` frame — that's not an HTTP error.
pub async fn handle(
    Path(id): Path<String>,
    State(state): State<AppState>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, AppError> {
    let _row = fetch_server_row(&state.pool, &id).await?;
    Ok(upgrade.on_upgrade(move |socket| run(socket, state, id)))
}

async fn run(socket: WebSocket, state: AppState, id: String) {
    let (sender, receiver) = socket.split();
    let (close_tx, close_rx) = oneshot::channel::<()>();
    let read_task = tokio::spawn(watch_close(receiver, close_tx));
    write_loop(sender, state, id, close_rx).await;
    read_task.abort();
}

async fn watch_close(mut receiver: SplitStream<WebSocket>, close_tx: oneshot::Sender<()>) {
    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(Message::Close(_)) | Err(_) => break,
            _ => {}
        }
    }
    let _ = close_tx.send(());
}

async fn write_loop(
    mut sender: SplitSink<WebSocket, Message>,
    state: AppState,
    id: String,
    mut close_rx: oneshot::Receiver<()>,
) {
    let rx_opt: Option<watch::Receiver<UpdatePhase>> = state
        .update_phase_buses
        .lock()
        .expect("update_phase_buses poisoned")
        .get(&id)
        .cloned();

    let Some(mut rx) = rx_opt else {
        let _ = sender
            .send(
                Frame::Hello {
                    phase: UpdatePhase::Queued,
                }
                .into_message(),
            )
            .await;
        let _ = sender
            .send(
                Frame::End {
                    reason: "no-update-in-progress",
                }
                .into_message(),
            )
            .await;
        let _ = sender
            .send(Message::Close(Some(CloseFrame {
                code: 1000,
                reason: Utf8Bytes::from(""),
            })))
            .await;
        return;
    };

    let current = *rx.borrow_and_update();
    if sender
        .send(Frame::Hello { phase: current }.into_message())
        .await
        .is_err()
    {
        return;
    }
    if let Some(result) = terminal(current) {
        let _ = sender.send(Frame::Done { result }.into_message()).await;
        return;
    }

    let mut hb = interval(HEARTBEAT);
    hb.set_missed_tick_behavior(MissedTickBehavior::Skip);
    hb.tick().await;

    loop {
        tokio::select! {
            biased;
            _ = &mut close_rx => return,
            _ = hb.tick() => {
                if sender.send(Message::Ping(Bytes::new())).await.is_err() {
                    return;
                }
            }
            changed = rx.changed() => {
                if changed.is_err() {
                    // Sender dropped before reaching a terminal — orchestrator
                    // task may have panicked. Surface as `failed`.
                    let _ = sender.send(Frame::Done { result: DoneResult::Failed }.into_message()).await;
                    return;
                }
                let phase = *rx.borrow_and_update();
                if sender.send(Frame::Progress { phase }.into_message()).await.is_err() {
                    return;
                }
                if let Some(result) = terminal(phase) {
                    let _ = sender.send(Frame::Done { result }.into_message()).await;
                    return;
                }
            }
        }
    }
}
