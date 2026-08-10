<!--
SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

# Anvil v2 — Foundation rehaul (Sub-project A)

**Date:** 2026-05-03
**Author:** Hadi (with Claude as scribe)
**Status:** Brainstormed and signed off — ready for an implementation plan
**Sub-project:** A of {A · Foundation, B · Mod ecosystem, C · Player management, D · File browser sidecar}

---

## 1. Context

Anvil v1.0 (M1–M5) ships full server lifecycle, OIDC, live logs/RCON, and a CurseForge ServerFiles modpack pipeline. The audit (`docs/polish-audit.md`, 2026-05-03) found:

- Three deferred M5 UI surfaces missing (CF sub-form, update-available indicator, detail-page tabs).
- Speculative backend code that's wired in but never read (`force_version`, `changelog_excerpt`, `provider_to_config`).
- A flat detail page (no tabs) that can't host the additional surfaces (mods, players, files, settings) the user wants.
- Visual rough edges (focus rings, raw kebab-case strings shown to users, silent `UserBadge` failures, etc.).
- Multiple blocked features: CPU control, expanded MC version coverage, mod browsing across CF + Modrinth, multi-runtime (Fabric / NeoForge / Forge / Paper), in-app file browser, player management.

Rather than incrementally polishing a v1 surface that needs to host much more, **v2 is a foundation rehaul plus a tightly-scoped capability bump**. The remaining capabilities (mod ecosystem, players, files) are large enough to deserve their own design cycles and are decomposed into Sub-projects B, C, D.

---

## 2. Scope (this spec — Sub-project A)

**In scope:**

1. **New design system** — tokens, typography, accent, spacing, motion. Workshop-aesthetic POV (see §4).
2. **New component primitives** — Button (variants), Modal, Sheet (right slide-over), Card, Tabs, Badge, Toast, Dropdown, Skeleton, IconButton, Tooltip, SegmentedControl, RangeSlider, Select. Replace the existing M3 ad-hoc components.
3. **New layout** — top-level `CommandBar` (path-style breadcrumb) **replaces** the floating `UserBadge`. No sidebar.
4. **Server list page** (`/`) redesigned — whole-row clickable, source markers (CF/Modrinth/local color bars), inline update indicators (`↑n` after the name), summary line, action menu reveal-on-hover.
5. **Detail page** (`/servers/<name>`) redesigned — prominent header with status + key stats, conditional update banner, **full tab strip**: `overview · console · mods · players · files · settings`. Tabs for Mods / Players / Files render placeholder bodies in A; their real content ships in B/C/D respectively. Console + Settings ship complete in A.
6. **Create page** (`/servers/new`) — replaces the modal. Two-column: live "build slip" sticky on the left, numbered sections on the right, bottom action bar with live validity status.
7. **Update FSM display** — wires the existing M5 WebSocket (`/api/servers/{id}/update/stream`) into the UI as a slide-over surface that runs on click of `[update]` from the banner or Mods tab.
8. **CPU control** — new server resource field (`cpu_millicores` int), wired through `create` + `PATCH /settings`, surfaced in the create page Resources section and the detail page header stats line.
9. **Expanded MC version list** — replace the hardcoded 6-version allowlist in `validation.rs` with the current Mojang manifest scope (last ~20 release versions). Source: Mojang version manifest (`https://launchermeta.mojang.com/mc/game/version_manifest_v2.json`).
10. **Polish-audit items folded in** — every UI/UX item from §1, §2 of `polish-audit.md` lands in this sub-project (focus rings, kebab-case→friendly copy, hide-vs-disable, silent-loading-fix on `UserBadge`, consistent skeleton pattern, etc.). Backend §3, §5 items are addressed where they touch the changed surfaces; remaining items are §10 cleanup.

**Out of scope (explicitly deferred):**

- **Sub-project B** — Mod ecosystem: Modrinth provider, runtime registry (Fabric/NeoForge/Forge/Paper images + launch commands), browse-catalog backend (CF + Modrinth search), individual-mod install/remove flow, runtime/version compatibility matrix. The Mods tab in A renders a "coming in v2.1" placeholder; the browse slide-over **component** is built (it's a primitive) but the search backend isn't.
- **Sub-project C** — Player management: RCON-driven endpoints (`/players`, kick, ban, op/deop, gamemode), Players tab content. Tab placeholder in A.
- **Sub-project D** — File browser sidecar: which web-FS image, sidecar lifecycle (always-on vs. on-demand with TTL), auth model, Files tab content. Tab placeholder in A.
- **Routing rewrite to fully-RESTful URLs** — see §6 for the specific compromise we're picking.

---

## 3. Anti-overengineering guardrails

Per CLAUDE.md and the user's explicit guidance during brainstorming:

- **One signature accent.** Copper `#d29150` for the brand mark, primary-action borders, and active-tab indicators. Nothing else gets it. State colours are for state.
- **No design system framework** (no Radix, no shadcn, no Headless UI). Tailwind v4 utilities + project-local primitives. The v1 already has Tailwind v4; we extend, we don't replace.
- **No new top-level dependencies without asking.** Mojang version manifest fetch reuses `reqwest` already in `Cargo.toml`. The drag-handle / range-slider primitive is hand-rolled (input[type=range] is sufficient).
- **No unrelated refactor.** If a backend file isn't touched by this sub-project, leave it alone — it goes in §10 polish-audit cleanup or a later sub-project.
- **No tests for code that didn't change.** The audit's §6.1 integration-test gap is real but it is **not in scope here**. Adding tests is a separate effort.

---

## 4. Design POV

**Anvil is a workshop tool, not a SaaS dashboard.**

The user is a maker who cares about every layer matching what he meant. The panel must feel the same: type-led, monochrome with one warm signature accent, status communicated by weight + a single small dot, no glass / gradients / icon-soup. The data is the design.

**Explicitly rejected (the AI-slop list):**
- purple→blue gradients · text gradients of any kind
- glassmorphism · frosted-glass cards
- Lucide-icon sidebars with rect-highlight active items
- Inter / Poppins / Roboto
- default Tailwind palette as-is
- pill-shaped status badges with shadows
- 3D abstract figures, faceless humans
- "+ new" buttons with the green Tailwind primary
- centered-everything layouts
- generic kebab→hamburger hover-reveal patterns

**Mockup references (in `.superpowers/brainstorm/30364-1777801494/content/`, not committed):**
- `workshop-direction.html` — server list, sets the principles
- `detail-overview.html` — detail page Overview tab
- `detail-mods.html` — detail page Mods tab (modpack-driven view; B fills the actual content)
- `browse-catalog.html` — right slide-over; primitive in A, search backend in B
- `create-page.html` — full create page with build slip

---

## 5. Design tokens

**CSS variables in `frontend/app/globals.css` under `@theme {}`** (Tailwind v4 syntax). The existing globals.css is 9 lines; this expands it.

```
/* Surfaces */
--color-bg:       #0a0a0c   /* page background */
--color-surface:  #0e0f12   /* cards, hover rows */
--color-elevated: #15161b   /* hover state on already-elevated */
--color-border:   #1e1e22   /* default 1px border */
--color-border-soft: #15151a /* dividers and table row separators */
--color-border-strong: #2e2e34 /* hover-state border */

/* Text */
--color-text-primary: #f2f2f5   /* headings, server names */
--color-text-body:    #e6e7eb   /* default body */
--color-text-muted:   #8a8a92   /* secondary stats, labels */
--color-text-dim:     #6b6b73   /* tertiary, placeholders */
--color-text-faint:   #4f4f56   /* uppercase labels, hairline copy */

/* Signature accent (used surgically) */
--color-accent:        #d29150   /* copper — brand, primary CTA borders, active-tab */
--color-accent-bg:     #1a1208   /* primary CTA background */
--color-accent-border: #3a2a18   /* primary CTA border (rest state) */
--color-accent-bracket: #6e4a26  /* the [ ] glyphs around CTA labels */

/* State (only for state) */
--color-state-running: #8aaf45  /* slightly off the default Tailwind green */
--color-state-warning: #cdaa66  /* transitional, beta-channel marker */
--color-state-error:   #c97f6f  /* muted, not aggressive */
--color-state-running-glow: rgba(138,175,69,0.18)  /* dot box-shadow */

/* Source markers (catalog) */
--color-source-curseforge: var(--color-accent)
--color-source-modrinth:   #6cb04a
--color-source-local:      var(--color-text-faint)

/* Typography */
--font-mono: 'Fira Code', ui-monospace, monospace
--font-sans: 'Fira Sans', system-ui, sans-serif

/* Radius (we keep things squared) */
--radius-none: 0
--radius-sm:   2px   /* mod-source bars, fine accents */
--radius-md:   4px   /* default buttons, inputs, panels */

/* Spacing scale — not new; we just commit to consistent use of Tailwind's */
/* Existing 4-based scale: 4 8 12 16 20 24 32 48 64 */

/* Motion */
--motion-fast:    120ms
--motion-default: 150ms
--motion-slow:    250ms  /* sheet slide-in */
```

**Type scale** (Fira Sans for prose, Fira Code for data — including server names and table cells):
- 24/600 — page / server H1 (Fira Code for server names, letter-spacing -0.01em)
- 16/600 — section heads
- 14/500 — dense subhead, modal titles
- 13/400 — default body
- 12/400 — cmdbar / table cells / form labels-when-mono
- 11/500 — uppercase labels (letter-spacing 0.06–0.08em)
- 10/500 — section numbers, slip section labels (letter-spacing 0.10–0.12em)

---

## 6. Component primitives

Every primitive lives in `frontend/app/components/<Name>.tsx`. All follow the same shape: typed props, no internal state for visual variants, `data-testid` consumable, no Radix.

| Primitive | Purpose | Notes |
|---|---|---|
| `Button` | Primary / secondary / danger / ghost | `variant`, `size`. Primary CTA renders `[label]` brackets via inner spans (visual signature). Adds `focus-visible:ring`. |
| `IconButton` | Icon-only square button | `aria-label` required. Same focus-visible ring. |
| `Modal` | Center modal | Replaces v1 `Modal.tsx`. Same Esc + backdrop-close. Adds focus-trap, focus ring on `✕`, swap `✕` U+2715 for an SVG. |
| `Sheet` | Right slide-over | New. 480 / 640 / 720px width prop. Esc to close. Backdrop scrim. Focus-trap. Used by browse-catalog (primitive only in A; B fills it) and update-FSM display. |
| `Card` | Bordered panel | Optional `header` prop. Pads 18px / 16px. |
| `Tabs` | Tab strip | Active tab gets copper underline + bright text. Optional `count` per tab. Optional `mark` dot for "things changed". |
| `Badge` | Inline status mark | Variants: `running` / `stopped` / `starting` / `error` / `update`. Used in tables and stat lines. |
| `Toast` | Bottom-right transient | New. For lifecycle action confirmations ("server stopped"). 4s default. |
| `Dropdown` | Action menu (`⋯`) | Used for row action overflow on the server list. |
| `Skeleton` | Loading placeholder | Replaces all `loading…` text strings. Three variants: `row`, `block`, `text`. |
| `Tooltip` | Hover annotation | Used on copy-button, status-dot ambiguity, etc. |
| `SegmentedControl` | release/beta/alpha-style toggles | Used on settings strip + create page. |
| `RangeSlider` | Memory + CPU | Wraps `input[type=range]` + tick marks + value display. |
| `PathBreadcrumb` | The cmdbar's path segments | Renders `anvil / servers / <name>` with the SVG anvil mark. |
| `BuildSlip` | Live spec sheet on the create page | Reads draft state via React Context (`CreateFormContext`), renders sections per §8.3. |

Existing primitives **kept and reused**: `StatusBadge` (renamed to fit the new `Badge`), `ConfirmDeleteDialog` (uses new `Modal`), `LiveLogPanel` (re-skinned), `RconCommand` (re-skinned).

Existing primitives **removed**: `UserBadge` (replaced by `CommandBar` user segment).

---

## 7. Layout architecture

### 7.1 `CommandBar` (replaces sidebar + UserBadge)

Top-of-page, ~50px tall, full width, hairline border-bottom. Left: anvil SVG mark + brand + path segments. Right: user identity + logout icon. Renders on every authenticated page.

**Path segment routing:**
- `/` → `anvil / servers`
- `/servers/<name>` → `anvil / servers / <name>` — segments are clickable up the tree
- `/servers/new` → `anvil / servers / new`
- `/servers/<name>/<tab>` → `anvil / servers / <name> / <tab>` — tab is part of the path so deep-links work

### 7.2 Top-level navigation

The cmdbar's path segments **are** the navigation. The detail page's tabs **are** the secondary navigation. There is no third level. The future "mods catalog" and "activity / audit log" surfaces (in B and later) get top-level paths (`anvil / mods`, `anvil / activity`).

### 7.3 URL strategy

**Decision:** use server **name** in URLs, not UUID. Names are already `UNIQUE` in SQLite. This makes URLs readable (`/servers/atm-11`) and deep-links shareable.

**Static-export compatibility:** Next.js `output: 'export'` requires statically known paths. We use `[name]` dynamic segment with `generateStaticParams: () => []` (no paths pre-generated). The backend's existing SPA fallback (`tower-http::services::ServeDir` `not_found_service` returning `index.html` with 200) handles every dynamic URL. **This needs verification during impl** — if Next.js 16 rejects empty `generateStaticParams`, fall back to `?name=<name>` query param routing.

The current `?id=<uuid>` URL is a v1 carry-over that is being explicitly retired; one-time redirect from `/servers/detail?id=<uuid>` to `/servers/<name>` is added for stale bookmarks.

---

## 8. Page architecture

### 8.1 Server list (`/`)

Single page, polled at 5s when document is visible (per audit §2.2 — wrap with `document.visibilityState`).

- **Section 1 — summary line.** "4 servers · 3 running · 1 stopped · 2 updates available". `[+ new]` button right-aligned (route to `/servers/new`).
- **Section 2 — table.** Columns: name · status · runtime · version · memory · address. Whole row click → detail. `:hover` reveals action cluster on right (start/stop/restart/⋯). Empty state with anvil-mark placeholder + helpful copy.

**Update indicator:** `↑n` after the server name where `n = count of pending updates`. Click goes to detail page Mods tab.

**Source colour bar:** 4×14px copper / green / gray bar to the left of names indicating CF / Modrinth / local-modded. Vanilla rows omit the bar.

### 8.2 Detail page (`/servers/[name]`)

Polled at 5s for status changes; WebSocket for log/update streams. Pauses poll when document hidden.

- **Section 1 — server header.** Large server name + glow status dot + uptime. Stats line below: runtime · version · cpu (n / m) · memory · storage · players. Action cluster top-right: stop / restart / ⋯ overflow (the overflow contains: edit settings, delete (when stopped), open in console).
- **Section 2 — update banner (conditional).** Renders only when `update_available && !update_in_progress`. Shows `current → target (channel)`. Actions: skip · `[update]`.
- **Section 3 — tab strip.** `overview · console · mods · players · files · settings`. Tab path encoded in URL (`/servers/<name>/<tab>`).
- **Tab bodies:**
  - `overview` ✓ Two-column: left = connection card + 8-line console preview; right = at-a-glance stats + recent activity. Polled at 5s.
  - `console` ✓ Reuses M3 `LiveLogPanel` + `RconCommand`, full-height layout, raw `EndReason` strings replaced with friendly text via a lookup map.
  - `mods` placeholder. Renders "Mod browsing arrives in v2.1" + the modpack identity (if any) read-only. (B replaces this.)
  - `players` placeholder. (C replaces this.)
  - `files` placeholder. (D replaces this.)
  - `settings` ✓ Edit memory / cpu / mc_version (applies on next start). Modpack auto-update mode + version-skip list (when source_kind != vanilla). Read-only storage info. Danger zone (delete server when stopped).

**Update FSM display:** when a user clicks `[update]`, opens a `Sheet` (right, 640px) that subscribes to `useUpdateStream(serverName)`. Renders the FSM phases with the active phase highlighted and the failed phase surfaced if rollback fires. Existing M5 backend untouched.

### 8.3 Create page (`/servers/new`)

Replaces the M2 modal. No data prefilled (always a fresh draft). Two columns:

- **Left (sticky 320px)** — `BuildSlip` reads form context, renders sections {identity, source, resources, storage, network} as a spec sheet. Empty fields show `—`. State badge top-right: `draft` / `valid` / `submitting`.
- **Right** — six numbered sections (`01..06`), top-down. Section 03 (source / pack) is **conditional on type**:
  - `vanilla` → MC version select only (from expanded version list)
  - `paper` → Paper version select (placeholder; real Paper version-list integration is part of B)
  - `modpack` → "paste a CF/Modrinth project URL" input + `[browse]` (opens `Sheet`; B fills) + version select + channel auto-track (release/beta/alpha)
  - `modded` → runtime select (forge/fabric/neoforge) + MC version select (mod selection happens later on the Mods tab; B fills)
- **Bottom action bar** — left side shows live validity (`● all sections valid · ready to forge` / `× missing: name`); right side `cancel` + `[create server]`.

**On submit:** POST `/api/servers` with the new payload (see §9.2). On 201, navigate to `/servers/<name>` (Overview tab).

**For type=modpack** and **type=modded**, the version/source pickers in A render in degraded form (paste-URL works, browse-button opens an empty `Sheet`). Full functionality lands in B.

---

## 9. Backend changes

### 9.1 Database

New migration `0004_m6_cpu_field.sql`:

```sql
ALTER TABLE servers ADD COLUMN cpu_millicores INTEGER NOT NULL DEFAULT 1000;
```

`1000` = 1 core, matching how kube-rs takes `Quantity::from("1")` for cores. Backfill default of 1000 keeps every existing v1 server alive without manual intervention.

### 9.2 API

**Create payload** — extends the existing `POST /api/servers` body:

```json
{
  "name": "atm-11-friends",
  "mc_version": "1.21.1",
  "memory_mi": 8192,
  "cpu_millicores": 4000,        // NEW
  "exposure_mode": "loadbalancer",
  "storage_size_gi": 50,
  "storage_class": "tank",       // optional, omit for default
  "server_type": "modpack",      // existing M5 enum, extended in B
  "curseforge": {                // optional sub-form, populated by B
    "project_id": 123,
    "file_id": 456,
    "channel": "release"
  }
}
```

**Settings PATCH** — `PATCH /api/servers/{id}/settings` accepts `cpu_millicores` and `memory_mi` (already had `force_version`, `auto_update_mode`, `version_skip`). Both apply on next start, never live.

`force_version` stays in the request shape (it's wired across DB + API + frontend already), but **the dead `pick_target_version` fallback path is fixed in this sub-project** — see §9.4 — so writing `force_version` actually does something.

**Cluster capabilities** — `GET /api/cluster/capabilities` adds `available_cpu_cores` (read from `Node.status.allocatable.cpu`, summed across schedulable nodes). Surfaces in the create page Resources section help text.

**Versions** — new endpoint `GET /api/cluster/mc-versions` returns the cached Mojang version manifest (release-only, last 20 versions). Cached 24h. Replaces the hardcoded `KNOWN_MC_VERSIONS` allowlist in `validation.rs` — validation now hits the cache. Cache miss falls back to the hardcoded list (panel keeps working offline).

**No change** to the OIDC / auth surface. No change to log/RCON/restart/start/stop/delete/update endpoints.

### 9.3 k8s

`k8s_builders.rs` writes a `resources.limits.cpu` quantity from `cpu_millicores` (e.g. `4000m`). No `requests` block — keeps the M2 decision (commit `2a4c711`).

### 9.4 Audit-driven cleanup folded in

From `polish-audit.md`:
- §3.3 — add `validate_storage_size_gi` (10..=500), tighten `slug` (≤200 chars), tighten `force_version` (regex `^[A-Za-z0-9._-]{1,128}$`), cap `version_skip` length (≤50).
- §3.4 — remove unused `wiremock` from `[dev-dependencies]`.
- §5.1 — drop `latest_changelog_excerpt` from `serverDetailSchema` and `routes/servers/get.rs` (no producer, no consumer; killing the dead end-to-end path is cleaner than building a CF changelog fetcher in this sub-project).
- §5.3, §5.4 — replace `provider_to_config` / `serde_project_id` with proper `ModpackProvider::project_id()` trait method on the CF impl. Wire `pick_target_version`'s fallback to actually work, and switch `CurseForgeClient::fetch_files` from single-page-50 to a paginated loop with a 500-file cap.
- §5.5 — keep `cf_api_key_present` (B will read it).
- §6.6 — remove `identifier.sqlite` from the repo, add to `.gitignore`.

Items deferred to a later cleanup pass: §3.1 (path-naming inconsistency — touches frontend too, defer), §3.2 (`/update` overload — defer), §6.3 (chrono/time consolidation — minor), §6.5 (`next/image` allow-list — non-issue).

---

## 10. Frontend changes — concrete file deltas

| File | Change | Verb |
|---|---|---|
| `frontend/app/globals.css` | Expand `@theme` block with the §5 token scale | edit |
| `frontend/app/components/Button.tsx` | Add `focus-visible:ring`, primary `[label]` brackets, ghost variant | edit |
| `frontend/app/components/Modal.tsx` | Focus trap, SVG `✕`, focus ring on close | edit |
| `frontend/app/components/Sheet.tsx` | New right slide-over primitive | new |
| `frontend/app/components/Card.tsx` | New | new |
| `frontend/app/components/Tabs.tsx` | New | new |
| `frontend/app/components/Badge.tsx` | New (replaces `StatusBadge` callsite-by-callsite) | new |
| `frontend/app/components/Toast.tsx` | New | new |
| `frontend/app/components/Dropdown.tsx` | New | new |
| `frontend/app/components/Skeleton.tsx` | New | new |
| `frontend/app/components/Tooltip.tsx` | New | new |
| `frontend/app/components/SegmentedControl.tsx` | New | new |
| `frontend/app/components/RangeSlider.tsx` | New | new |
| `frontend/app/components/CommandBar.tsx` | New (replaces `UserBadge.tsx`) | new |
| `frontend/app/components/BuildSlip.tsx` | New (create page only) | new |
| `frontend/app/components/UserBadge.tsx` | Delete | delete |
| `frontend/app/layout.tsx` | Mount `CommandBar` instead of `UserBadge` | edit |
| `frontend/app/page.tsx` | Redesigned server list per §8.1 | edit |
| `frontend/app/servers/[name]/page.tsx` | Detail page with tab strip | new |
| `frontend/app/servers/[name]/[tab]/page.tsx` | Tab body router | new |
| `frontend/app/servers/detail/page.tsx` | Replaced by `[name]`; left in place during migration window with redirect | edit |
| `frontend/app/servers/new/page.tsx` | New full-page create flow | new |
| `frontend/app/components/NewServerModal.tsx` | Delete (callsite removed; modal route gone) | delete |
| `frontend/app/components/StatusBadge.tsx` | Delete (replaced by `Badge`) | delete |
| `frontend/app/components/ServerTable.tsx` | Redesigned per §8.1; renamed `ServerList.tsx` | rename + edit |
| `frontend/app/components/LiveLogPanel.tsx` | Re-skin + friendly `EndReason` strings | edit |
| `frontend/app/components/RconCommand.tsx` | Re-skin | edit |
| `frontend/app/lib/api.ts` | New schemas: `mcVersionsSchema`, `cpuCoresField`. Drop `latest_changelog_excerpt`. | edit |
| `frontend/app/lib/update-stream.ts` | Align `onopen`/`hello` semantics with `logs-stream.ts` | edit |
| `frontend/next.config.ts` | Verify `output: 'export'` + `[name]` segment compatibility | verify |

---

## 11. Migration

1. **DB.** Migration `0004_m6_cpu_field.sql` runs on startup. Backfill: `cpu_millicores = 1000` for every existing row.
2. **k8s reconcile.** Existing servers continue running with their current StatefulSet specs (no live mutation). When the user next clicks "restart" or PATCHes settings, the new spec lands.
3. **Frontend URL change.** `/servers/detail?id=<uuid>` is kept for one release with a client-side redirect (read `id`, look up name, replace history with `/servers/<name>`). Deleted in the release after.
4. **Mod-pack data.** No schema change to `modpack_versions` / `source_config`. The B sub-project may extend.

Zero-downtime within the homelab single-pod scope. No external clients call the API; we don't need API versioning.

---

## 12. Verification (acceptance criteria)

End-to-end checks the implementation must pass before A is "done":

- [ ] `cargo test --all`, `cargo clippy --all-targets --features serve-dir -- -D warnings`, `cargo clippy --all-targets --features embed -- -D warnings`, `cargo fmt --check` — green.
- [ ] `pnpm lint`, `pnpm typecheck`, `pnpm build` — green.
- [ ] An existing v1 vanilla server loads in the v2 UI without errors and reflects backfilled `cpu_millicores=1000`.
- [ ] Create flow (vanilla, type=vanilla): `/servers/new` → name + 1.21.4 + 4 GiB + 2 cores + 20 GiB tank + clusterip → submits → navigates to `/servers/<name>` → row appears in the list with the correct CPU stat.
- [ ] Detail page deep-link (`/servers/<name>/console`) loads on the Console tab directly.
- [ ] Action keyboard nav: Tab to lifecycle button → focus-visible ring is **clearly** visible.
- [ ] `LiveLogPanel` `EndReason` text shows e.g. "the server's pod went away" not `pod-unavailable`.
- [ ] An update available on a CF server renders the banner; clicking `[update]` opens the FSM Sheet and frames stream live to it.
- [ ] PATCH-settings memory + cpu changes apply on next start (verify via the `kubectl get sts mc-<id> -o yaml` resources block).
- [ ] `kubectl get pod` shows the panel container running on `cargo build --release --features embed` deployment.
- [ ] Storage write paths: data volume PVC stays attached through restart; world preserved across stop/start cycle.
- [ ] Visual signature check: the rendered UI matches the workshop direction — no purple/blue gradients, no glassmorphism, no Inter/Poppins, no Lucide-style icon sidebar with rect-highlight, copper accent only on brand mark / primary-CTA borders / active-tab underline.
- [ ] Expanded MC version list: `GET /api/cluster/mc-versions` returns ≥15 release versions and the create page version select shows them.

---

## 13. Open questions

(Captured for the implementation plan to answer.)

1. **Tab body routing.** Are the per-tab routes proper Next.js segments (`/servers/[name]/[tab]/page.tsx`) or query params (`/servers/<name>?tab=mods`)? Segments are cleaner; query is more static-export-safe. Default: segments with `generateStaticParams: () => []`; fall back to query if segments break the export.
2. **`Toast` for which actions?** Lifecycle (start/stop/restart/delete) → toast. Settings PATCH → toast. Update FSM → no toast (it has its own Sheet). Confirm this list during impl.
3. **Confirmation dialogs.** Audit §2.4 flagged "no confirmation on start/stop/restart". Decision: keep delete as the only confirm-required action; start/stop/restart are reversible. Add a Toast for the lifecycle actions instead, with an undo affordance for stop only (5s window).
4. **Mojang version manifest fetching.** Where does it get cached? `AppState` field? Or a dedicated `state.versions` Mutex<HashMap>? Lean toward `AppState`.
5. **Removing v1 components vs. soft-deprecating.** Default: remove (anti-overengineering). Keep `LiveLogPanel`/`RconCommand` since they're being re-skinned in place.

---

## 14. What ships at the end of A

A user opening the panel after this sub-project ships sees:

1. A workshop-aesthetic server list with whole-row click, source markers, update indicators.
2. A multi-tab detail page where Overview, Console, and Settings work; Mods/Players/Files show "coming in v2.1" placeholders.
3. A new dedicated `/servers/new` page with the build-slip metaphor, including CPU control and the expanded MC version list.
4. Click-to-update on banner-flagged servers, with the FSM streaming live in a slide-over.
5. Every audit-flagged UI rough edge resolved.
6. Backend: clean of dead/speculative M5 code; CPU support; expanded MC versions; tightened validation; `wiremock` and `identifier.sqlite` gone.

Sub-projects B, C, D layer in mod browsing, player management, and the file-browser sidecar — each with its own design cycle.
