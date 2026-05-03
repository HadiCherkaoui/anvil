//! Pure parsing of Minecraft RCON outputs and log lines.
//!
//! All functions are I/O-free and `kube`-free. The route handlers feed
//! them strings; tests cover real-world MC output samples.

use chrono::Utc;

/// Online-player snapshot derived from `RCON list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnlinePlayers {
    pub count: u32,
    pub max: u32,
    pub players: Vec<String>,
}

/// One row of `RCON banlist players`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BanEntry {
    pub name: String,
    pub reason: String,
}

/// One row of `RCON banlist ips`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BanIpEntry {
    pub ip: String,
    pub reason: String,
}

/// Direction of a single log-derived player event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerEventKind {
    Joined,
    Left,
}

/// A single join / leave event parsed from a pod log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerEvent {
    pub kind: PlayerEventKind,
    pub player: String,
    /// Wall-clock millis when the line was parsed (we don't get a date
    /// from the `[HH:MM:SS]` log prefix; using parse-time gives correct
    /// relative ordering at the cost of ≤ poll-interval absolute drift).
    pub ts_ms: i64,
}

/// Returns the current wall-clock time as Unix milliseconds.
///
/// Route handlers pass this value to [`parse_log_join_leave`]; tests
/// pass a fixed literal instead, so this function is not called in tests.
#[must_use]
pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// Parses `RCON list` output into [`OnlinePlayers`].
///
/// Vanilla format: `There are N of a max of M players online: a, b`
/// (with or without trailing period). Returns the empty snapshot on
/// any unparseable input.
#[must_use]
pub fn parse_list_output(s: &str) -> OnlinePlayers {
    let trimmed = s.trim();
    let Some(rest) = trimmed.strip_prefix("There are ") else {
        return OnlinePlayers {
            count: 0,
            max: 0,
            players: vec![],
        };
    };
    let Some((count_str, after_count)) = rest.split_once(" of a max of ") else {
        return OnlinePlayers {
            count: 0,
            max: 0,
            players: vec![],
        };
    };
    let Some((max_str, after_max)) = after_count.split_once(" players online") else {
        return OnlinePlayers {
            count: 0,
            max: 0,
            players: vec![],
        };
    };
    let count = count_str.parse::<u32>().unwrap_or(0);
    let max = max_str.parse::<u32>().unwrap_or(0);
    let names_part = after_max
        .trim_start_matches(':')
        .trim()
        .trim_end_matches('.');
    let players = if names_part.is_empty() {
        Vec::new()
    } else {
        names_part
            .split(',')
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty())
            .collect()
    };
    OnlinePlayers {
        count,
        max,
        players,
    }
}

/// Parses `RCON whitelist list` output into the list of usernames.
///
/// Vanilla formats: `There are N whitelisted players: a, b, c` and
/// `There are no whitelisted players`. Returns empty on unparseable.
#[must_use]
pub fn parse_whitelist_output(s: &str) -> Vec<String> {
    let trimmed = s.trim().trim_end_matches('.');
    if trimmed == "There are no whitelisted players" {
        return Vec::new();
    }
    let Some((_, after_colon)) = trimmed.split_once(':') else {
        return Vec::new();
    };
    after_colon
        .trim()
        .split(',')
        .map(|n| n.trim().to_owned())
        .filter(|n| !n.is_empty())
        .collect()
}

/// Parses `RCON banlist players` output into [`BanEntry`] rows.
#[must_use]
pub fn parse_banlist_players_output(s: &str) -> Vec<BanEntry> {
    parse_banlist_lines(s, "There are no bans")
        .into_iter()
        .map(|(target, reason)| BanEntry {
            name: target,
            reason,
        })
        .collect()
}

/// Parses `RCON banlist ips` output into [`BanIpEntry`] rows.
#[must_use]
pub fn parse_banlist_ips_output(s: &str) -> Vec<BanIpEntry> {
    parse_banlist_lines(s, "There are no IP bans")
        .into_iter()
        .map(|(target, reason)| BanIpEntry { ip: target, reason })
        .collect()
}

/// Shared shape parser used for both `banlist players` and `banlist ips`.
/// Returns (target, reason) pairs.
fn parse_banlist_lines(s: &str, empty_keyword: &str) -> Vec<(String, String)> {
    // Empty-state messages: "There are no bans" / "There is no ban" /
    // "There are no IP bans" / "There is no IP ban". The keyword we
    // search for is the unique noun part (e.g. "no IP bans" or "no bans").
    // Anchor to the first line only — a ban reason can legitimately contain
    // the keyword and must not trigger a false empty result.
    let first_line = s.lines().next().unwrap_or("").trim();
    let empty_short = empty_keyword
        .strip_prefix("There are ")
        .unwrap_or(empty_keyword);
    if first_line.contains(empty_short) {
        return Vec::new();
    }
    s.lines()
        .skip(1)
        .filter_map(|line| {
            let line = line.trim().trim_end_matches('.');
            let (target, after) = line.split_once(" was banned by ")?;
            let (_, reason) = after.split_once(": ")?;
            Some((target.to_owned(), reason.to_owned()))
        })
        .collect()
}

/// Parses one pod-log line for a join/leave event.
///
/// Accepts both the vanilla `[HH:MM:SS] [Server thread/INFO]: …` shape
/// and the Forge/Paper `[HH:MM:SS INFO]: …` shape. Returns `None` for
/// any line that doesn't match a `<player> joined the game` or
/// `<player> left the game` suffix.
///
/// `ts_ms` is the wall-clock time stamped on the event (the log prefix
/// has no date, so callers pass the parse-time value).
#[must_use]
pub fn parse_log_join_leave(line: &str, ts_ms: i64) -> Option<PlayerEvent> {
    let body = strip_log_prefix(line)?;
    if let Some(name) = body.strip_suffix(" joined the game") {
        return Some(PlayerEvent {
            kind: PlayerEventKind::Joined,
            player: name.to_owned(),
            ts_ms,
        });
    }
    if let Some(name) = body.strip_suffix(" left the game") {
        return Some(PlayerEvent {
            kind: PlayerEventKind::Left,
            player: name.to_owned(),
            ts_ms,
        });
    }
    None
}

/// Strips the `[HH:MM:SS] [thread/LEVEL]:` (vanilla) or
/// `[HH:MM:SS LEVEL]:` (Forge/Paper) prefix and returns the body, or
/// `None` if the line doesn't have a recognized prefix.
fn strip_log_prefix(line: &str) -> Option<&str> {
    let idx = line.find("]:")?;
    let after = line[idx + 2..].trim_start();
    if after.is_empty() { None } else { Some(after) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_parses_zero_online() {
        let out = parse_list_output("There are 0 of a max of 20 players online:");
        assert_eq!(
            out,
            OnlinePlayers {
                count: 0,
                max: 20,
                players: vec![]
            }
        );
    }

    #[test]
    fn list_parses_two_online() {
        let out = parse_list_output("There are 2 of a max of 20 players online: alice, bob");
        assert_eq!(
            out,
            OnlinePlayers {
                count: 2,
                max: 20,
                players: vec!["alice".into(), "bob".into()]
            }
        );
    }

    #[test]
    fn list_parses_with_trailing_period() {
        let out = parse_list_output("There are 1 of a max of 20 players online: alice.");
        assert_eq!(out.players, vec!["alice".to_owned()]);
    }

    #[test]
    fn list_handles_unparseable_input_as_empty() {
        let out = parse_list_output("garbage from a wedged server");
        assert_eq!(
            out,
            OnlinePlayers {
                count: 0,
                max: 0,
                players: vec![]
            }
        );
    }

    #[test]
    fn whitelist_parses_empty() {
        for s in [
            "There are no whitelisted players",
            "There are no whitelisted players.",
        ] {
            assert_eq!(parse_whitelist_output(s), Vec::<String>::new());
        }
    }

    #[test]
    fn whitelist_parses_three() {
        let out = parse_whitelist_output("There are 3 whitelisted players: alice, bob, charlie");
        assert_eq!(out, vec!["alice", "bob", "charlie"]);
    }

    #[test]
    fn whitelist_handles_one() {
        let out = parse_whitelist_output("There are 1 whitelisted player: alice");
        assert_eq!(out, vec!["alice"]);
    }

    #[test]
    fn banlist_players_parses_empty() {
        for s in ["There are no bans", "There are no bans."] {
            assert_eq!(parse_banlist_players_output(s), Vec::<BanEntry>::new());
        }
    }

    #[test]
    fn banlist_players_parses_two() {
        let s = "There are 2 bans:\nalice was banned by Server: spam.\nbob was banned by Server: griefing.";
        let out = parse_banlist_players_output(s);
        assert_eq!(
            out,
            vec![
                BanEntry {
                    name: "alice".into(),
                    reason: "spam".into()
                },
                BanEntry {
                    name: "bob".into(),
                    reason: "griefing".into()
                },
            ]
        );
    }

    #[test]
    fn banlist_players_parses_default_reason() {
        let s = "There are 1 ban:\nalice was banned by Server: Banned by an operator.";
        let out = parse_banlist_players_output(s);
        assert_eq!(
            out,
            vec![BanEntry {
                name: "alice".into(),
                reason: "Banned by an operator".into()
            }]
        );
    }

    #[test]
    fn banlist_players_does_not_false_empty_on_reason_text() {
        let s = "There is 1 ban:\nalice was banned by Server: no bans have been pardoned yet.";
        let out = parse_banlist_players_output(s);
        assert_eq!(
            out,
            vec![BanEntry {
                name: "alice".into(),
                reason: "no bans have been pardoned yet".into(),
            }]
        );
    }

    #[test]
    fn banlist_ips_parses_empty() {
        for s in ["There are no IP bans", "There are no IP bans."] {
            assert_eq!(parse_banlist_ips_output(s), Vec::<BanIpEntry>::new());
        }
    }

    #[test]
    fn banlist_ips_parses_one() {
        let s = "There is 1 IP ban:\n10.0.0.5 was banned by Server: range hop.";
        let out = parse_banlist_ips_output(s);
        assert_eq!(
            out,
            vec![BanIpEntry {
                ip: "10.0.0.5".into(),
                reason: "range hop".into()
            }]
        );
    }

    #[test]
    fn log_parses_vanilla_join() {
        let line = "[01:23:45] [Server thread/INFO]: alice joined the game";
        let ev = parse_log_join_leave(line, 1_714_000_000_000).expect("expected join");
        assert_eq!(ev.kind, PlayerEventKind::Joined);
        assert_eq!(ev.player, "alice");
        assert_eq!(ev.ts_ms, 1_714_000_000_000);
    }

    #[test]
    fn log_parses_vanilla_leave() {
        let line = "[01:23:45] [Server thread/INFO]: alice left the game";
        let ev = parse_log_join_leave(line, 0).expect("expected leave");
        assert_eq!(ev.kind, PlayerEventKind::Left);
        assert_eq!(ev.player, "alice");
    }

    #[test]
    fn log_parses_isoish_timestamp_form() {
        let line = "[01:23:45 INFO]: alice joined the game";
        let ev = parse_log_join_leave(line, 0).expect("expected join");
        assert_eq!(ev.player, "alice");
    }

    #[test]
    fn log_ignores_unrelated_lines() {
        for line in [
            "[01:23:45] [Server thread/INFO]: Done (3.500s)! For help, type \"help\"",
            "[01:23:45] [Server thread/INFO]: alice issued server command: /list",
            "garbage",
            "",
        ] {
            assert!(
                parse_log_join_leave(line, 0).is_none(),
                "expected None for {line:?}"
            );
        }
    }
}
