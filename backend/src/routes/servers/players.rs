//! `GET /api/servers/{id}/players` — bulk read.
//! `POST /api/servers/{id}/players/action` — discriminated 11-variant body.
//! `POST /api/servers/{id}/players/broadcast` — `/say MESSAGE`.
//!
//! All three are RCON-only. The bulk read additionally scrapes the last
//! ~2000 pod log lines for join/leave events; that scrape is best-effort
//! (history is empty on error, the rest of the response succeeds).

// Imports used only by the handlers in Tasks 5 and 6 are commented out here
// and will be uncommented when those handlers are added.
use serde::{Deserialize, Serialize};
use serde_json::json;

// Handler-only imports (uncommented in Tasks 5 + 6):
// use axum::Json;
// use axum::extract::{Path, State};
// use axum::http::StatusCode;
// use chrono::Utc;
// use k8s_openapi::api::core::v1::Pod;
// use kube::Api;
// use kube::api::LogParams;
// use crate::AppState;
// use crate::players::{
//     self, BanEntry, BanIpEntry, OnlinePlayers, PlayerEvent, PlayerEventKind,
// };
// use crate::routes::servers::create::insert_audit;
// use crate::routes::servers::rcon::{run_rcon_batch, run_rcon_one};

use crate::error::AppError;
use crate::validation::{
    validate_chat_message, validate_gamemode, validate_ip_v4_or_v6, validate_kick_reason,
    validate_mc_username,
};

// --- bulk-read response shapes ------------------------------------------------

/// Online-player snapshot for the wire response.
#[derive(Debug, Serialize)]
pub struct OnlinePlayersDto {
    pub count: u32,
    pub max: u32,
    pub players: Vec<String>,
}

/// One banned-player entry for the wire response.
#[derive(Debug, Serialize)]
pub struct BanEntryDto {
    pub name: String,
    pub reason: String,
}

/// One banned-IP entry for the wire response.
#[derive(Debug, Serialize)]
pub struct BanIpEntryDto {
    pub ip: String,
    pub reason: String,
}

/// Banlist (players + IPs) for the wire response.
#[derive(Debug, Serialize)]
pub struct BanlistDto {
    pub players: Vec<BanEntryDto>,
    pub ips: Vec<BanIpEntryDto>,
}

/// One join/leave event for the wire response.
#[derive(Debug, Serialize)]
pub struct PlayerEventDto {
    pub kind: &'static str,
    pub player: String,
    pub ts_ms: i64,
}

/// Full response body for `GET /api/servers/{id}/players`.
#[derive(Debug, Serialize)]
pub struct PlayersResponse {
    pub online: OnlinePlayersDto,
    pub whitelist: Vec<String>,
    pub banlist: BanlistDto,
    pub history: Vec<PlayerEventDto>,
}

// `From` conversions — wired up when the handler imports the parsing types
// in Tasks 5 + 6. Defined as standalone fns below to avoid importing the
// parsing module here (where it would be dead code until then).
//
// impl From<OnlinePlayers> for OnlinePlayersDto { ... }
// impl From<BanEntry> for BanEntryDto { ... }
// impl From<BanIpEntry> for BanIpEntryDto { ... }
// impl From<PlayerEvent> for PlayerEventDto { ... }

// --- action enum --------------------------------------------------------------

/// Body of `POST /api/servers/{id}/players/action`.
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
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
#[allow(dead_code, clippy::too_many_lines)]
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

// Suppress dead-code warnings on constants used by handlers not yet written.
const _: () = {
    let _ = HISTORY_TAIL_LINES;
    let _ = HISTORY_MAX_EVENTS;
};

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
