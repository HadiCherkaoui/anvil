# Anvil v2 — Player Management (Sub-project C)

**Date:** 2026-05-03
**Author:** Hadi (with Claude as scribe)
**Status:** Brainstormed — ready for an implementation plan
**Sub-project:** C of {A · Foundation, B · Mod ecosystem, C · Player management, D · File browser sidecar}

---

## 1. Context

A's foundation rehaul (M6) shipped on 2026-05-03 and intentionally
left the **Players** tab body as a one-line placeholder
(`frontend/app/servers/tabs/PlayersBody.tsx:7`). B filled the **Mods**
tab on the same day. C is the third leg: a working Players surface
inspired by Crafty Controller's player management — kick / ban / op /
whitelist / chat — but tailored for anvil's audience.

Driving constraint: ONE cluster, ~3 friends, internal use, NOT a
SaaS. Reuse the v2 design tokens, primitives, and patterns from A
and B. Do not pull in new RBAC capabilities, dependencies, or DB
migrations.

---

## 2. Scope

**In scope:**

1. **Players tab body** — replaces the placeholder Card. Renders for
   every source kind that exposes RCON (vanilla, paper, modded,
   curseforge, modrinth).
2. **Online list** — `RCON list` parsed into `{count, max, players}`,
   polled every 10 s while the tab is visible.
3. **Whitelist read/write** — `RCON whitelist list` for the read,
   `whitelist add/remove` via the action endpoint. (Modern MC
   auto-reloads `whitelist.json` on add/remove — no `whitelist
   reload` needed.)
4. **Banlist read/write** — `RCON banlist players` and `banlist ips`
   for the read, `ban / ban-ip / pardon / pardon-ip` via the action
   endpoint.
5. **Per-player verbs** — kick, ban, ban-ip, pardon, pardon-ip, op,
   deop, gamemode, /tell. Eleven discriminated variants, one
   endpoint.
6. **Broadcast** — `RCON say <message>` via a dedicated endpoint
   (no player target).
7. **Recent activity** — last ~50 join/leave events parsed from the
   pod logs scraped on each poll. No persistence.
8. **Confirmation pattern** — yes-no `Modal` for destructive verbs
   (kick / ban / ban-ip / pardon / pardon-ip / whitelist-remove).
   Type-name confirmation stays reserved for delete-server. Op /
   deop / whitelist-add (from row context) skip confirmation
   entirely. Gamemode and tell open a `Modal` to collect input
   (mode select, message body) and use that submit click as
   implicit confirmation. `Toast` on every success.
9. **Validation** — Mojang username regex (3–16, `[A-Za-z0-9_]`),
   kick/ban reason cap (≤100, no control chars), chat message cap
   (≤256, no control chars), gamemode enum, IP v4/v6 for pardon-ip.
10. **Bulk-read endpoint + single-action endpoint + broadcast
    endpoint** — three new routes total.
11. **RCON batch helper** — extract `run_rcon_batch` / `run_rcon_one`
    from the existing single-shot `rcon::handle` so the bulk read
    runs four commands on one connection.

**Out of scope (explicitly deferred or excluded):**

- **Ops list surface.** Vanilla MC has no `op list` RCON command.
  Reading `/data/ops.json` would require `pods/exec` RBAC; we choose
  the smallest blast radius. Op / deop remain available as per-row
  actions; the user knows who's op because they assigned it. Defer
  the ops-list view to D's file browser sidecar.
- **Background log-tail task / `player_events` table.** History is
  served on demand from the pod's existing log stream. No new state.
- **Per-player session stats / playtime / leaderboards / geo-IP /
  activity charts.** Crafty Controller territory; not warranted for
  4 friends.
- **Teleport, kill, weather, time, gamerule, datapack management.**
  Available in-game via `/op`'d users; no need for panel surface.
- **Multi-server bulk operations** ("kick X from all servers").
  YAGNI.
- **Sub-project D** — File browser sidecar.

---

## 3. Anti-overengineering guardrails

- **RCON-only.** No `pods/exec`, no file-system read into pod
  volumes. The `pods/log` capability we already use suffices for
  history. No new k8s RBAC.
- **No DB migration.** Player data lives on the MC server; the panel
  is a thin RCON wrapper plus a log-line scraper.
- **No new primitives.** `PlayerActionMenu` / `PlayerActionDialog` /
  `AddToWhitelistDialog` / `BroadcastDialog` are *components*, not
  *primitives* — they compose `Dropdown`, `Modal`, `Button`, `Toast`.
- **No background tasks.** History scrape runs synchronously inside
  the bulk-read handler. The polling cadence is the throttle.
- **One action endpoint, not eleven.** Discriminated body with 11
  variants in `players.rs` dispatches to the right RCON command.
  Keeps audit-log writes and validation in one place.
- **One RCON connection per bulk read.** `run_rcon_batch` opens once,
  runs four commands, closes — saves three TCP+auth handshakes per
  poll.
- **No live join/leave WebSocket.** The existing `/logs/stream` is
  not extended. The 10 s poll is the throttle.

---

## 4. Design POV

Reuse A's tokens 1:1. Copper accent only on:

- the `[broadcast]` button bracket (primary CTA inside the tab)
- the per-row action-menu chevron when the menu is open
- the `[+ add]` button bracket on the whitelist Card

Mono (`Fira Code`) for usernames, IPs, and counts; sans (`Fira Sans`)
for labels and copy. State colors stay state-only — banned rows do
*not* render in `--color-state-error`; they render mono with the
reason in `--color-text-muted`. No new colors. No new fonts.

The Players tab is a *workshop tool for an op*: scannable,
keyboard-friendly, no dashboard chrome. Each Card has a tight header
line and a list. Empty states are one short line of copy.

---

## 5. Data model

**No DB changes.** All wire data is derived from RCON and pod logs
on each request.

### 5.1 Wire types

```rust
// backend/src/players.rs (parsing types — no Serialize)
pub struct OnlinePlayers { pub count: u32, pub max: u32, pub players: Vec<String> }
pub struct BanEntry      { pub name: String, pub reason: String }
pub struct BanIpEntry    { pub ip:   String, pub reason: String }
pub enum   PlayerEventKind { Joined, Left }
pub struct PlayerEvent   { pub kind: PlayerEventKind, pub player: String, pub ts_ms: i64 }

// backend/src/routes/servers/players.rs (Serialize for the wire)
#[derive(Serialize)]
pub struct PlayersResponse {
    pub online:    OnlinePlayersDto,
    pub whitelist: Vec<String>,
    pub banlist:   BanlistDto,
    pub history:   Vec<PlayerEventDto>,
}
```

### 5.2 Action body (request side)

```rust
#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum PlayerAction {
    Kick           { player: String, reason: Option<String> },
    Ban            { player: String, reason: Option<String> },
    BanIp          { player: String, reason: Option<String> },
    Pardon         { player: String },
    PardonIp       { ip: String },
    Op             { player: String },
    Deop           { player: String },
    Gamemode       { player: String, mode: String },
    Tell           { player: String, message: String },
    WhitelistAdd   { player: String },
    WhitelistRemove{ player: String },
}
```

### 5.3 Audit log

Each action variant writes one row via the existing `insert_audit`
helper in `backend/src/routes/servers/create.rs`:

| Action variant     | Audit `action` value     | `details` JSON                                |
|--------------------|--------------------------|-----------------------------------------------|
| Kick               | `player.kick`            | `{"player":"X","reason":"…"}`                 |
| Ban                | `player.ban`             | `{"player":"X","reason":"…"}`                 |
| BanIp              | `player.ban_ip`          | `{"player":"X","reason":"…"}`                 |
| Pardon             | `player.pardon`          | `{"player":"X"}`                              |
| PardonIp           | `player.pardon_ip`       | `{"ip":"…"}`                                  |
| Op                 | `player.op`              | `{"player":"X"}`                              |
| Deop               | `player.deop`            | `{"player":"X"}`                              |
| Gamemode           | `player.gamemode`        | `{"player":"X","mode":"creative"}`            |
| Tell               | `player.tell`            | `{"player":"X","message_len":42}` (no body)   |
| WhitelistAdd       | `player.whitelist_add`   | `{"player":"X"}`                              |
| WhitelistRemove    | `player.whitelist_remove`| `{"player":"X"}`                              |
| (broadcast)        | `player.broadcast`       | `{"message_len":42}` (no body)                |

Tell and broadcast omit message bodies from the audit log on
purpose — they may carry private chat the panel owner doesn't want
mirrored.

---

## 6. Backend

### 6.1 New parsing module — `backend/src/players.rs`

Pure functions over RCON output strings. No I/O. No `kube`. Easy to
unit-test.

```rust
pub fn parse_list_output(s: &str) -> OnlinePlayers;
pub fn parse_whitelist_output(s: &str) -> Vec<String>;
pub fn parse_banlist_players_output(s: &str) -> Vec<BanEntry>;
pub fn parse_banlist_ips_output(s: &str) -> Vec<BanIpEntry>;
pub fn parse_log_join_leave(line: &str) -> Option<PlayerEvent>;
```

Real-world MC output samples to cover in tests:

| Command | Empty output | Populated output |
|---|---|---|
| `list` | `There are 0 of a max of 20 players online:` | `There are 2 of a max of 20 players online: alice, bob` |
| `whitelist list` | `There are no whitelisted players` | `There are 3 whitelisted players: alice, bob, charlie` |
| `banlist players` | `There are no bans` | `There are 2 bans:\nalice was banned by Server: spam.\nbob was banned by Server: griefing.` |
| `banlist ips` | `There are no IP bans` | `There are 1 IP ban:\n10.0.0.5 was banned by Server: …` |
| log join | n/a | `[01:23:45] [Server thread/INFO]: alice joined the game` (also `[01:23:45 INFO]:` shape on some Forge variants — handle both) |
| log leave | n/a | `[01:23:45] [Server thread/INFO]: alice left the game` |

The `[HH:MM:SS]` prefix gives us a wall-clock time but no date —
combine with the pod's log line ingestion time (best effort: now() at
parse time, so events come back in correct relative order even if
absolute ts is off by ≤ poll interval).

### 6.2 Validation additions — `backend/src/validation.rs`

```rust
pub fn validate_mc_username(s: &str) -> Result<&str, AppError>;
// 3..=16 bytes, regex ^[A-Za-z0-9_]+$. Mojang's official rule.

pub fn validate_kick_reason(s: &str) -> Result<&str, AppError>;
// 0..=100 bytes; reject any char in 0x00..0x1F or 0x7F (control chars
// including \n, \r, \t). Whitespace at the ends is trimmed before check.

pub fn validate_chat_message(s: &str) -> Result<&str, AppError>;
// 0..=256 bytes; same control-char rejection as kick_reason.

pub fn validate_gamemode(s: &str) -> Result<&str, AppError>;
// in {"survival","creative","adventure","spectator"}.

pub fn validate_ip_v4_or_v6(s: &str) -> Result<&str, AppError>;
// std::net::IpAddr::from_str — accepts both families.
```

All five emit `AppError::BadRequest { code, message }` with stable
kebab-case codes (`username_invalid`, `reason_too_long`,
`reason_has_control_char`, `message_too_long`, `gamemode_invalid`,
`ip_invalid`).

### 6.3 RCON batch helper — extracted from `backend/src/routes/servers/rcon.rs`

```rust
/// Opens one RCON connection, runs every cmd in order, returns the
/// outputs in order. All under one timeout. Single secret read,
/// single status gate.
pub async fn run_rcon_batch(
    state: &AppState,
    server_id: &str,
    cmds: &[&str],
) -> Result<Vec<String>, AppError>;

/// Convenience for the single-cmd case. Wraps `run_rcon_batch`.
pub async fn run_rcon_one(
    state: &AppState,
    server_id: &str,
    cmd: &str,
) -> Result<String, AppError>;
```

The existing `handle` for `POST /api/servers/{id}/rcon` becomes:

```rust
let trimmed = validate_cmd(&request.cmd)?;
let output = run_rcon_one(&state, &id, trimmed).await?;
insert_audit(&state.pool, &id, "rcon", Some(json!({"cmd": trimmed})), Utc::now().timestamp()).await?;
Ok(Json(RconResponse { output }))
```

Behavior unchanged for that endpoint. The single 5 s timeout
covers the connect + auth + every command. An MC server handling 4
trivial commands stays well under 5 s in practice.

### 6.4 New route module — `backend/src/routes/servers/players.rs`

Three handlers:

```text
GET    /api/servers/{id}/players           bulk read
POST   /api/servers/{id}/players/action    discriminated body
POST   /api/servers/{id}/players/broadcast { message }
```

#### Bulk read implementation

```text
1. Status gate: server must be Running. Reuse derive_status truth table.
   On stopped → 409 server_not_running, no further work.
2. run_rcon_batch(["list","whitelist list","banlist players","banlist ips"])
3. parse each output via the players.rs parsers
4. Best-effort: pods.logs(name, &LogParams { tail_lines: Some(2000), ..default() })
   - On error, history = []. Bulk read succeeds.
   - On success, parse_log_join_leave each line, sort desc by ts, cap at 50.
5. Build PlayersResponse, JSON.
```

The pod-logs call sees the same per-pod retention as
`pods.log_stream`; on a chatty server, 2000 lines may cover ~1 hour;
on a quiet one, ~1 week. Both are fine for "recent activity" framing.

#### Action handler implementation

```text
1. Server-running gate (same as bulk).
2. Parse the discriminated `PlayerAction` from the body.
3. Validate the variant's fields:
   - Every variant carrying `player` (Kick / Ban / BanIp /
     WhitelistAdd / WhitelistRemove / Op / Deop / Tell / Pardon /
     Gamemode) → `validate_mc_username` on `player`
   - Kick/Ban/BanIp.reason → `validate_kick_reason` if Some
   - PardonIp.ip → `validate_ip_v4_or_v6`
   - Tell.message → `validate_chat_message`
   - Gamemode.mode → `validate_gamemode`
4. Build the RCON command string per variant:
   Kick { player, reason: Some(r) } → "kick {player} {r}"
   Kick { player, reason: None }    → "kick {player}"
   Ban  { player, reason: Some(r) } → "ban {player} {r}"
   Ban  { player, reason: None }    → "ban {player}"
   BanIp { player, reason: Some(r) }→ "ban-ip {player} {r}"     // MC accepts a username; resolves to IP server-side
   BanIp { player, reason: None }   → "ban-ip {player}"
   Pardon { player }                → "pardon {player}"
   PardonIp { ip }                  → "pardon-ip {ip}"
   Op { player }                    → "op {player}"
   Deop { player }                  → "deop {player}"
   Gamemode { player, mode }        → "gamemode {mode} {player}"  // MC's order: mode then target
   Tell { player, message }         → "tell {player} {message}"
   WhitelistAdd { player }          → "whitelist add {player}"
   WhitelistRemove { player }       → "whitelist remove {player}"
5. run_rcon_one(cmd).
6. insert_audit per the §5.3 mapping.
7. 204 No Content.
```

#### Broadcast handler implementation

```text
1. Running gate.
2. validate_chat_message on `message`.
3. run_rcon_one("say {message}")
4. insert_audit("player.broadcast", {"message_len": …})
5. 204 No Content.
```

### 6.5 Wiring

- `backend/src/lib.rs` — `pub mod players;` (the parsing module).
- `backend/src/routes/servers/mod.rs` — `pub mod players;` plus
  three router entries inside the existing per-server router builder.
- `backend/src/routes/servers/rcon.rs` — `run_rcon_batch` and
  `run_rcon_one` exported alongside the existing `handle`.
- `backend/src/validation.rs` — five new public validators with
  unit tests.

### 6.6 Cargo / dep changes

**None.** `rcon`, `kube`, `serde`, `chrono`, `tokio`, `axum`, `regex`
are all already in `backend/Cargo.toml`.

---

## 7. Frontend

### 7.1 Schemas + API — `frontend/app/lib/api.ts`

Additions (all with Zod schemas + inferred types + thin
fetch-wrappers, matching the existing module's shape):

```ts
export const onlinePlayersSchema   = z.object({ count, max, players });
export const banEntrySchema        = z.object({ name, reason });
export const banIpEntrySchema      = z.object({ ip,   reason });
export const playerEventSchema     = z.object({ kind: z.enum(["joined","left"]), player, ts_ms });
export const playersResponseSchema = z.object({ online, whitelist, banlist, history });

export const playerActionSchema    = z.discriminatedUnion("action", [/* 11 variants */]);

export async function fetchPlayers(id: string, signal: AbortSignal): Promise<PlayersResponse>;
export async function runPlayerAction(id: string, action: PlayerAction): Promise<void>;
export async function broadcastMessage(id: string, message: string): Promise<void>;
```

`PlayerAction` and `PlayersResponse` are exported types.
`runPlayerAction` and `broadcastMessage` use `noContentOrThrow` (the
existing 204 helper).

### 7.2 Polling hook — `frontend/app/lib/use-players.ts`

```ts
export function usePlayers(serverId: string, opts: { enabled: boolean }): {
  data: PlayersResponse | null;
  status: "loading" | "live" | "stale" | "error";
  lastError: string | null;
  refresh: () => void;
};
```

Internals:

- `useEffect` runs on `(serverId, enabled)` change.
- On enable: fetch immediately, then `setInterval(10_000)`.
- `document.addEventListener("visibilitychange", …)` pauses the
  interval when `visibilityState !== "visible"` and resumes (with an
  immediate fetch) when it returns.
- AbortController per fetch; aborted on unmount or next fetch.
- `refresh()` triggers an out-of-band fetch (used by post-action
  callbacks).

### 7.3 PlayersBody composition — `frontend/app/servers/tabs/PlayersBody.tsx`

The placeholder is replaced. Full layout, top-down:

```text
┌────────────────────────────────────────────┐
│ [broadcast]                  refreshed Xs ago │   ← 1-line bar
├────────────────────────────────────────────┤
│ Card "online now · 2 / 20"                  │
│   alice              ⋯                       │
│   bob                ⋯                       │
├────────────────────────────────────────────┤
│ Card "whitelist · 3 names"                  │
│   alice              ⋯                       │
│   bob                ⋯                       │
│   charlie            ⋯                       │
│   [+ add]                                    │
├────────────────────────────────────────────┤
│ Card "banned · 1 player · 0 ips"             │
│   eve  · griefing    ⋯                       │
├────────────────────────────────────────────┤
│ Card "recent activity"                       │
│   alice joined · 3m ago                      │
│   bob   left   · 12m ago                     │
└────────────────────────────────────────────┘
```

Server stopped → `PlayersBody` collapses to a single `Card` with the
copy `server is stopped — start the server to manage players` and a
secondary `[start server]` button (calls existing `startServer`).
No fetches fire when stopped.

The detail page passes `server.status` to `PlayersBody`. The hook's
`enabled` is `status === "running"`.

### 7.4 New components

| File | Purpose |
|---|---|
| `frontend/app/components/PlayerActionMenu.tsx` | Wraps `Dropdown`. Props: `source: "online" \| "whitelist" \| "banlist"`, plus `name` (or `ip` when `source==="banlist"` and the row is an IP entry). Renders the right action set per source: online → kick / op / deop / gamemode / tell / ban / ban-ip / whitelist-add; whitelist → remove / op / deop / ban; banlist → pardon (or pardon-ip). Each item dispatches to a `PlayerActionDialog` instance. |
| `frontend/app/components/PlayerActionDialog.tsx` | Modal-based confirm. One component, four variants by action type: simple yes/no (pardon, pardon-ip, whitelist-remove); with optional reason input (kick, ban, ban-ip); with required message textarea (tell); with required mode select (gamemode). Op / deop / whitelist-add (from row context) bypass this dialog and dispatch immediately. Calls `runPlayerAction`, calls `useToast().push("kicked X")` on success, calls `onDone()` to nudge `refresh()`. |
| `frontend/app/components/AddToWhitelistDialog.tsx` | Modal with one input + Mojang-username regex check. On submit calls `runPlayerAction({action:"whitelist-add", player})`. |
| `frontend/app/components/BroadcastDialog.tsx` | Modal with `<textarea>` + char counter (256 cap) + send. Calls `broadcastMessage`, toasts `broadcast sent`. |

All four reuse the existing `Modal`, `Button`, `Dropdown` primitives,
the `useToast()` hook, and the schemas from `api.ts`.

### 7.5 Reused primitives, untouched

`Card`, `Button`, `Modal`, `Dropdown`, `Skeleton`, `Toast`, `Tooltip`,
`Badge`, `IconButton`. **Nothing new in `components/` other than the
four files in §7.4.**

### 7.6 Detail page wiring

`frontend/app/servers/[name]/[tab]/page.tsx` already routes the
`players` tab to `PlayersBody`. No router change. The tab strip's
counts are unaffected.

---

## 8. k8s

- **No RBAC change.** RCON traffic uses the existing in-cluster
  Service. The pod-logs call uses `pods/log` (existing). No
  `pods/exec`, no `secrets/list` widening.
- **No StatefulSet shape change.** No new env vars, no init-container.
- **No new Service** for the panel surface.

---

## 9. Migration

**None.** No DB schema change, no k8s reconcile pass, no
configuration migration. New endpoints are additive; existing
endpoints unchanged in behavior. New frontend code is one tab body
swap.

---

## 10. Verification (acceptance for C)

- [ ] `cargo test --all`,
      `cargo clippy --all-targets --features serve-dir -- -D warnings`,
      `cargo clippy --all-targets --features embed -- -D warnings`,
      `cargo fmt --check` — green.
- [ ] `pnpm lint`, `pnpm typecheck`, `pnpm build` — green.
- [ ] Parser unit tests cover all six output shapes from §6.1, plus
      both log timestamp shapes.
- [ ] Players tab on a stopped server: shows the gate empty state;
      no `/players` requests fire (verified via DevTools network).
- [ ] Players tab on a running server with no players: online card
      `0 / N`; whitelist + banlist render whatever the server has;
      activity card lists recent joins/leaves from logs.
- [ ] **Whitelist add:** `[+ add]` → username → submit → row appears
      within one poll; toast `whitelisted X`.
- [ ] **Whitelist remove:** menu → remove → confirm → row leaves;
      toast `removed X from whitelist`.
- [ ] **Kick:** action menu → kick → optional reason → confirm →
      online count drops; toast `kicked X`.
- [ ] **Ban:** menu → ban → reason → confirm → row in banned card;
      toast `banned X`.
- [ ] **Ban-IP:** menu → ban-ip → confirm → entry in IPs subsection.
- [ ] **Pardon / Pardon-IP:** menu on banned row → pardon → confirm
      → row leaves banned card; toast `pardoned X`.
- [ ] **Op / Deop:** menu → op or deop (no confirm) → toast.
      Verify side effect: opped player can run commands in-game.
- [ ] **Gamemode:** menu → gamemode → pick mode → confirm → in-game
      effect.
- [ ] **Tell:** menu → tell → message → send → recipient sees
      whisper; toast `sent message to X`.
- [ ] **Broadcast:** broadcast button → message → send → all online
      see `[Server] msg`; toast `broadcast sent`.
- [ ] **Recent activity:** join + leave a player → both events
      appear within one poll, ordered desc.
- [ ] **Validation rejection:** add-whitelist with `bad name!` →
      400 + frontend renders the validator's message; no RCON fires.
- [ ] **Polling pause:** switch tabs → no `/players` requests while
      hidden; switching back resumes within ≤10 s.
- [ ] **Audit log:** every action endpoint call writes one
      `audit_log` row with the `player.<verb>` action and the
      §5.3 details payload.

---

## 11. Open questions

These are genuinely unresolved; the rest are settled in the
decisions above.

1. **Pod-logs `tail_lines` budget.** 2000 is a guess. On a chatty
   server it may cover ~1 hour; on a quiet one, days. If the
   "recent activity" Card feels empty too often during impl, bump
   to 5000 or add a `since_seconds` fallback. Leaving the constant
   private to `players.rs` so the bump is a one-line diff.
2. **`PlayerActionDialog` shape.** One component with discriminated
   variants by action vs. 4 separate dialog components. Lean toward
   one — the variant code is small and the open/close lifecycle is
   the same. Decide at impl time when the JSX is concrete.
3. **`Tell` audit-log body.** §5.3 records only `message_len` —
   should it record the message itself for completeness, or are
   private chats too sensitive to mirror? Default: omit. Easy to
   add later if it becomes a debugging need.
4. **Banned-IP source row.** When a user clicks `ban-ip alice`,
   vanilla MC resolves `alice → IP` server-side. The action audit
   row says `player.ban_ip` with `{player:"alice"}`; the resulting
   ban appears in the IPs subsection (`banlist ips`) with the IP,
   not the username. Acceptable trade — the audit row reflects the
   action input, the UI reflects the server state.

---

## 12. What ships at the end of C

A user opening any anvil-managed running server's Players tab sees:

1. Live online list polled every 10 s, paused while the browser
   tab is hidden.
2. Whitelist + banlist read live from RCON; both manageable via
   per-row action menus.
3. Per-player verbs: kick, ban, ban-ip, pardon, pardon-ip, op,
   deop, gamemode, /tell.
4. Server-wide broadcast via /say.
5. Recent join/leave activity from pod logs (last ~50 events).
6. Confirmations for destructive verbs via a yes-no `Modal`;
   `Toast` on every success.
7. Stopped servers gate to a single "server is stopped" empty
   state with a `[start server]` shortcut.
8. Backend gains a parsing module, a 5-validator addition, an
   11-variant action endpoint, a bulk read, a broadcast endpoint,
   and an `RCON batch` helper extraction.
9. **No new RBAC. No new DB migration. No new dependencies.**

Sub-project D (file browser sidecar) is the remaining v2 leg.

---

## 13. Critical files modified

**Backend (Rust):**

- `backend/src/players.rs` — NEW. Parsing module + tests.
- `backend/src/validation.rs` — Add 5 validators with tests.
- `backend/src/routes/servers/rcon.rs` — Extract `run_rcon_batch`
  and `run_rcon_one`; rewire existing `handle`.
- `backend/src/routes/servers/players.rs` — NEW. Three handlers,
  the `PlayerAction` enum, and the bulk-read response type.
- `backend/src/routes/servers/mod.rs` — `pub mod players;` plus
  three router entries.
- `backend/src/lib.rs` — `pub mod players;`.

**Frontend (TypeScript):**

- `frontend/app/lib/api.ts` — Add `playersResponseSchema`,
  `playerActionSchema`, three fetch-wrappers.
- `frontend/app/lib/use-players.ts` — NEW. Polling hook with
  visibility pause.
- `frontend/app/servers/tabs/PlayersBody.tsx` — Rewrite over the
  placeholder.
- `frontend/app/components/PlayerActionMenu.tsx` — NEW.
- `frontend/app/components/PlayerActionDialog.tsx` — NEW.
- `frontend/app/components/AddToWhitelistDialog.tsx` — NEW.
- `frontend/app/components/BroadcastDialog.tsx` — NEW.

**Docs:**

- `docs/superpowers/specs/2026-05-03-anvil-v2-player-management-design.md`
  — this document.
- `docs/superpowers/plans/2026-05-03-anvil-v2-player-management-impl.md`
  — generated by `superpowers:writing-plans` after spec sign-off.
- `docs/milestones.md` — mark C complete after ship.
