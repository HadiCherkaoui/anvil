# Anvil Milestones

## M0 — Bootstrap

**Goal:** Repo, docs, scaffold, brainstormed v1 spec. No application code.

Deliverables:
- [x] GitLab repo created and cloned
- [x] CLAUDE.md, README.md, .gitignore
- [x] docs/cluster-profile.md — cluster capability matrix
- [x] docs/milestones.md (this file)
- [x] ADRs 0001–0004
- [x] Skeleton: backend/, frontend/, deploy/ Helm chart, scripts/, Dockerfile, .gitlab-ci.yml
- [x] First commit on origin/main
- [x] docs/spec-v1.md — brainstormed v1 spec, signed off

---

## M1 — Walking Skeleton

**Goal:** Prove end-to-end k8s API access from Rust before building lifecycle endpoints.
A single deployable artifact that lists `StatefulSet`s from the configured namespace.

Deliverables:
- Rust workspace + axum server: `GET /health` → 200
- `kube::Config::infer()` working in-cluster (kubeconfig fallback for local dev)
- `GET /api/servers` → list of `StatefulSet`s in `mc` namespace, JSON
- SQLite initialized with `sqlx` offline migrations: `servers` + `audit_log` tables (minimal)
- `cargo test` passes with at least one integration test against a kube context
- Container builds; runs in cluster via Helm
- `tower-http::ServeDir` integrated for `./frontend/out` (empty for now), feature-flagged

**Not in M1:** create/start/stop/delete (M2), frontend code (M3), auth (M4)

---

## M2 — Server Management Core

**Status:** complete (tagged v0.2.0). Backend + frontend shipped together — the M2 task
brief absorbed M3's frontend scope.

**Goal:** Full server lifecycle from creation to deletion, drivable from a Next.js UI.

Deliverables:
- [x] `POST /api/servers` — creates `Secret` + `StatefulSet` (replicas=0) + `Service`
      in configured namespace; returns 202
- [x] `POST /api/servers/:id/start` — patches `/scale` subresource to replicas=1
- [x] `POST /api/servers/:id/stop` — patches `/scale` subresource to replicas=0
- [x] `POST /api/servers/:id/restart` — async stop → wait pod gone → start (90 s timeout);
      returns 202 with `{status: restarting}`
- [x] `DELETE /api/servers/:id` — `StatefulSet` → wait pod gone → PVC → `Service` →
      `Secret` → SQLite row; rejects 409 `must_be_stopped` if running
- [x] `GET /api/servers/:id` — detail with live status + endpoint
- [x] `GET /api/servers/:id/logs` — last 200 lines, snapshot (not streaming; M3 adds WS)
- [x] `GET /api/cluster/capabilities` — exposure-mode availability + StorageClass list,
      cached 5 min (ADR 0005)
- [x] All mutations write `audit_log` entries
- [x] `GET /api/servers` JOINs SQLite metadata with live k8s status (StatefulSet + Pod
      + Service per server)
- [x] Migration 0002: UUID PK, `memory_mi`, `exposure_mode`, `nodeport`, unix-second
      timestamps
- [x] Helm: `mcDefaults.nodeHost`, `mcDefaults.loadbalancerSupported`, `ClusterRole`
      for StorageClass list
- [x] Frontend: server list, "New server" modal, per-server detail page, polling
- [x] v0.2.0 tag

**Not in M2:** WebSocket log streaming, RCON command input (M3+), auth (M4),
modpacks (M5)

---

## M3 — Live logs (WebSocket) + RCON

**Status:** in progress (2026-05-02). Note: the original M3 brief covered the static
frontend, which M2 absorbed. This milestone re-targets the live-feedback loop the panel
was missing.

**Goal:** Watch a server boot in real time and send RCON commands from the panel.

Deliverables:
- [x] Amend M2 builders: per-server **headless Service** `mc-<id>-headless` (clusterIP:
      None, port 25575); StatefulSet.serviceName points at it; container exposes both
      25565 and 25575. Public Service unchanged (RCON stays internal).
- [x] `GET /api/servers/{id}/logs/stream` — WebSocket. Typed JSON frames:
      `hello` / `log` / `error` / `end`. WS-level Ping every 30 s. On pod restart,
      server-side re-attaches up to 60 s and emits a fresh `hello`. On 60 s
      pod-unavailable, sends `end{pod-unavailable}` and closes.
- [x] `POST /api/servers/{id}/rcon` — per-request connection, 5 s end-to-end timeout.
      Reads the password from the `mc-<id>-rcon` Secret. Returns 409
      `server_not_running` if the pod isn't Running. Password is never echoed.
- [x] Frontend `useLogsStream` hook: Zod-validated frames, exponential-backoff
      reconnect (1 s → 30 s cap), bounded buffer (2000 lines), ANSI strip, line
      classification (info/warn/error).
- [x] Detail page: live tail panel with auto-scroll-unless-user-scrolled-up; RCON
      command form below; status dot for connection.
- [ ] End-to-end verification on the homelab cluster (boot, `say hi`, pod-delete
      reconnect, 409 while stopped, no leaked tasks).
- [ ] v0.3.0 tag

**Not in M3:** Authn/authz on the WS handshake (M4), replaying missed lines after a
pod restart (the gap is shown; M2 snapshot endpoint covers backlog), persistent log
archive, RCON over TLS / port-forward fallback (we trust the cluster network).

---

## (delivered in v2) Integrated file browser per server

Shipped as an in-panel browser over each server's data PVC (list / upload /
download / rename / delete via a helper Pod) — see the Files tab. The earlier
idea of deep-linking to an external file manager was dropped in favour of the
built-in browser.

---

## M4 — Authentication (Authentik OIDC)

**Status:** complete (tagged v0.4.0). The panel is no longer open; every
`/api/*` request except `/api/health` and the public auth routes requires a
valid session cookie. Manual setup steps live in `docs/authentik-setup.md`.

**Goal:** Lock the panel behind Authentik SSO.

Deliverables:
- [x] OIDC authorization-code-with-PKCE flow against Authentik (openidconnect 4)
- [x] HS256 session JWT in an `HttpOnly; Secure; SameSite=Lax` cookie, signed
      with `ANVIL_SESSION_KEY`; provider metadata cached for 1h
- [x] `require_session` middleware on all `/api/*` except `/api/health`,
      `/api/auth/login`, and `/api/auth/callback`
- [x] `GET /api/auth/login` (302 to Authentik), `GET /api/auth/callback`
      (mints session JWT), `GET /api/auth/me`, `POST /api/auth/logout`
      (returns Authentik end-session URL)
- [x] `ANVIL_ALLOWED_SUBS` allowlist (empty = any authenticated Authentik
      user with the application bound)
- [x] Frontend: 401-redirect chokepoint in `app/lib/api.ts`; floating
      `<UserBadge>` widget (avatar + name + sign-out)
- [x] Helm chart: `oidc.*` values, chart-managed Secret (or `existingSecret`),
      `anvil.requireOidc` validator that fails the render when oidc-on
      without TLS-on
- [x] `docs/authentik-setup.md` runbook
- [x] v0.4.0 tag

**Not in M4:** per-user ACLs (YAGNI), role hierarchies beyond Authentik groups,
revocation lists (sessions live for 8h or until `ANVIL_SESSION_KEY` rotates)

---

## M5 — Modpack Support (CurseForge ServerFiles)

**Status:** complete (tagged v1.0.0). Original problem statement met — ATM-11 (and any other
CurseForge pack with a ServerFiles file) can be created, started, polled for new versions,
and updated with backup + rollback from the panel.

**Goal:** Drop Crafty Controller. Servers launch from CurseForge ServerFiles, the panel polls
upstream hourly, one-click update with backup/swap/start/verify and tar-based rollback on
failure.

Deliverables:
- [x] `ModpackProvider` trait + `VanillaProvider` (M2 refactor) + `CurseForgeServerPack`
- [x] `CurseForgeClient` (reqwest, x-api-key header, 1h `/files` cache, slug→id resolver)
- [x] Hourly `tokio` poller writes `modpack_versions`; `auto_update_mode=apply` fires the
      orchestrator inline
- [x] `POST /api/servers/:id/update` orchestrator FSM:
      announce (RCON, best-effort) → stop → backup Job (tar to `mc-snapshots`) →
      swap Job (download + preserve/wipe + server.properties merge) → start →
      verify boot ("Done (" within `provider.boot_timeout`) → update DB
      → rollback Job restores last archive on failure
- [x] `GET /api/servers/:id/update/stream` WebSocket — typed phase frames
      (queued → announcing → … → succeeded | rolled-back | failed)
- [x] `PATCH /api/servers/:id/settings` — auto_update_mode, version_skip, force_version
- [x] `GET /api/modpack/curseforge/resolve?slug=…` — backend resolves URL slug to project id
      so the API key never leaves the panel
- [x] Migration `0003_m5_modpack.sql`: `servers.source_kind` + `modpack_versions` +
      `idx_audit_server_action`
- [x] Helm chart 1.0.0: `secrets.cfApiKey{,ExistingSecret}`, `modpack.{pollIntervalMinutes,
      snapshotsPvc}`, Job RBAC, `mc-snapshots` PVC template, `cf-api-key` Secret + envFrom
- [x] Frontend Zod schemas + API functions for the modpack endpoints; `useUpdateStream` hook
      mirroring the `useLogsStream` reconnect/buffer pattern
- [x] v1.0.0 tag

**Not in M5:** Modrinth provider (the trait is in place; M6+ adds it alongside).
VolumeSnapshot path — the cluster lacks a configured `VolumeSnapshotClass` for
`zfs.csi.openebs.io`; tar-to-PVC is the v1 path. Auto-apply maintenance windows (YAGNI).
CurseForge search UI (project ID / URL paste only).
Per-server backup retention controls — hardcoded to keep last 3.

**Deferred UI** (the API is in place — incremental work for the next pass):
NewServerModal CF sub-form (project_id input + URL paste + channel selector),
ServerTable update-available badge, server detail page tab bar with Update + Settings tabs.

---

## v2 series — foundation rehaul + capability bump

The deferred UI from M5 plus the larger capability gaps (mods, players, files)
were decomposed into four sub-projects:

- **A — Foundation rehaul** ✅ (2026-05-03): design system, CommandBar, multi-tab
  detail page, `/servers/new` page, CPU control, expanded MC versions. Spec:
  `docs/superpowers/specs/2026-05-03-anvil-v2-foundation-design.md`.
- **B — Mod ecosystem** ✅ (2026-05-03): Modrinth provider, Paper / Modded
  runtimes, unified `/api/catalog/search`, mod-sync FSM with `/mods/apply` WS,
  `CatalogSheet`, full `ModsBody` for modded servers. Spec:
  `docs/superpowers/specs/2026-05-03-anvil-v2-mod-ecosystem-design.md`.
- **C — Player management** ✅ (2026-05-03): full Players tab body, RCON-only,
  bulk-read endpoint + 11-variant action endpoint + broadcast endpoint, recent
  activity from pod logs. **No new RBAC, no new DB migration, no new
  dependencies.** Spec:
  `docs/superpowers/specs/2026-05-03-anvil-v2-player-management-design.md`.
- **D — File browser** ✅ (2026-05-04): in-anvil FS endpoints over `kube-rs`
  pods/exec — list / download / upload (≤ 100 MiB, streamed) / mkdir / rename /
  delete (single + recursive). Stopped servers handled by a lazy-spawned
  helper Pod (`mc-{id}-files`) torn down on Start. **Adds one new RBAC
  verb (`pods/exec: create`) and extends the existing pods rule with
  `create+delete` for the helper Pod, one Helm value (`mcDefaults.alpineImage`),
  no DB migration, no new top-level dependencies (kube `ws`+`runtime`
  features enabled, `async-stream@0.3` added).** Spec:
  `docs/superpowers/specs/2026-05-03-anvil-v2-file-browser-design.md`.
