# Anvil — Mod dependencies · per-mod updates · paper plugin pre-select (Spec 4)

**Date:** 2026-05-06
**Author:** Hadi (with Claude as scribe)
**Status:** Brainstormed — awaiting user signoff
**Spec series:** 4 of 4. Companions: Spec 1 (bugs & small UX, signed off), Spec 2 (PVC/files/status), Spec 3 (MC version change). A fifth spec covering Backups was added after this one was scoped — see Spec 5.

---

## 1. Context

Three items from the 2026-05-06 triage that all touch the Modrinth/CurseForge integration layer:

- **D#6** — User installs a mod; required dependencies aren't auto-pulled. Today the user has to know what each mod needs and add deps manually one-by-one. `MrVersion` doesn't even deserialize the upstream `dependencies` field (`mr_client.rs:51-63`).
- **D#16b** — User can't tell when an installed mod has an update. The poller (`poller.rs`) only checks modpack-level updates (`source_kind IN ('curseforge', 'modrinth')`). Per-mod / per-plugin updates aren't tracked. The `Bump` op type exists in the schema and backend (`PendingOp::Bump`) but no UI consumes it.
- **D#13b** — Create form's `paper` branch shows only an MC picker; `modded` shows `+ pre-pick mods`. No symmetric `+ pre-pick plugins` for paper. Symmetric mod/plugin handling missing throughout the create flow.

Bundled because all three need the same upstream-metadata work and the same provider clients.

---

## 2. Scope

| Item | Action |
|---|---|
| D#6 | Auto-pull required dependencies on add (mods + plugins) |
| D#16b | Per-mod / per-plugin update notifications + Bump UI |
| D#13b | Paper plugin pre-select in create form (symmetric to modded mods) |

**Out of scope:**

- **Optional dependency suggestions.** Per user signoff 2026-05-06: ignore optional deps entirely. No suggested-list UI.
- **Incompatible-dep gate.** Don't block adds when a required-by-something existing mod conflicts with an incoming one. The server boot will fail, user notices, fixes.
- **Dependency conflict resolution** (two mods requiring different versions of the same dep). First-write wins; whichever Add op landed in `pending` first sets the version. User adjusts via `Bump` if needed.
- **Compat-check endpoint for the version-change sheet** (Spec 3 §9). Defer to its own follow-up — it's the same upstream-query shape but with a different consumer, no need to bundle.
- **Hangar / Spigot / BukkitDev plugin sources.** Modrinth covers Paper plugins this version targets. CF too if the user picks CF as a plugin source. Other registries deferred until requested.
- **A "Mod / Plugin" trait abstraction.** Two callsites (modded mods, paper plugins) sharing helper functions, not a registry.

---

## 3. Anti-overengineering guardrails

- **Two callsites is not three.** Mods (modded) and plugins (paper) are concrete; helpers are functions, not traits.
- **No new top-level deps.** Reuses existing `mr_client`, `cf_client`, `reqwest`, `sqlx`.
- **Recursion depth cap = 5.** Hard limit on transitive dep resolution. Cycles short-circuit cleanly via a `visited: HashSet<(provider, project_id)>` per-resolution.
- **One new SQLite migration.** Single new table `mod_updates` covering both mods and plugins (provider column distinguishes; nothing actually mod-specific in the schema).
- **No new RBAC.**
- **No `Bump` UI redesign.** Just emit `PendingOp::Bump` on click; the existing apply pipeline handles the rest.

---

## 4. Design

### 4.1 Upstream dependency parsing

#### 4.1.1 Modrinth — `backend/src/modpack/mr_client.rs`

Extend `MrVersion`:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct MrDependency {
    pub version_id: Option<String>,
    pub project_id: Option<String>,
    pub file_name: Option<String>,
    pub dependency_type: String, // "required" | "optional" | "incompatible" | "embedded"
}

pub struct MrVersion {
    // ... existing fields ...
    #[serde(default)]
    pub dependencies: Vec<MrDependency>,
}
```

`#[serde(default)]` keeps existing call-sites working when the field is missing (older cached payloads, etc.).

#### 4.1.2 CurseForge — `backend/src/modpack/cf_client.rs`

CF's shape (per their public API):

```json
"dependencies": [
  { "modId": 12345, "relationType": 3 }
]
```

`relationType` enum: `1=embedded, 2=optional, 3=required, 4=tool, 5=incompatible, 6=include`.

Add a `CfDependency` deserialize struct + extend the file/version response struct.

#### 4.1.3 Internal normalised type

In `backend/src/modpack/deps.rs` (new):

```rust
pub enum DepKind { Required, Optional }

pub struct DependencySpec {
    pub provider: Provider,           // Modrinth | CurseForge
    pub project_id: String,
    pub pinned_version_id: Option<String>,
    pub kind: DepKind,
}

pub fn from_modrinth(deps: &[MrDependency]) -> Vec<DependencySpec> { ... }
pub fn from_curseforge(deps: &[CfDependency]) -> Vec<DependencySpec> { ... }
```

Filter rules:

- Modrinth `dependency_type == "required"` → `Required`
- Modrinth `dependency_type == "optional"` → `Optional` (kept in struct, **filtered out at the resolver**)
- All other Modrinth types → drop
- CF `relationType == 3` → `Required`
- CF `relationType == 2` → `Optional`
- All other CF relations → drop

### 4.2 D#6 — auto-pull required dependencies

#### 4.2.1 Resolver — `backend/src/modpack/dep_resolver.rs` (new)

```rust
pub struct ResolveContext<'a> {
    pub mc_version: &'a str,
    pub loader: &'a str,            // "fabric" | "forge" | "neoforge" | "paper"
    pub installed: HashSet<(Provider, String)>,  // (provider, project_id)
    pub pending: HashSet<(Provider, String)>,
}

/// Returns ModEntry for each required dep transitively reachable from `seed`,
/// excluding anything already installed or pending.
pub async fn resolve_required(
    seed: &ModEntry,
    ctx: &mut ResolveContext<'_>,
    http: &ModpackHttp<'_>,
) -> Result<Vec<ModEntry>>;
```

**Algorithm** (iterative BFS, depth-capped):

1. Push `seed` onto a queue. Track depth per entry.
2. Pop next. If already in `visited` → skip. If `depth > 5` → skip + log warning.
3. Fetch the entry's upstream version → read deps via §4.1.3.
4. Filter to `Required`. Skip those in `installed` or `pending` or `visited`.
5. For each remaining dep, resolve a `ModEntry`:
   - If `pinned_version_id`: fetch that version directly.
   - Else: query the project's versions list, filter by `mc_version` + `loader`, pick the newest. If none compatible, **skip + log warning** (the user will see boot-time failure if it matters; we don't fail the add over an unresolvable transitive dep).
6. Add `ModEntry` to output, push onto queue with `depth + 1`.

Returns `Vec<ModEntry>` in resolution order.

#### 4.2.2 Add path — `backend/src/routes/servers/mods.rs` and `plugins.rs`

When the route accepts an `Add` op:

1. Build `ModEntry` for the picked mod (current behaviour).
2. Call `resolve_required(&entry, &mut ctx, &http)`.
3. Append `entry` + resolved entries to `pending` as `Add` ops.
4. Persist to SQLite.
5. Return `{ "added": [list of ModEntry], "added_count": N }` so the FE knows what landed.

**Same logic in `routes/servers/create.rs`** for `initial_mods` (modded) and `initial_plugins` (paper, new in §4.4).

#### 4.2.3 Frontend — toast wording

`ModsBody.tsx` and `PaperPluginsBody`'s add path: response now has `added` array. Toast updates:

- 1 mod (no deps) → `"added X"` (current)
- 1 mod + 2 deps → `"added X + 2 dependencies"`
- 1 mod + 1 dep → `"added X + 1 dependency"`

### 4.3 D#16b — per-mod / per-plugin update notifications

#### 4.3.1 SQLite migration — `mod_updates`

```sql
CREATE TABLE IF NOT EXISTS mod_updates (
    server_id              TEXT NOT NULL,
    provider               TEXT NOT NULL,             -- 'modrinth' | 'curseforge'
    project_id             TEXT NOT NULL,
    current_version_id     TEXT NOT NULL,
    latest_version_id      TEXT NOT NULL,
    latest_version_name    TEXT NOT NULL,
    latest_published_at    TEXT,                      -- ISO 8601 string from upstream
    checked_at             INTEGER NOT NULL,          -- unix seconds
    PRIMARY KEY (server_id, provider, project_id),
    FOREIGN KEY (server_id) REFERENCES servers(id) ON DELETE CASCADE
);
```

Migration file: `backend/migrations/0007_mod_updates.sql` (next free number after `0006_drop_server_type.sql`). Generate with `sqlx migrate add mod_updates`.

The same table covers mods (modded servers) and plugins (paper) — schema is provider-agnostic.

#### 4.3.2 Poller — extend `backend/src/modpack/poller.rs`

Today the poller iterates `source_kind IN ('curseforge','modrinth')`. Add a parallel iteration for `source_kind IN ('modded','paper')`:

```rust
async fn poll_individual_mods(state: &AppState) -> Result<()> {
    let servers = sqlx::query!(
        "SELECT id, mc_version, source_kind, source_config FROM servers
         WHERE source_kind IN ('modded', 'paper')"
    ).fetch_all(&state.pool).await?;

    for srv in servers {
        let (loader, mods_or_plugins) = parse_source_config(&srv)?;
        for entry in mods_or_plugins {
            check_one_mod_for_update(state, &srv.id, &srv.mc_version, &loader, &entry).await;
        }
    }
    Ok(())
}
```

`check_one_mod_for_update`:

1. Query the project's versions list filtered by `(mc_version, loader)`.
2. Pick newest.
3. If newest's `version_id != entry.version_id` and (no row in `mod_updates` OR `latest_version_id` differs): UPSERT.
4. If newest's `version_id == entry.version_id`: DELETE the `mod_updates` row (if any).

Failures (network, rate-limit, parse) — log + continue; don't fail the whole poller.

Cadence: same hourly tick as existing poller. Reuse the existing tokio scheduled task in `poller.rs`.

#### 4.3.3 API — surface updates on server detail

`GET /api/servers/:id` extends `ServerDetail` with:

```rust
pub struct ModUpdateInfo {
    pub provider: String,
    pub project_id: String,
    pub current_version_id: String,
    pub latest_version_id: String,
    pub latest_version_name: String,
}

// in ServerDetail:
pub mod_updates: Vec<ModUpdateInfo>,
```

Computed by `SELECT ... FROM mod_updates WHERE server_id = ?` in `routes/servers/get.rs`. Empty vec when nothing pending.

Zod in `frontend/app/lib/api.ts` extends `serverDetailSchema` accordingly.

#### 4.3.4 Frontend — Mods/Plugins UI

**`ModsBody.tsx`** for modded servers:

- Each row in the installed mods list checks `detail.mod_updates.find(u => u.provider === m.provider && u.project_id === m.project_id)`.
- If found:
  - Append `↑` chip after the version name showing `<latest_version_name>`.
  - Add an `update` button on the row that fires `addPendingMod(id, { op: "bump", project_id, target_version_id: latest_version_id })`. (Backend route already handles `PendingOp::Bump`; the existing `apply` flow installs the new file and updates the mods list.)
- Top of card: when `detail.mod_updates.length > 0`, show `"X update(s) available"` summary + an `update all` button that fires Bump for each.

**`PaperPluginsBody`** — identical pattern. Same context / context refresh from Spec 1 §4.2.

**Tab badge** — already supported in `ServerDetailView.tsx:181` via `detail.update_available`. Extend the source: tab marks (`mark: true`) when either modpack `update_available` OR `mod_updates.length > 0`.

### 4.4 D#13b — paper plugin pre-select on create

#### 4.4.1 Frontend — `frontend/app/servers/new/page.tsx`

When `draft.type === "paper"`:

- After the MC picker, render `+ pre-pick plugins` button (gated on `draft.mc_version !== null`).
- Click → opens existing `CatalogSheet` with `mode="plugin"`, `loader="paper"`, `mc={draft.mc_version}`.
- Picked plugins → `draft.initial_plugins: PluginEntry[]`.
- Mirror Spec 1 §5.8 — render the picked-plugins list with remove buttons.

`PluginEntry` and `ModEntry` have the same shape today; reuse `ModEntry` until divergence forces a split. (Anti-overengineering: don't split for symmetry's sake.)

`INITIAL` adds `initial_plugins: []`. Switching `type` to / from paper resets `initial_plugins`.

`CreateServerRequest` extends:

```ts
paper?: {
  initial_plugins: PluginEntry[];
};
```

#### 4.4.2 Backend — `routes/servers/create.rs`

Same pattern as `initial_mods` (Spec 1 §5.7):

1. After SQLite write, if `paper.initial_plugins` non-empty, queue them as pending plugin Adds in `source_config`.
2. Run §4.2.2's resolver — required deps included.
3. Spawn the apply Job (same `mods_apply::run` but with `SyncTarget::Plugins`) — auto-apply on create.

Backend `CreateRequest` `paper` branch already exists for `paper` source kind; just add the `initial_plugins` field.

---

## 5. Data flow / deployment

- One new SQLite migration (`mod_updates` table).
- No Helm / RBAC changes.
- No new outbound dependencies (already calling Modrinth + CF).
- Poller tick load: per modded/paper server, one upstream call per installed mod. ATM-11-class servers have ~150 mods; with ~5 servers the user runs, ~750 calls/hour. Modrinth's anonymous rate limit is 300/min. Plenty of headroom; spread within the hour and we're fine. CF rate limits depend on the API key tier — defensive: insert a 100ms gap between calls per server.

---

## 6. Error handling

| Path | Failure | Behaviour |
|---|---|---|
| Resolver — version_id pinned but missing upstream | Skip dep + log warning. Return what was resolved. |
| Resolver — no compatible version for project_id | Skip dep + log warning. Same as above. |
| Resolver — depth > 5 | Skip + log. |
| Add path — main mod fetch fails | 502 `upstream_unreachable`, no pending insert. |
| Add path — main mod ok, dep fetch fails partway | Insert what's resolved + the seed; surface in response which deps couldn't be resolved. Toast: `"added X + 2 deps · 1 unresolved"`. |
| Poller per-mod | Log + continue. Don't fail poller iteration. |
| GET /api/servers/:id with mod_updates query failure | Log + return empty `mod_updates`. Don't fail the whole detail fetch. |

---

## 7. Testing

| Area | Test |
|---|---|
| §4.1 Modrinth deserialise | Backend unit: fixture JSON with deps → asserts MrVersion populated |
| §4.1 CF deserialise + relation mapping | Backend unit: fixture JSON for each `relationType` → asserts mapping |
| §4.2 Resolver | Backend unit: seed with 2 required deps + 1 optional → resolves 2; depth limit; cycle handling |
| §4.2 Add route | Backend integration: POST add → assert pending contains seed + deps |
| §4.3 Poller | Backend integration: seed `mods` row + mock upstream returning newer → assert `mod_updates` UPSERTed |
| §4.3 Detail surface | Backend integration: detail response includes `mod_updates` |
| §4.4 Paper create with plugins | Backend integration: POST create with `initial_plugins` → apply Job spawned |
| FE | Manual repro — list shows ↑, update button works, add toast wording |

---

## 8. Open questions

None. All locked:

1. Optional deps — ignored (no suggested-list UI).
2. Incompatible deps — silently skipped (no gate).
3. Dep conflict resolution — first-wins.
4. `mod_updates` table covers both mods and plugins.
5. Paper plugins use `ModEntry` shape (no `PluginEntry` split).

---

## 9. Future work

- **Compat-check endpoint** for Spec 3's version-change sheet — same upstream-query shape, different consumer. Cleanup task once both ship.
- **Plugin compatibility hints during version change** — wired off the compat-check endpoint above.
- **Hangar / BukkitDev plugin sources** if Modrinth coverage proves insufficient.
- **Dependency conflict resolution** with a confirm-modal flow if first-wins becomes a real problem.
- **Optional-deps suggested list** if you want it back.

---

## 10. Implementation prompt

Generated by writing-plans skill in the next workflow step (after all five specs are signed off).
