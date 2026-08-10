<!--
SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

# Server difficulty, hardcore, and Properties tab — design

**Date:** 2026-05-06
**Audience:** Anvil panel; itzg/minecraft-server backed servers.
**Driver:** expose the most-tweaked subset of Minecraft `server.properties` through the panel
without coupling to the on-disk file — itzg's image already overlays env vars onto
`server.properties` on every boot, so all writes go through env.

## Goals

1. New "Properties" tab on the server detail page exposing ~16 commonly tweaked
   `server.properties` fields with typed inputs and inline help linking to
   `minecraft.wiki/w/Server.properties` anchors.
2. Server-creation form gains a small "world" section: difficulty, hardcore, gamemode.
3. Apply semantics match the existing `memory_mi` setting: PATCH writes SQLite + strategic-merges
   the StatefulSet env, takes effect on next pod start. Toast: "applies on next start".
4. Ops/whitelist *names* stay RCON-only (existing Players tab); the `white-list` toggle in this
   tab only flips enforcement.

## Non-goals

- Exposing every `server.properties` env var (~50 in itzg). The deferred ones include `online-mode`,
  resource-pack URL/SHAs, generator-settings, level-type, op-permission-level, broadcast options,
  network-compression-threshold, rate-limit, sync-chunk-writes, max-build-height/world-size/tick-time.
  These can be folded in later if asked; `properties` JSON column has room.
- Live RCON dispatch on settings change. Difficulty *can* live-toggle via `/difficulty <x>` but
  hardcore can't, and consistency with the memory pattern is more valuable than 3s of immediacy.
- Migrating the existing RCON-driven ops/whitelist mutation flow to env vars. That decision was
  considered and rejected: RCON's "on the fly" semantics fit the use case better.

## Scope

### Properties exposed

| Field | Env | Type | Range / Values | Default |
|---|---|---|---|---|
| difficulty | `DIFFICULTY` | enum | peaceful, easy, normal, hard | normal |
| hardcore | `HARDCORE` | bool | — | false |
| gamemode | `GAMEMODE` | enum | survival, creative, adventure, spectator | survival |
| force_gamemode | `FORCE_GAMEMODE` | bool | — | false |
| max_players | `MAX_PLAYERS` | int | 1..=200 | 20 |
| view_distance | `VIEW_DISTANCE` | int | 3..=32 | 10 |
| simulation_distance | `SIMULATION_DISTANCE` | int | 3..=32 | 10 |
| pvp | `PVP` | bool | — | true |
| white_list | `WHITE_LIST` | bool | — | false |
| spawn_protection | `SPAWN_PROTECTION` | int | 0..=256 | 16 |
| spawn_animals | `SPAWN_ANIMALS` | bool | — | true |
| spawn_monsters | `SPAWN_MONSTERS` | bool | — | true |
| spawn_npcs | `SPAWN_NPCS` | bool | — | true |
| allow_flight | `ALLOW_FLIGHT` | bool | — | false |
| allow_nether | `ALLOW_NETHER` | bool | — | true |
| enable_command_block | `ENABLE_COMMAND_BLOCK` | bool | — | false |

### UI placement

- **Create form** (`/servers/new`): new Section 06 "world" between resources and storage —
  difficulty, hardcore, gamemode only. Other 13 fields fall back to defaults.
- **Server detail tabs**: insert "properties" between "backups" and "settings".

### Tab card layout

- **World**: difficulty, hardcore, gamemode, force_gamemode
- **Players**: max_players, view_distance, simulation_distance, pvp, white_list
- **Spawn**: spawn_protection, spawn_animals, spawn_monsters, spawn_npcs
- **Features**: allow_flight, allow_nether, enable_command_block

Each row = label + input + (i) icon → `Tooltip` opening one-line description and a
`wiki ↗` link to the appropriate `minecraft.wiki/w/Server.properties#anchor`.

Save button at the bottom of the tab. Toast on success: `settings saved · applies on next start`.

## Backend

### Schema

New migration `0009_server_properties.sql`:

```sql
ALTER TABLE servers ADD COLUMN properties TEXT NOT NULL DEFAULT '{}';
```

JSON column. Empty `{}` decodes to `ServerProperties::default()` via `#[serde(default)]` per field.
Existing rows backfill cleanly to `'{}'` and continue to work.

### New module: `backend/src/server_properties.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerProperties {
    pub difficulty: Difficulty,
    pub hardcore: bool,
    pub gamemode: Gamemode,
    pub force_gamemode: bool,
    pub max_players: u32,
    pub view_distance: u32,
    pub simulation_distance: u32,
    pub pvp: bool,
    pub white_list: bool,
    pub spawn_protection: u32,
    pub spawn_animals: bool,
    pub spawn_monsters: bool,
    pub spawn_npcs: bool,
    pub allow_flight: bool,
    pub allow_nether: bool,
    pub enable_command_block: bool,
}

impl Default for ServerProperties { /* MC vanilla defaults from table above */ }

impl ServerProperties {
    pub fn validate(&self) -> Result<(), AppError>;     // enum + range checks
    pub fn to_env(&self) -> Vec<EnvVar>;                // emits all 16, always
}
```

`Difficulty` and `Gamemode` are small enums with Serde rename to lowercase, `Display` for env
emission. `deny_unknown_fields` on the struct means front-end typos blow up at deserialize
time rather than silently drop.

`to_env` always emits all 16 — the stored JSON is canonical. Booleans serialize as `"true"` /
`"false"`; integers as their decimal string; enums as their lowercase name.

### Layering into the StatefulSet env

Provider env (`extra_env` on each `ModpackProvider`) is unchanged. Two call sites append
`props.to_env()` to the resulting vec:

1. `routes/servers/create.rs::handle` after `resolved.provider.extra_env(&ctx)`
2. `routes/servers/settings.rs::build_full_env_for_running_runtime` before returning

No name collisions — providers emit `EULA / TYPE / VERSION / *MEMORY / JVM_XX_OPTS / ENABLE_RCON
/ RCON_PASSWORD` (plus modpack vars `FORGE_VERSION / NEOFORGE_VERSION / MODRINTH_VERSION
/ CF_FILE_ID / CF_SLUG`); properties emit the 16 above.

### Routes

#### `POST /api/servers`

`CreateRequest` gains:

```rust
#[serde(default)]
pub properties: Option<ServerProperties>,
```

When `Some`, validate and persist verbatim. When `None`, persist `ServerProperties::default()`.
Frontend sends `Some(...)` populated with the user's three World fields + defaults for the rest.

#### `PATCH /api/servers/:id/settings`

`SettingsRequest` gains:

```rust
pub properties: Option<ServerProperties>,
```

Full-replacement semantics when present. Validate → SQLite UPDATE on the new column → rebuild
the full env via `build_full_env_for_running_runtime` → strategic-merge onto the StatefulSet
(same exact path as `memory_mi`). Audit row gains a `properties` field in `details` JSON.

#### `GET /api/servers/:id` and `/by-name/:name`

Response struct `ServerDetail` gains:

```rust
pub properties: ServerProperties,
```

`fetch_server_row` is extended to read the new column; `fetch_detail` deserializes the JSON
(default on parse error so a corrupt row doesn't 500 the whole detail fetch).

### Validation

A single `ServerProperties::validate(&self)` returning `Result<(), AppError>`:
- enums are infallible via Serde (BadRequest at deserialize time)
- integers checked against the table ranges; on failure return
  `AppError::BadRequest { code: "properties_<field>_invalid", message: ... }`

## Frontend

### TypeScript types (`app/lib/api.ts`)

```ts
export type Difficulty = "peaceful" | "easy" | "normal" | "hard";
export type Gamemode = "survival" | "creative" | "adventure" | "spectator";

export interface ServerProperties {
  difficulty: Difficulty;
  hardcore: boolean;
  gamemode: Gamemode;
  force_gamemode: boolean;
  max_players: number;
  view_distance: number;
  simulation_distance: number;
  pvp: boolean;
  white_list: boolean;
  spawn_protection: number;
  spawn_animals: boolean;
  spawn_monsters: boolean;
  spawn_npcs: boolean;
  allow_flight: boolean;
  allow_nether: boolean;
  enable_command_block: boolean;
}

export const DEFAULT_PROPERTIES: ServerProperties = { /* table defaults */ };
```

`ServerDetail` gains `properties: ServerProperties`. `CreateServerRequest` gains
`properties?: ServerProperties`. `updateServerSettings` body type gains `properties?: ServerProperties`.

A Zod schema `ServerPropertiesSchema` validates every detail-fetch response — defends against
backend regressions.

### Create page (`app/servers/new/page.tsx`)

`CreateDraft` gains `properties: ServerProperties`. `INITIAL` initializes to `DEFAULT_PROPERTIES`.
A new `<Section number="06" title="world">` between the existing 05 (storage) and the submit
controls renders three fields:

- difficulty: `SegmentedControl` over `DIFFICULTY_OPTIONS`
- hardcore: `Toggle` (or `SegmentedControl` of off/on for visual consistency)
- gamemode: `SegmentedControl` over `GAMEMODE_OPTIONS`

Submit sends `properties: draft.properties`.

### New tab `app/servers/tabs/PropertiesBody.tsx`

```tsx
export function PropertiesBody(): ReactElement {
  const { detail } = useServerDetail();
  const [props, setProps] = useState<ServerProperties>(detail.properties);
  const dirty = !shallowEqual(props, detail.properties);
  const save = () => updateServerSettings(detail.id, { properties: props }) ...
  return (
    <div className="...">
      <Card header="world">…</Card>
      <Card header="players">…</Card>
      <Card header="spawn">…</Card>
      <Card header="features">…</Card>
      <SaveBar dirty={dirty} onSave={save} />
    </div>
  );
}
```

Each row: label + input + `<Tooltip>` wrapping an info `(i)` icon. Tooltip body is a small
React node containing one sentence and a `wiki ↗` external link.

A field-spec table (`PROPERTY_HELP: Record<keyof ServerProperties, { tip: string; wiki: string }>`)
keeps tooltip prose colocated with the property names.

### Tab registration (`app/servers/ServerDetailView.tsx`)

- Import `PropertiesBody`.
- Add `"properties"` to `TabId` and `TAB_IDS` between `"backups"` and `"settings"`.
- Render `<PropertiesBody />` in the switch.
- Tab label: `"properties"`.

### Components

- Reuse: `Card`, `SegmentedControl`, `Tooltip`, `Button`, `Toast`.
- New: a small `Toggle` is desirable but `SegmentedControl` with `[off, on]` does the job —
  go with `SegmentedControl` to avoid adding a component.
- New: `NumberStepper` for integer fields (max_players, view_distance, simulation_distance,
  spawn_protection). Could also reuse `RangeSlider` — but step-based is friendlier for these
  small ranges. Decision: minimal `<input type="number" min max step>` styled to match the
  existing inputs; no new component.

## Testing

### Backend unit

- `ServerProperties::default` returns the documented defaults.
- `ServerProperties::to_env` emits 16 `EnvVar`s with correct stringified values.
- `ServerProperties::validate` rejects: `max_players=0`, `max_players=201`, `view_distance=2`,
  `view_distance=33`, `simulation_distance` out-of-range, `spawn_protection=257`.
- Serde round-trip for default and a non-default sample.
- Empty `{}` JSON decodes to defaults.

### Backend integration (`backend/tests/`)

- Extend the `settings_handler` test fixture: PATCH with `properties` writes the column
  and `build_full_env_for_running_runtime` includes the new env.
- Create handler with `properties: { difficulty: "hard", ... }` persists JSON and the
  built StatefulSet env contains `DIFFICULTY=hard`.

### Frontend

- No new tests. Existing convention is no FE tests.

## Risks / open questions

- **Hardcore retroactivity.** Toggling hardcore on an existing world doesn't transform it.
  Surfaced in the UI tooltip ("hardcore mode bans players on death; meaningful only from a
  fresh world"). No code-level enforcement.
- **Difficulty enum casing.** Vanilla MC accepts `peaceful/easy/normal/hard` lowercase.
  itzg also accepts numeric (0..3). We commit to lowercase strings.
- **`white_list` enforcement vs. names.** Toggling `WHITE_LIST=true` while `whitelist.json`
  is empty kicks all non-op players. UI tooltip flags this; no auto-population — names go
  through the existing Players tab.
- **Server-port is intentionally not exposed.** k8s Service handles mapping. Documented as
  a permanent omission.

## Out-of-scope (deferred follow-ups)

- Online-mode toggle with a paired tooltip about cracked clients.
- Resource pack: URL, SHA1, SHA256, prompt text, required toggle.
- Generator settings / level-type / level-seed (level-seed is meaningful at create time only).
- Op permission level (1..=4).
- Broadcast console / RCON to ops.
- Network compression threshold, rate limit, prevent-proxy-connections.
- Idle timeout, max-build-height, max-world-size, max-tick-time.

These slot into the same `properties` JSON when added; no schema migration needed.
