# API Reference

This document is the complete reference for Anvil's HTTP and WebSocket API.
For the bigger picture (what Anvil is, how it works internally, how the
pieces fit together), see [`architecture.md`](architecture.md).

The router source of truth is
[`backend/src/routes/mod.rs`](../backend/src/routes/mod.rs).

## Contents

- [Conventions](#conventions)
- [Authentication](#authentication)
- [Error envelope and status codes](#error-envelope-and-status-codes)
- [Auth endpoints](#auth-endpoints)
- [Health and cluster info](#health-and-cluster-info)
- [Catalog (mod / modpack / plugin search)](#catalog-mod--modpack--plugin-search)
- [Servers — lifecycle](#servers--lifecycle)
- [Servers — config patches](#servers--config-patches)
- [Logs (snapshot + WebSocket)](#logs-snapshot--websocket)
- [RCON](#rcon)
- [Modpack updates](#modpack-updates)
- [Mods and plugins](#mods-and-plugins)
- [Players](#players)
- [Backups](#backups)
- [Files](#files)
- [Metrics](#metrics)
- [WebSocket frame schemas](#websocket-frame-schemas)

---

## Conventions

- All paths are mounted under `/api`. The base URL is whatever the panel is
  reachable on (e.g. `https://anvil.example.com`).
- Request and response bodies are JSON unless noted (file upload is raw
  bytes; file download is `application/octet-stream`).
- Path identifiers use `{id}` (server UUID) or `{name}` (the user-facing
  name, lowercase DNS-1123). Paths that accept `{name}` are explicitly
  flagged.
- Timestamps in API responses are **unix seconds** (integer) unless noted.
  `audit_log.ts` is unix seconds; OIDC and Mojang upstream values are
  RFC-3339 strings.
- All `code` values returned in error envelopes are stable contracts — the
  human-readable `error` field can change.
- Discriminated unions in request bodies use a string `op` / `action` /
  similar field, not Serde's adjacent-tagged form. The shape varies per
  variant.

## Authentication

Every `/api/*` route requires a valid session cookie except:

| Path | Why |
|---|---|
| `GET /api/health` | Liveness probe; cluster health checks. |
| `GET /api/auth/login` | Initiates the OIDC dance. |
| `GET /api/auth/callback` | OIDC redirect target. |

The session cookie is `anvil_session`, an `HttpOnly; Secure; SameSite=Lax`
HS256 JWT. It is minted by `/api/auth/callback` after a successful
Authentik sign-in. It is *not* the Authentik ID token — it is Anvil's own
short-lived token signed with `ANVIL_SESSION_KEY`.

WebSocket upgrades require the same cookie. A valid session at upgrade time
is good for the lifetime of the connection.

For server-to-server use cases there is **no API token**. The intended
clients are the bundled Next.js SPA and the user's browser.

## Error envelope and status codes

Every error response has the shape:

```json
{ "error": "<human-readable message>", "code": "<machine-readable code>" }
```

Status codes follow the table below. The `code` strings are stable;
clients should branch on `code`, not on `error`.

| HTTP status | Common `code` values |
|---|---|
| **400** | `name_invalid`, `memory_invalid`, `mc_version_unknown`, `mc_version_required`, `storage_size_invalid`, `exposure_mode_invalid`, `source_kind_invalid`, `cf_disabled`, `cf_config_missing`, `cf_project_not_found`, `cf_id_invalid`, `cf_unavailable`, `no_server_pack_files`, `modrinth_config_missing`, `modrinth_unavailable`, `modrinth_id_invalid`, `no_modpack_versions`, `modded_config_missing`, `runtime_invalid`, `loader_version_required`, `loader_version_unsupported`, `nothing_to_change`, `not_modded`, `not_paper`, `mod_filename_invalid`, `search_query_invalid`, `catalog_type_invalid`, `catalog_provider_invalid`, `cmd_empty`, `cmd_too_long`, `username_invalid`, `reason_too_long`, `reason_has_control_char`, `message_too_long`, `message_has_control_char`, `gamemode_invalid`, `ip_invalid`, `invalid_name`, `path_required`, `path_is_root`, `path_invalid`, `path_too_long`, `segment_empty`, `segment_dot`, `segment_traversal`, `segment_too_long`, `segment_leading_dash`, `segment_invalid_char`, `recursive_required`, `shrink_unsupported`, `source_config_invalid`, `force_version_invalid`, `version_skip_invalid`, `payload_too_large` |
| **401** | `unauthorized` |
| **403** | `oidc_provider_error`, `sub_not_allowed` |
| **404** | `not_found` |
| **409** | `name_taken`, `nodeport_range_exhausted`, `must_be_stopped`, `server_not_running`, `update_in_progress`, `no_update_target`, `nothing_pending`, `apply_in_progress`, `version_change_unsupported`, `expansion_unsupported`, `parent_not_directory`, `helper_unsafe_to_kill`, `pvc_not_initialized`, `server_transitioning`, `server_error` |
| **500** | `internal`, `db_unavailable`, `k8s_unavailable` |
| **502** | `lb_unavailable` |

The codepath that produces them is in
[`backend/src/error.rs`](../backend/src/error.rs).

---

## Auth endpoints

### `GET /api/auth/login`

Starts the OIDC dance. No auth required.

- **Response:** `302 Found` to the Authentik authorize URL. Sets the
  encrypted `anvil_oidc_state` cookie.
- **Errors:** `500 internal` if discovery fails.

### `GET /api/auth/callback`

OIDC redirect target. Exchanges the code, mints the session JWT, redirects
to `/`. No auth required.

- **Query:** `code`, `state`, optional `error`.
- **Response:** `302 Found` with `Location: /` and the new
  `anvil_session` cookie.
- **Errors:**
  - `401 unauthorized` — missing code / state, CSRF mismatch.
  - `403 oidc_provider_error` — IdP returned `?error=`.
  - `403 sub_not_allowed` — subject not in `ANVIL_ALLOWED_SUBS`.
  - `500 internal` — exchange or token verification failed.

### `GET /api/auth/me`

Current user from the session cookie. Requires session.

- **Response 200:**
  ```json
  { "sub": "uuid", "name": "Alice", "email": "alice@example.com", "picture": null }
  ```

### `POST /api/auth/logout`

Clears the session cookie and returns Authentik's end-session URL. Requires
session.

- **Response 200:**
  ```json
  { "logoutUrl": "https://auth.example.com/application/o/anvil/end-session/" }
  ```

---

## Health and cluster info

### `GET /api/health`

Liveness probe. No auth.

- **Response 200:**
  ```json
  { "ok": true, "version": "1.0.0" }
  ```

### `GET /api/cluster/capabilities`

What exposure modes and storage classes the cluster supports. Cached for
5 minutes.

- **Response 200:**
  | Field | Type | Notes |
  |---|---|---|
  | `loadbalancer` | bool | Whether `exposure_mode=loadbalancer` is accepted. |
  | `nodeport` | bool | Always `true`. |
  | `clusterip` | bool | Always `true`. |
  | `available_storage_classes` | string[] | All `StorageClass` names, sorted. |
  | `expandable_storage_classes` | string[] | Subset with `allowVolumeExpansion=true`. |
  | `default_storage_class` | string \| null | The class annotated `is-default-class=true`. |
  | `cf_api_key_present` | bool | Whether CurseForge support is enabled. |
- **Errors:** `500 k8s_unavailable` on cache miss.

### `GET /api/cluster/mc-versions`

Last 20 Minecraft release versions, sourced from Mojang's manifest. Cached
24 hours; falls back to a hardcoded baseline if Mojang is unreachable.

- **Response 200:**
  ```json
  { "versions": ["1.21.4", "1.21.3", "..."], "source": "mojang" }
  ```
  `source` is `"mojang"` on a fresh fetch, `"fallback"` after upstream
  failure.

### `GET /api/runtimes/{runtime}/versions`

Forge or NeoForge loader versions, grouped by Minecraft version. Cached
1 hour; stale cache served on upstream failure.

- **Path:** `runtime` ∈ `forge` | `neoforge`.
- **Response 200:**
  ```json
  {
    "mc_versions": ["1.21.4", "1.21.3"],
    "by_mc": { "1.21.4": ["21.4.81", "21.4.80"] }
  }
  ```
- **Errors:** `404 not_found` (unknown runtime), `500 internal` (no cache
  and upstream down).

---

## Catalog (mod / modpack / plugin search)

### `GET /api/catalog/search`

Full-text search across Modrinth (always) and CurseForge (modpacks only,
when configured). Sorted by download count descending.

- **Query:**
  | Name | Type | Required | Notes |
  |---|---|---|---|
  | `type` | string | yes | `mod` \| `modpack` \| `plugin` |
  | `q` | string | yes | 1–256 chars |
  | `loader` | string | no | `fabric` \| `forge` \| `neoforge` \| `paper` |
  | `mc` | string | no | MC version filter |
  | `limit` | u32 | no | 1–50, default 20 |
  | `offset` | u32 | no | default 0 |
- **Response 200:** `{ "results": [ <CatalogHit>, ... ] }`

  `CatalogHit`:
  | Field | Type | Notes |
  |---|---|---|
  | `provider` | string | `modrinth` \| `curseforge` |
  | `project_id` | string | provider-specific |
  | `slug` | string | URL slug |
  | `name` | string | display name |
  | `summary` | string | short description |
  | `icon_url` | string \| null | |
  | `downloads` | u64 | |
  | `follows` | u64 | |
  | `project_type` | string | `mod` \| `modpack` \| `plugin` |
  | `loaders` | string[] | |
  | `game_versions` | string[] | |
  | `author` | string \| null | |
  | `updated` | string | RFC-3339 |
- **Errors:** `400 catalog_type_invalid`, `400 search_query_invalid`,
  `400 runtime_invalid`. Upstream errors are swallowed and produce empty
  results.

### `GET /api/catalog/projects/{provider}/{id}/versions`

Installable versions of one project, optionally filtered.

- **Path:** `provider` ∈ `modrinth` | `curseforge`. `id` is the
  project id (or slug, on Modrinth).
- **Query:** `loader`, `mc` (both optional).
- **Response 200:** `{ "versions": [ <CatalogVersion>, ... ] }`

  `CatalogVersion`:
  | Field | Type | Notes |
  |---|---|---|
  | `version_id` | string | |
  | `version_name` | string | |
  | `channel` | string | `release` \| `beta` \| `alpha` |
  | `loaders` | string[] | empty for CurseForge |
  | `game_versions` | string[] | empty for CurseForge |
  | `date_published` | string | RFC-3339 |
  | `primary_filename` | string | |
  | `primary_url` | string | |
  | `primary_sha512` | string \| null | Modrinth only |
- **Errors:** `400 catalog_provider_invalid`, `400 runtime_invalid`,
  `400 cf_id_invalid`, `400 cf_disabled`, `400 modrinth_unavailable`,
  `400 cf_unavailable`, `400 modrinth_id_invalid`.

---

## Servers — lifecycle

### `GET /api/servers`

List of all panel-managed servers. Joins SQLite metadata with live k8s
status.

- **Response 200:** `{ "servers": [ <ServerSummary>, ... ] }`

  `ServerSummary`:
  | Field | Type | Notes |
  |---|---|---|
  | `id` | string (UUID) | |
  | `name` | string | DNS-1123 label |
  | `status` | string | `running` \| `stopped` \| `starting` \| `stopping` \| `error` |
  | `mc_version` | string | |
  | `memory_mi` | i64 | mebibytes |
  | `exposure_mode` | string | `loadbalancer` \| `nodeport` \| `clusterip` |
  | `endpoint` | `{host,port}` \| null | null until LB IP is assigned |
  | `created_at` | i64 | unix seconds |
  | `source_kind` | string | `vanilla` \| `curseforge` \| `modrinth` \| `modded` \| `paper` |
  | `update_available` | bool | |
  | `latest_version_name` | string \| null | |
  | `update_in_progress` | bool | |

### `POST /api/servers`

Create a managed server. Provisions Secret + headless Service + StatefulSet
(replicas=0) + public Service. Returns 202; the server is created stopped.

- **Request body:**
  | Field | Type | Required | Notes |
  |---|---|---|---|
  | `name` | string | yes | DNS-1123 label |
  | `mc_version` | string | conditional | required for `vanilla`, `modded`, `paper` |
  | `memory_mi` | i64 | yes | 1024–16384, multiple of 1024 |
  | `exposure_mode` | string | no | default from `ANVIL_MC_SVC_TYPE` |
  | `storage_class` | string | no | default from `ANVIL_MC_STORAGE_CLASS` |
  | `storage_size_gi` | i64 | no | default 10 |
  | `source_kind` | string | no | default `vanilla` |
  | `curseforge` | object | when `source_kind=curseforge` | `{ project_id: u32, channel: "release"\|"beta"\|"alpha" }` |
  | `modrinth` | object | when `source_kind=modrinth` | `{ project_id: string, channel: string }` |
  | `modded` | object | when `source_kind=modded` | `{ runtime, initial_mods?, loader_version? }` |
  | `paper` | object | optional for `paper` | `{ initial_plugins?: ModEntry[] }` |

  `ModEntry`:
  | Field | Type |
  |---|---|
  | `provider` | `modrinth` \| `curseforge` |
  | `project_id`, `project_slug`, `project_name` | string |
  | `version_id`, `version_name` | string |
  | `filename`, `download_url` | string |
  | `sha512` | string \| null |
- **Response 202:**
  ```json
  { "id": "uuid", "name": "my-smp" }
  ```
- **Errors:** `400 name_invalid`, `400 memory_invalid`,
  `400 mc_version_unknown`, `400 mc_version_required`,
  `400 exposure_mode_invalid`, `400 storage_size_invalid`,
  `400 source_kind_invalid`, `400 cf_*`, `400 modrinth_*`,
  `400 modded_config_missing`, `400 runtime_invalid`,
  `400 mod_filename_invalid`, `409 name_taken`,
  `409 nodeport_range_exhausted`, `502 lb_unavailable`.

### `GET /api/servers/{id}` and `GET /api/servers/by-name/{name}`

Full detail for one server. The `by-name` variant resolves the unique
`name` column to the UUID and returns the same shape.

- **Response 200:** `ServerDetail` extends `ServerSummary` with:
  | Field | Type | Notes |
  |---|---|---|
  | `storage_class` | string \| null | |
  | `storage_size_gi` | i64 | |
  | `nodeport` | i32 \| null | |
  | `last_started_at` | i64 \| null | unix seconds |
  | `source_config` | object \| null | provider-specific JSON |
  | `latest_version_id` | i64 \| null | |
  | `files_helper_running` | bool | |
  | `mod_updates` | `ModUpdateInfo[]` | empty for vanilla / modpack |

  `ModUpdateInfo`:
  | Field | Type |
  |---|---|
  | `provider`, `project_id` | string |
  | `current_version_id`, `latest_version_id`, `latest_version_name` | string |
- **Errors:** `404 not_found`.

### `DELETE /api/servers/{id}`

Tears down: StatefulSet → wait for pod → PVC → public Service → headless
Service → Secret → SQLite row.

- **Response:** `204 No Content`.
- **Errors:** `404 not_found`, `409 must_be_stopped` (replicas ≥ 1),
  `500 internal` (pod failed to terminate within 2 minutes).

### `POST /api/servers/{id}/start`

Scales the StatefulSet to 1 replica. Tears down any files-helper Pod first.

- **Response 200:** full `ServerDetail`.
- **Errors:** `404 not_found`, `500 k8s_unavailable`.

### `POST /api/servers/{id}/stop`

Scales the StatefulSet to 0 replicas.

- **Response 200:** full `ServerDetail`.
- **Errors:** `404 not_found`, `500 k8s_unavailable`.

### `POST /api/servers/{id}/restart`

Async: spawns a tokio task that stops, waits for pod termination, starts.
Returns 202 immediately.

- **Response 202:**
  ```json
  { "id": "uuid", "status": "restarting" }
  ```
- **Errors:** `404 not_found`.

---

## Servers — config patches

### `PATCH /api/servers/{id}/version`

Change MC version (and optionally loader version for modded). Spawns a
background version-change FSM. Progress observable via `/update/stream`.

- **Request body:**
  | Field | Type | Required | Notes |
  |---|---|---|---|
  | `mc_version` | string | yes | |
  | `loader_version` | string | conditional | required for forge/neoforge modded |
- **Response 202:** `{ "status": "started", "server_id": "uuid" }`.
- **Errors:** `404 not_found`, `409 version_change_unsupported`
  (curseforge/modrinth servers can't change version this way),
  `400 mc_version_unknown`, `400 loader_version_required`,
  `400 loader_version_unsupported`, `400 nothing_to_change`,
  `409 update_in_progress`.

### `PATCH /api/servers/{id}/settings`

Update memory and/or modpack settings. All fields optional; only present
fields are touched.

- **Request body:**
  | Field | Type | Notes |
  |---|---|---|
  | `memory_mi` | i64 | 1024–16384; live-patches StatefulSet env |
  | `auto_update_mode` | `never` \| `notify` \| `apply` | modpack only |
  | `version_skip` | string[] | versions to skip; modpack only |
  | `force_version` | string \| null | pin to a version; `null` clears |
- **Response:** `204 No Content`.
- **Errors:** `404 not_found`, `400 not_modded`, `400 memory_invalid`,
  `400 force_version_invalid`, `400 version_skip_invalid`.

### `PATCH /api/servers/{id}/storage`

Grow the data PVC. Shrinking is rejected.

- **Request body:** `{ "size_gi": u32 }` (must be strictly larger).
- **Response 200:** `{ "size_gi": 20 }`.
- **Errors:** `404 not_found`, `400 shrink_unsupported`,
  `409 expansion_unsupported` (StorageClass cannot expand volumes),
  `500 internal`.

---

## Logs (snapshot + WebSocket)

### `GET /api/servers/{id}/logs`

Last 200 lines of pod stdout/stderr as a snapshot. Empty array if pod
absent.

- **Response 200:**
  ```json
  { "lines": ["[Server thread/INFO]: Done (1.234s)!", "..."] }
  ```
- **Errors:** `404 not_found`, `500 k8s_unavailable`.

### `GET /api/servers/{id}/logs/stream` (WebSocket)

Live tail. The pre-upgrade HTTP step returns 404 if the server is unknown.
On successful upgrade, server emits text frames; pings every 30s.

Frames (`type` discriminator, kebab-case):

```
{ "type": "hello", "pod": "mc-{id}-0", "attached_at": "2026-05-02T12:00:00Z" }
{ "type": "log", "line": "[Server thread/INFO]: ..." }
{ "type": "error", "code": "pod-not-found", "message": "..." }
{ "type": "end", "reason": "pod-unavailable" | "client-closed" | "server-shutdown" }
```

Behaviour:
- Replays the last 2000 lines as historical context, then live-tails.
- On pod EOF/restart, server-side re-attaches automatically (sends a fresh
  `hello`).
- Waits up to 60s for the pod to reach Running before sending
  `end{reason:"pod-unavailable"}`.

---

## RCON

### `POST /api/servers/{id}/rcon`

Sends one RCON command and returns its response. Fresh TCP+RCON session
per call (5s end-to-end timeout). The password lives in the
`mc-{id}-rcon` Secret and is never echoed.

- **Request body:**
  ```json
  { "cmd": "list" }
  ```
  Trimmed; max 1024 bytes.
- **Response 200:**
  ```json
  { "output": "There are 3 of a max of 20 players online: ..." }
  ```
  May be empty (e.g. `say` produces no output).
- **Errors:** `400 cmd_empty`, `400 cmd_too_long`, `404 not_found`,
  `409 server_not_running`, `500 internal` (RCON IO/auth/timeout).

---

## Modpack updates

### `POST /api/servers/{id}/update`

Kicks the [update orchestrator FSM](architecture.md#update-orchestrator-fsm).
Returns immediately.

- **Request body:** `{ "version_id": string }` (optional; omit to use the
  cached latest version from `modpack_versions`).
- **Response 202:**
  ```json
  { "status": "started", "server_id": "uuid", "target_version_id": "12345" }
  ```
- **Errors:** `404 not_found`, `400 not_modded` (server is vanilla, modded,
  or paper), `409 update_in_progress`, `409 no_update_target` (no
  `version_id` and no cached latest).

### `GET /api/servers/{id}/update/stream` (WebSocket)

Streams `UpdatePhase` transitions. Sends `end{reason:"no-update-in-progress"}`
and closes if no update is running.

Frames:

```
{ "type": "hello", "phase": "queued" }
{ "type": "progress", "phase": "stopping" }
{ "type": "done", "result": "succeeded" | "failed-rolled-back" | "failed" }
{ "type": "end", "reason": "no-update-in-progress" }
```

`phase` values, in order: `queued`, `announcing`, `stopping`, `backing-up`,
`swapping`, `starting`, `verifying`, `succeeded`, `restoring`,
`rolling-back`, `rolled-back`, `failed`.

`done.result`:
- `succeeded` — update applied cleanly.
- `failed-rolled-back` — update failed but rollback restored the snapshot.
- `failed` — update failed *and* rollback failed (or the orchestrator
  panicked).

---

## Mods and plugins

The mods endpoints serve `source_kind=modded` servers. The plugins
endpoints serve `source_kind=paper` servers. Both work via a draft list
(`pending_*`) plus an apply step that runs an FSM.

### `POST /api/servers/{id}/mods`

Append a pending mod operation. Required deps are auto-resolved on `add`.

- **Request body** (discriminated by `op`):
  ```json
  { "op": "add", "mod_entry": { ... } }
  { "op": "remove", "filename": "sodium-0.5.8.jar" }
  { "op": "bump", "filename": "old.jar", "to_version_id": "...",
    "to_version_name": "...", "to_filename": "new.jar",
    "to_download_url": "https://...", "to_sha512": null }
  ```
- **Response 200:**
  ```json
  { "added": [ <ModEntry>, ... ], "added_count": 2 }
  ```
  `added` is non-empty only for `add` (seed + resolved deps).
- **Errors:** `404 not_found`, `400 not_modded`, `400 mod_filename_invalid`.

### `DELETE /api/servers/{id}/mods/pending/{idx}`

Remove one pending op by 0-based index.

- **Response:** `204 No Content`.
- **Errors:** `404 not_found`, `400 not_modded`.

### `POST /api/servers/{id}/mods/apply`

Kick the mod-sync FSM.

- **Response 202:**
  ```json
  { "status": "started", "server_id": "uuid", "pending_count": 3 }
  ```
- **Errors:** `404 not_found`, `400 not_modded`, `409 nothing_pending`,
  `409 apply_in_progress`.

### `GET /api/servers/{id}/mods/apply/stream` (WebSocket)

Same frame schema as `/update/stream`, but `done.result` is `succeeded` or
`failed` only (no rollback). No-apply sentinel:
`end{reason:"no-apply-in-progress"}`.

### `GET /api/servers/{id}/plugins`

Current installed and pending plugins for a Paper server.

- **Response 200:**
  ```json
  { "plugins": [ <ModEntry>, ... ], "pending_plugins": [ <ModEntry>, ... ] }
  ```
- **Errors:** `404 not_found`, `400 not_paper`.

### `POST /api/servers/{id}/plugins`

Stage adding a plugin. Body is a single `ModEntry` (not wrapped).

- **Response 200:** `{ "added": [ <ModEntry>, ... ], "added_count": 2 }`.
- **Errors:** `404 not_found`, `400 not_paper`, `400 mod_filename_invalid`.

### `DELETE /api/servers/{id}/plugins/{filename}`

Stage removing a plugin from `pending_plugins`.

- **Response:** `204 No Content`.

### `POST /api/servers/{id}/plugins/apply` and `GET .../plugins/apply/stream`

Same shape as `/mods/apply` and `/mods/apply/stream`.

---

## Players

All player endpoints are RCON-driven. They require the server to be
running.

### `GET /api/servers/{id}/players`

Runs `list`, `whitelist list`, `banlist players`, `banlist ips` on one
RCON connection. Best-effort scrape of the last 2000 pod log lines for
join/leave history (max 50 events).

- **Response 200:**
  ```json
  {
    "online": { "count": 2, "max": 20, "players": ["Alice", "Bob"] },
    "whitelist": ["Alice", "Bob"],
    "banlist": {
      "players": [{ "name": "Griefer", "reason": "griefing" }],
      "ips": [{ "ip": "1.2.3.4", "reason": "range hop" }]
    },
    "history": [
      { "kind": "joined", "player": "Alice", "ts_ms": 1746532800000 }
    ]
  }
  ```
- **Errors:** `404 not_found`, `409 server_not_running`,
  `500 internal` (RCON failure).

### `POST /api/servers/{id}/players/action`

Execute one of 11 player-management RCON commands.

- **Request body** (discriminated by `action`, kebab-case):
  | `action` | Extra fields |
  |---|---|
  | `kick` | `player`, optional `reason` |
  | `ban` | `player`, optional `reason` |
  | `ban-ip` | `player`, optional `reason` |
  | `pardon` | `player` |
  | `pardon-ip` | `ip` (IPv4 or IPv6) |
  | `op` | `player` |
  | `deop` | `player` |
  | `gamemode` | `player`, `mode` ∈ `survival`/`creative`/`adventure`/`spectator` |
  | `tell` | `player`, `message` |
  | `whitelist-add` | `player` |
  | `whitelist-remove` | `player` |
- **Response:** `204 No Content`.
- **Errors:** `400 username_invalid`, `400 reason_too_long`,
  `400 reason_has_control_char`, `400 message_too_long`,
  `400 message_has_control_char`, `400 gamemode_invalid`, `400 ip_invalid`,
  `404 not_found`, `409 server_not_running`, `500 internal`.

### `POST /api/servers/{id}/players/broadcast`

Sends `/say <message>` via RCON.

- **Request body:** `{ "message": string }`.
- **Response:** `204 No Content`.

---

## Backups

Backups are tar-to-PVC snapshots of `/data`, retained per-server up to a
hardcoded ceiling (last 3). They live on the shared `mc-snapshots` PVC.

### `POST /api/servers/{id}/backups`

Start a backup FSM (announce → stop → tar → start → verify) in the
background.

- **Request body:**
  ```json
  { "name": "pre-update" }
  ```
  `name` is optional, max 64 chars, no newlines.
- **Response 202:**
  ```json
  { "status": "started", "backup_id": "bkp-uuid" }
  ```
- **Errors:** `404 not_found`, `400 invalid_name`, `409 update_in_progress`.

### `GET /api/servers/{id}/backups`

List backups for the server, newest first.

- **Response 200:** array of:
  | Field | Type |
  |---|---|
  | `id` | string |
  | `name` | string \| null |
  | `created_at` | i64 (unix seconds) |
  | `mc_version` | string |
  | `size_bytes` | i64 \| null |

### `POST /api/servers/{id}/backups/{backup_id}/restore`

Start a restore FSM in the background. Phases mirror the update orchestrator.

- **Response 202:** `{ "status": "started" }`.
- **Errors:** `404 not_found` (server or backup missing),
  `409 update_in_progress`.

### `DELETE /api/servers/{id}/backups/{backup_id}`

Spawns and waits (up to 1 minute) for a one-shot `rm` Job, then deletes the
SQLite row.

- **Response:** `204 No Content`.
- **Errors:** `404 not_found`, `500 internal` (Job timeout / failure),
  `500 k8s_unavailable`.

---

## Files

All file operations target `/data` inside the running MC pod. When the
server is stopped, Anvil lazily spawns a helper Pod (`mc-{id}-files`). See
[architecture.md → File browser](architecture.md#file-browser-via-podsexec).

Common pre-handler errors:
- `409 pvc_not_initialized` — data PVC not yet bound.
- `409 server_transitioning` — server is starting or stopping.
- `409 server_error` — server is in error state (not safe to attach a helper).
- `500 internal` — helper pod spawn / wait failed.

### `GET /api/servers/{id}/files?path=/`

List a directory inside `/data`.

- **Query:** `path` (optional; default `/`, `/data`-relative).
- **Response 200:**
  ```json
  {
    "path": "/mods",
    "entries": [
      { "name": "sodium.jar", "type": "f", "size": 1048576, "mtime": 1746532800 }
    ]
  }
  ```
  `type` ∈ `f` (file) | `d` (dir) | `l` (symlink) | `o` (other).

### `PUT /api/servers/{id}/files?path=/foo/bar`

Upload a file. Body is raw bytes (not multipart). Atomic
tmp-then-rename. **Max body 100 MiB.**

- **Query:** `path` (required; can't be `/`).
- **Response:** `204 No Content`.
- **Errors:** `400 path_required`, `400 path_is_root`, `400 path_*`,
  `409 parent_not_directory`, `500 payload_too_large`.

### `GET /api/servers/{id}/files/raw?path=/foo/bar`

Download a single file.

- **Query:** `path` (required).
- **Response 200:** raw bytes with headers
  ```
  Content-Type: application/octet-stream
  Content-Length: <bytes>
  Content-Disposition: attachment; filename="<basename>"
  ```
- **Errors:** `400 path_required`, `400 path_*`, `404 not_found`.

### `POST /api/servers/{id}/files/action`

Filesystem action. Body discriminated by `action`:

| `action` | Extra fields |
|---|---|
| `mkdir` | `path` |
| `rename` | `from`, `to` |
| `delete` | `path`, `recursive` (bool) |

- **Response:** `204 No Content`.
- **Errors:** `400 path_*`, `400 recursive_required` (delete on a
  directory without `recursive=true`), `500 internal`.

### `DELETE /api/servers/{id}/files/helper`

Manually tear down the files-helper Pod.

- **Response:** `204 No Content` (helper torn down) or
  `200 { "already_gone": true }`.
- **Errors:** `404 not_found`, `409 helper_unsafe_to_kill` (server is
  running or starting), `500 k8s_unavailable`.

---

## Metrics

### `GET /api/servers/{id}/metrics`

Live CPU and memory from `metrics-server`. Returns `null` for both fields
if metrics-server isn't installed or hasn't scraped this pod yet.

- **Response 200:**
  ```json
  { "cpu_millicores": 250, "memory_mi": 1536 }
  ```
- **Errors:** `500 internal` (metrics API returned an unexpected error;
  404 and 503 from upstream are silently mapped to `null` fields).

---

## WebSocket frame schemas

The three streaming endpoints share a tagged-union pattern. The
discriminator is the `type` field; values are kebab-case.

### Logs stream

```
{ "type": "hello", "pod": "mc-{id}-0", "attached_at": "2026-05-02T12:00:00Z" }
{ "type": "log", "line": "..." }
{ "type": "error", "code": "pod-not-found", "message": "..." }
{ "type": "end", "reason": "pod-unavailable" | "client-closed" | "server-shutdown" }
```

### Update stream

```
{ "type": "hello", "phase": "queued" }
{ "type": "progress", "phase": "stopping" }
{ "type": "done", "result": "succeeded" | "failed-rolled-back" | "failed" }
{ "type": "end", "reason": "no-update-in-progress" }
```

### Mods/Plugins apply stream

Identical to update-stream except `done.result` is `succeeded` or `failed`
only, and the no-apply sentinel reason is `"no-apply-in-progress"`.

### Heartbeat

All three streams send WebSocket Pings every 30 seconds. Frontend hooks
expect them; absence for > 90 seconds counts as a dropped connection and
triggers exponential-backoff reconnect (1s → 30s cap).
