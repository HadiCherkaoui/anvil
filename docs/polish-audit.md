# Anvil — Polish Audit

**Date:** 2026-05-03
**Scope:** Pre-production polish pass before shipping to the homelab via FluxCD.
**State:** All M1–M5 milestones implemented. M5 work uncommitted on `main` (24
modified + 7 new files). `docs/milestones.md` claims `v1.0.0` is tagged but the
tag is not in `git log`.

## Audit method

Code-only walkthrough of `backend/src/`, `frontend/app/`, and `deploy/`, plus
`kubectl get` against the homelab cluster (context `k0s-cluster`, namespace
`mc` Active, `tank` storage class default — matches `docs/cluster-profile.md`).

A live UI click-through was not performed because the panel mandates OIDC at
config-load time (`backend/src/config.rs:157` and following — `ANVIL_OIDC_*`
are required even for local dev) and there is no `ANVIL_REQUIRE_OIDC=false`
toggle. Running locally would require either a live Authentik tenant or code
changes (forbidden during audit). Smoke commands are appended below for the
user to run if they want to validate any item live.

---

## 1. UI rough edges (visual / state)

1. **No `focus-visible` ring on `Button`.** `frontend/app/components/Button.tsx:30-31`
   sets only `transition-colors`; keyboard navigation is invisible. All
   variants (primary/secondary/danger) are affected.

2. **Modal close `✕` lacks a focus ring.** `frontend/app/components/Modal.tsx:52-58`
   uses raw U+2715 with `text-slate-400 hover:text-slate-100`, no visible
   focus state, no tooltip / `title` attribute.

3. **`ServerTable` "open" `<Link>` has no focus override.** `ServerTable.tsx:137-142`
   relies on the browser default outline. Because the table row sets
   `hover:bg-slate-900/40`, focused links are nearly invisible against the
   row hover state.

4. **`UserBadge` is silent on load and on error.** `UserBadge.tsx:8-20` returns
   `null` while `getMe()` is in flight (200–400 ms blank top-right corner on
   every page load) and silently swallows fetch errors with `.catch(() =>
   undefined)` — a transient network blip leaves the badge invisible
   forever.

5. **`LiveLogPanel` surfaces raw kebab-case `EndReason` strings.**
   `LiveLogPanel.tsx:104` interpolates `ended (${endedReason})` directly. The
   reasons come from `logs-stream.ts` and include `pod-unavailable`,
   `connecting`, etc. Users see internal protocol vocabulary.

6. **Endpoint formatting differs between list and detail.** `ServerTable.tsx:30`
   shows `—` when null; `servers/detail/page.tsx:147` shows `"address
   pending…"`. Same data, two presentations.

7. **Developer-facing error copy on `/servers/detail` without `?id=`.**
   `servers/detail/page.tsx:117` renders `"missing id query param"` to the
   end user.

8. **Empty design-tokens layer.** `frontend/app/globals.css` is 9 lines —
   only two CSS vars (`--font-fira-sans`, `--font-fira-code`) wired into
   `font-sans`/`font-mono` via `@theme`. Every colour, radius, and spacing
   value is an inline Tailwind utility. There is no semantic-colour layer
   (e.g. `--color-status-running`), no Tailwind `@utility`, no `@variant`.

9. **Status-dot `connecting` colour is amber, same as `reconnecting`.**
   `LiveLogPanel.tsx:97-101`. Distinguishable only by the label text — at a
   glance both look like "something's wrong."

10. **Form-validation help text appears under the field but only when
    invalid.** `NewServerModal.tsx:151-156` — there is no inline help when
    the field is empty (the rules only become visible after the user has
    typed an invalid name). The `required` attribute means the browser
    surfaces its own message, but the well-written rules text never gets
    seen on the happy path.

11. **`✕` close icon is a glyph, not an icon.** Project's visual identity
    (spec-v1 §1.5) calls for "Lucide-style line SVG, 24×24 viewBox,
    2-stroke" — Modal.tsx uses U+2715 instead.

12. **Lifecycle action errors render as a single bare `<p>` line.**
    `servers/detail/page.tsx:206-208` shows `actionError` as one red line
    above the Connect section — no border, icon, or dismiss control. Easy
    to miss after the click.

---

## 2. UX rough edges (workflows / friction)

1. **The three deferred M5 UI pieces from `docs/milestones.md` are still
   missing.** Backend is feature-complete; frontend has the data on the wire
   but no UI consumes it.
   - **NewServerModal CF sub-form.** No server-type radio / toggle, no
     project-URL input, no channel selector. `cfChannelSchema` and
     `createServerRequestSchema` (with `server_type` / `curseforge`) are
     defined in `app/lib/api.ts` and never read.
     `NewServerModal.tsx:62-251` only handles vanilla.
   - **ServerTable update-available badge.** `serverSummarySchema` already
     carries `update_available`, `update_in_progress`, `latest_version_name`
     (`api.ts:49-51`). `ServerTable.tsx` renders none of them.
   - **Detail-page tabs (Overview · Console · Update · Settings).**
     `servers/detail/page.tsx` is one linear column. No tabs primitive
     exists. `useUpdateStream` (`app/lib/update-stream.ts`) and
     `applyUpdate` / `updateServerSettings` are exported and never called.

2. **5 s polling runs on the home page even when the table is empty / all
   stopped.** `app/page.tsx:16,54-56`. Not expensive, but it never stops —
   if the user leaves the tab open, it pulls the API every 5 s forever.

3. **Detail page polls every 5 s plus the WebSocket log stream.**
   `servers/detail/page.tsx:28,85-87`. The poll is needed to catch
   status changes during start/stop, but it doesn't pause when the page
   is hidden (`document.visibilityState`) or back off.

4. **No confirmation for `start` / `stop` / `restart`.** Only `delete` has
   `ConfirmDeleteDialog`. Accidentally clicking "stop" on a running server
   while users are connected is one click away.

5. **Detail-page URL is `/servers/detail?id=<uuid>`.** Search-param routing
   was a deliberate static-export trade-off (avoids
   `generateStaticParams`), but it means the URL is not RESTful, browser
   bookmarks expose internal IDs (uuid) rather than names, and the SPA
   fallback can't catch typos because every `/servers/detail*` URL is the
   same page.

6. **Lifecycle buttons render disabled rather than hidden by status.**
   `servers/detail/page.tsx:166-203` always renders all four buttons
   (start / stop / restart / delete), greyed-out when the action isn't
   valid. Reading the button row to know "what can I do now?" requires
   scanning all four. Hiding the inactionable two would tighten it.

7. **`ConfirmDeleteDialog` requires typing the server name to confirm.**
   That's good. But there is no comparable safeguard on `stop` / `delete`
   when the wrong server is in the URL.

8. **No surfacing of the M5 update FSM in the UI.** The backend emits
   `queued → announcing → stopping → backing-up → swapping → starting →
   verifying → succeeded | rolled-back | failed` over a WebSocket. The
   frontend hook exists; nothing shows the user where in the FSM their
   update is.

9. **Auto-update mode (`disabled` / `notify` / `apply`) is settable via
   PATCH but unsurfaced in the UI.** `settingsRequestSchema` is in
   `api.ts`; no form anywhere.

10. **No "what changed in this version?" affordance.**
    `latest_changelog_excerpt` is on `serverDetailSchema` (`api.ts:70`) but
    always `null` — see §5.1 below.

---

## 3. Backend rough edges

1. **Endpoint naming inconsistency.** Plural `/api/servers`, singular
   `/api/modpack/curseforge/resolve` (`backend/src/routes/modpack.rs`).
   Action paths (`/start`, `/stop`, `/restart`) coexist with sub-resource
   paths (`/logs`, `/settings`) under `/api/servers/:id`. Trade-offs are
   reasonable; the inconsistency is real.

2. **`/api/servers/:id/update` is overloaded.** `POST` triggers an update
   (`routes/servers/update.rs`); `GET .../update/stream` is the WebSocket
   (`routes/servers/update_stream.rs`). The `update` segment is both an
   action verb and a sub-resource parent.

3. **Missing input validation.**
   - `storage_size_gi` — no bounds. `validation.rs` has no
     `validate_storage_size_gi`. `routes/servers/create.rs` accepts any
     non-negative integer.
   - Modpack `slug` — only a non-empty trim check in
     `routes/modpack.rs`. No length cap, no regex.
   - `force_version` (`routes/servers/settings.rs:65-70`) — accepted as an
     arbitrary `Option<Option<String>>` and written to `source_config` JSON
     verbatim. No format check.
   - `version_skip` list — no length cap, no per-entry validation.

4. **`wiremock 0.6` is in dev-deps and unused.** `Cargo.toml`
   `[dev-dependencies]` declares it; no integration test imports it. The
   CF-client tests it was clearly added for were never written.

5. **`ANVIL_OIDC_*` envs are required even for local dev.**
   `config.rs:157,159,161,163,164` all `.context(...)` instead of
   defaulting. There is no `ANVIL_REQUIRE_OIDC=false` toggle. Helm
   templating already handles "OIDC off" via `requireOidc` validator —
   but the binary itself can't start without OIDC values.

6. **Error variants on settings PATCH only document partial cases.**
   `routes/servers/settings.rs:28-31` documents `404` and `400 not_modded`
   but the `Internal` error from `serde_json::from_str` (line 51) and the
   `BadRequest source_config_invalid` path (line 53) are not in the
   doc-comment.

7. **Logging is sparse but uneven.** `tracing` events fire on
   `update.succeeded` / `update.failed` and inside `error.rs`, but
   handlers like `restart`, `delete`, `create` emit nothing on success.
   No "audit-shaped" log line per mutation. (The `audit_log` SQLite table
   exists; it is the audit trail, not stdout.)

8. **`restart.rs` imports `tracing::event` and `Level` but does not
   appear to use them at any logged-line site.** Dead imports — should
   trigger clippy `unused_imports` if they aren't used; quick visual
   check, not verified by build.

---

## 4. Functionality I might want that's missing

1. **CurseForge create flow in the UI.** Backend works; frontend doesn't
   expose it. (Item 2.1 above.)

2. **Update-available indicator + one-click update from the home table.**
   The data is on `serverSummarySchema`; the UI doesn't show it.

3. **A surface for the update FSM** — modal / slide-over / banner that
   subscribes to `useUpdateStream(serverId)` and shows the live phase.

4. **Settings tab on the detail page** — modpack auto-update mode +
   version-skip list editing. `PATCH /api/servers/:id/settings` exists.

5. **Player count / who's online** — RCON `list` would do it. Out of M5
   scope per spec, but it's a small handler. Decision needed: include or
   defer to M6?

6. **A "FileBrowser" deep-link from the detail page.** Listed as
   "deferred" in milestones.md; trivial once the URL pattern is locked
   down. Worth deciding.

7. **A consistent "loading skeleton" pattern.** Today the home page
   shows `loading servers…` text, the detail page shows `loading…`,
   `UserBadge` shows nothing. Three different empty-loading shapes.

---

## 5. Functionality that exists but smells speculative

1. **`changelog_excerpt`** — defined on `VersionInfo`
   (`backend/src/modpack/mod.rs:55`), written to DB by the poller
   (`poller.rs:130-137`, always `None`), surfaced in the API response
   (`routes/servers/get.rs:48,109,158` as `latest_changelog_excerpt`),
   and re-exposed in the frontend Zod schema (`api.ts:70`). No code
   path populates it from CurseForge. No CF-API call fetches a
   changelog. End-to-end dead.

2. **`force_version`** — settable via `PATCH /settings`
   (`routes/servers/settings.rs:65-70`); cleared on success by the
   orchestrator (`orchestrator.rs:592`); but `pick_target_version`
   does not read it (`orchestrator.rs:336-362`). The persisted value
   has no consumer. The orchestrator's fallback path that *would* use
   it (`serde_project_id`, `provider_to_config`) is itself dead — see
   §5.3.

3. **`provider_to_config` and `serde_project_id`** —
   `orchestrator.rs:366-384`. `provider_to_config` is hardcoded to
   return `None`; `serde_project_id` then propagates that `None`;
   `pick_target_version` errors with `"provider has no project id"`.
   The fallback "look up by id from the cached file list" path (lines
   348-361) is therefore unreachable. Today this only matters if a
   user tries to update to a version that isn't the latest — the
   handler errors out instead of using the cache.

4. **`CurseForgeClient::fetch_files` uses `pageSize=50` single-shot.**
   `cf_client.rs:208`. The CF API paginates `/mods/{id}/files`. Old
   modpacks with many historical versions silently truncate. Benign
   today (the only consumer is `latest`); becomes a bug the moment
   anyone fixes §5.2 and §5.3.

5. **`cf_api_key_present` on `ClusterCapabilities`.** Exposed by
   `routes/cluster.rs` and present in the frontend schema
   (`api.ts:82`). No frontend component reads it. The intended use
   was to gate the CF sub-form in NewServerModal — the sub-form
   doesn't exist, so the gate has no consumer.

6. **`SettingsRequest::force_version: Option<Option<String>>`.**
   The double-Option is the JSON-`null`-vs-absent distinction, which
   is correct for a PATCH — but until §5.2 is fixed, the field is
   write-only.

---

## 6. Tech debt worth flagging (NOT for me to fix unilaterally)

1. **Integration-test gap for the lifecycle, WS streams, RCON, modpack
   orchestrator, CF client cache, poller, and `PATCH /settings`.**
   `backend/tests/` has only `health.rs`, `auth.rs`, `db_servers.rs`.
   Nothing exercises the actual k8s call paths (kube is mocked in
   `auth.rs`); nothing exercises the orchestrator FSM or rollback;
   nothing exercises the WS frame contracts. The backend is large
   and largely untested at the integration level.

2. **`update-stream.ts` and `logs-stream.ts` diverge.**
   `update-stream.ts` flips `status` to `"open"` on `socket.onopen`;
   `logs-stream.ts` flips to `"live"` on the `hello` frame. Cancel
   tracking differs too (ref vs closure). Two near-identical hooks
   with subtly different lifecycle semantics — drift candidate.

3. **`time` and `chrono` both compiled in.** `chrono` is used directly
   throughout handlers; `time` is pulled in transitively by
   `axum-extra`'s cookie code. Could pin to one if it ever matters; not
   urgent.

4. **Detail-page routing is a dead-end if we ever want
   `/servers/<name>`.** Switching from `?id=` to `[id]` segment
   would touch the home table link, the detail page, the
   `BackLink`, and any future deep-links. Not bad enough to redo
   now; flagging for the record.

5. **`/api/auth/me` returns `picture: nullable` but `UserBadge`
   doesn't tolerate the picture URL hostname not being on the
   Next.js allow-list** — handled with `eslint-disable-next-line` +
   plain `<img>`. Documented in the code; not actually broken; just
   note that any future migration to `next/image` will require the
   Authentik avatar host in `next.config.ts`.

6. **`identifier.sqlite` (25 KB) is checked in at the repo root.**
   `git status` shows it as tracked. Looks like a stray dev DB.
   Worth confirming whether it should be in `.gitignore`.

---

## 7. Smoke-test commands (for the user to run live, optional)

These verify the backend behaves under a real Authentik session — useful only
if you want to validate any §3 / §5 finding live before triage.

```bash
# Start backend (assumes ANVIL_OIDC_* and a session cookie are already set up
# from M4 testing). From repo root:
cd backend && \
  ANVIL_MC_STORAGE_CLASS=tank \
  ANVIL_LB_SUPPORTED=false \
  ANVIL_NODE_HOST=192.168.1.10 \
  ANVIL_OIDC_ISSUER_URL=https://auth.cherkaoui.ch/application/o/anvil/ \
  ANVIL_OIDC_CLIENT_ID=... \
  ANVIL_OIDC_CLIENT_SECRET=... \
  ANVIL_OIDC_REDIRECT_URL=http://localhost:8080/api/auth/callback \
  ANVIL_SESSION_KEY="$(openssl rand -base64 48)" \
  cargo run --features serve-dir

# In a browser at http://localhost:8080:
# - log in via Authentik (the redirect_url has to match what Authentik allows)
# - extract the `anvil_session` cookie value, save as $S

# §1.5 — raw EndReason in the UI
# Trigger by visiting any detail page after the server pod is gone.

# §3.3 — storage_size_gi has no bounds
curl -X POST localhost:8080/api/servers \
  -H "Cookie: anvil_session=$S" \
  -H "Content-Type: application/json" \
  -d '{"name":"smoke","mc_version":"1.21.4","memory_mi":2048,
       "exposure_mode":"clusterip","storage_size_gi":100000}'

# §3.3 — slug with no length cap
curl "localhost:8080/api/modpack/curseforge/resolve?slug=$(yes a | head -10000 | tr -d '\n')" \
  -H "Cookie: anvil_session=$S"

# §5.2 — force_version is write-only
curl -X PATCH localhost:8080/api/servers/<id>/settings \
  -H "Cookie: anvil_session=$S" \
  -H "Content-Type: application/json" \
  -d '{"force_version": "anything"}'
# 204 — but the orchestrator never reads it back.
```

---

## Where this audit ends

This document is the input to Phase 2 (TRIAGE). I will walk you through it
item by item and capture KEEP / CHANGE / REMOVE / ADD / DEFER decisions in
`docs/polish-plan.md`.
