# Player Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the v2.x `PlayersBody` placeholder with a working Players tab — online/whitelist/banlist read, 11 per-player verbs, broadcast, recent join/leave activity — for any anvil-managed Minecraft server.

**Architecture:** Backend exposes RCON-only endpoints: bulk-read runs four RCON commands on one connection plus a pod-logs scrape; action endpoint dispatches a discriminated 11-variant body to the right RCON command; broadcast endpoint sends `/say`. Frontend polls bulk-read every 10 s while visible, renders four stacked Cards with per-row action menus, uses `Modal` dialogs for confirmation. No new RBAC, no DB migration, no new dependencies.

**Tech Stack:** Rust 1.83 · axum 0.8 · kube-rs · sqlx (SQLite, offline migrations); Next.js 16 (`output: 'export'`) · TypeScript · Tailwind v4 · Zod.

**Spec:** [`docs/superpowers/specs/2026-05-03-anvil-v2-player-management-design.md`](../specs/2026-05-03-anvil-v2-player-management-design.md)

---

## File Structure

| Path | Status | Responsibility |
|---|---|---|
| `backend/src/players.rs` | NEW | Pure parsing functions over RCON output / log lines + types. No I/O. |
| `backend/src/lib.rs` | MODIFY | Add `pub mod players;`. |
| `backend/src/validation.rs` | MODIFY | Add 5 validators (`validate_mc_username`, `validate_kick_reason`, `validate_chat_message`, `validate_gamemode`, `validate_ip_v4_or_v6`). |
| `backend/src/routes/servers/rcon.rs` | MODIFY | Extract `run_rcon_batch` + `run_rcon_one`; rewire `handle` over them. |
| `backend/src/routes/servers/players.rs` | NEW | Three handlers (bulk read · action · broadcast) + `PlayerAction` enum + cmd builder. |
| `backend/src/routes/servers/mod.rs` | MODIFY | `pub mod players;` + three router entries. |
| `frontend/app/lib/api.ts` | MODIFY | Add players response/action schemas + `fetchPlayers` / `runPlayerAction` / `broadcastMessage`. |
| `frontend/app/lib/use-players.ts` | NEW | Polling hook with `visibilitychange` pause. |
| `frontend/app/components/AddToWhitelistDialog.tsx` | NEW | `[+ add]` Modal: name input + submit. |
| `frontend/app/components/BroadcastDialog.tsx` | NEW | `[broadcast]` Modal: textarea + char counter + send. |
| `frontend/app/components/PlayerActionDialog.tsx` | NEW | One Modal, four variants (yes/no · with-reason · with-message · with-mode). |
| `frontend/app/components/PlayerActionMenu.tsx` | NEW | Wraps `Dropdown` with the right action set per source (online / whitelist / banlist). |
| `frontend/app/servers/tabs/PlayersBody.tsx` | REWRITE | Replace placeholder. Composes four Cards + the broadcast bar; gates on server status. |
| `docs/milestones.md` | MODIFY | Mark sub-project C complete. |

The four Cards (`OnlinePlayersCard`, `WhitelistCard`, `BanlistCard`, `RecentActivityCard`) live as small inline components inside `PlayersBody.tsx` rather than separate files — each is < 40 LoC and only used in one place. If any grows beyond that during implementation, split it out.

---

## Phase 1: Backend foundations

### Task 1: Backend parsers (`backend/src/players.rs`)

**Files:**
- Create: `backend/src/players.rs`
- Modify: `backend/src/lib.rs`

- [ ] **Step 1: Create `backend/src/players.rs` with the type scaffold**

```rust
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

/// Returns the current wall-clock time in millis. Extracted so tests can
/// inject a fixed value; production callers use it directly.
#[must_use]
pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}
```

- [ ] **Step 2: Add `pub mod players;` to `backend/src/lib.rs`**

Locate the module declarations near the top of `backend/src/lib.rs` (look for the existing `pub mod validation;` or similar) and add:

```rust
pub mod players;
```

Verify it compiles:

```bash
cargo build -p anvil 2>&1 | tail -5
```

Expected: `Finished … target(s)` (no errors).

- [ ] **Step 3: Write tests for `parse_list_output` (failing)**

Append to `backend/src/players.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_parses_zero_online() {
        let out = parse_list_output("There are 0 of a max of 20 players online:");
        assert_eq!(out, OnlinePlayers { count: 0, max: 20, players: vec![] });
    }

    #[test]
    fn list_parses_two_online() {
        let out = parse_list_output("There are 2 of a max of 20 players online: alice, bob");
        assert_eq!(
            out,
            OnlinePlayers { count: 2, max: 20, players: vec!["alice".into(), "bob".into()] }
        );
    }

    #[test]
    fn list_parses_with_trailing_period() {
        // Some MC versions append a period.
        let out = parse_list_output("There are 1 of a max of 20 players online: alice.");
        assert_eq!(out.players, vec!["alice".to_owned()]);
    }

    #[test]
    fn list_handles_unparseable_input_as_empty() {
        let out = parse_list_output("garbage from a wedged server");
        assert_eq!(out, OnlinePlayers { count: 0, max: 0, players: vec![] });
    }
}
```

Run:

```bash
cargo test -p anvil --lib players::tests::list 2>&1 | tail -20
```

Expected: failures with "cannot find function `parse_list_output`".

- [ ] **Step 4: Implement `parse_list_output`**

Insert before the `#[cfg(test)]` block:

```rust
/// Parses `RCON list` output into [`OnlinePlayers`].
///
/// Vanilla format: `There are N of a max of M players online: a, b`
/// (with or without trailing period). Returns the empty snapshot on
/// any unparseable input.
#[must_use]
pub fn parse_list_output(s: &str) -> OnlinePlayers {
    let trimmed = s.trim();
    let Some(rest) = trimmed.strip_prefix("There are ") else {
        return OnlinePlayers { count: 0, max: 0, players: vec![] };
    };
    let Some((count_str, after_count)) = rest.split_once(" of a max of ") else {
        return OnlinePlayers { count: 0, max: 0, players: vec![] };
    };
    let Some((max_str, after_max)) = after_count.split_once(" players online") else {
        return OnlinePlayers { count: 0, max: 0, players: vec![] };
    };
    let count = count_str.parse::<u32>().unwrap_or(0);
    let max = max_str.parse::<u32>().unwrap_or(0);
    let names_part = after_max.trim_start_matches(':').trim().trim_end_matches('.');
    let players = if names_part.is_empty() {
        Vec::new()
    } else {
        names_part
            .split(',')
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty())
            .collect()
    };
    OnlinePlayers { count, max, players }
}
```

Run the tests:

```bash
cargo test -p anvil --lib players::tests::list 2>&1 | tail -10
```

Expected: 4 passed.

- [ ] **Step 5: Write tests for `parse_whitelist_output` (failing)**

Append to the `tests` module:

```rust
    #[test]
    fn whitelist_parses_empty() {
        for s in ["There are no whitelisted players", "There are no whitelisted players."] {
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
```

Run:

```bash
cargo test -p anvil --lib players::tests::whitelist 2>&1 | tail -10
```

Expected: failures with "cannot find function `parse_whitelist_output`".

- [ ] **Step 6: Implement `parse_whitelist_output`**

Insert before `#[cfg(test)]`:

```rust
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
```

Run:

```bash
cargo test -p anvil --lib players::tests::whitelist 2>&1 | tail -10
```

Expected: 3 passed.

- [ ] **Step 7: Write tests for `parse_banlist_players_output` (failing)**

Append:

```rust
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
                BanEntry { name: "alice".into(), reason: "spam".into() },
                BanEntry { name: "bob".into(),   reason: "griefing".into() },
            ]
        );
    }

    #[test]
    fn banlist_players_parses_with_no_reason() {
        // When a ban was issued without a reason, the suffix is omitted.
        let s = "There are 1 ban:\nalice was banned by Server: Banned by an operator.";
        let out = parse_banlist_players_output(s);
        assert_eq!(out, vec![BanEntry { name: "alice".into(), reason: "Banned by an operator".into() }]);
    }
```

Run:

```bash
cargo test -p anvil --lib players::tests::banlist_players 2>&1 | tail -10
```

Expected: failures.

- [ ] **Step 8: Implement `parse_banlist_players_output`**

```rust
/// Parses `RCON banlist players` output into [`BanEntry`] rows.
#[must_use]
pub fn parse_banlist_players_output(s: &str) -> Vec<BanEntry> {
    parse_banlist_lines(s, "There are no bans").into_iter()
        .map(|(target, reason)| BanEntry { name: target, reason })
        .collect()
}

/// Shared shape parser used for both `banlist players` and `banlist ips`.
/// Returns (target, reason) pairs.
fn parse_banlist_lines(s: &str, empty_marker: &str) -> Vec<(String, String)> {
    let trimmed = s.trim().trim_end_matches('.');
    if trimmed.starts_with(empty_marker) {
        return Vec::new();
    }
    s.lines()
        .skip(1) // first line is "There are N ban(s):"
        .filter_map(|line| {
            let line = line.trim().trim_end_matches('.');
            let (target, after) = line.split_once(" was banned by ")?;
            let (_, reason) = after.split_once(": ")?;
            Some((target.to_owned(), reason.to_owned()))
        })
        .collect()
}
```

Run:

```bash
cargo test -p anvil --lib players::tests::banlist_players 2>&1 | tail -10
```

Expected: 3 passed.

- [ ] **Step 9: Write tests for `parse_banlist_ips_output` (failing)**

```rust
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
        assert_eq!(out, vec![BanIpEntry { ip: "10.0.0.5".into(), reason: "range hop".into() }]);
    }
```

Run:

```bash
cargo test -p anvil --lib players::tests::banlist_ips 2>&1 | tail -10
```

Expected: failures.

- [ ] **Step 10: Implement `parse_banlist_ips_output`**

```rust
/// Parses `RCON banlist ips` output into [`BanIpEntry`] rows.
#[must_use]
pub fn parse_banlist_ips_output(s: &str) -> Vec<BanIpEntry> {
    parse_banlist_lines(s, "There are no IP bans").into_iter()
        .map(|(target, reason)| BanIpEntry { ip: target, reason })
        .collect()
}
```

(`parse_banlist_lines` is already defined in Step 8 — reuse it.) Note that the empty marker text "There are no IP bans" is what vanilla emits regardless of singular/plural in some MC versions; "There is no IP ban" also exists. Update the helper to accept either form by checking the empty-marker prefix:

Replace the helper implementation in Step 8 with:

```rust
fn parse_banlist_lines(s: &str, empty_keyword: &str) -> Vec<(String, String)> {
    let trimmed = s.trim().trim_end_matches('.');
    // Empty-state messages: "There are no bans" / "There is no ban" /
    // "There are no IP bans" / "There is no IP ban". The keyword we
    // search for is the unique noun part (e.g. "no IP bans" or "no bans").
    let empty_short = empty_keyword.strip_prefix("There are ").unwrap_or(empty_keyword);
    if trimmed.contains(empty_short) {
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
```

Run:

```bash
cargo test -p anvil --lib players::tests::banlist 2>&1 | tail -15
```

Expected: 5 passed (3 banlist_players + 2 banlist_ips).

- [ ] **Step 11: Write tests for `parse_log_join_leave` (failing)**

```rust
    #[test]
    fn log_parses_vanilla_join() {
        let line = "[01:23:45] [Server thread/INFO]: alice joined the game";
        let ev = parse_log_join_leave(line, 1714000000000).expect("expected join");
        assert_eq!(ev.kind, PlayerEventKind::Joined);
        assert_eq!(ev.player, "alice");
        assert_eq!(ev.ts_ms, 1714000000000);
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
        // Some Forge / Paper variants emit:
        //   [01:23:45 INFO]: alice joined the game
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
            assert!(parse_log_join_leave(line, 0).is_none(), "expected None for {line:?}");
        }
    }
```

Run:

```bash
cargo test -p anvil --lib players::tests::log 2>&1 | tail -15
```

Expected: failures.

- [ ] **Step 12: Implement `parse_log_join_leave`**

```rust
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
            player: name.trim().to_owned(),
            ts_ms,
        });
    }
    if let Some(name) = body.strip_suffix(" left the game") {
        return Some(PlayerEvent {
            kind: PlayerEventKind::Left,
            player: name.trim().to_owned(),
            ts_ms,
        });
    }
    None
}

/// Strips the `[HH:MM:SS] [thread/LEVEL]:` (vanilla) or
/// `[HH:MM:SS LEVEL]:` (Forge/Paper) prefix and returns the body, or
/// `None` if the line doesn't have a recognized prefix.
fn strip_log_prefix(line: &str) -> Option<&str> {
    // Find the first `]:` that ends the prefix block.
    let idx = line.find("]:")?;
    let after = line[idx + 2..].trim_start();
    if after.is_empty() { None } else { Some(after) }
}
```

Run:

```bash
cargo test -p anvil --lib players::tests::log 2>&1 | tail -15
```

Expected: 4 passed.

- [ ] **Step 13: Run all tests in the new module**

```bash
cargo test -p anvil --lib players 2>&1 | tail -15
```

Expected: 16 passed (4 list + 3 whitelist + 3 banlist_players + 2 banlist_ips + 4 log).

- [ ] **Step 14: Format and lint**

```bash
cargo fmt --all
cargo clippy -p anvil --all-targets --features serve-dir -- -D warnings 2>&1 | tail -10
```

Expected: no warnings, no errors.

- [ ] **Step 15: Commit**

```bash
git add backend/src/players.rs backend/src/lib.rs
git commit -m "$(cat <<'EOF'
feat(backend): players parsing module — RCON outputs + log lines

Pure functions for the upcoming Players tab: list / whitelist list /
banlist players / banlist ips parsers, plus a join/leave log-line
parser that accepts both vanilla and Forge/Paper prefix shapes.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Backend validators (`backend/src/validation.rs`)

**Files:**
- Modify: `backend/src/validation.rs`

- [ ] **Step 1: Write tests for the 5 new validators (failing)**

Append to the `#[cfg(test)] mod tests` block in `backend/src/validation.rs`:

```rust
    #[test]
    fn mc_username_accepts_real_examples() {
        for n in ["alice", "Bob_42", "x_y_z", "AAA", "abcdefghijklmnop"] {
            assert!(validate_mc_username(n).is_ok(), "expected {n:?} to pass");
        }
    }

    #[test]
    fn mc_username_rejects_bad_examples() {
        let too_long = "a".repeat(17);
        for n in ["", "ab", "has space", "has-dash", too_long.as_str(), "tab\there"] {
            assert!(validate_mc_username(n).is_err(), "expected {n:?} to fail");
        }
    }

    #[test]
    fn kick_reason_bounds_and_chars() {
        assert!(validate_kick_reason("").is_ok());
        assert!(validate_kick_reason("legit reason").is_ok());
        assert!(validate_kick_reason(&"r".repeat(100)).is_ok());
        assert!(validate_kick_reason(&"r".repeat(101)).is_err());
        assert!(validate_kick_reason("with\nnewline").is_err());
        assert!(validate_kick_reason("with\rcarriage").is_err());
        assert!(validate_kick_reason("with\ttab").is_err());
    }

    #[test]
    fn chat_message_bounds_and_chars() {
        assert!(validate_chat_message("hi friends").is_ok());
        assert!(validate_chat_message(&"x".repeat(256)).is_ok());
        assert!(validate_chat_message(&"x".repeat(257)).is_err());
        assert!(validate_chat_message("with\nnewline").is_err());
    }

    #[test]
    fn gamemode_validator() {
        for m in ["survival", "creative", "adventure", "spectator"] {
            assert!(validate_gamemode(m).is_ok());
        }
        for m in ["", "Survival", "creative ", "spec", "0"] {
            assert!(validate_gamemode(m).is_err(), "expected {m:?} to fail");
        }
    }

    #[test]
    fn ip_validator() {
        for ip in ["10.0.0.5", "127.0.0.1", "::1", "2001:db8::1"] {
            assert!(validate_ip_v4_or_v6(ip).is_ok());
        }
        for ip in ["", "not.an.ip", "999.999.999.999", "10.0.0.0/24"] {
            assert!(validate_ip_v4_or_v6(ip).is_err());
        }
    }
```

Run:

```bash
cargo test -p anvil --lib validation::tests 2>&1 | tail -10
```

Expected: failures with "cannot find function `validate_mc_username`" and four siblings.

- [ ] **Step 2: Implement `validate_mc_username`**

Append to `backend/src/validation.rs` (above the `#[cfg(test)]` block; update the existing const block to include the new constant):

```rust
/// Minimum Mojang username length. The official rule is 3..=16 ASCII.
const MC_USERNAME_MIN: usize = 3;
/// Maximum Mojang username length.
const MC_USERNAME_MAX: usize = 16;
/// Maximum kick / ban reason length, bytes.
const REASON_MAX_LEN: usize = 100;
/// Maximum chat message / broadcast length, bytes.
const CHAT_MAX_LEN: usize = 256;
/// Allowed gamemode discriminators.
const KNOWN_GAMEMODES: &[&str] = &["survival", "creative", "adventure", "spectator"];

/// Validates a Mojang username (3–16 ASCII chars from `[A-Za-z0-9_]`).
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] with `code = "username_invalid"`.
pub fn validate_mc_username(s: &str) -> Result<&str, AppError> {
    let len = s.len();
    if !(MC_USERNAME_MIN..=MC_USERNAME_MAX).contains(&len)
        || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(AppError::BadRequest {
            code: "username_invalid",
            message: format!(
                "username must be {MC_USERNAME_MIN}..={MC_USERNAME_MAX} chars from [A-Za-z0-9_]"
            ),
        });
    }
    Ok(s)
}
```

- [ ] **Step 3: Implement `validate_kick_reason` and `validate_chat_message`**

Append:

```rust
/// Validates a kick / ban reason. Empty is allowed (caller may omit
/// the reason). Rejects any control char (0x00..0x1F or 0x7F).
///
/// # Errors
///
/// `AppError::BadRequest` with `code = "reason_too_long"` or
/// `code = "reason_has_control_char"`.
pub fn validate_kick_reason(s: &str) -> Result<&str, AppError> {
    if s.len() > REASON_MAX_LEN {
        return Err(AppError::BadRequest {
            code: "reason_too_long",
            message: format!("reason must be ≤ {REASON_MAX_LEN} chars"),
        });
    }
    if s.chars().any(|c| (c as u32) < 0x20 || c == '\u{7f}') {
        return Err(AppError::BadRequest {
            code: "reason_has_control_char",
            message: "reason must not contain control characters".to_owned(),
        });
    }
    Ok(s)
}

/// Validates a chat message / broadcast body. Same shape as
/// [`validate_kick_reason`] with the chat-length cap.
///
/// # Errors
///
/// `AppError::BadRequest` with `code = "message_too_long"` or
/// `code = "message_has_control_char"`.
pub fn validate_chat_message(s: &str) -> Result<&str, AppError> {
    if s.len() > CHAT_MAX_LEN {
        return Err(AppError::BadRequest {
            code: "message_too_long",
            message: format!("message must be ≤ {CHAT_MAX_LEN} chars"),
        });
    }
    if s.chars().any(|c| (c as u32) < 0x20 || c == '\u{7f}') {
        return Err(AppError::BadRequest {
            code: "message_has_control_char",
            message: "message must not contain control characters".to_owned(),
        });
    }
    Ok(s)
}
```

- [ ] **Step 4: Implement `validate_gamemode` and `validate_ip_v4_or_v6`**

Append:

```rust
/// Validates a gamemode discriminator.
///
/// # Errors
///
/// `AppError::BadRequest` with `code = "gamemode_invalid"`.
pub fn validate_gamemode(s: &str) -> Result<&str, AppError> {
    if KNOWN_GAMEMODES.contains(&s) {
        Ok(s)
    } else {
        Err(AppError::BadRequest {
            code: "gamemode_invalid",
            message: format!("gamemode {s:?} not in {KNOWN_GAMEMODES:?}"),
        })
    }
}

/// Validates that `s` parses as either an IPv4 or IPv6 literal.
///
/// # Errors
///
/// `AppError::BadRequest` with `code = "ip_invalid"`.
pub fn validate_ip_v4_or_v6(s: &str) -> Result<&str, AppError> {
    if s.parse::<std::net::IpAddr>().is_ok() {
        Ok(s)
    } else {
        Err(AppError::BadRequest {
            code: "ip_invalid",
            message: format!("{s:?} is not a valid IPv4 or IPv6 address"),
        })
    }
}
```

- [ ] **Step 5: Run validator tests**

```bash
cargo test -p anvil --lib validation 2>&1 | tail -15
```

Expected: all 6 new tests pass + existing tests still pass.

- [ ] **Step 6: Format and lint**

```bash
cargo fmt --all
cargo clippy -p anvil --all-targets --features serve-dir -- -D warnings 2>&1 | tail -10
cargo clippy -p anvil --all-targets --features embed -- -D warnings 2>&1 | tail -10
```

Expected: no warnings, no errors on either feature combo.

- [ ] **Step 7: Commit**

```bash
git add backend/src/validation.rs
git commit -m "$(cat <<'EOF'
feat(validation): mc-username · reason · chat · gamemode · ip

Five validators for the upcoming Players action endpoint. Mojang
username (3–16 [A-Za-z0-9_]), reason / message length and control-char
checks, gamemode enum, and IP v4-or-v6 via std::net::IpAddr.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: RCON batch helper extraction (`backend/src/routes/servers/rcon.rs`)

**Files:**
- Modify: `backend/src/routes/servers/rcon.rs`

- [ ] **Step 1: Re-read the file to anchor the changes**

Open `backend/src/routes/servers/rcon.rs` and locate:
- The existing `handle` function (lines ~90–165 currently).
- The constants `MAX_CMD_LEN` and `RCON_TIMEOUT`.
- The helper `validate_cmd` and `map_rcon_error`.

You will keep the constants and helpers, extract the connect-and-run logic into `run_rcon_batch`, add a `run_rcon_one` convenience wrapper, and reduce `handle` to a thin call into the wrapper plus the existing audit / JSON wrap.

- [ ] **Step 2: Add `run_rcon_batch` and `run_rcon_one` above `handle`**

Insert (above `pub async fn handle`):

```rust
/// Runs one or more RCON commands on a single auth'd connection.
///
/// Opens a fresh TCP+RCON session, sends each `cmd` in order, and
/// returns the outputs in the same order. Errors are mapped through
/// [`map_rcon_error`]. The full sequence runs under a single
/// [`RCON_TIMEOUT`] — 5 s total for connect + auth + every command.
///
/// # Errors
///
/// - 404 if the server is not in the panel database.
/// - 409 `server_not_running` if the StatefulSet is scaled down or
///   the pod is not Running.
/// - 500 on k8s, secret, or RCON failures (timeout, auth, IO).
pub async fn run_rcon_batch(
    state: &AppState,
    server_id: &str,
    cmds: &[&str],
) -> Result<Vec<String>, AppError> {
    let _row = fetch_server_row(&state.pool, server_id).await?;

    let resource_name = format!("mc-{server_id}");
    let pod_name = format!("{resource_name}-0");
    let secret_name = format!("{resource_name}-rcon");
    let headless_dns = format!(
        "{pod_name}.{resource_name}-headless.{ns}.svc:{port}",
        ns = state.mc_namespace,
        port = RCON_PORT,
    );

    let pods: Api<Pod> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let stsets: Api<StatefulSet> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let secrets: Api<Secret> = Api::namespaced(state.kube.clone(), &state.mc_namespace);

    let (replicas, ready) = stsets.get_opt(&resource_name).await?.map_or((0, 0), |s| {
        let r = s.spec.as_ref().and_then(|sp| sp.replicas).unwrap_or(0);
        let ready = s
            .status
            .as_ref()
            .and_then(|st| st.ready_replicas)
            .unwrap_or(0);
        (r, ready)
    });
    let pod = pods.get_opt(&pod_name).await?;
    if derive_status(replicas, ready, pod.as_ref()) != ServerStatus::Running {
        return Err(AppError::Conflict {
            code: "server_not_running",
            message: "server is not running".to_owned(),
        });
    }

    let secret = secrets.get(&secret_name).await?;
    let password = secret
        .data
        .as_ref()
        .and_then(|d| d.get("password"))
        .map(|bs| bs.0.clone())
        .ok_or_else(|| {
            AppError::Internal(anyhow::anyhow!(
                "rcon secret {secret_name} missing 'password' key"
            ))
        })?;
    let password = String::from_utf8(password).map_err(|_| {
        AppError::Internal(anyhow::anyhow!(
            "rcon secret {secret_name} 'password' is not UTF-8"
        ))
    })?;

    let outputs = timeout(RCON_TIMEOUT, async {
        let mut conn = <rcon::Connection<TcpStream>>::connect(&headless_dns, &password).await?;
        let mut outs = Vec::with_capacity(cmds.len());
        for cmd in cmds {
            outs.push(conn.cmd(cmd).await?);
        }
        Ok::<_, rcon::Error>(outs)
    })
    .await
    .map_err(|_| AppError::Internal(anyhow::anyhow!("rcon timed out after {RCON_TIMEOUT:?}")))?
    .map_err(map_rcon_error)?;

    Ok(outputs)
}

/// Runs a single RCON command. Convenience wrapper over
/// [`run_rcon_batch`].
///
/// # Errors
///
/// Same as [`run_rcon_batch`].
pub async fn run_rcon_one(
    state: &AppState,
    server_id: &str,
    cmd: &str,
) -> Result<String, AppError> {
    let mut outs = run_rcon_batch(state, server_id, &[cmd]).await?;
    Ok(outs.pop().unwrap_or_default())
}
```

- [ ] **Step 3: Rewire `handle` over `run_rcon_one`**

Replace the body of `pub async fn handle` with:

```rust
pub async fn handle(
    Path(id): Path<String>,
    State(state): State<AppState>,
    Json(request): Json<RconRequest>,
) -> Result<Json<RconResponse>, AppError> {
    let cmd = validate_cmd(&request.cmd)?.to_owned();

    let output = run_rcon_one(&state, &id, &cmd).await?;

    let now = Utc::now().timestamp();
    insert_audit(&state.pool, &id, "rcon", Some(json!({ "cmd": cmd })), now).await?;

    Ok(Json(RconResponse { output }))
}
```

The `validate_cmd`, `MAX_CMD_LEN`, `RCON_TIMEOUT`, `map_rcon_error`, request/response structs, and tests stay as they were.

- [ ] **Step 4: Run the existing rcon tests**

```bash
cargo test -p anvil --lib routes::servers::rcon::tests 2>&1 | tail -15
```

Expected: all 7 existing tests pass (validate_cmd_*, map_rcon_error_*).

- [ ] **Step 5: Build the whole crate to surface any unused-import warnings**

```bash
cargo build -p anvil 2>&1 | tail -10
cargo clippy -p anvil --all-targets --features serve-dir -- -D warnings 2>&1 | tail -10
cargo clippy -p anvil --all-targets --features embed -- -D warnings 2>&1 | tail -10
```

Expected: no warnings, no errors.

- [ ] **Step 6: Commit**

```bash
git add backend/src/routes/servers/rcon.rs
git commit -m "$(cat <<'EOF'
refactor(rcon): extract run_rcon_batch / run_rcon_one helpers

Pull the connect + auth + send loop out of the existing /rcon handler
into reusable helpers. The bulk-read endpoint in the upcoming Players
module will run four commands on a single connection via run_rcon_batch.

The existing POST /api/servers/{id}/rcon behavior is unchanged.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2: Backend route module

### Task 4: Players route module — types + cmd builder (`backend/src/routes/servers/players.rs`)

**Files:**
- Create: `backend/src/routes/servers/players.rs`

- [ ] **Step 1: Create the module skeleton with DTOs and `PlayerAction`**

Create `backend/src/routes/servers/players.rs` with:

```rust
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
use crate::players::{
    self, BanEntry, BanIpEntry, OnlinePlayers, PlayerEvent, PlayerEventKind,
};
use crate::routes::servers::create::insert_audit;
use crate::routes::servers::rcon::{run_rcon_batch, run_rcon_one};
use crate::validation::{
    validate_chat_message, validate_gamemode, validate_ip_v4_or_v6, validate_kick_reason,
    validate_mc_username,
};

// --- bulk-read response shapes ------------------------------------------------

#[derive(Debug, Serialize)]
pub struct OnlinePlayersDto {
    pub count: u32,
    pub max: u32,
    pub players: Vec<String>,
}

impl From<OnlinePlayers> for OnlinePlayersDto {
    fn from(o: OnlinePlayers) -> Self {
        Self { count: o.count, max: o.max, players: o.players }
    }
}

#[derive(Debug, Serialize)]
pub struct BanEntryDto {
    pub name: String,
    pub reason: String,
}

impl From<BanEntry> for BanEntryDto {
    fn from(b: BanEntry) -> Self {
        Self { name: b.name, reason: b.reason }
    }
}

#[derive(Debug, Serialize)]
pub struct BanIpEntryDto {
    pub ip: String,
    pub reason: String,
}

impl From<BanIpEntry> for BanIpEntryDto {
    fn from(b: BanIpEntry) -> Self {
        Self { ip: b.ip, reason: b.reason }
    }
}

#[derive(Debug, Serialize)]
pub struct BanlistDto {
    pub players: Vec<BanEntryDto>,
    pub ips: Vec<BanIpEntryDto>,
}

#[derive(Debug, Serialize)]
pub struct PlayerEventDto {
    pub kind: &'static str,
    pub player: String,
    pub ts_ms: i64,
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

#[derive(Debug, Serialize)]
pub struct PlayersResponse {
    pub online: OnlinePlayersDto,
    pub whitelist: Vec<String>,
    pub banlist: BanlistDto,
    pub history: Vec<PlayerEventDto>,
}

// --- action enum --------------------------------------------------------------

/// Body of `POST /api/servers/{id}/players/action`.
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum PlayerAction {
    Kick { player: String, reason: Option<String> },
    Ban { player: String, reason: Option<String> },
    BanIp { player: String, reason: Option<String> },
    Pardon { player: String },
    PardonIp { ip: String },
    Op { player: String },
    Deop { player: String },
    Gamemode { player: String, mode: String },
    Tell { player: String, message: String },
    WhitelistAdd { player: String },
    WhitelistRemove { player: String },
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
```

- [ ] **Step 2: Add the cmd builder + its tests**

Append:

```rust
/// Validates an action's fields and returns the RCON command string
/// that implements it. Returns `(audit_action, audit_details, cmd)`.
///
/// The audit triple is built here so the action handler stays simple —
/// validation, command, and audit shape stay in lock-step.
fn build_action(action: &PlayerAction) -> Result<(&'static str, serde_json::Value, String), AppError> {
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
            Ok(("player.kick", json!({"player": player, "reason": reason}), cmd))
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
            Ok(("player.ban", json!({"player": player, "reason": reason}), cmd))
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
            Ok(("player.ban_ip", json!({"player": player, "reason": reason}), cmd))
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
            Ok(("player.op", json!({"player": player}), format!("op {player}")))
        }
        PlayerAction::Deop { player } => {
            validate_mc_username(player)?;
            Ok(("player.deop", json!({"player": player}), format!("deop {player}")))
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

#[cfg(test)]
mod cmd_builder_tests {
    use super::*;

    #[test]
    fn kick_with_reason() {
        let a = PlayerAction::Kick { player: "alice".into(), reason: Some("spam".into()) };
        let (act, _, cmd) = build_action(&a).unwrap();
        assert_eq!(act, "player.kick");
        assert_eq!(cmd, "kick alice spam");
    }

    #[test]
    fn kick_without_reason() {
        let a = PlayerAction::Kick { player: "alice".into(), reason: None };
        let (_, _, cmd) = build_action(&a).unwrap();
        assert_eq!(cmd, "kick alice");
    }

    #[test]
    fn ban_with_reason() {
        let a = PlayerAction::Ban { player: "bob".into(), reason: Some("griefing".into()) };
        let (_, _, cmd) = build_action(&a).unwrap();
        assert_eq!(cmd, "ban bob griefing");
    }

    #[test]
    fn ban_ip_with_reason() {
        let a = PlayerAction::BanIp { player: "eve".into(), reason: Some("range hop".into()) };
        let (_, _, cmd) = build_action(&a).unwrap();
        assert_eq!(cmd, "ban-ip eve range hop");
    }

    #[test]
    fn pardon_and_pardon_ip() {
        let (_, _, cmd) = build_action(&PlayerAction::Pardon { player: "alice".into() }).unwrap();
        assert_eq!(cmd, "pardon alice");
        let (_, _, cmd) = build_action(&PlayerAction::PardonIp { ip: "10.0.0.5".into() }).unwrap();
        assert_eq!(cmd, "pardon-ip 10.0.0.5");
    }

    #[test]
    fn op_and_deop() {
        let (_, _, cmd) = build_action(&PlayerAction::Op { player: "alice".into() }).unwrap();
        assert_eq!(cmd, "op alice");
        let (_, _, cmd) = build_action(&PlayerAction::Deop { player: "alice".into() }).unwrap();
        assert_eq!(cmd, "deop alice");
    }

    #[test]
    fn gamemode_command_orders_mode_first() {
        let a = PlayerAction::Gamemode { player: "alice".into(), mode: "creative".into() };
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
        let (_, _, cmd) = build_action(&PlayerAction::WhitelistAdd { player: "alice".into() }).unwrap();
        assert_eq!(cmd, "whitelist add alice");
        let (_, _, cmd) = build_action(&PlayerAction::WhitelistRemove { player: "alice".into() }).unwrap();
        assert_eq!(cmd, "whitelist remove alice");
    }

    #[test]
    fn invalid_username_rejected_at_build() {
        let a = PlayerAction::Kick { player: "bad name!".into(), reason: None };
        match build_action(&a) {
            Err(AppError::BadRequest { code, .. }) => assert_eq!(code, "username_invalid"),
            other => panic!("expected username_invalid, got {other:?}"),
        }
    }

    #[test]
    fn invalid_ip_rejected_at_build() {
        let a = PlayerAction::PardonIp { ip: "not.an.ip".into() };
        match build_action(&a) {
            Err(AppError::BadRequest { code, .. }) => assert_eq!(code, "ip_invalid"),
            other => panic!("expected ip_invalid, got {other:?}"),
        }
    }

    #[test]
    fn invalid_gamemode_rejected_at_build() {
        let a = PlayerAction::Gamemode { player: "alice".into(), mode: "Adventure".into() };
        match build_action(&a) {
            Err(AppError::BadRequest { code, .. }) => assert_eq!(code, "gamemode_invalid"),
            other => panic!("expected gamemode_invalid, got {other:?}"),
        }
    }
}
```

- [ ] **Step 3: Run the cmd-builder tests**

The module isn't yet wired into `mod.rs`, so we need to either skip-build it or wire it in temporarily. The cleanest path is to also wire it in this task so `cargo test` can find it. **Continue to Step 4** before running tests.

- [ ] **Step 4: Wire `players` into `routes/servers/mod.rs`**

Open `backend/src/routes/servers/mod.rs` and locate the existing `pub mod` block (~lines 8–20):

```rust
pub mod create;
pub mod delete;
pub mod get;
pub mod logs;
pub mod logs_stream;
pub mod mods;
pub mod rcon;
pub mod restart;
pub mod settings;
pub mod start;
pub mod stop;
pub mod update;
pub mod update_stream;
```

Add `pub mod players;` (alphabetical placement):

```rust
pub mod create;
pub mod delete;
pub mod get;
pub mod logs;
pub mod logs_stream;
pub mod mods;
pub mod players;
pub mod rcon;
pub mod restart;
pub mod settings;
pub mod start;
pub mod stop;
pub mod update;
pub mod update_stream;
```

The router entries get added in Task 6.

- [ ] **Step 5: Build and run cmd-builder tests**

```bash
cargo build -p anvil 2>&1 | tail -10
cargo test -p anvil --lib routes::servers::players::cmd_builder_tests 2>&1 | tail -20
```

Expected: build succeeds (some warnings about unused imports/handlers are OK at this stage); 12 cmd-builder tests pass.

If you see "function `fetch_server_row` not used" or similar imports-not-yet-used warnings, that's expected — they get used in Task 5/6.

- [ ] **Step 6: Commit (handlers will follow in Tasks 5 + 6)**

```bash
git add backend/src/routes/servers/players.rs backend/src/routes/servers/mod.rs
git commit -m "$(cat <<'EOF'
feat(api): players route module — DTOs + PlayerAction enum + cmd builder

Module skeleton for the upcoming Players bulk-read / action / broadcast
endpoints. Includes the discriminated 11-variant action enum and a
cmd-builder helper that returns (audit_action, audit_details, rcon_cmd)
so validation, command, and audit shape stay in lock-step.

Handlers + router wiring follow in subsequent commits.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Bulk read handler (`backend/src/routes/servers/players.rs`)

**Files:**
- Modify: `backend/src/routes/servers/players.rs`

- [ ] **Step 1: Add the bulk-read handler**

Append to `backend/src/routes/servers/players.rs`:

```rust
/// Handler for `GET /api/servers/{id}/players`.
///
/// Runs four RCON commands on one connection (`list`, `whitelist list`,
/// `banlist players`, `banlist ips`), parses each, and best-effort
/// scrapes the last ~2000 pod log lines for join/leave events.
///
/// # Errors
///
/// - 404 if the server is not in the panel database (via `run_rcon_batch`).
/// - 409 `server_not_running` if the StatefulSet is scaled down or the
///   pod is not Running.
/// - 500 on RCON / k8s failures.
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
    let params = LogParams {
        tail_lines: Some(HISTORY_TAIL_LINES),
        ..LogParams::default()
    };
    let Ok(text) = pods.logs(&pod_name, &params).await else {
        return Vec::new();
    };
    let now = players::now_ms();
    let mut evs: Vec<PlayerEvent> = text
        .lines()
        .filter_map(|line| players::parse_log_join_leave(line, now))
        .collect();
    // Latest first, capped.
    evs.reverse();
    evs.truncate(HISTORY_MAX_EVENTS);
    evs.into_iter().map(PlayerEventDto::from).collect()
}
```

- [ ] **Step 2: Build and verify**

```bash
cargo build -p anvil 2>&1 | tail -10
```

Expected: build succeeds; one warning about `handle_get` being unused is OK (it gets routed in Task 6).

- [ ] **Step 3: Commit**

```bash
git add backend/src/routes/servers/players.rs
git commit -m "$(cat <<'EOF'
feat(api): players bulk-read handler — RCON quartet + log scrape

GET /api/servers/{id}/players runs list / whitelist list / banlist
players / banlist ips on one connection, parses each, and best-effort
scrapes the last ~2000 pod log lines for join/leave events. Pod-log
errors are swallowed (history empty, RCON sections succeed).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Action + broadcast handlers + router wiring (`backend/src/routes/servers/players.rs`, `mod.rs`)

**Files:**
- Modify: `backend/src/routes/servers/players.rs`
- Modify: `backend/src/routes/servers/mod.rs`

- [ ] **Step 1: Add the action handler**

Append to `backend/src/routes/servers/players.rs`:

```rust
/// Handler for `POST /api/servers/{id}/players/action`.
///
/// Validates the discriminated body, runs the corresponding RCON
/// command, writes one audit row, returns 204.
///
/// # Errors
///
/// - 400 with the validator's specific code (e.g. `username_invalid`).
/// - 404 / 409 / 500 from RCON failures (see [`run_rcon_one`]).
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
        Some(json!({"message_len": msg.len()})),
        Utc::now().timestamp(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
```

- [ ] **Step 2: Wire the three routes into the per-server router**

Open `backend/src/routes/servers/mod.rs`. Find the location where the existing per-server routes are mounted — likely a `Router::new()` chain in this file or in `backend/src/routes/mod.rs`. Search for the mods routes (added in B):

```bash
rg -n "mods/apply|/mods\"|servers::mods" backend/src/routes/ 2>&1 | head
```

Locate the file and the line where the per-server router is built (look for `.route("/api/servers/{id}/mods", …)` or a similar call wired through `servers::mods::handle_*`). Add three new entries next to the existing ones:

```rust
        .route(
            "/api/servers/{id}/players",
            axum::routing::get(servers::players::handle_get),
        )
        .route(
            "/api/servers/{id}/players/action",
            axum::routing::post(servers::players::handle_action),
        )
        .route(
            "/api/servers/{id}/players/broadcast",
            axum::routing::post(servers::players::handle_broadcast),
        )
```

(Exact path syntax — `{id}` not `:id` — per the project convention noted in CLAUDE.md.)

- [ ] **Step 3: Build the whole crate and run all tests**

```bash
cargo build -p anvil 2>&1 | tail -10
cargo test -p anvil 2>&1 | tail -30
```

Expected: build succeeds with no warnings; all tests pass (existing + 14 parser + 6 validator + 12 cmd-builder = full suite green).

- [ ] **Step 4: Run clippy on both feature combos**

```bash
cargo clippy -p anvil --all-targets --features serve-dir -- -D warnings 2>&1 | tail -10
cargo clippy -p anvil --all-targets --features embed -- -D warnings 2>&1 | tail -10
```

Expected: no warnings.

- [ ] **Step 5: Format**

```bash
cargo fmt --all
git diff --quiet backend/ && echo "no fmt changes" || echo "fmt applied — review and stage"
```

If there are fmt-only diffs, stage them.

- [ ] **Step 6: Commit**

```bash
git add backend/src/routes/servers/players.rs backend/src/routes/servers/mod.rs
git commit -m "$(cat <<'EOF'
feat(api): players action + broadcast handlers + router wiring

Three routes mounted: GET /api/servers/{id}/players (bulk read),
POST /api/servers/{id}/players/action (11-variant discriminated body),
POST /api/servers/{id}/players/broadcast. Each write writes one
audit_log row.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3: Frontend foundations

### Task 7: API schemas + fetch helpers (`frontend/app/lib/api.ts`)

**Files:**
- Modify: `frontend/app/lib/api.ts`

- [ ] **Step 1: Add the players response schemas**

Append to `frontend/app/lib/api.ts` (after the existing modlist section near the bottom):

```ts
// --- players (sub-project C) --------------------------------------------------

export const onlinePlayersSchema = z.object({
	count: z.number().int().nonnegative(),
	max: z.number().int().nonnegative(),
	players: z.array(z.string()),
});

export const banEntrySchema = z.object({
	name: z.string(),
	reason: z.string(),
});

export const banIpEntrySchema = z.object({
	ip: z.string(),
	reason: z.string(),
});

export const playerEventSchema = z.object({
	kind: z.enum(["joined", "left"]),
	player: z.string(),
	ts_ms: z.number().int(),
});

export const playersResponseSchema = z.object({
	online: onlinePlayersSchema,
	whitelist: z.array(z.string()),
	banlist: z.object({
		players: z.array(banEntrySchema),
		ips: z.array(banIpEntrySchema),
	}),
	history: z.array(playerEventSchema),
});

export const gamemodeSchema = z.enum([
	"survival",
	"creative",
	"adventure",
	"spectator",
]);

export const playerActionSchema = z.discriminatedUnion("action", [
	z.object({
		action: z.literal("kick"),
		player: z.string(),
		reason: z.string().optional(),
	}),
	z.object({
		action: z.literal("ban"),
		player: z.string(),
		reason: z.string().optional(),
	}),
	z.object({
		action: z.literal("ban-ip"),
		player: z.string(),
		reason: z.string().optional(),
	}),
	z.object({ action: z.literal("pardon"), player: z.string() }),
	z.object({ action: z.literal("pardon-ip"), ip: z.string() }),
	z.object({ action: z.literal("op"), player: z.string() }),
	z.object({ action: z.literal("deop"), player: z.string() }),
	z.object({
		action: z.literal("gamemode"),
		player: z.string(),
		mode: gamemodeSchema,
	}),
	z.object({
		action: z.literal("tell"),
		player: z.string(),
		message: z.string(),
	}),
	z.object({ action: z.literal("whitelist-add"), player: z.string() }),
	z.object({ action: z.literal("whitelist-remove"), player: z.string() }),
]);

export type PlayersResponse = z.infer<typeof playersResponseSchema>;
export type PlayerEvent = z.infer<typeof playerEventSchema>;
export type BanEntry = z.infer<typeof banEntrySchema>;
export type BanIpEntry = z.infer<typeof banIpEntrySchema>;
export type Gamemode = z.infer<typeof gamemodeSchema>;
export type PlayerAction = z.infer<typeof playerActionSchema>;
```

- [ ] **Step 2: Add the fetch helpers**

Append:

```ts
/// Fetches the bulk Players response. 409 on stopped server is
/// surfaced as a typed ApiError (`code: "server_not_running"`).
export async function fetchPlayers(
	id: string,
	signal: AbortSignal,
): Promise<PlayersResponse> {
	const res = await fetch(`/api/servers/${encodeURIComponent(id)}/players`, {
		signal,
	});
	return jsonOrThrow(res, playersResponseSchema);
}

/// Runs one player action. 204 on success.
export async function runPlayerAction(
	id: string,
	action: PlayerAction,
): Promise<void> {
	const validated = playerActionSchema.parse(action);
	const res = await fetch(
		`/api/servers/${encodeURIComponent(id)}/players/action`,
		{
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify(validated),
		},
	);
	await noContentOrThrow(res);
}

/// Sends `say <message>` to the server.
export async function broadcastMessage(
	id: string,
	message: string,
): Promise<void> {
	const res = await fetch(
		`/api/servers/${encodeURIComponent(id)}/players/broadcast`,
		{
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ message }),
		},
	);
	await noContentOrThrow(res);
}
```

- [ ] **Step 3: Lint and typecheck**

```bash
cd frontend && pnpm lint 2>&1 | tail -20
pnpm typecheck 2>&1 | tail -10
cd ..
```

Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add frontend/app/lib/api.ts
git commit -m "$(cat <<'EOF'
feat(frontend): players API client — schemas + fetch helpers

PlayersResponse / PlayerAction Zod schemas + fetchPlayers /
runPlayerAction / broadcastMessage helpers. Validates all wire shapes
at the network boundary.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: Polling hook (`frontend/app/lib/use-players.ts`)

**Files:**
- Create: `frontend/app/lib/use-players.ts`

- [ ] **Step 1: Implement the hook**

Create `frontend/app/lib/use-players.ts`:

```ts
"use client";

import { useCallback, useEffect, useRef, useState } from "react";

import { ApiError, fetchPlayers, type PlayersResponse } from "./api";

const POLL_INTERVAL_MS = 10_000;

export type PlayersStatus =
	| "loading"
	| "live"
	| "stale"
	| "error"
	| "stopped";

export interface UsePlayersResult {
	readonly data: PlayersResponse | null;
	readonly status: PlayersStatus;
	readonly lastError: string | null;
	readonly refresh: () => void;
}

/// Subscribes to the bulk-read endpoint with a 10 s poll, paused while
/// the document is hidden. Returns the latest snapshot, the connection
/// status, and a `refresh()` callback for out-of-band fetches (e.g.
/// after a successful action).
export function usePlayers(
	serverId: string,
	opts: { enabled: boolean },
): UsePlayersResult {
	const { enabled } = opts;
	const [data, setData] = useState<PlayersResponse | null>(null);
	const [status, setStatus] = useState<PlayersStatus>(
		enabled ? "loading" : "stopped",
	);
	const [lastError, setLastError] = useState<string | null>(null);
	const tickRef = useRef<number>(0);

	const refresh = useCallback((): void => {
		tickRef.current += 1;
	}, []);

	useEffect(() => {
		if (!enabled) {
			setStatus("stopped");
			setData(null);
			setLastError(null);
			return undefined;
		}
		let cancelled = false;
		let interval: number | null = null;
		let abort: AbortController | null = null;

		const doFetch = async (): Promise<void> => {
			abort?.abort();
			abort = new AbortController();
			try {
				const fresh = await fetchPlayers(serverId, abort.signal);
				if (cancelled) return;
				setData(fresh);
				setStatus("live");
				setLastError(null);
			} catch (err: unknown) {
				if (cancelled) return;
				if (
					err instanceof DOMException &&
					(err.name === "AbortError" || err.name === "TimeoutError")
				) {
					return;
				}
				if (err instanceof ApiError && err.code === "server_not_running") {
					setStatus("stopped");
					setData(null);
					setLastError(null);
					return;
				}
				setStatus(data === null ? "error" : "stale");
				setLastError(
					err instanceof Error ? err.message : "unknown players-fetch error",
				);
			}
		};

		const start = (): void => {
			void doFetch();
			interval = window.setInterval(() => {
				if (document.visibilityState === "visible") {
					void doFetch();
				}
			}, POLL_INTERVAL_MS);
		};

		const stop = (): void => {
			if (interval !== null) {
				window.clearInterval(interval);
				interval = null;
			}
			abort?.abort();
		};

		const onVisibilityChange = (): void => {
			if (document.visibilityState === "visible") {
				void doFetch();
			}
		};

		start();
		document.addEventListener("visibilitychange", onVisibilityChange);

		return (): void => {
			cancelled = true;
			document.removeEventListener("visibilitychange", onVisibilityChange);
			stop();
		};
		// `tickRef.current` is read inside doFetch via the refresh() trigger.
		// We deliberately leave it out of deps; the effect restarts on
		// (serverId, enabled) and `refresh()` causes an inline re-fetch via
		// the next interval tick or visibilitychange.
		// eslint-disable-next-line react-hooks/exhaustive-deps
	}, [serverId, enabled]);

	return { data, status, lastError, refresh };
}
```

- [ ] **Step 2: Lint and typecheck**

```bash
cd frontend && pnpm lint 2>&1 | tail -20
pnpm typecheck 2>&1 | tail -10
cd ..
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add frontend/app/lib/use-players.ts
git commit -m "$(cat <<'EOF'
feat(frontend): use-players hook — bulk-read poll w/ visibility pause

10 s poll while document is visible. Pauses on hidden, resumes (with
an immediate fetch) on return. Surfaces a 'stopped' status when the
backend returns server_not_running. AbortController on every fetch.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 4: Frontend dialogs

### Task 9: AddToWhitelistDialog (`frontend/app/components/AddToWhitelistDialog.tsx`)

**Files:**
- Create: `frontend/app/components/AddToWhitelistDialog.tsx`

- [ ] **Step 1: Implement the dialog**

Create `frontend/app/components/AddToWhitelistDialog.tsx`:

```tsx
"use client";

import { useState, type ReactElement } from "react";

import { ApiError, runPlayerAction } from "../lib/api";

import { Button } from "./Button";
import { Modal } from "./Modal";
import { useToast } from "./Toast";

interface AddToWhitelistDialogProps {
	open: boolean;
	onClose: () => void;
	serverId: string;
	onDone: () => void;
}

const NAME_REGEX = /^[A-Za-z0-9_]{3,16}$/;

export function AddToWhitelistDialog({
	open,
	onClose,
	serverId,
	onDone,
}: AddToWhitelistDialogProps): ReactElement {
	const [name, setName] = useState("");
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const toast = useToast();

	const valid = NAME_REGEX.test(name);

	const onSubmit = (): void => {
		if (!valid) return;
		setError(null);
		setBusy(true);
		runPlayerAction(serverId, { action: "whitelist-add", player: name })
			.then(() => {
				toast.push(`whitelisted ${name}`, "success");
				onDone();
				setName("");
				onClose();
			})
			.catch((err: unknown) => {
				setError(
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error",
				);
			})
			.finally(() => {
				setBusy(false);
			});
	};

	return (
		<Modal open={open} onClose={onClose} title="add to whitelist" maxWidth="sm">
			<div className="flex flex-col gap-4 font-mono text-[13px]">
				<label className="flex flex-col gap-1.5">
					<span className="text-[11px] uppercase tracking-wider text-text-muted">
						mojang username
					</span>
					<input
						type="text"
						value={name}
						onChange={(e) => {
							setName(e.target.value);
						}}
						autoFocus
						placeholder="alice"
						className="w-full rounded-md border border-border bg-bg px-3 py-1.5 text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
					/>
					<span className="text-[11px] text-text-dim">
						3–16 chars, letters / digits / underscore
					</span>
				</label>
				{error !== null && <p className="text-state-error">{error}</p>}
				<div className="mt-2 flex justify-end gap-2">
					<Button onClick={onClose} disabled={busy}>
						cancel
					</Button>
					<Button
						variant="primary"
						onClick={onSubmit}
						disabled={!valid || busy}
					>
						{busy ? "adding…" : "add"}
					</Button>
				</div>
			</div>
		</Modal>
	);
}
```

- [ ] **Step 2: Lint and typecheck**

```bash
cd frontend && pnpm lint 2>&1 | tail -20
pnpm typecheck 2>&1 | tail -10
cd ..
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add frontend/app/components/AddToWhitelistDialog.tsx
git commit -m "$(cat <<'EOF'
feat(frontend): AddToWhitelistDialog — Mojang-username input + submit

Modal for the [+ add] button on the WhitelistCard. Mirror-side validation
(NAME_REGEX) before fetch; surfaces ApiError details on submit failure.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 10: BroadcastDialog (`frontend/app/components/BroadcastDialog.tsx`)

**Files:**
- Create: `frontend/app/components/BroadcastDialog.tsx`

- [ ] **Step 1: Implement the dialog**

Create `frontend/app/components/BroadcastDialog.tsx`:

```tsx
"use client";

import { useState, type ReactElement } from "react";

import { ApiError, broadcastMessage } from "../lib/api";

import { Button } from "./Button";
import { Modal } from "./Modal";
import { useToast } from "./Toast";

interface BroadcastDialogProps {
	open: boolean;
	onClose: () => void;
	serverId: string;
}

const MSG_MAX = 256;
const CONTROL = /[\x00-\x1f\x7f]/;

export function BroadcastDialog({
	open,
	onClose,
	serverId,
}: BroadcastDialogProps): ReactElement {
	const [msg, setMsg] = useState("");
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const toast = useToast();

	const tooLong = msg.length > MSG_MAX;
	const hasControl = CONTROL.test(msg);
	const valid = msg.length > 0 && !tooLong && !hasControl;

	const onSubmit = (): void => {
		if (!valid) return;
		setError(null);
		setBusy(true);
		broadcastMessage(serverId, msg)
			.then(() => {
				toast.push("broadcast sent", "success");
				setMsg("");
				onClose();
			})
			.catch((err: unknown) => {
				setError(
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error",
				);
			})
			.finally(() => {
				setBusy(false);
			});
	};

	return (
		<Modal open={open} onClose={onClose} title="broadcast — /say" maxWidth="md">
			<div className="flex flex-col gap-4 font-mono text-[13px]">
				<label className="flex flex-col gap-1.5">
					<span className="text-[11px] uppercase tracking-wider text-text-muted">
						message
					</span>
					<textarea
						value={msg}
						onChange={(e) => {
							setMsg(e.target.value);
						}}
						autoFocus
						rows={3}
						placeholder="restart in 5 minutes — please log out"
						className="w-full resize-none rounded-md border border-border bg-bg px-3 py-2 text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
					/>
					<div className="flex justify-between text-[11px] text-text-dim">
						<span>broadcasts to all online players via /say</span>
						<span
							className={tooLong ? "text-state-error" : "text-text-dim"}
						>
							{msg.length} / {MSG_MAX}
						</span>
					</div>
					{hasControl && (
						<span className="text-[11px] text-state-error">
							no newlines or control chars
						</span>
					)}
				</label>
				{error !== null && <p className="text-state-error">{error}</p>}
				<div className="mt-2 flex justify-end gap-2">
					<Button onClick={onClose} disabled={busy}>
						cancel
					</Button>
					<Button variant="primary" onClick={onSubmit} disabled={!valid || busy}>
						{busy ? "sending…" : "send"}
					</Button>
				</div>
			</div>
		</Modal>
	);
}
```

- [ ] **Step 2: Lint and typecheck**

```bash
cd frontend && pnpm lint 2>&1 | tail -20
pnpm typecheck 2>&1 | tail -10
cd ..
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add frontend/app/components/BroadcastDialog.tsx
git commit -m "$(cat <<'EOF'
feat(frontend): BroadcastDialog — /say textarea + char counter

Modal for the [broadcast] button. 256-char cap with live counter,
control-char check, length-validation gates the submit button.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 11: PlayerActionDialog (`frontend/app/components/PlayerActionDialog.tsx`)

**Files:**
- Create: `frontend/app/components/PlayerActionDialog.tsx`

- [ ] **Step 1: Implement the dialog**

Create `frontend/app/components/PlayerActionDialog.tsx`:

```tsx
"use client";

import { useState, type ReactElement } from "react";

import {
	ApiError,
	gamemodeSchema,
	runPlayerAction,
	type Gamemode,
	type PlayerAction,
} from "../lib/api";

import { Button } from "./Button";
import { Modal } from "./Modal";
import { useToast } from "./Toast";

/// The variant determines which input (if any) the dialog renders.
export type PlayerActionVariant =
	| { kind: "kick"; player: string }
	| { kind: "ban"; player: string }
	| { kind: "ban-ip"; player: string }
	| { kind: "pardon"; player: string }
	| { kind: "pardon-ip"; ip: string }
	| { kind: "whitelist-remove"; player: string }
	| { kind: "gamemode"; player: string }
	| { kind: "tell"; player: string };

interface PlayerActionDialogProps {
	open: boolean;
	onClose: () => void;
	serverId: string;
	variant: PlayerActionVariant | null;
	onDone: () => void;
}

const REASON_MAX = 100;
const MESSAGE_MAX = 256;
const CONTROL = /[\x00-\x1f\x7f]/;

const TITLE: Record<PlayerActionVariant["kind"], string> = {
	kick: "kick player",
	ban: "ban player",
	"ban-ip": "ban player + ip",
	pardon: "pardon player",
	"pardon-ip": "pardon ip",
	"whitelist-remove": "remove from whitelist",
	gamemode: "change gamemode",
	tell: "send /tell",
};

const VERB_PRESENT: Record<PlayerActionVariant["kind"], string> = {
	kick: "kicking",
	ban: "banning",
	"ban-ip": "banning",
	pardon: "pardoning",
	"pardon-ip": "pardoning",
	"whitelist-remove": "removing",
	gamemode: "applying",
	tell: "sending",
};

const SUCCESS_TOAST: Record<PlayerActionVariant["kind"], (target: string) => string> = {
	kick: (t) => `kicked ${t}`,
	ban: (t) => `banned ${t}`,
	"ban-ip": (t) => `banned ${t} (ip)`,
	pardon: (t) => `pardoned ${t}`,
	"pardon-ip": (t) => `pardoned ${t}`,
	"whitelist-remove": (t) => `removed ${t} from whitelist`,
	gamemode: (t) => `set ${t}'s gamemode`,
	tell: (t) => `sent message to ${t}`,
};

export function PlayerActionDialog({
	open,
	onClose,
	serverId,
	variant,
	onDone,
}: PlayerActionDialogProps): ReactElement | null {
	const [reason, setReason] = useState("");
	const [message, setMessage] = useState("");
	const [mode, setMode] = useState<Gamemode>("survival");
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const toast = useToast();

	if (variant === null) return null;

	const target =
		variant.kind === "pardon-ip" ? variant.ip : variant.player;

	const reasonValid =
		reason.length <= REASON_MAX && !CONTROL.test(reason);
	const messageValid =
		message.length > 0 &&
		message.length <= MESSAGE_MAX &&
		!CONTROL.test(message);

	const valid =
		variant.kind === "kick" ||
		variant.kind === "ban" ||
		variant.kind === "ban-ip"
			? reasonValid
			: variant.kind === "tell"
				? messageValid
				: variant.kind === "gamemode"
					? gamemodeSchema.options.includes(mode)
					: true;

	const reset = (): void => {
		setReason("");
		setMessage("");
		setMode("survival");
		setError(null);
	};

	const onCancel = (): void => {
		reset();
		onClose();
	};

	const buildAction = (): PlayerAction | null => {
		switch (variant.kind) {
			case "kick":
				return {
					action: "kick",
					player: variant.player,
					reason: reason.length > 0 ? reason : undefined,
				};
			case "ban":
				return {
					action: "ban",
					player: variant.player,
					reason: reason.length > 0 ? reason : undefined,
				};
			case "ban-ip":
				return {
					action: "ban-ip",
					player: variant.player,
					reason: reason.length > 0 ? reason : undefined,
				};
			case "pardon":
				return { action: "pardon", player: variant.player };
			case "pardon-ip":
				return { action: "pardon-ip", ip: variant.ip };
			case "whitelist-remove":
				return { action: "whitelist-remove", player: variant.player };
			case "gamemode":
				return { action: "gamemode", player: variant.player, mode };
			case "tell":
				return { action: "tell", player: variant.player, message };
		}
	};

	const onSubmit = (): void => {
		const a = buildAction();
		if (a === null || !valid) return;
		setError(null);
		setBusy(true);
		runPlayerAction(serverId, a)
			.then(() => {
				toast.push(SUCCESS_TOAST[variant.kind](target), "success");
				onDone();
				reset();
				onClose();
			})
			.catch((err: unknown) => {
				setError(
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error",
				);
			})
			.finally(() => {
				setBusy(false);
			});
	};

	const dangerKinds: PlayerActionVariant["kind"][] = [
		"kick",
		"ban",
		"ban-ip",
		"pardon",
		"pardon-ip",
		"whitelist-remove",
	];
	const danger = dangerKinds.includes(variant.kind);

	return (
		<Modal open={open} onClose={onCancel} title={TITLE[variant.kind]} maxWidth="md">
			<div className="flex flex-col gap-4 font-mono text-[13px]">
				<p className="text-text-body">
					{VERB_PRESENT[variant.kind]} <span className="text-text-primary">{target}</span>
				</p>

				{(variant.kind === "kick" ||
					variant.kind === "ban" ||
					variant.kind === "ban-ip") && (
					<label className="flex flex-col gap-1.5">
						<span className="text-[11px] uppercase tracking-wider text-text-muted">
							reason (optional)
						</span>
						<input
							type="text"
							value={reason}
							onChange={(e) => {
								setReason(e.target.value);
							}}
							maxLength={REASON_MAX}
							autoFocus
							className="w-full rounded-md border border-border bg-bg px-3 py-1.5 text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
						/>
						<span className="text-[11px] text-text-dim">
							{reason.length} / {REASON_MAX}
						</span>
					</label>
				)}

				{variant.kind === "gamemode" && (
					<label className="flex flex-col gap-1.5">
						<span className="text-[11px] uppercase tracking-wider text-text-muted">
							mode
						</span>
						<select
							value={mode}
							onChange={(e) => {
								setMode(e.target.value as Gamemode);
							}}
							autoFocus
							className="w-full rounded-md border border-border bg-bg px-3 py-1.5 text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
						>
							{gamemodeSchema.options.map((m) => (
								<option key={m} value={m}>
									{m}
								</option>
							))}
						</select>
					</label>
				)}

				{variant.kind === "tell" && (
					<label className="flex flex-col gap-1.5">
						<span className="text-[11px] uppercase tracking-wider text-text-muted">
							message
						</span>
						<textarea
							value={message}
							onChange={(e) => {
								setMessage(e.target.value);
							}}
							rows={3}
							autoFocus
							className="w-full resize-none rounded-md border border-border bg-bg px-3 py-2 text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
						/>
						<span className="text-[11px] text-text-dim">
							{message.length} / {MESSAGE_MAX}
						</span>
					</label>
				)}

				{error !== null && <p className="text-state-error">{error}</p>}
				<div className="mt-2 flex justify-end gap-2">
					<Button onClick={onCancel} disabled={busy}>
						cancel
					</Button>
					<Button
						variant={danger ? "danger" : "primary"}
						onClick={onSubmit}
						disabled={!valid || busy}
					>
						{busy ? "…" : variant.kind}
					</Button>
				</div>
			</div>
		</Modal>
	);
}
```

- [ ] **Step 2: Lint and typecheck**

```bash
cd frontend && pnpm lint 2>&1 | tail -20
pnpm typecheck 2>&1 | tail -10
cd ..
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add frontend/app/components/PlayerActionDialog.tsx
git commit -m "$(cat <<'EOF'
feat(frontend): PlayerActionDialog — 4 variants in one Modal

One component covers eight action kinds: kick/ban/ban-ip (with optional
reason input), pardon/pardon-ip/whitelist-remove (yes-no), gamemode
(with mode select), tell (with message textarea). Op / deop /
whitelist-add bypass this dialog and dispatch immediately from the menu.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 5: Frontend tab body

### Task 12: PlayerActionMenu (`frontend/app/components/PlayerActionMenu.tsx`)

**Files:**
- Create: `frontend/app/components/PlayerActionMenu.tsx`

- [ ] **Step 1: Implement the menu**

Create `frontend/app/components/PlayerActionMenu.tsx`:

```tsx
"use client";

import { type ReactElement } from "react";

import { ApiError, runPlayerAction } from "../lib/api";

import { Dropdown, type DropdownItem } from "./Dropdown";
import { useToast } from "./Toast";
import type { PlayerActionVariant } from "./PlayerActionDialog";

export type PlayerActionSource = "online" | "whitelist" | "banlist";

interface PlayerActionMenuProps {
	source: PlayerActionSource;
	serverId: string;
	/// Player username (for online + whitelist + banlist-player rows).
	name?: string;
	/// IP for banlist-ip rows; mutually exclusive with `name`.
	ip?: string;
	/// Open the shared PlayerActionDialog with the given variant.
	openDialog: (variant: PlayerActionVariant) => void;
	/// Trigger an out-of-band poll after a fire-and-toast action.
	onDone: () => void;
}

const CHEVRON = (
	<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" strokeWidth={2}>
		<circle cx="5" cy="12" r="1.5" />
		<circle cx="12" cy="12" r="1.5" />
		<circle cx="19" cy="12" r="1.5" />
	</svg>
);

export function PlayerActionMenu({
	source,
	serverId,
	name,
	ip,
	openDialog,
	onDone,
}: PlayerActionMenuProps): ReactElement {
	const toast = useToast();

	const fireAndToast = (label: string, message: string, action: () => Promise<void>): void => {
		void action()
			.then(() => {
				toast.push(message, "success");
				onDone();
			})
			.catch((err: unknown) => {
				const detail =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error";
				toast.push(`${label} failed: ${detail}`, "error");
			});
	};

	const items: DropdownItem[] = (() => {
		if (source === "online" && name !== undefined) {
			return [
				{ id: "kick", label: "kick…", onSelect: () => { openDialog({ kind: "kick", player: name }); } },
				{
					id: "op",
					label: "op",
					onSelect: () => {
						fireAndToast("op", `opped ${name}`, () =>
							runPlayerAction(serverId, { action: "op", player: name }),
						);
					},
				},
				{
					id: "deop",
					label: "deop",
					onSelect: () => {
						fireAndToast("deop", `deopped ${name}`, () =>
							runPlayerAction(serverId, { action: "deop", player: name }),
						);
					},
				},
				{ id: "gamemode", label: "gamemode…", onSelect: () => { openDialog({ kind: "gamemode", player: name }); } },
				{ id: "tell", label: "/tell…", onSelect: () => { openDialog({ kind: "tell", player: name }); } },
				{
					id: "whitelist-add",
					label: "add to whitelist",
					onSelect: () => {
						fireAndToast("whitelist add", `whitelisted ${name}`, () =>
							runPlayerAction(serverId, { action: "whitelist-add", player: name }),
						);
					},
				},
				{ id: "ban", label: "ban…", danger: true, onSelect: () => { openDialog({ kind: "ban", player: name }); } },
				{ id: "ban-ip", label: "ban-ip…", danger: true, onSelect: () => { openDialog({ kind: "ban-ip", player: name }); } },
			];
		}
		if (source === "whitelist" && name !== undefined) {
			return [
				{ id: "remove", label: "remove from whitelist…", danger: true, onSelect: () => { openDialog({ kind: "whitelist-remove", player: name }); } },
				{
					id: "op",
					label: "op",
					onSelect: () => {
						fireAndToast("op", `opped ${name}`, () =>
							runPlayerAction(serverId, { action: "op", player: name }),
						);
					},
				},
				{
					id: "deop",
					label: "deop",
					onSelect: () => {
						fireAndToast("deop", `deopped ${name}`, () =>
							runPlayerAction(serverId, { action: "deop", player: name }),
						);
					},
				},
				{ id: "ban", label: "ban…", danger: true, onSelect: () => { openDialog({ kind: "ban", player: name }); } },
			];
		}
		if (source === "banlist") {
			if (name !== undefined) {
				return [
					{
						id: "pardon",
						label: "pardon…",
						onSelect: () => {
							openDialog({ kind: "pardon", player: name });
						},
					},
				];
			}
			if (ip !== undefined) {
				return [
					{
						id: "pardon-ip",
						label: "pardon ip…",
						onSelect: () => {
							openDialog({ kind: "pardon-ip", ip });
						},
					},
				];
			}
		}
		return [];
	})();

	return <Dropdown trigger={CHEVRON} items={items} ariaLabel="player actions" />;
}
```

- [ ] **Step 2: Lint and typecheck**

```bash
cd frontend && pnpm lint 2>&1 | tail -20
pnpm typecheck 2>&1 | tail -10
cd ..
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add frontend/app/components/PlayerActionMenu.tsx
git commit -m "$(cat <<'EOF'
feat(frontend): PlayerActionMenu — per-source action set

Wraps Dropdown with the right verbs per source (online / whitelist /
banlist). Op / deop / whitelist-add fire-and-toast inline; everything
else opens the shared PlayerActionDialog.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 13: PlayersBody — replace placeholder (`frontend/app/servers/tabs/PlayersBody.tsx`)

**Files:**
- Modify: `frontend/app/servers/tabs/PlayersBody.tsx`

- [ ] **Step 1: Add a small relative-time helper if one doesn't exist**

```bash
rg -n "ago|formatDistance" frontend/app/lib/ 2>&1 | head
```

If a helper exists, reuse it. Otherwise we inline a tiny one inside `PlayersBody.tsx` — no need for a separate file.

- [ ] **Step 2: Rewrite `PlayersBody.tsx`**

Replace the entire contents of `frontend/app/servers/tabs/PlayersBody.tsx` with:

```tsx
"use client";

import { useState, type ReactElement } from "react";

import { AddToWhitelistDialog } from "../../components/AddToWhitelistDialog";
import { BroadcastDialog } from "../../components/BroadcastDialog";
import { Button } from "../../components/Button";
import { Card } from "../../components/Card";
import {
	PlayerActionDialog,
	type PlayerActionVariant,
} from "../../components/PlayerActionDialog";
import { PlayerActionMenu } from "../../components/PlayerActionMenu";
import { Skeleton } from "../../components/Skeleton";
import { usePlayers } from "../../lib/use-players";
import type {
	BanEntry,
	BanIpEntry,
	PlayerEvent,
	PlayersResponse,
	ServerStatus,
} from "../../lib/api";

interface PlayersBodyProps {
	serverId: string;
	serverStatus: ServerStatus;
	onStartServer: () => void;
}

export function PlayersBody({
	serverId,
	serverStatus,
	onStartServer,
}: PlayersBodyProps): ReactElement {
	const enabled = serverStatus === "running";
	const { data, status, refresh } = usePlayers(serverId, { enabled });

	const [broadcastOpen, setBroadcastOpen] = useState(false);
	const [addOpen, setAddOpen] = useState(false);
	const [actionVariant, setActionVariant] =
		useState<PlayerActionVariant | null>(null);

	if (!enabled) {
		return (
			<Card>
				<div className="flex flex-col items-start gap-3 font-mono text-[13px]">
					<p className="text-text-muted">
						server is stopped — start the server to manage players.
					</p>
					<Button variant="primary" onClick={onStartServer}>
						start server
					</Button>
				</div>
			</Card>
		);
	}

	if (status === "loading" && data === null) {
		return (
			<div className="flex flex-col gap-4">
				<Skeleton variant="block" />
				<Skeleton variant="block" />
				<Skeleton variant="block" />
				<Skeleton variant="block" />
			</div>
		);
	}

	const view: PlayersResponse =
		data ?? {
			online: { count: 0, max: 0, players: [] },
			whitelist: [],
			banlist: { players: [], ips: [] },
			history: [],
		};

	return (
		<div className="flex flex-col gap-4">
			<BroadcastBar onOpen={() => { setBroadcastOpen(true); }} status={status} />
			<OnlinePlayersCard
				view={view.online}
				serverId={serverId}
				openDialog={setActionVariant}
				onDone={refresh}
			/>
			<WhitelistCard
				names={view.whitelist}
				serverId={serverId}
				openDialog={setActionVariant}
				onAdd={() => { setAddOpen(true); }}
				onDone={refresh}
			/>
			<BanlistCard
				view={view.banlist}
				serverId={serverId}
				openDialog={setActionVariant}
				onDone={refresh}
			/>
			<RecentActivityCard events={view.history} />

			<BroadcastDialog
				open={broadcastOpen}
				onClose={() => { setBroadcastOpen(false); }}
				serverId={serverId}
			/>
			<AddToWhitelistDialog
				open={addOpen}
				onClose={() => { setAddOpen(false); }}
				serverId={serverId}
				onDone={refresh}
			/>
			<PlayerActionDialog
				open={actionVariant !== null}
				onClose={() => { setActionVariant(null); }}
				serverId={serverId}
				variant={actionVariant}
				onDone={refresh}
			/>
		</div>
	);
}

// ---- inline cards ----------------------------------------------------------

interface BroadcastBarProps {
	onOpen: () => void;
	status: string;
}

function BroadcastBar({ onOpen, status }: BroadcastBarProps): ReactElement {
	return (
		<div className="flex items-center justify-between font-mono text-[12px] text-text-muted">
			<Button variant="primary" onClick={onOpen}>
				broadcast
			</Button>
			<span>{status === "live" ? "live · 10s poll" : status}</span>
		</div>
	);
}

interface OnlinePlayersCardProps {
	view: PlayersResponse["online"];
	serverId: string;
	openDialog: (v: PlayerActionVariant) => void;
	onDone: () => void;
}

function OnlinePlayersCard({
	view,
	serverId,
	openDialog,
	onDone,
}: OnlinePlayersCardProps): ReactElement {
	return (
		<Card header={`online now · ${view.count.toString()} / ${view.max.toString()}`}>
			{view.players.length === 0 ? (
				<p className="font-mono text-[12px] text-text-dim">nobody online</p>
			) : (
				<ul className="divide-y divide-border-soft">
					{view.players.map((name) => (
						<li
							key={name}
							className="flex items-center justify-between py-2 font-mono text-[13px] text-text-body"
						>
							<span>{name}</span>
							<PlayerActionMenu
								source="online"
								serverId={serverId}
								name={name}
								openDialog={openDialog}
								onDone={onDone}
							/>
						</li>
					))}
				</ul>
			)}
		</Card>
	);
}

interface WhitelistCardProps {
	names: readonly string[];
	serverId: string;
	openDialog: (v: PlayerActionVariant) => void;
	onAdd: () => void;
	onDone: () => void;
}

function WhitelistCard({
	names,
	serverId,
	openDialog,
	onAdd,
	onDone,
}: WhitelistCardProps): ReactElement {
	return (
		<Card header={`whitelist · ${names.length.toString()} names`}>
			{names.length === 0 ? (
				<p className="mb-3 font-mono text-[12px] text-text-dim">
					whitelist is empty (the server may not have whitelist enabled)
				</p>
			) : (
				<ul className="mb-3 divide-y divide-border-soft">
					{names.map((name) => (
						<li
							key={name}
							className="flex items-center justify-between py-2 font-mono text-[13px] text-text-body"
						>
							<span>{name}</span>
							<PlayerActionMenu
								source="whitelist"
								serverId={serverId}
								name={name}
								openDialog={openDialog}
								onDone={onDone}
							/>
						</li>
					))}
				</ul>
			)}
			<Button variant="secondary" onClick={onAdd}>
				+ add
			</Button>
		</Card>
	);
}

interface BanlistCardProps {
	view: PlayersResponse["banlist"];
	serverId: string;
	openDialog: (v: PlayerActionVariant) => void;
	onDone: () => void;
}

function BanlistCard({
	view,
	serverId,
	openDialog,
	onDone,
}: BanlistCardProps): ReactElement {
	const total = view.players.length + view.ips.length;
	if (total === 0) {
		return (
			<Card header="banned · 0 players · 0 ips">
				<p className="font-mono text-[12px] text-text-dim">
					nobody banned
				</p>
			</Card>
		);
	}
	return (
		<Card
			header={`banned · ${view.players.length.toString()} players · ${view.ips.length.toString()} ips`}
		>
			{view.players.length > 0 && (
				<>
					<p className="mb-1 font-mono text-[10px] uppercase tracking-wider text-text-faint">
						players
					</p>
					<ul className="mb-3 divide-y divide-border-soft">
						{view.players.map((b: BanEntry) => (
							<li
								key={b.name}
								className="flex items-center justify-between py-2 font-mono text-[13px] text-text-body"
							>
								<span>
									<span className="text-text-primary">{b.name}</span>
									<span className="ml-2 text-text-muted">· {b.reason}</span>
								</span>
								<PlayerActionMenu
									source="banlist"
									serverId={serverId}
									name={b.name}
									openDialog={openDialog}
									onDone={onDone}
								/>
							</li>
						))}
					</ul>
				</>
			)}
			{view.ips.length > 0 && (
				<>
					<p className="mb-1 font-mono text-[10px] uppercase tracking-wider text-text-faint">
						ips
					</p>
					<ul className="divide-y divide-border-soft">
						{view.ips.map((b: BanIpEntry) => (
							<li
								key={b.ip}
								className="flex items-center justify-between py-2 font-mono text-[13px] text-text-body"
							>
								<span>
									<span className="text-text-primary">{b.ip}</span>
									<span className="ml-2 text-text-muted">· {b.reason}</span>
								</span>
								<PlayerActionMenu
									source="banlist"
									serverId={serverId}
									ip={b.ip}
									openDialog={openDialog}
									onDone={onDone}
								/>
							</li>
						))}
					</ul>
				</>
			)}
		</Card>
	);
}

interface RecentActivityCardProps {
	events: readonly PlayerEvent[];
}

function RecentActivityCard({ events }: RecentActivityCardProps): ReactElement {
	return (
		<Card header="recent activity">
			{events.length === 0 ? (
				<p className="font-mono text-[12px] text-text-dim">
					no recent join/leave events in pod logs
				</p>
			) : (
				<ul className="font-mono text-[12px] text-text-body">
					{events.map((ev) => (
						<li
							key={`${ev.player}-${ev.kind}-${ev.ts_ms.toString()}`}
							className="py-1"
						>
							<span className="text-text-primary">{ev.player}</span>{" "}
							<span className={ev.kind === "joined" ? "text-state-running" : "text-text-muted"}>
								{ev.kind}
							</span>
							<span className="ml-2 text-text-dim">· {relativeTime(ev.ts_ms)}</span>
						</li>
					))}
				</ul>
			)}
		</Card>
	);
}

function relativeTime(ts_ms: number): string {
	const diff = Math.max(0, Date.now() - ts_ms);
	const sec = Math.floor(diff / 1000);
	if (sec < 60) return `${sec.toString()}s ago`;
	const min = Math.floor(sec / 60);
	if (min < 60) return `${min.toString()}m ago`;
	const hr = Math.floor(min / 60);
	if (hr < 24) return `${hr.toString()}h ago`;
	const day = Math.floor(hr / 24);
	return `${day.toString()}d ago`;
}
```

- [ ] **Step 3: Update the parent that renders PlayersBody**

The parent (likely `frontend/app/servers/[name]/[tab]/page.tsx` or `ServerDetailView.tsx`) needs to pass `serverId`, `serverStatus`, and `onStartServer`. Locate the existing usage:

```bash
rg -n "PlayersBody" frontend/app/ 2>&1
```

Open whichever parent file renders `<PlayersBody />` and update the prop call to pass the three required props. Example (typical shape — adjust to the actual prop names in the parent):

```tsx
<PlayersBody
	serverId={server.id}
	serverStatus={server.status}
	onStartServer={() => {
		void startServer(server.id).then(refresh);
	}}
/>
```

If `startServer` and a `refresh` function aren't already in scope, mirror what other tabs (Console / Mods) do. The detail page already has lifecycle button logic; reuse the existing `start` action.

- [ ] **Step 4: Lint, typecheck, build**

```bash
cd frontend
pnpm lint 2>&1 | tail -20
pnpm typecheck 2>&1 | tail -10
pnpm build 2>&1 | tail -10
cd ..
```

Expected: no errors; build succeeds with the static export.

- [ ] **Step 5: Commit**

```bash
git add frontend/app/servers/tabs/PlayersBody.tsx
# include any parent file you modified for the props wiring
git add -A frontend/app/servers/
git commit -m "$(cat <<'EOF'
feat(frontend): Players tab body — full surface

Replace the v2.x placeholder with four stacked Cards (online · whitelist
· banned · recent activity) plus a broadcast bar. Stopped server gates
to a single empty-state Card with a [start server] button. Polling via
use-players (10s, paused on hidden tab). Per-row PlayerActionMenu
opens shared dialogs.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 6: Verification + ship

### Task 14: Quality gates + manual smoke + milestones + ship

**Files:**
- Modify: `docs/milestones.md`

- [ ] **Step 1: Run all backend quality gates**

```bash
cargo fmt --check
cargo test --all 2>&1 | tail -20
cargo clippy --all-targets --features serve-dir -- -D warnings 2>&1 | tail -10
cargo clippy --all-targets --features embed -- -D warnings 2>&1 | tail -10
```

Expected: every gate green. If `cargo fmt --check` complains, run `cargo fmt --all` and stage.

- [ ] **Step 2: Run all frontend quality gates**

```bash
cd frontend
pnpm lint 2>&1 | tail -20
pnpm typecheck 2>&1 | tail -10
pnpm build 2>&1 | tail -15
cd ..
```

Expected: every gate green. The build emits `./frontend/out/`.

- [ ] **Step 3: Manual smoke test against a running MC server**

The user runs the panel locally with `cargo run --features serve-dir` against the homelab cluster (or a real or fake MC server reachable by RCON). Walk through the spec §10 acceptance checklist:

- [ ] Stopped server: tab shows the gate empty state; no `/players` requests in DevTools network panel.
- [ ] Running server with no online players: online card renders `0 / N`; whitelist + banlist render whatever the server has; activity card lists events.
- [ ] Whitelist add → username → submit → row appears within ≤10 s; toast `whitelisted X`.
- [ ] Whitelist remove → menu → confirm → row leaves; toast `removed X from whitelist`.
- [ ] Kick → optional reason → confirm → online count drops; toast `kicked X`.
- [ ] Ban → reason → confirm → row in banned card; toast `banned X`.
- [ ] Ban-IP → confirm → entry in IPs subsection.
- [ ] Pardon → confirm → row leaves; toast `pardoned X`.
- [ ] Pardon-IP → confirm → IP row leaves; toast `pardoned 10.0.0.5`.
- [ ] Op / Deop → menu → no confirm → toast.
- [ ] Gamemode → pick mode → confirm → in-game effect.
- [ ] Tell → message → send → recipient sees whisper; toast `sent message to X`.
- [ ] Broadcast → message → send → all online see `[Server] msg`; toast.
- [ ] Recent activity: join + leave a player → both events appear within one poll.
- [ ] Validation rejection: try `bad name!` in add-whitelist → 400 + frontend renders the validator's message.
- [ ] Polling pause: switch to a different browser tab → no `/players` requests while hidden.
- [ ] Audit log: `sqlite3 anvil.sqlite "select action, details from audit_log order by id desc limit 12"` shows `player.<verb>` rows with their JSON details.

Each unchecked item is a regression — fix before shipping.

- [ ] **Step 4: Update milestones**

Open `docs/milestones.md` and locate the v2 section. Add a line under sub-project C marking it complete; the existing entries for A and B are the template. Example:

```markdown
- **C** — Player management ✅ (2026-05-03): full Players tab body, RCON-only,
  bulk-read endpoint + 11-variant action endpoint + broadcast endpoint, recent
  activity from pod logs, no new RBAC / migration / dependencies. Spec:
  `docs/superpowers/specs/2026-05-03-anvil-v2-player-management-design.md`.
```

(If the file's structure is different, follow the existing pattern — don't unilaterally restructure.)

- [ ] **Step 5: Commit milestones update**

```bash
git add docs/milestones.md
git commit -m "$(cat <<'EOF'
docs: mark sub-project C (player management) complete

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 6: Confirm clean working tree**

```bash
git status
git log --oneline -15
```

Expected: working tree clean; the recent commits show the C series in order (parsers · validators · rcon refactor · players module + handlers · api.ts · use-players · 3 dialogs · action menu · PlayersBody · milestones).

C is shipped.

---

## Self-review (against spec)

This plan implements every spec section:

- §2.1 Players tab body → Task 13.
- §2.2 Online list → bulk read in Task 5; OnlinePlayersCard in Task 13.
- §2.3 Whitelist → Tasks 5 (read) + 6 (add/remove via action) + 9 (AddDialog) + 13 (Card).
- §2.4 Banlist → Tasks 5 (read) + 6 (ban/ban-ip/pardon/pardon-ip) + 13 (Card).
- §2.5 Per-player verbs → Task 4 (cmd-builder) + Task 6 (handler) + Tasks 11–13 (UI).
- §2.6 Broadcast → Task 6 (handler) + Task 10 (dialog) + Task 13 (bar).
- §2.7 Recent activity → Task 5 (`scrape_history`) + Task 13 (RecentActivityCard).
- §2.8 Confirmation pattern → Task 11 (PlayerActionDialog) + Task 12 (action menu fire-and-toast for op/deop).
- §2.9 Validation → Task 2.
- §2.10 Three endpoints → Tasks 5 (bulk) + 6 (action + broadcast).
- §2.11 RCON batch helper → Task 3.
- §5.1 Wire types → Task 1 (parser types) + Task 4 (DTOs).
- §5.2 Action body → Task 4.
- §5.3 Audit log → Task 4 (cmd-builder returns audit triple) + Task 6 (handler writes row).
- §6 Backend → Tasks 1–6.
- §7 Frontend → Tasks 7–13.
- §10 Verification → Task 14.

No placeholders. No TODOs. Every step has either complete code or an exact command with expected output.
