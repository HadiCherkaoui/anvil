//! `/api/servers/{id}/mods*` — modlist editing + apply + apply-stream WS.
//!
//! Pending ops are appended to `source_config.pending` by `POST /mods` and
//! removed by `DELETE /mods/pending/{idx}`. `POST /mods/apply` kicks the
//! mod-sync FSM in [`mods_apply::run`], which uses [`UpdateGuard`] +
//! `snapshot_pvc_lock` for one-at-a-time semantics. `GET /mods/apply/stream`
//! mirrors the update WS frame shape so the frontend reuses one phase viewer.

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
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{oneshot, watch};
use tokio::time::{MissedTickBehavior, interval};

use std::collections::HashSet;

use crate::AppState;
use crate::error::AppError;
use crate::modpack::ModpackHttp;
use crate::modpack::dep_resolver::{ResolveContext, resolve_required};
use crate::modpack::guard::{UpdateGuard, recent_terminal};
use crate::modpack::modded::{Config as ModdedConfig, ModEntry, PendingOp};
use crate::modpack::mods_apply::{self, SyncTarget};
use crate::modpack::orchestrator::UpdatePhase;
use crate::routes::servers::create::insert_audit;
use crate::routes::servers::get::fetch_server_row;
use crate::validation::validate_mod_filename;

const HEARTBEAT: Duration = Duration::from_secs(30);

/// Request body for `POST /api/servers/{id}/mods`.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PendingOpRequest {
    Add {
        mod_entry: ModEntry,
    },
    Remove {
        filename: String,
    },
    Bump {
        filename: String,
        to_version_id: String,
        to_version_name: String,
        to_filename: String,
        to_download_url: String,
        #[serde(default)]
        to_sha512: Option<String>,
    },
}

impl From<PendingOpRequest> for PendingOp {
    fn from(r: PendingOpRequest) -> Self {
        match r {
            PendingOpRequest::Add { mod_entry } => Self::Add { mod_entry },
            PendingOpRequest::Remove { filename } => Self::Remove { filename },
            PendingOpRequest::Bump {
                filename,
                to_version_id,
                to_version_name,
                to_filename,
                to_download_url,
                to_sha512,
            } => Self::Bump {
                filename,
                to_version_id,
                to_version_name,
                to_filename,
                to_download_url,
                to_sha512,
            },
        }
    }
}

/// Response body for `POST /api/servers/{id}/mods`.
///
/// `added` lists every mod that was added by this call — for an Add op
/// that resolves required deps, this is the seed plus the resolved deps.
/// For Remove and Bump ops, `added` is empty.
#[derive(Debug, Serialize)]
pub struct AddResponse {
    pub added: Vec<ModEntry>,
    pub added_count: usize,
}

/// `POST /api/servers/{id}/mods` — append a pending op to the modlist draft.
///
/// # Errors
///
/// - 404 if the server doesn't exist.
/// - 400 `not_modded` if the server isn't a modded source kind.
/// - 400 `mod_filename_invalid` if any filename fails validation.
pub async fn add_pending(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(req): Json<PendingOpRequest>,
) -> Result<Json<AddResponse>, AppError> {
    match &req {
        PendingOpRequest::Add { mod_entry } => validate_mod_filename(&mod_entry.filename)?,
        PendingOpRequest::Remove { filename } => validate_mod_filename(filename)?,
        PendingOpRequest::Bump {
            filename,
            to_filename,
            ..
        } => {
            validate_mod_filename(filename)?;
            validate_mod_filename(to_filename)?;
        }
    }

    let mut cfg = load_modded_cfg(&state, &id).await?;

    let (new_ops, added_entries): (Vec<PendingOp>, Vec<ModEntry>) = match &req {
        PendingOpRequest::Add { mod_entry } => {
            let extra = resolve_for_add(&state, &cfg, mod_entry, "modrinth_or_seed_provider").await;
            let mut ops: Vec<PendingOp> = Vec::with_capacity(1 + extra.len());
            ops.push(PendingOp::Add {
                mod_entry: mod_entry.clone(),
            });
            for dep in &extra {
                ops.push(PendingOp::Add {
                    mod_entry: dep.clone(),
                });
            }
            let mut added = Vec::with_capacity(1 + extra.len());
            added.push(mod_entry.clone());
            added.extend(extra);
            (ops, added)
        }
        _ => (vec![req.into()], Vec::new()),
    };

    cfg.pending.extend(new_ops);
    save_modded_cfg(&state, &id, &cfg).await?;
    let now = Utc::now().timestamp();
    let _ = insert_audit(
        &state.pool,
        &id,
        "mods_pending_add",
        Some(json!({
            "pending_count": cfg.pending.len(),
            "added_count": added_entries.len(),
        })),
        now,
    )
    .await;

    let added_count = added_entries.len();
    Ok(Json(AddResponse {
        added: added_entries,
        added_count,
    }))
}

async fn resolve_for_add(
    state: &AppState,
    cfg: &ModdedConfig,
    seed: &ModEntry,
    _label: &str,
) -> Vec<ModEntry> {
    let installed: HashSet<(String, String)> = cfg
        .mods
        .iter()
        .map(|m| (m.provider.clone(), m.project_id.clone()))
        .collect();
    let mut pending: HashSet<(String, String)> = cfg
        .pending
        .iter()
        .filter_map(|p| match p {
            PendingOp::Add { mod_entry } => {
                Some((mod_entry.provider.clone(), mod_entry.project_id.clone()))
            }
            _ => None,
        })
        .collect();
    pending.insert((seed.provider.clone(), seed.project_id.clone()));

    let loader = cfg.runtime.type_env().to_lowercase();
    let mut ctx = ResolveContext {
        mc_version: &cfg.mc_version,
        loader: loader.as_str(),
        installed,
        pending,
    };
    let http = ModpackHttp {
        cf: state.cf_client.as_deref(),
        mr: state.mr_client.as_ref(),
    };
    match resolve_required(seed, &mut ctx, &http).await {
        Ok(extra) => extra,
        Err(err) => {
            tracing::warn!(error = %err, "dep resolver failed; proceeding without extras");
            Vec::new()
        }
    }
}

/// `DELETE /api/servers/{id}/mods/pending/{idx}` — drop one pending op.
///
/// # Errors
///
/// - 404 if the server doesn't exist or `idx` is out of range.
/// - 400 `not_modded` if the server isn't a modded source kind.
pub async fn remove_pending(
    Path((id, idx)): Path<(String, usize)>,
    State(state): State<AppState>,
) -> Result<StatusCode, AppError> {
    let mut cfg = load_modded_cfg(&state, &id).await?;
    if idx >= cfg.pending.len() {
        return Err(AppError::NotFound);
    }
    cfg.pending.remove(idx);
    save_modded_cfg(&state, &id, &cfg).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Response body for `POST /api/servers/{id}/mods/apply`.
#[derive(Debug, Serialize)]
pub struct ApplyResponse {
    pub status: &'static str,
    pub server_id: String,
    pub pending_count: usize,
}

/// `POST /api/servers/{id}/mods/apply` — kick the mod-sync FSM.
///
/// # Errors
///
/// - 404 if the server doesn't exist.
/// - 400 `not_modded` if the server isn't a modded source kind.
/// - 409 `nothing_pending` if there are no pending ops.
/// - 409 `apply_in_progress` if another update/apply is already running.
pub async fn apply(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<(StatusCode, Json<ApplyResponse>), AppError> {
    let cfg = load_modded_cfg(&state, &id).await?;
    if cfg.pending.is_empty() {
        return Err(AppError::Conflict {
            code: "nothing_pending",
            message: "no pending mod changes to apply".to_owned(),
        });
    }
    let pending_count = cfg.pending.len();

    let Some(guard) = UpdateGuard::try_acquire(
        &id,
        state.update_locks.clone(),
        state.update_phase_buses.clone(),
        state.update_errors.clone(),
    ) else {
        return Err(AppError::Conflict {
            code: "apply_in_progress",
            message: "an update or apply is already running for this server".to_owned(),
        });
    };

    let task_state = state.clone();
    let task_id = id.clone();
    tokio::spawn(async move {
        mods_apply::run(task_state, task_id, guard, SyncTarget::Mods).await;
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

/// `GET /api/servers/{id}/mods/apply/stream` — WS for the mod-sync FSM phases.
///
/// Frame shape mirrors `update_stream` so the frontend can reuse the phase
/// viewer.
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

async fn load_modded_cfg(state: &AppState, id: &str) -> Result<ModdedConfig, AppError> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT source_kind, source_config FROM servers WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await?;
    let (kind, raw) = row.ok_or(AppError::NotFound)?;
    if kind != "modded" {
        return Err(AppError::BadRequest {
            code: "not_modded",
            message: "modlist endpoints only apply to modded servers".to_owned(),
        });
    }
    serde_json::from_str(&raw)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("source_config not modded JSON: {e}")))
}

async fn save_modded_cfg(state: &AppState, id: &str, cfg: &ModdedConfig) -> Result<(), AppError> {
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
        // A fast apply (e.g. removing a single mod on a stopped server)
        // can complete before the WS connects. Surface the recent
        // terminal phase if it landed within the side-channel TTL so
        // the user sees `done{succeeded}` instead of `no-apply-in-progress`.
        if let Some(phase) = recent_terminal(&state, &id) {
            let _ = sender.send(Frame::Hello { phase }.into_message()).await;
            if let Some(result) = terminal(phase) {
                let _ = sender.send(Frame::Done { result }.into_message()).await;
            }
            let _ = sender
                .send(Message::Close(Some(CloseFrame {
                    code: 1000,
                    reason: Utf8Bytes::from(""),
                })))
                .await;
            return;
        }
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
