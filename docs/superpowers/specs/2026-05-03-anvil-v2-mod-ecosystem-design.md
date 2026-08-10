<!--
SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

# Anvil v2 — Mod Ecosystem (Sub-project B)

**Date:** 2026-05-03
**Author:** Hadi (with Claude as scribe)
**Status:** Brainstormed and signed off — ready for an implementation plan
**Sub-project:** B of {A · Foundation, B · Mod ecosystem, C · Player management, D · File browser sidecar}

---

## 1. Context

A's foundation rehaul shipped in M6: design system, `CommandBar`,
multi-tab detail page, dedicated `/servers/new`, CPU control,
expanded MC versions. Three tab placeholders were intentionally
left wired ("Mods · Players · Files — coming in v2.x"); the
right-slide `Sheet` primitive shipped without a search backend.

B fills the **Mods** half: a Modrinth provider alongside CurseForge,
two new server source-kinds (`modrinth`, `modded`, `paper`), a
unified catalog search, and the modded-server individual-mod
add/remove flow. C and D land the other tab content separately.

Driving constraint: ONE cluster, ~3 friends, internal use, NOT a
SaaS. YAGNI ruthlessly. Mirror the M5 patterns wherever they fit
(provider trait, sync Job, WS-driven FSM, snapshots PVC).

---

## 2. Scope

**In scope:**

1. **Modrinth provider** — second `ModpackProvider` implementation
   for `.mrpack` modpack files, mirroring `CurseForgeServerPack`.
2. **`ModpackProvider` trait reshape** — decouple from
   `CurseForgeClient`; methods now take a `ModpackHttp<'_>` borrow
   carrying both clients.
3. **`ModrinthClient`** — typed HTTP client for `api.modrinth.com/v2`
   with caching mirroring `CurseForgeClient`. No API key required;
   sets a polite `User-Agent`.
4. **New source kinds** — `modrinth` (Modrinth modpack), `modded`
   (Fabric/Forge/NeoForge runtime + explicit mod list), `paper`
   (Paper runtime; Mods-tab placeholder for plugin browse).
5. **Runtime registry** — table of `(loader → itzg env)` mappings
   for the four loaders; pure data, ~10 lines. Reuses the existing
   `itzg/minecraft-server:java21` image with `TYPE=FABRIC | FORGE |
   NEOFORGE | PAPER`.
6. **Catalog API** — `GET /api/catalog/search?type={mod,modpack}&q=…`
   that fans out to CF + Modrinth (modpacks) or Modrinth-only (mods)
   with `loader`/`mc` facet filters. `GET /api/catalog/projects/
   {provider}/{id}/versions` for the install-version picker.
7. **Mod sync orchestrator** — `apply_mods` FSM: announcing → stopping
   → syncing-mods → starting → verifying. Reuses `UpdateGuard`,
   reuses snapshots PVC (single-Job-at-a-time mutex), reuses the
   WS-bus pattern. `POST /api/servers/{id}/mods/apply` + WS
   `/api/servers/{id}/mods/apply/stream`.
8. **Modlist persistence** — for `modded` servers, `source_config`
   carries `{runtime, mods: [{provider, project_id, version_id,
   filename, download_url, sha512}]}`. PATCH endpoint mutates the
   list (add / remove / version-bump) without applying.
9. **Mods tab body** — full add/remove/apply UX for `modded`;
   read-only inventory for `modpack` (CF + Modrinth); placeholder
   for `paper`; tab hidden for `vanilla`.
10. **Catalog Sheet body** — search input, facet filter row, scrollable
    result list with source-marker bars, hover-reveal install button.
    Reused by the create page (modpack discovery) and the Mods tab
    (mod install). Keeps the A-shipped Sheet primitive untouched.
11. **Create-page extensions** — `paper` and `modded` types (today
    the SegmentedControl says "paper, fabric/forge runtimes arrive
    in v2.1"); section 03 (source) gains a runtime-picker for
    `modded` and a Modrinth-paste flow for `modrinth`. Hooks the
    catalog Sheet into the `[browse]` button.
12. **Polish-audit §5.5 follow-through** — `cf_api_key_present` is
    finally read by the create page (gate the CF-paste flow when
    CF is disabled). `modrinth_enabled` joins it on
    `clusterCapabilitiesSchema` (always true; Modrinth needs no key).

**Out of scope (deferred or excluded):**

- **Plugin browsing for Paper.** Different ecosystem (BukkitAPI),
  Modrinth-only catalog, separate facet logic. Defer to v2.2.
- **Per-mod auto-update polling.** The hourly poller in B handles
  modpacks only. A "↑n updates available" badge for modded servers
  needs a per-mod compatibility re-check that's not warranted yet.
- **Mod jar caching** on the snapshots PVC. Each apply re-downloads;
  Modrinth jars are 1–10 MB on a cluster with bandwidth. Cache later
  if it ever hurts.
- **Rollback on sync failure.** Mods-folder corruption is recoverable
  by clicking apply again. Worlds are untouched. No tar archive of
  `mods/` taken; the M5 backup-before-swap pattern doesn't carry over.
- **Sub-project C / D** — Player management, File browser sidecar.

---

## 3. Anti-overengineering guardrails

- **Reuse `itzg/minecraft-server:java21` for every runtime.** No new
  images. `TYPE` env var switches between `VANILLA | PAPER | FABRIC
  | FORGE | NEOFORGE`. The runtime registry is a `match` arm, not a
  trait.
- **No mod-jar cache.** Re-download per apply. Anti-OE; one less moving
  part.
- **No per-mod auto-update poll.** Pinned versions. If the user wants a
  newer mod, they swap rows in the Mods tab.
- **Single sync orchestrator.** Don't introduce a second FSM type;
  reuse `UpdateGuard`, reuse `snapshot_pvc_lock`, reuse the
  WS-bus map.
- **Modrinth client mirrors `CurseForgeClient` shape.** Same caching
  TTL, same `Mutex<HashMap>`, same envelope deserialization. Don't
  abstract a "provider HTTP" trait yet — two implementations is not
  a pattern.
- **Five `source_kind`s, not seven.** `vanilla | curseforge | modrinth
  | modded | paper`. The four loaders share the `modded` kind; the
  loader is a field of `source_config`. Don't fan out discriminators.
- **`ModpackHttp` is a borrow struct, not a trait.** Two clients.
  Wrap, pass, done.

---

## 4. Design POV

Reuse A's tokens 1:1. Copper accent only on:
- the active source-marker bar in the catalog list
- the `[install]` / `[apply]` primary CTA brackets
- the active "browse" segment in any tab control

State colors stay state-only. The Mods tab header shows `n
installed · m pending` in mono; pending count uses `--color-state-warning`
when > 0. No new colors. No new fonts. No new radii.

---

## 5. Data model

### 5.1 `source_kind` expansion

| `source_kind` | `source_config` shape (selected fields) |
|---|---|
| `vanilla` | `{}` (existing) |
| `curseforge` | `{ project_id, channel, version_skip, force_version, current_version_id, current_version_name, auto_update_mode }` (existing) |
| `modrinth` | `{ project_id: String /* slug or VAQ-style */, channel, version_skip, force_version, current_version_id: String, current_version_name, auto_update_mode }` |
| `modded` | `{ runtime: "fabric"\|"forge"\|"neoforge", mc_version, mods: [Mod], pending: [PendingOp] }` |
| `paper` | `{ paper_build: String? }` (loose; Paper Build picker is its own minor surface) |

### 5.2 `Mod` (per row in `source_config.mods`)

```json
{
  "provider": "modrinth",
  "project_id": "AANobbMI",
  "project_slug": "sodium",
  "project_name": "Sodium",
  "version_id": "8VJ4TfX1",
  "version_name": "0.5.13",
  "filename": "sodium-fabric-0.5.13+mc1.21.1.jar",
  "download_url": "https://cdn.modrinth.com/data/AANobbMI/versions/...jar",
  "sha512": "abc123…"
}
```

### 5.3 `PendingOp`

```json
{ "op": "add",    "mod": { /* full Mod */ } }
{ "op": "remove", "filename": "sodium-fabric-0.5.13+mc1.21.1.jar" }
{ "op": "bump",   "filename": "...", "to_version_id": "9XYZ", "to_filename": "..." }
```

`PendingOp[]` is the "draft of pending changes" the Mods tab edits.
On apply, the orchestrator computes the resulting modlist and runs
the sync Job; on success it replaces `mods` with the new list and
clears `pending`.

### 5.4 Migration

**No new migration.** `servers.source_kind` is `TEXT NOT NULL DEFAULT
'vanilla'` (added in 0003) and accepts the three new discriminators
without a schema change. Pending modlist ops live in
`source_config` JSON, which is also `TEXT`. New fields are validated
in handlers, not via schema constraints. If implementation surfaces
a need for a column (catalog cache, audit-shape change), introduce
the migration there with a real bump — don't ship a placeholder.

---

## 6. Backend

### 6.1 `ModpackProvider` trait reshape (`backend/src/modpack/mod.rs`)

```rust
pub struct ModpackHttp<'a> {
    pub cf: Option<&'a CurseForgeClient>,
    pub mr: &'a ModrinthClient,
}

#[async_trait::async_trait]
pub trait ModpackProvider: Send + Sync + std::fmt::Debug {
    fn kind(&self) -> &'static str;
    fn project_id(&self) -> Option<String> { None }   // was Option<u32>; widened
    fn pod_image(&self) -> &str;
    fn launch_command(&self) -> Option<Vec<String>>;
    fn extra_env(&self, ctx: &ProviderContext<'_>) -> Vec<EnvVar>;
    fn boot_timeout(&self) -> Duration;

    async fn latest(&self, http: &ModpackHttp<'_>) -> Result<Option<VersionInfo>>;
    async fn fetch_url(&self, http: &ModpackHttp<'_>, version: &VersionInfo) -> Result<String>;
}
```

`project_id` widens from `u32` to `String` because Modrinth uses
opaque ids (e.g. `AANobbMI`); CF returns `Some(self.id.to_string())`.
`VersionInfo.id` likewise widens to `String`. Both changes ripple
through orchestrator/poller/jobs but each callsite is small.

`from_db` gains arms for `modrinth | modded | paper`. Each builds
the matching provider type.

### 6.2 `ModrinthClient` (`backend/src/modpack/mr_client.rs`)

Mirrors `CurseForgeClient`:

```rust
const MR_API: &str = "https://api.modrinth.com/v2";
const MR_USER_AGENT: &str = "anvil/0.5.0 (https://github.com/hadicherkaoui/anvil)";
const CACHE_TTL: Duration = Duration::from_secs(60 * 60);

pub struct ModrinthClient { /* Arc<Inner> with http + Mutex<HashMap<id, CacheEntry>> */ }

impl ModrinthClient {
    pub fn new() -> Result<Self>;
    pub async fn project(&self, id_or_slug: &str) -> Result<MrProject>;
    pub async fn list_versions(&self, project_id: &str) -> Result<Arc<Vec<MrVersion>>>;
    pub async fn search(&self, q: &SearchQuery<'_>) -> Result<Vec<MrSearchHit>>;
    pub async fn version(&self, version_id: &str) -> Result<MrVersion>;
}
```

Always-on (no API-key gate). `state.mr_client: Arc<ModrinthClient>`
non-`Option`. Polite `User-Agent`. Same `Mutex<HashMap>` cache for
`list_versions`. `search` not cached (user-driven, distinct queries).

### 6.3 New providers

- `ModrinthServerPack` (`modpack/modrinth.rs`) — modpack via
  `.mrpack`. Provider methods mirror `CurseForgeServerPack`:
  `pod_image = itzg/minecraft-server:java21`, `extra_env` sets
  `TYPE=AUTO_MODRINTH` + `MODRINTH_PROJECT={slug_or_id}` +
  `MODRINTH_DOWNLOAD_DEPENDENCIES=required` + `MEMORY` + RCON.
  itzg's `.mrpack` support owns the unzip / resource-pack-fetch /
  loader-install dance. `boot_timeout = 15min` (matches CF for
  symmetry). `latest` queries Modrinth versions filtered by
  `loaders` (always Minecraft; `.mrpack` files have a single loader)
  and channel; sorts by `date_published` desc. `fetch_url` returns
  the file URL of the picked version's primary file.

- `ModdedRuntime` (`modpack/modded.rs`) — base server with explicit
  modlist. `pod_image` = itzg image; `extra_env` sets `TYPE=FABRIC |
  FORGE | NEOFORGE` + `VERSION=<mc>` + `MEMORY` + RCON. **Mod jars
  are NOT delivered via itzg's `MODS=` env** — anvil's `mod-sync`
  Job is the sole writer to `/data/mods`, run before the StatefulSet
  scales up. This keeps anvil's modlist the unambiguous source of
  truth (itzg's `MODS=` env would re-download on every boot,
  competing with the sync). `latest` returns `Ok(None)` — mods are
  pinned; per-mod update polling is a follow-up. `fetch_url`
  `unreachable!()` like vanilla.

- `PaperServerProvider` (`modpack/paper.rs`) — `pod_image` = itzg
  image; `extra_env` sets `TYPE=PAPER` + `VERSION` + `PAPER_CHANNEL`
  if relevant. `latest`/`fetch_url` mirror vanilla: `Ok(None)` /
  `unreachable!()`. No mod/plugin add UX in B.

### 6.4 Catalog API (`backend/src/routes/catalog.rs`)

```text
GET /api/catalog/search
  query: type=mod|modpack, q=<text>, loader=fabric|forge|neoforge|paper,
         mc=<version>, channel=release|beta|alpha, limit=20, offset=0
  → { results: [{ provider, project_id, slug, name, summary,
                  icon_url, downloads, follows, project_type,
                  loaders: [..], game_versions: [..], updated }] }
  - type=modpack → fan out to CF /mods/search (gameId=432) + Modrinth /search,
                   merge, sort by combined heuristic (downloads desc).
  - type=mod    → Modrinth /search only with facets enforcing loader+mc.

GET /api/catalog/projects/{provider}/{id}/versions
  query: loader, mc (optional), channel
  → { versions: [{ version_id, version_name, channel, files: [{filename, url, sha512, primary}],
                   loaders, game_versions, date_published }] }
```

Validation: `q` length-capped to 100; `loader`/`mc`/`channel` enums
validated; provider in {`curseforge`, `modrinth`}.

`GET /api/cluster/capabilities` adds `modrinth_enabled: true` (always).

### 6.5 Mod-sync orchestrator (`backend/src/modpack/mods_apply.rs`)

```rust
pub enum ModsApplyPhase { Queued, Announcing, Stopping, Syncing, Starting, Verifying, Succeeded, Failed }

pub async fn run(state: AppState, server_id: String, guard: UpdateGuard) {
    // 1. announce (rcon best-effort, reuse orchestrator::announce_and_save)
    // 2. acquire snapshot_pvc_lock
    // 3. scale to 0; wait pod gone
    // 4. spawn mod-sync Job (alpine + curl + rm-extras). Job env carries
    //    KEEP_FILENAMES (newline-joined) and DESIRED_URLS (TSV: url<TAB>sha512).
    //    Both well under 32KB at realistic mod counts.
    // 5. wait Job
    // 6. release snapshot_pvc_lock (only data PVC needed below)
    // 7. scale to 1; wait pod Running; wait `Done (` boot marker
    // 8. persist: source_config.mods = desired, source_config.pending = []
}
```

Routes:
- `POST /api/servers/{id}/mods` — body `{ op: "add"|"remove"|"bump",
  ... }` — appends to `source_config.pending`. 204 on success.
- `DELETE /api/servers/{id}/mods/pending/{idx}` — drops a pending op.
  204.
- `POST /api/servers/{id}/mods/apply` — kicks the orchestrator.
  Returns 202 with `target` summary. Only succeeds when
  `pending.len() > 0`.
- `GET /api/servers/{id}/mods/apply/stream` — WS, mirrors update
  WS shape.

The new mod-sync `Job` builder lives next to existing builders in
`backend/src/modpack/jobs.rs` as `build_mod_sync_job`:

```rust
let cmd = "
  set -eu
  apk add --no-cache curl >/dev/null
  mkdir -p /data/mods
  # 1. delete any jar that's not in the keep list
  while IFS= read -r keep; do [ -n \"$keep\" ] && echo \"$keep\"; done <<< \"$KEEP_FILENAMES\" > /tmp/keep
  for jar in /data/mods/*.jar; do
    [ -e \"$jar\" ] || continue
    base=$(basename \"$jar\")
    grep -qxF \"$base\" /tmp/keep || rm -f \"$jar\"
  done
  # 2. download missing jars
  while IFS=$'\\t' read -r url sha; do
    [ -z \"$url\" ] && continue
    target=/data/mods/$(basename \"${url%%\\?*}\")
    [ -e \"$target\" ] && continue
    curl -fL \"$url\" -o \"$target.tmp\"
    if [ -n \"$sha\" ]; then
      echo \"$sha  $target.tmp\" | sha512sum -c -
    fi
    mv \"$target.tmp\" \"$target\"
  done <<< \"$DESIRED_URLS\"
";
```

Env vars carry the keep-list (filenames newline-delimited) and
desired-URL/SHA pairs (TSV newline-delimited). Both are < 32KB at
realistic mod counts.

### 6.6 Helm/env additions

- No new envs required. Modrinth uses no API key.
- `ANVIL_MODPACK_SNAPSHOTS_PVC` becomes unconditionally required —
  every modpack-or-modded server uses the snapshots PVC for swap /
  sync orchestration. The chart already provisions it; flip the
  config check from "required iff CF enabled" to "required, period"
  in `Config::from_env()`.

### 6.7 Validation additions (`backend/src/validation.rs`)

- `validate_runtime(&str)` — in `["fabric","forge","neoforge","paper"]`.
- `validate_modrinth_id_or_slug(&str)` — Modrinth ids are 8-char
  base62 (`[A-Za-z0-9]{8}`); slugs are `[a-z0-9_-]{1,40}`. Accept
  either.
- `validate_search_query(&str)` — non-empty after trim, ≤ 100 chars.
- `validate_catalog_provider(&str)` — in `["curseforge","modrinth"]`.
- `validate_mod_filename(&str)` — must end `.jar`, must be a basename
  (no `/`), 1..=200 chars, restricted alphabet `[A-Za-z0-9._+-]`.
  Defends the sync Job's `rm -f /data/mods/$base` from path
  injection at the DB level.

### 6.8 Poller behaviour

`backend/src/modpack/poller.rs` already skips vanilla
(`source_kind != 'vanilla'`). Tighten to "modpack-shaped only":

```sql
WHERE source_kind IN ('curseforge','modrinth')
```

`modded` and `paper` rows have no upstream pack to poll. Per-mod
update polling on `modded` rows is a B.1 follow-up.

---

## 7. Frontend

### 7.1 Catalog Sheet content

`frontend/app/components/CatalogSheet.tsx` (new, alongside `Sheet.tsx`):

- Wraps `Sheet` with `width=720`.
- Props: `mode: "modpack" | "mod"`, `loader?: Loader`,
  `mc?: string`, `onPick: (hit: CatalogHit) => void`.
- Header: search input + facet chips (loader, mc, channel) when
  `mode="mod"`. For `mode="modpack"` shows just search + a
  `[modrinth | curseforge | both]` segmented control.
- Body: scrollable result list. Each row: 4×14px source-bar (copper
  for CF, modrinth-green for MR), icon + name + author + downloads,
  hover-reveal `[install]` button.
- Loading: `Skeleton` rows.
- Empty/error: copy-only states.

Used by:
- `app/servers/new/page.tsx` — open from `[browse]` on type=modpack
  or modded (modded shows mod browse pre-create).
- `app/servers/tabs/ModsBody.tsx` — open from the `+ add mods` button
  for modded servers.

### 7.2 Mods tab body (`frontend/app/servers/tabs/ModsBody.tsx`)

Branches on `detail.source_kind`:

- `vanilla` — should not render (parent hides the tab); defensive
  placeholder in case of stale routing.
- `paper` — Card with copy: `plugin browsing arrives with the v2.2
  paper toolkit. install plugins via an external file manager for now.`
- `curseforge` / `modrinth` — read-only inventory list. Title:
  `bundled in {pack name}`. Subtitle: `pack-driven — changes get
  wiped at next pack update`. For B's scope, render
  `currentVersionName` + a `mod inventory listing v2.2 — view mods/
  via an external file manager` placeholder. Live `.mrpack` manifest read deferred
  to B.1.

- `modded` — full UX:
  - Header: `n installed · m pending` (pending in warning color
    when > 0)
  - `[+ add mods]` button → opens CatalogSheet in mode=mod
  - Two-section list:
    - **installed** rows: name, version, source-bar, hover `[remove]`
      → adds a `remove` PendingOp via PATCH
    - **pending** rows: rendered with `+`/`-`/`↑` icon, hover
      `[discard]` → DELETEs the PendingOp
  - Footer (when pending > 0): `[apply now]` `[discard all]`.
    `apply` POSTs to `/mods/apply`, opens an `UpdateSheet`-shaped
    modal subscribed to `/mods/apply/stream`.

### 7.3 Create page additions (`frontend/app/servers/new/page.tsx`)

Type SegmentedControl extends to `[vanilla, paper, modpack, modded]`:

- `paper` — Section 03 shows mc-version select only; Paper-build
  picker deferred (itzg auto-picks latest stable build).
- `modded` — Section 03 shows runtime SegmentedControl (`[fabric,
  forge, neoforge]`) + mc-version select. Optional `[+ pre-pick
  mods]` button opens CatalogSheet pre-filtered to the picked
  loader+mc; picks land in the create draft as initial `mods`.
- `modpack` — Section 03 keeps the slug-paste flow for CF and adds a
  `[browse]` button that opens CatalogSheet in mode=modpack. Picking
  a modpack auto-fills the slug + provider on the draft. Provider
  segmented control (curseforge / modrinth) toggles which paste
  format is accepted (CF accepts `slug`; Modrinth accepts `slug` or
  `id`).

The `useMcVersions` hook is reused; the runtime list is hardcoded in
the component (`["fabric","forge","neoforge"]`).

### 7.4 API client additions (`frontend/app/lib/api.ts`)

New schemas + functions:
- `catalogHitSchema`, `catalogVersionSchema`, `searchCatalog`,
  `fetchCatalogVersions`
- `modPendingOpSchema`, `modItemSchema`, `addModPending`,
  `removeModPending`, `applyMods`
- Extends `clusterCapabilitiesSchema` with `modrinth_enabled:
  z.boolean().default(true)`.
- Extends `sourceKindSchema` to
  `z.enum(["vanilla","curseforge","modrinth","modded","paper"])`.
- Extends `serverDetailSchema.source_config` parsing where the Mods
  tab needs structured access — keep `z.unknown()` at the wire
  boundary; cast through a per-kind narrower schema in the tab body.

### 7.5 Component reuse

- `Sheet` — untouched.
- `Card`, `Button`, `Tabs`, `SegmentedControl`, `Skeleton`,
  `Toast` — reused.
- New: `CatalogSheet`, `ModRow` (installed list row), `PendingRow`,
  `RuntimePicker` (thin wrapper over `SegmentedControl`), `ApplySheet`
  (ApplySheet may just be `UpdateSheet` parameterized — reuse if 1:1).

---

## 8. k8s

- No new RBAC rules. The mod-sync Job uses the same Job permissions
  the M5 swap Job already needs.
- The mod-sync Job mounts only the per-server data PVC; it does NOT
  mount the snapshots PVC (no archive needed). This means it doesn't
  need the snapshot lock, but for orchestration simplicity we still
  acquire it (one-Job-at-a-time keeps the cluster gentle).
- The itzg image already accepts `MODS=url1,url2,...` to download
  bare jars on first boot. We deliberately *do not* use that — the
  source of truth is anvil's modlist, not whatever url list the pod
  bootstrap saw. The sync Job is the only writer to `/data/mods`.

---

## 9. Migration

1. **DB.** No new migration. `source_kind` is an unconstrained `TEXT`
   column; new discriminators flow through with no schema bump.
2. **k8s reconcile.** No StatefulSet shape change. Existing servers
   stay on their current spec; new server kinds use new env layouts.
3. **Frontend.** Extending `sourceKindSchema` from a 2-variant to a
   5-variant union surfaces every exhaustive `switch` callsite as a
   typecheck error; expected. Each gets a defensive arm or hide-tab
   fallback.
4. **Snapshot PVC requirement.** Becomes unconditional. The chart
   already provisions it; only the `Config::from_env()` check needs
   to drop the CF-key-gated branch.

Zero-downtime within the homelab single-pod scope.

---

## 10. Verification (acceptance for B)

- [ ] `cargo test --all`,
      `cargo clippy --all-targets --features serve-dir -- -D warnings`,
      `cargo clippy --all-targets --features embed -- -D warnings`,
      `cargo fmt --check` — green.
- [ ] `pnpm lint`, `pnpm typecheck`, `pnpm build` — green.
- [ ] Existing CF (ATM-11) server still creates, updates, and reads
      back identically.
- [ ] **Modrinth modpack create** — paste a Modrinth pack slug in
      the create page, server creates, starts, reaches `Done (`.
- [ ] **Modded create** — pick fabric + 1.21.1, optionally pre-pick
      Sodium + Lithium + Iris in the catalog Sheet; server creates;
      starts; logs show the loader installs and mods load.
- [ ] **Mods tab pending → apply** — on a running modded server,
      install Sodium via search, see `1 pending`; click apply, watch
      the FSM Sheet stream phases; server restarts; mod loaded per
      `say modlist` RCON.
- [ ] **Strict facets** — search for `optifine` on a fabric/1.21.1
      server returns 0 hits; search for `sodium` on the same returns
      hits; switching the search facet to forge changes the hit set.
- [ ] **Runtime switch warns** — on the create page, picking a
      runtime, picking 3 mods, switching runtime opens a modal that
      lists the 3 mods and confirms before clearing.
- [ ] **Catalog Sheet** — opens from create page (modpack) and Mods
      tab (mod), source bars correct, install button picks land in
      the right place.
- [ ] **Paper create** — server creates with `TYPE=PAPER`, boots,
      Mods tab shows the placeholder, no add UX exposed.

---

## 11. Open questions

Genuinely unresolved; rest are design decisions captured above.

1. **CF modpack `download_url == null`** — CF lets project owners
   disable API distribution; the file is then unreachable from
   anvil. Today this would surface as a 502 at apply-time. Decide
   during impl: pre-flight check on resolve (warn at create-time)
   vs. fail-loud at apply-time. Lean pre-flight, with the
   `[install]` button replaced by a `× distribution disabled` badge
   on the catalog row.
2. **`ApplySheet` parameterization** — if the markup overlaps
   `UpdateSheet` ≥ 90%, parameterize via a `phaseLabels` prop;
   otherwise duplicate. Decide at implementation time when both
   are concretely shaped.
3. **Modpack-inventory live read in B?** — for B we ship the Mods
   tab on `curseforge`/`modrinth` rows as a stub showing the pack
   name + `view mods/ via an external file manager` copy. A live `.mrpack`
   manifest read (Modrinth) and a `manifest.json` read inside the
   already-downloaded ServerFiles zip (CF) would surface the full
   bundled mod list, but pulls a non-trivial new path into B. Defer
   to B.1 unless it slips into impl naturally.

---

## 12. What ships at the end of B

A user opening the panel sees:

1. The create page now offers `[vanilla, paper, modpack, modded]`.
   `modpack` discovers via paste-URL or browse-Sheet (CF + Modrinth);
   `modded` picks runtime + mc + optional pre-picked mods.
2. Modrinth modpacks (.mrpack) work end-to-end alongside CF.
3. Modded servers can install/remove individual Modrinth mods via the
   Mods tab; pending changes batch into one apply that re-uses the
   M5 stop/sync/start FSM with a new Sheet.
4. Strict (loader, mc) facet filtering on search — no incompatible
   mods reach the install button.
5. Paper boots; Mods tab shows the deferred-plugins placeholder.
6. Backend gains a `ModrinthClient`, three new providers (Modrinth,
   Modded, Paper), a unified `/api/catalog/search`, and a sync FSM
   (`/mods/apply` + WS). The existing CF + update FSM is untouched
   except for `ModpackProvider` trait method signatures.

Sub-projects C (Players) and D (File browser sidecar) layer in next.

---

## 13. Critical files modified

**Backend (Rust):**

- `backend/src/modpack/mod.rs` — `ModpackProvider` trait reshape;
  widen `project_id` to `Option<String>`; widen `VersionInfo.id` to
  `String`; add `from_db` arms for `modrinth | modded | paper`; add
  `ModpackHttp` borrow struct.
- `backend/src/modpack/mr_client.rs` — NEW. Mirrors `cf_client.rs`.
- `backend/src/modpack/modrinth.rs` — NEW. Mirrors `curseforge.rs`.
- `backend/src/modpack/modded.rs` — NEW. Runtime + modlist provider.
- `backend/src/modpack/paper.rs` — NEW. Thin Paper provider.
- `backend/src/modpack/mods_apply.rs` — NEW. Sync FSM + run loop.
- `backend/src/modpack/jobs.rs` — add `build_mod_sync_job`.
- `backend/src/modpack/orchestrator.rs` — adapt `pick_target_version`
  + `fetch_url` callsites to `ModpackHttp`. No FSM changes.
- `backend/src/modpack/poller.rs` — same callsite adapt; `modded`
  and `paper` short-circuit (no upstream poll).
- `backend/src/lib.rs` (`AppState`) — add `mr_client: Arc<ModrinthClient>`;
  drop the `Option` wrapping (`mr` always present).
- `backend/src/config.rs` — drop the `cf_api_key`-gated branch on
  `modpack_snapshots_pvc`; require it unconditionally now that
  Modrinth (always-on) needs the snapshots PVC for the apply Job.
- `backend/src/routes/mod.rs` — mount `/api/catalog/*` and
  `/api/servers/{id}/mods*` routes.
- `backend/src/routes/catalog.rs` — NEW. Search + version listing.
- `backend/src/routes/servers/mods.rs` — NEW. Pending CRUD + apply
  + apply WS.
- `backend/src/routes/servers/mod.rs` — wire the new module.
- `backend/src/routes/servers/create.rs` — add `paper` / `modded` /
  `modrinth` arms in the `server_type` switch; resolve provider for
  each.
- `backend/src/routes/servers/settings.rs` — no behaviour change in B.
  Editing `runtime` or `mc_version` post-create is intentionally
  out of scope (the picked mods are pinned to a specific
  loader+mc; switching either invalidates them in messy ways).
  To switch runtime, the user deletes and recreates. Document this
  as the chosen tradeoff in the Settings tab copy.
- `backend/src/routes/cluster.rs` — add `modrinth_enabled: true` to
  the response shape.
- `backend/src/validation.rs` — `validate_runtime`,
  `validate_modrinth_id_or_slug`, `validate_search_query`,
  `validate_catalog_provider`, `validate_mod_filename`.
- `backend/src/k8s_builders.rs` — no changes (image is the same).
- `backend/Cargo.toml` — no new deps; `reqwest` already present.

**Frontend (TS):**

- `frontend/app/lib/api.ts` — extend `sourceKindSchema`, add
  catalog/mods schemas + functions.
- `frontend/app/components/CatalogSheet.tsx` — NEW.
- `frontend/app/components/ApplySheet.tsx` — NEW (or parameterized
  reuse of `UpdateSheet`).
- `frontend/app/components/ModRow.tsx` — NEW.
- `frontend/app/components/PendingRow.tsx` — NEW.
- `frontend/app/servers/tabs/ModsBody.tsx` — replace placeholder
  with the type-branching content from §7.2.
- `frontend/app/servers/new/page.tsx` — add paper / modded type
  flows; wire `[browse]` + runtime picker per §7.3.
- `frontend/app/components/BuildSlip.tsx` — render `runtime` and
  pre-picked mod count for the modded path.
- `frontend/app/lib/use-mod-apply-stream.ts` — NEW (mirrors
  `use-update-stream.ts`).
