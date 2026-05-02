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

## (deferred) FileBrowser deep-link per server

A "Files" link from the detail page to FileBrowser at `files.cherkaoui.ch`. Trivial
once the URL pattern is locked down. Skipped from M2/M3 because the brief never
specified the host pattern.

---

## M4 — Authentication (Authentik OIDC)

**Goal:** Lock the panel behind Authentik SSO.

Deliverables:
- OIDC authorization-code flow against Authentik
- JWT validation middleware in axum
- Frontend redirects unauthenticated users to Authentik
- Friends gain access via Authentik group membership

**Not in M4:** per-user ACLs (YAGNI), role hierarchies beyond Authentik groups

---

## M5 — Modpack Support

**Goal:** Servers can launch from Modrinth (and later CurseForge) modpacks. First milestone
where a trait abstraction is warranted (two providers).

Deliverables:
- `ModpackProvider` trait: `search`, `resolve`
- Modrinth provider implementation
- Server-create form: modpack picker, version selector
- CurseForge provider (stretch)
