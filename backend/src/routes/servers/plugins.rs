//! `/api/servers/{id}/plugins*` — Paper plugin list editing + apply + WS.
//!
//! Mirrors the `/mods` shape one-for-one: pending edits go into
//! `paper.pending_plugins` (the full desired list staged for apply), and
//! [`POST /plugins/apply`] kicks the shared sync FSM in [`mods_apply`]
//! with [`SyncTarget::Plugins`]. The WS endpoint reuses the same frame
//! shape as `mods/apply/stream` so the frontend phase viewer is shared.

use std::time::Duration;

use axum::Json;
use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use bytes::Bytes;
use chrono::Utc;
use futures_util::sink::SinkExt as _;
use futures_util::stream::{SplitSink, SplitStream, StreamExt as _};
use serde::Serialize;
use serde_json::json;
use tokio::sync::{oneshot, watch};
use tokio::time::{MissedTickBehavior, interval};

use crate::AppState;
use crate::error::AppError;
use crate::modpack::guard::UpdateGuard;
use crate::modpack::modded::ModEntry;
use crate::modpack::mods_apply::{self, SyncTarget};
use crate::modpack::orchestrator::UpdatePhase;
use crate::modpack::paper::Config as PaperConfig;
use crate::routes::servers::create::insert_audit;
use crate::routes::servers::get::fetch_server_row;
use crate::validation::validate_mod_filename;

const HEARTBEAT: Duration = Duration::from_secs(30);

/// Response body for `GET /api/servers/{id}/plugins`.
#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub plugins: Vec<ModEntry>,
    pub pending_plugins: Vec<ModEntry>,
}

/// `GET /api/servers/{id}/plugins` — current and pending plugin lists.
///
/// # Errors
///
/// - 404 if the server doesn't exist.
/// - 400 `not_paper` if the server isn't a Paper source kind.
pub async fn list(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<ListResponse>, AppError> {
    let cfg = load_paper_cfg(&state, &id).await?;
    Ok(Json(ListResponse {
        plugins: cfg.plugins,
        pending_plugins: cfg.pending_plugins,
    }))
}

/// `POST /api/servers/{id}/plugins` — stage adding a plugin to the next apply.
///
/// The full [`ModEntry`] is supplied by the catalog pick; the handler
/// initialises `pending_plugins` from `plugins` if it's the first edit
/// since the last apply, then upserts by filename.
///
/// # Errors
///
/// - 404 if the server doesn't exist.
/// - 400 `not_paper` if the server isn't a Paper source kind.
/// - 400 `mod_filename_invalid` if the filename fails validation.
pub async fn add_pending(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(entry): Json<ModEntry>,
) -> Result<StatusCode, AppError> {
    validate_mod_filename(&entry.filename)?;

    let mut cfg = load_paper_cfg(&state, &id).await?;
    if cfg.pending_plugins.is_empty() {
        cfg.pending_plugins = cfg.plugins.clone();
    }
    cfg.pending_plugins.retain(|p| p.filename != entry.filename);
    cfg.pending_plugins.push(entry);
    save_paper_cfg(&state, &id, &cfg).await?;

    let now = Utc::now().timestamp();
    let _ = insert_audit(
        &state.pool,
        &id,
        "plugins_pending_add",
        Some(json!({"pending_count": cfg.pending_plugins.len()})),
        now,
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /api/servers/{id}/plugins/{filename}` — stage removing a plugin.
///
/// Initialises `pending_plugins` from `plugins` if needed, then drops the
/// entry by filename. If the result is identical to `plugins`, resets
/// `pending_plugins` to empty so the UI shows "no pending changes".
///
/// # Errors
///
/// - 404 if the server doesn't exist.
/// - 400 `not_paper` if the server isn't a Paper source kind.
/// - 400 `mod_filename_invalid` if the filename fails validation.
pub async fn remove_pending(
    Path((id, filename)): Path<(String, String)>,
    State(state): State<AppState>,
) -> Result<StatusCode, AppError> {
    validate_mod_filename(&filename)?;

    let mut cfg = load_paper_cfg(&state, &id).await?;
    if cfg.pending_plugins.is_empty() {
        cfg.pending_plugins = cfg.plugins.clone();
    }
    cfg.pending_plugins.retain(|p| p.filename != filename);
    if cfg.pending_plugins == cfg.plugins {
        cfg.pending_plugins = Vec::new();
    }
    save_paper_cfg(&state, &id, &cfg).await?;

    let now = Utc::now().timestamp();
    let _ = insert_audit(
        &state.pool,
        &id,
        "plugins_pending_remove",
        Some(json!({"filename": filename, "pending_count": cfg.pending_plugins.len()})),
        now,
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// Response body for `POST /api/servers/{id}/plugins/apply`.
#[derive(Debug, Serialize)]
pub struct ApplyResponse {
    pub status: &'static str,
    pub server_id: String,
    pub pending_count: usize,
}

/// `POST /api/servers/{id}/plugins/apply` — kick the plugin-sync FSM.
///
/// # Errors
///
/// - 404 if the server doesn't exist.
/// - 400 `not_paper` if the server isn't a Paper source kind.
/// - 409 `nothing_pending` if `pending_plugins` is empty.
/// - 409 `apply_in_progress` if another update/apply is already running.
pub async fn apply(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<ApplyResponse>), AppError> {
    let cfg = load_paper_cfg(&state, &id).await?;
    if cfg.pending_plugins.is_empty() {
        return Err(AppError::Conflict {
            code: "nothing_pending",
            message: "no pending plugin changes to apply".to_owned(),
        });
    }
    let pending_count = cfg.pending_plugins.len();

    let Some(guard) = UpdateGuard::try_acquire(
        &id,
        state.update_locks.clone(),
        state.update_phase_buses.clone(),
    ) else {
        return Err(AppError::Conflict {
            code: "apply_in_progress",
            message: "an update or apply is already running for this server".to_owned(),
        });
    };

    let task_state = state.clone();
    let task_id = id.clone();
    tokio::spawn(async move {
        mods_apply::run(task_state, task_id, guard, SyncTarget::Plugins).await;
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(ApplyResponse {
            status: "started",
            server_id: id,
            pending_count,
        }),
    ))
}

/// `GET /api/servers/{id}/plugins/apply/stream` — WS for the plugin-sync FSM.
///
/// Frame shape mirrors `mods/apply/stream` and `update/stream` so the
/// frontend reuses one phase viewer.
///
/// # Errors
///
/// - 404 if the server doesn't exist.
pub async fn apply_stream(
    Path(id): Path<String>,
    State(state): State<AppState>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, AppError> {
    let _row = fetch_server_row(&state.pool, &id).await?;
    Ok(upgrade.on_upgrade(move |socket| run_ws(socket, state, id)))
}

async fn load_paper_cfg(state: &AppState, id: &str) -> Result<PaperConfig, AppError> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT source_kind, source_config FROM servers WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    let (kind, raw) = row.ok_or(AppError::NotFound)?;
    if kind != "paper" {
        return Err(AppError::BadRequest {
            code: "not_paper",
            message: "plugin endpoints only apply to paper servers".to_owned(),
        });
    }
    serde_json::from_str(&raw)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("source_config not paper JSON: {e}")))
}

async fn save_paper_cfg(state: &AppState, id: &str, cfg: &PaperConfig) -> Result<(), AppError> {
    let raw = serde_json::to_string(cfg).map_err(|e| AppError::Internal(e.into()))?;
    sqlx::query("UPDATE servers SET source_config = ? WHERE id = ?")
        .bind(&raw)
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(())
}

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
    Failed,
}

impl Frame {
    fn into_message(self) -> Message {
        let payload = serde_json::to_string(&self).expect("Frame serialization is infallible");
        Message::Text(Utf8Bytes::from(payload))
    }
}

fn terminal(p: UpdatePhase) -> Option<DoneResult> {
    match p {
        UpdatePhase::Succeeded => Some(DoneResult::Succeeded),
        UpdatePhase::Failed | UpdatePhase::RolledBack => Some(DoneResult::Failed),
        _ => None,
    }
}

async fn run_ws(socket: WebSocket, state: AppState, id: String) {
    let (sender, receiver) = socket.split();
    let (close_tx, close_rx) = oneshot::channel::<()>();
    let read_task = tokio::spawn(watch_close(receiver, close_tx));
    write_loop(sender, state, id, close_rx).await;
    read_task.abort();
}

async fn watch_close(mut rx: SplitStream<WebSocket>, close_tx: oneshot::Sender<()>) {
    while let Some(msg) = rx.next().await {
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
                    reason: "no-apply-in-progress",
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
