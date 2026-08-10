// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `GET /api/servers/{id}/players` — bulk read.
//! `POST /api/servers/{id}/players/action` — discriminated 11-variant body.
//! `POST /api/servers/{id}/players/broadcast` — `/say MESSAGE`.
//!
//! All three are RCON-only. The bulk read additionally scrapes the last
//! ~2000 pod log lines for join/leave events; that scrape is best-effort
//! (history is empty on error, the rest of the response succeeds).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use chrono::Utc;
use k8s_openapi::api::core::v1::Pod;
use kube::Api;
use kube::api::LogParams;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::AppState;
use crate::error::AppError;
use crate::players::{self, BanEntry, BanIpEntry, OnlinePlayers, PlayerEvent, PlayerEventKind};
use crate::routes::servers::create::insert_audit;
use crate::routes::servers::rcon::{run_rcon_batch, run_rcon_one};
use crate::validation::{
    validate_chat_message, validate_gamemode, validate_ip_v4_or_v6, validate_kick_reason,
    validate_mc_username,
};

// --- bulk-read response shapes ------------------------------------------------

/// Online-player snapshot for the wire response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct OnlinePlayersDto {
    pub count: u32,
    pub max: u32,
    pub players: Vec<String>,
}

/// One banned-player entry for the wire response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BanEntryDto {
    pub name: String,
    pub reason: String,
}

/// One banned-IP entry for the wire response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BanIpEntryDto {
    pub ip: String,
    pub reason: String,
}

/// Banlist (players + IPs) for the wire response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct BanlistDto {
    pub players: Vec<BanEntryDto>,
    pub ips: Vec<BanIpEntryDto>,
}

/// One join/leave event for the wire response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PlayerEventDto {
    pub kind: &'static str,
    pub player: String,
    pub ts_ms: i64,
}

/// Full response body for `GET /api/servers/{id}/players`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PlayersResponse {
    pub online: OnlinePlayersDto,
    pub whitelist: Vec<String>,
    pub banlist: BanlistDto,
    pub history: Vec<PlayerEventDto>,
}

// `From` conversions — wired up when the handler imports the parsing types
// in Tasks 5 + 6.

impl From<OnlinePlayers> for OnlinePlayersDto {
    fn from(o: OnlinePlayers) -> Self {
        Self {
            count: o.count,
            max: o.max,
            players: o.players,
        }
    }
}

impl From<BanEntry> for BanEntryDto {
    fn from(b: BanEntry) -> Self {
        Self {
            name: b.name,
            reason: b.reason,
        }
    }
}

impl From<BanIpEntry> for BanIpEntryDto {
    fn from(b: BanIpEntry) -> Self {
        Self {
            ip: b.ip,
            reason: b.reason,
        }
    }
}

impl From<PlayerEvent> for PlayerEventDto {
    fn from(e: PlayerEvent) -> Self {
        Self {
            kind: match e.kind {
                PlayerEventKind::Joined => "joined",
                PlayerEventKind::Left => "left",
            },
            player: e.player,
            ts_ms: e.ts_ms,
        }
    }
}

// --- action enum --------------------------------------------------------------

/// Body of `POST /api/servers/{id}/players/action`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum PlayerAction {
    Kick {
        player: String,
        reason: Option<String>,
    },
    Ban {
        player: String,
        reason: Option<String>,
    },
    BanIp {
        player: String,
        reason: Option<String>,
    },
    Pardon {
        player: String,
    },
    PardonIp {
        ip: String,
    },
    Op {
        player: String,
    },
    Deop {
        player: String,
    },
    Gamemode {
        player: String,
        mode: String,
    },
    Tell {
        player: String,
        message: String,
    },
    WhitelistAdd {
        player: String,
    },
    WhitelistRemove {
        player: String,
    },
}

/// Body of `POST /api/servers/{id}/players/broadcast`.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct BroadcastRequest {
    pub message: String,
}

/// Maximum lines pulled from the pod log for the recent-activity scrape.
const HISTORY_TAIL_LINES: i64 = 2_000;
/// Maximum events returned in the bulk-read `history` field.
const HISTORY_MAX_EVENTS: usize = 50;

// --- cmd builder -------------------------------------------------------------

/// Validates an action's fields and returns the RCON command string
/// that implements it. Returns `(audit_action, audit_details, cmd)`.
///
/// The audit triple is built here so the action handler stays simple —
/// validation, command, and audit shape stay in lock-step.
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] if any of the variant's fields fails
/// its validator (`validate_mc_username`, `validate_kick_reason`,
/// `validate_chat_message`, `validate_gamemode`, `validate_ip_v4_or_v6`).
// `clippy::too_many_lines` stays — the 11-variant match is inherently
// > 100 lines and shouldn't be split for the sake of it.
#[allow(clippy::too_many_lines)]
fn build_action(
    action: &PlayerAction,
) -> Result<(&'static str, serde_json::Value, String), AppError> {
    match action {
        PlayerAction::Kick { player, reason } => {
            validate_mc_username(player)?;
            let r = match reason {
                Some(r) => validate_kick_reason(r)?,
                None => "",
            };
            let cmd = if r.is_empty() {
                format!("kick {player}")
            } else {
                format!("kick {player} {r}")
            };
            Ok((
                "player.kick",
                json!({"player": player, "reason": reason}),
                cmd,
            ))
        }
        PlayerAction::Ban { player, reason } => {
            validate_mc_username(player)?;
            let r = match reason {
                Some(r) => validate_kick_reason(r)?,
                None => "",
            };
            let cmd = if r.is_empty() {
                format!("ban {player}")
            } else {
                format!("ban {player} {r}")
            };
            Ok((
                "player.ban",
                json!({"player": player, "reason": reason}),
                cmd,
            ))
        }
        PlayerAction::BanIp { player, reason } => {
            validate_mc_username(player)?;
            let r = match reason {
                Some(r) => validate_kick_reason(r)?,
                None => "",
            };
            let cmd = if r.is_empty() {
                format!("ban-ip {player}")
            } else {
                format!("ban-ip {player} {r}")
            };
            Ok((
                "player.ban_ip",
                json!({"player": player, "reason": reason}),
                cmd,
            ))
        }
        PlayerAction::Pardon { player } => {
            validate_mc_username(player)?;
            Ok((
                "player.pardon",
                json!({"player": player}),
                format!("pardon {player}"),
            ))
        }
        PlayerAction::PardonIp { ip } => {
            validate_ip_v4_or_v6(ip)?;
            Ok((
                "player.pardon_ip",
                json!({"ip": ip}),
                format!("pardon-ip {ip}"),
            ))
        }
        PlayerAction::Op { player } => {
            validate_mc_username(player)?;
            Ok((
                "player.op",
                json!({"player": player}),
                format!("op {player}"),
            ))
        }
        PlayerAction::Deop { player } => {
            validate_mc_username(player)?;
            Ok((
                "player.deop",
                json!({"player": player}),
                format!("deop {player}"),
            ))
        }
        PlayerAction::Gamemode { player, mode } => {
            validate_mc_username(player)?;
            validate_gamemode(mode)?;
            Ok((
                "player.gamemode",
                json!({"player": player, "mode": mode}),
                format!("gamemode {mode} {player}"),
            ))
        }
        PlayerAction::Tell { player, message } => {
            validate_mc_username(player)?;
            validate_chat_message(message)?;
            Ok((
                "player.tell",
                json!({"player": player, "message_len": message.len()}),
                format!("tell {player} {message}"),
            ))
        }
        PlayerAction::WhitelistAdd { player } => {
            validate_mc_username(player)?;
            Ok((
                "player.whitelist_add",
                json!({"player": player}),
                format!("whitelist add {player}"),
            ))
        }
        PlayerAction::WhitelistRemove { player } => {
            validate_mc_username(player)?;
            Ok((
                "player.whitelist_remove",
                json!({"player": player}),
                format!("whitelist remove {player}"),
            ))
        }
    }
}

// --- bulk-read handler -------------------------------------------------------

/// Handler for `GET /api/servers/{id}/players`.
///
/// Runs four RCON commands on one connection (`list`, `whitelist list`,
/// `banlist players`, `banlist ips`), parses each, and best-effort
/// scrapes the last ~2000 pod log lines for join/leave events.
///
/// # Errors
///
/// - 404 if the server is not in the panel database (via `run_rcon_batch`).
/// - 409 `server_not_running` if the `StatefulSet` is scaled down or the
///   pod is not Running.
/// - 500 on RCON / k8s failures.
#[utoipa::path(
    get,
    path = "/api/servers/{id}/players",
    params(("id" = String, Path, description = "server UUID")),
    responses(
        (status = 200, description = "Player list, whitelist, banlist, and recent history", body = PlayersResponse),
        (status = 404, description = "Server not found"),
        (status = 409, description = "Server not running"),
        (status = 500, description = "RCON or k8s failure")
    ),
    tag = "players"
)]
pub async fn handle_get(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<PlayersResponse>, AppError> {
    let outs = run_rcon_batch(
        &state,
        &id,
        &["list", "whitelist list", "banlist players", "banlist ips"],
    )
    .await?;

    // run_rcon_batch contractually returns 4 outputs in the requested
    // order. Defensively guard against a future API change.
    let online = outs.first().map(String::as_str).unwrap_or_default();
    let whitelist = outs.get(1).map(String::as_str).unwrap_or_default();
    let banlist_p = outs.get(2).map(String::as_str).unwrap_or_default();
    let banlist_i = outs.get(3).map(String::as_str).unwrap_or_default();

    let online_dto: OnlinePlayersDto = players::parse_list_output(online).into();
    let whitelist_v: Vec<String> = players::parse_whitelist_output(whitelist);
    let banlist_dto = BanlistDto {
        players: players::parse_banlist_players_output(banlist_p)
            .into_iter()
            .map(BanEntryDto::from)
            .collect(),
        ips: players::parse_banlist_ips_output(banlist_i)
            .into_iter()
            .map(BanIpEntryDto::from)
            .collect(),
    };

    let history = scrape_history(&state, &id).await;

    Ok(Json(PlayersResponse {
        online: online_dto,
        whitelist: whitelist_v,
        banlist: banlist_dto,
        history,
    }))
}

/// Best-effort: pull the last `HISTORY_TAIL_LINES` lines of pod logs,
/// parse each as a join/leave event, return the latest
/// `HISTORY_MAX_EVENTS` sorted desc by ts.
///
/// Errors are swallowed — the bulk read still succeeds with an empty
/// history.
async fn scrape_history(state: &AppState, id: &str) -> Vec<PlayerEventDto> {
    let pod_name = format!("mc-{id}-0");
    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    // `timestamps: true` makes kube prepend an RFC3339 timestamp to
    // each line — we use that as the event time, so "Xs ago" reflects
    // when the join/leave actually happened, not when we scraped.
    let params = LogParams {
        tail_lines: Some(HISTORY_TAIL_LINES),
        timestamps: true,
        ..LogParams::default()
    };
    let Ok(text) = pods.logs(&pod_name, &params).await else {
        return Vec::new();
    };
    let now = players::now_ms();
    let mut evs: Vec<PlayerEvent> = text
        .lines()
        .filter_map(|line| {
            let (ts_ms, rest) = players::split_kube_ts_prefix(line).unwrap_or((now, line));
            players::parse_log_join_leave(rest, ts_ms)
        })
        .collect();
    // Latest first, capped.
    evs.reverse();
    evs.truncate(HISTORY_MAX_EVENTS);
    evs.into_iter().map(PlayerEventDto::from).collect()
}

// --- action + broadcast handlers --------------------------------------------

/// Handler for `POST /api/servers/{id}/players/action`.
///
/// Validates the discriminated body, runs the corresponding RCON
/// command, writes one audit row, returns 204.
///
/// # Errors
///
/// - 400 with the validator's specific code (e.g. `username_invalid`).
/// - 404 / 409 / 500 from RCON failures (see [`run_rcon_one`]).
#[utoipa::path(
    post,
    path = "/api/servers/{id}/players/action",
    params(("id" = String, Path, description = "server UUID")),
    request_body = PlayerAction,
    responses(
        (status = 204, description = "Action applied"),
        (status = 400, description = "Validation error"),
        (status = 404, description = "Server not found"),
        (status = 409, description = "Server not running"),
        (status = 500, description = "RCON or k8s failure")
    ),
    tag = "players"
)]
pub async fn handle_action(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(action): Json<PlayerAction>,
) -> Result<StatusCode, AppError> {
    let (audit_action, audit_details, cmd) = build_action(&action)?;
    run_rcon_one(&state, &id, &cmd).await?;
    insert_audit(
        &state.pool,
        &id,
        audit_action,
        Some(audit_details),
        Utc::now().timestamp(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Handler for `POST /api/servers/{id}/players/broadcast`.
///
/// # Errors
///
/// - 400 `message_too_long` / `message_has_control_char`.
/// - 404 / 409 / 500 from RCON failures.
#[utoipa::path(
    post,
    path = "/api/servers/{id}/players/broadcast",
    params(("id" = String, Path, description = "server UUID")),
    request_body = BroadcastRequest,
    responses(
        (status = 204, description = "Broadcast sent"),
        (status = 400, description = "Message validation error"),
        (status = 404, description = "Server not found"),
        (status = 409, description = "Server not running"),
        (status = 500, description = "RCON or k8s failure")
    ),
    tag = "players"
)]
pub async fn handle_broadcast(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(req): Json<BroadcastRequest>,
) -> Result<StatusCode, AppError> {
    let msg = validate_chat_message(&req.message)?.to_owned();
    run_rcon_one(&state, &id, &format!("say {msg}")).await?;
    insert_audit(
        &state.pool,
        &id,
        "player.broadcast",
        Some(json!({ "message_len": msg.len() })),
        Utc::now().timestamp(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod cmd_builder_tests {
    use super::*;

    #[test]
    fn kick_with_reason() {
        let a = PlayerAction::Kick {
            player: "alice".into(),
            reason: Some("spam".into()),
        };
        let (act, _, cmd) = build_action(&a).unwrap();
        assert_eq!(act, "player.kick");
        assert_eq!(cmd, "kick alice spam");
    }

    #[test]
    fn kick_without_reason() {
        let a = PlayerAction::Kick {
            player: "alice".into(),
            reason: None,
        };
        let (_, _, cmd) = build_action(&a).unwrap();
        assert_eq!(cmd, "kick alice");
    }

    #[test]
    fn ban_with_reason() {
        let a = PlayerAction::Ban {
            player: "bob".into(),
            reason: Some("griefing".into()),
        };
        let (_, _, cmd) = build_action(&a).unwrap();
        assert_eq!(cmd, "ban bob griefing");
    }

    #[test]
    fn ban_ip_with_reason() {
        let a = PlayerAction::BanIp {
            player: "eve".into(),
            reason: Some("range hop".into()),
        };
        let (_, _, cmd) = build_action(&a).unwrap();
        assert_eq!(cmd, "ban-ip eve range hop");
    }

    #[test]
    fn pardon_and_pardon_ip() {
        let (_, _, cmd) = build_action(&PlayerAction::Pardon {
            player: "alice".into(),
        })
        .unwrap();
        assert_eq!(cmd, "pardon alice");
        let (_, _, cmd) = build_action(&PlayerAction::PardonIp {
            ip: "10.0.0.5".into(),
        })
        .unwrap();
        assert_eq!(cmd, "pardon-ip 10.0.0.5");
    }

    #[test]
    fn op_and_deop() {
        let (_, _, cmd) = build_action(&PlayerAction::Op {
            player: "alice".into(),
        })
        .unwrap();
        assert_eq!(cmd, "op alice");
        let (_, _, cmd) = build_action(&PlayerAction::Deop {
            player: "alice".into(),
        })
        .unwrap();
        assert_eq!(cmd, "deop alice");
    }

    #[test]
    fn gamemode_command_orders_mode_first() {
        let a = PlayerAction::Gamemode {
            player: "alice".into(),
            mode: "creative".into(),
        };
        let (_, _, cmd) = build_action(&a).unwrap();
        assert_eq!(cmd, "gamemode creative alice");
    }

    #[test]
    fn tell_command() {
        let a = PlayerAction::Tell {
            player: "alice".into(),
            message: "hi friend".into(),
        };
        let (_, details, cmd) = build_action(&a).unwrap();
        assert_eq!(cmd, "tell alice hi friend");
        // Message body is omitted from the audit details — only length.
        assert_eq!(details["message_len"], serde_json::json!(9));
        assert!(details.get("message").is_none());
    }

    #[test]
    fn whitelist_add_remove() {
        let (_, _, cmd) = build_action(&PlayerAction::WhitelistAdd {
            player: "alice".into(),
        })
        .unwrap();
        assert_eq!(cmd, "whitelist add alice");
        let (_, _, cmd) = build_action(&PlayerAction::WhitelistRemove {
            player: "alice".into(),
        })
        .unwrap();
        assert_eq!(cmd, "whitelist remove alice");
    }

    #[test]
    fn invalid_username_rejected_at_build() {
        let a = PlayerAction::Kick {
            player: "bad name!".into(),
            reason: None,
        };
        match build_action(&a) {
            Err(AppError::BadRequest { code, .. }) => assert_eq!(code, "username_invalid"),
            other => panic!("expected username_invalid, got {other:?}"),
        }
    }

    #[test]
    fn invalid_ip_rejected_at_build() {
        let a = PlayerAction::PardonIp {
            ip: "not.an.ip".into(),
        };
        match build_action(&a) {
            Err(AppError::BadRequest { code, .. }) => assert_eq!(code, "ip_invalid"),
            other => panic!("expected ip_invalid, got {other:?}"),
        }
    }

    #[test]
    fn invalid_gamemode_rejected_at_build() {
        let a = PlayerAction::Gamemode {
            player: "alice".into(),
            mode: "Adventure".into(),
        };
        match build_action(&a) {
            Err(AppError::BadRequest { code, .. }) => assert_eq!(code, "gamemode_invalid"),
            other => panic!("expected gamemode_invalid, got {other:?}"),
        }
    }
}
