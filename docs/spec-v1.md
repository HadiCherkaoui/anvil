# Anvil v1 Specification

**Date:** 2026-05-02
**Status:** Brainstormed and signed off — ready for M1
**Scope:** M1 (walking skeleton) + M2 (lifecycle) + the M3 frontend shape that drives the M2 API design. M4 (auth) and M5 (modpacks) are out of scope here; their entries in `docs/milestones.md` are sufficient.

This doc is the source of truth for implementation. CLAUDE.md describes *how* we work; this describes *what* we build.

> **Sync note (2026-05-02):** M2 shipped with the following deltas vs the brainstormed
> spec — see `/api/cluster/capabilities`, ADR 0005, and migration 0002. The §2/§3 below
> are the M1 design; the deployed contract uses UUID `id` as the URL identifier and as
> the SQLite primary key, with `name` as a UNIQUE column; columns named `memory_mi`,
> `exposure_mode`, `storage_size_gi`, plus integer `unix-second` timestamps; and an
> additional `nodeport` column. The new `GET /api/cluster/capabilities` endpoint
> exposes `loadbalancer | nodeport | clusterip` booleans plus the
> `available_storage_classes` list (5-min cached). `POST /api/servers/:id/restart` is
> async — it returns 202 and the actual stop-wait-start runs in a tokio task.

---

## 1. UX

### 1.1 List view (`/`)

Layout **A — dense table**. One row per server, all data on screen.

Columns: `name` · `status` · `version` · `mem` · `endpoint` · `actions`

- `name` is monospaced (Fira Code), it's a kubernetes resource name.
- `status` is a colored pill (running / stopped / starting / stopping / error). Live from k8s, never cached.
- `endpoint` shows `host:port` when the LB ingress IP is assigned, `—` otherwise.
- `actions` are inline icon buttons: stop/start (context-dependent), open server, delete.
- Top bar: `anvil` brand, breadcrumb, **+ new server** button (right-aligned).

Empty state: "No servers yet." + the new-server button.

### 1.2 Create flow

**Centered modal** (option A from the brainstorm). Triggered by **+ new server**.

Fields, in order:
1. `name` — text. Validation: lowercase, no spaces, RFC 1123 label (k8s resource-name safe). Help text says so.
2. `mc_version` — select. Default to "latest stable known to the panel" (driven by `ANVIL_DEFAULT_MC_VER`).
3. `memory` — select with sensible buckets (1, 2, 4, 6, 8 GiB).

Advanced section (collapsed by default — leave room for it now, populate in M3):
- `storage_class` — defaults to `mc.storageClassName` env value.
- `storage_size_gib` — defaults to e.g. 20 GiB.
- `service_type` — defaults to `mc.serviceType` env value.

Actions: **cancel** + **create**. Submit → `POST /api/servers`. On 201, navigate to `/servers/{name}` (the detail page).

### 1.3 Per-server detail page (`/servers/{name}`)

Full route, **not a modal**. Crafty Controller-style multi-tabbed surface.

**Persistent header** (visible on every tab):
- Server name, status pill.
- Action buttons: **restart** (composed client-side: stop → poll → start), **stop**/**start** (toggles by status), kebab → **delete** (confirm modal; only enabled when stopped).
- Endpoint line below: `connect → host:port`.

**Tabs:**

| Tab      | Default? | Content                                                                                                                                                                                                                                                                                                                           |
|----------|----------|-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| Overview | ✓        | Status panel (uptime, pod name, restarts) + key/value config (version, memory, storage class, service type, created, k8s resources). Recent activity panel = audit log entries. Console preview = last 5 log lines + link to Console tab. **No "players online" stat in v1** — that's RCON-dependent (see Players tab).           |
| Console  |          | Full-height live log tail. WebSocket-driven. **No** command-input field in v1 (would need RCON — stretch for M5).                                                                                                                                                                                                                 |
| Players  |          | Online player list. Requires RCON or query protocol — **stretch for M5; M3 ships the tab hidden or shows a "requires RCON, M5+" placeholder.**                                                                                                                                                                                    |
| Settings |          | Editable: **`memory_mb`, `mc_version`** only — applies on next start (i.e., next stop→start cycle). `storage_class` and `service_type` are **immutable post-create** because changing them requires PVC/Service recreation (destructive). To change either, delete and recreate the server. Submit → `PATCH /api/servers/{name}`. |

**File browsing is intentionally not a tab.** Deferred decision (TBD): integrated mini-browser served by anvil over the PVC sub-path, or external `FileBrowser` deep-link. **Not in v1.** No Files button in the header until decided.

### 1.4 Auth

**None for v1 (M1–M3).** Single-user, LAN-only. M4 wires Authentik OIDC.

### 1.5 Visual identity

Per the design system (ui-ux-pro-max):

- **Style:** Dark Mode (OLED). Slate-900 background (`#0F172A`).
- **CTA / running:** green-500 (`#22C55E`) with subtle glow.
- **Warning / starting:** amber (`#F59E0B`), pulsing dot.
- **Stopped:** slate-500 (`#94A3B8`).
- **Error / destructive:** red-500 (`#EF4444`).
- **Typography:** Fira Sans (UI body), Fira Code (server names, IDs, IPs, log content).
- **Iconography:** Lucide-style line SVG, 24×24 viewBox, 2-stroke. **No emojis in UI.**

---

## 2. Backend HTTP API

`/api/*` paths. JSON request/response. Server `name` is the URL identifier (immutable, k8s-safe).

### 2.1 Endpoints

| Method   | Path                        | Milestone | Notes                                                                                                      |
|----------|-----------------------------|-----------|------------------------------------------------------------------------------------------------------------|
| `GET`    | `/api/health`               | M1        | `{ "ok": true, "version": "<crate version>" }` — `version` lets the panel report which build is running.   |
| `GET`    | `/api/servers`              | M1        | List for the dense table                                                                                   |
| `POST`   | `/api/servers`              | M2        | Create from modal A                                                                                        |
| `GET`    | `/api/servers/{name}`       | M2        | Detail (Overview tab)                                                                                      |
| `PATCH`  | `/api/servers/{name}`       | M2        | Edit `memory_mb` and/or `mc_version` only — applies on next start. Other fields are immutable post-create. |
| `DELETE` | `/api/servers/{name}`       | M2        | Requires status == stopped, else 409                                                                       |
| `POST`   | `/api/servers/{name}/start` | M2        | replicas → 1                                                                                               |
| `POST`   | `/api/servers/{name}/stop`  | M2        | replicas → 0                                                                                               |
| `GET`    | `/api/servers/{name}/logs`  | M2        | **WebSocket upgrade** for live tail                                                                        |
| `GET`    | `/api/servers/{name}/audit` | M2        | Audit log entries                                                                                          |

**No `/restart` endpoint.** The frontend composes it: `POST /stop` → poll status until stopped → `POST /start`.

**No `/players` endpoint** in v1. M5 if it ever lands.

**No auth endpoints.** M4.

### 2.2 Shapes

`GET /api/servers` → 200:
```json
{
  "servers": [
    {
      "name": "smp",
      "status": "running",
      "mc_version": "1.21.4",
      "memory_mb": 4096,
      "endpoint": { "host": "172.26.20.21", "port": 25565 },
      "created_at": "2026-04-18T12:00:00Z"
    }
  ]
}
```

`GET /api/servers/{name}` → 200:
```json
{
  "name": "smp",
  "status": "running",
  "mc_version": "1.21.4",
  "memory_mb": 4096,
  "storage_class": "tank",
  "storage_size_gib": 20,
  "service_type": "LoadBalancer",
  "endpoint": { "host": "172.26.20.21", "port": 25565 },
  "created_at": "2026-04-18T12:00:00Z",
  "uptime_sec": 8051,
  "pod_name": "smp-0",
  "restarts": 0,
  "k8s_resources": {
    "statefulset": "smp",
    "service": "smp",
    "pvc": "data-smp-0"
  }
}
```

`POST /api/servers` ← request:
```json
{
  "name": "survival-2",
  "mc_version": "1.21.4",
  "memory_mb": 4096
}
```
Optional fields: `storage_class`, `storage_size_gib`, `service_type`. Defaults from env per ADR 0004. Response: `201` with the same shape as `GET /api/servers/{name}`.

`PATCH /api/servers/{name}` ← partial update:
```json
{ "memory_mb": 8192, "mc_version": "1.21.5" }
```
Only `memory_mb` and `mc_version` are accepted. Any other field → `400` `field_immutable`. Response: `200` with full server. Edits do NOT apply hot — they take effect on the next start (i.e., next stop→start cycle).

`POST /api/servers/{name}/start` → `200` with full server (status flips to `starting`).
`POST /api/servers/{name}/stop` → `200` with full server (status flips to `stopping`).
`DELETE /api/servers/{name}` → `204`. `409` with `{"error": "...", "code": "must_be_stopped"}` otherwise.

`GET /api/servers/{name}/audit` → 200:
```json
{
  "entries": [
    { "ts": "2026-05-02T15:42:08Z", "action": "started", "details": null,            "actor": null },
    { "ts": "2026-05-02T15:38:51Z", "action": "stopped", "details": null,            "actor": null },
    { "ts": "2026-04-18T12:00:00Z", "action": "created", "details": "{\"mem\":4096}", "actor": null }
  ]
}
```

### 2.3 Logs WebSocket

Path: `/api/servers/{name}/logs` (HTTP upgrade to WebSocket).

Frame format: text frames, one log line per frame, raw line as emitted by the server pod (timestamps already prefixed by Minecraft's logger). The client tails on connect; server stream maps to `kube::api::Api<Pod>::log_stream` with `LogParams { follow: true, tail_lines: Some(200), .. }`.

Close codes:
- `1000` — server pod terminated normally (status went to stopped).
- `1011` — internal error (k8s call failed). Client may retry.

### 2.4 Status enum

`status: "running" | "stopped" | "starting" | "stopping" | "error"`. Derived **live**:

| StatefulSet `replicas` | `readyReplicas` | Pod phase                         | → `status` |
|------------------------|-----------------|-----------------------------------|------------|
| 1                      | 1               | Running                           | running    |
| 1                      | 0               | Pending or ContainerCreating      | starting   |
| 0                      | n/a             | Pod gone                          | stopped    |
| 0                      | n/a             | Pod still Terminating             | stopping   |
| any                    | any             | CrashLoopBackOff or other failure | error      |

Computed in the backend; no client-side derivation.

### 2.5 Errors

```json
{ "error": "<human-readable message>", "code": "<machine-readable kebab-case>" }
```

| HTTP code | error `code`s                                                             |
|-----------|---------------------------------------------------------------------------|
| 400       | `name_invalid`, `memory_invalid`, `mc_version_unknown`, `field_immutable` |
| 404       | `not_found`                                                               |
| 409       | `name_taken`, `must_be_stopped`                                           |
| 500       | `k8s_unavailable`, `db_unavailable`, `internal`                           |
| 502       | `lb_unavailable` (e.g., requested `LoadBalancer` but no provider)         |

---

## 3. SQLite schema

Two tables. **Status is never stored.** Source of truth for runtime state is the k8s API.

```sql
-- migrations/0001_init.sql

CREATE TABLE servers (
  name              TEXT PRIMARY KEY,
  mc_version        TEXT NOT NULL,
  memory_mb         INTEGER NOT NULL,
  storage_class     TEXT NOT NULL,            -- snapshotted at create time
  storage_size_gib  INTEGER NOT NULL,         -- snapshotted at create time
  service_type      TEXT NOT NULL,            -- snapshotted at create time
  created_at        TEXT NOT NULL             -- RFC3339
);

CREATE TABLE audit_log (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  ts           TEXT NOT NULL,                 -- RFC3339
  server_name  TEXT NOT NULL,                 -- NOT a foreign key — survives server deletion
  action       TEXT NOT NULL,                 -- created | started | stopped | edited | deleted
  details      TEXT,                          -- nullable JSON blob, action-specific
  actor        TEXT                           -- NULL until M4
);

CREATE INDEX idx_audit_server_ts ON audit_log (server_name, ts DESC);
```

`storage_class`, `storage_size_gib`, `service_type` are snapshotted at create time so changing the env-defaults later doesn't retroactively alter existing servers' apparent config.

`audit_log.server_name` is intentionally NOT a foreign key so audit history persists after `DELETE /api/servers/{name}`.

`sqlx` runs migrations from `backend/migrations/` on startup. Offline mode (`sqlx prepare`) is used so CI doesn't need a live DB to compile.

---

## 4. Build / serve boundary

Frontend is Next.js with `output: 'export'`; backend is axum. One binary in production.

### 4.1 Two modes

Next.js 16 disallows `rewrites()` alongside `output: 'export'`, so the dual-process HMR
proxy mode is not available. The supported workflow is:

**1. Single-binary local** (one process, no Rust recompile on FE change):
- `cd frontend && pnpm build` produces `frontend/out/`.
- `cd ../backend && cargo run --features serve-dir` → `tower-http::services::ServeDir`
  reads `frontend/out` from disk; SPA fallback serves `frontend/out/index.html` for
  unmatched non-`/api` routes.
- Browser hits `:8080` (one port for both the API and the bundle — no CORS).

**2. Release / container**:
- `cd frontend && pnpm build`.
- `cd ../backend && cargo build --release --features embed`.
- `rust_embed::Embed` derives a struct that bakes `frontend/out` into the binary at
  compile time. Same router shape as mode 1; SPA fallback serves the embedded
  `index.html`.
- This is the Dockerfile output.

### 4.2 Cargo feature flags

Two **mutually-exclusive** features, neither active by default:

```toml
[features]
default = []
serve-dir = ["tower-http/fs"]            # dev: tower-http::ServeDir
embed     = ["dep:rust-embed", "dep:mime_guess"]  # release: bake bundle into binary
```

```rust
// In src/static_serve.rs:
#[cfg(all(feature = "serve-dir", feature = "embed"))]
compile_error!("features `serve-dir` and `embed` are mutually exclusive");

#[cfg(feature = "serve-dir")]
mod serve_dir_impl { /* ServeDir + on-disk SPA fallback */ }

#[cfg(feature = "embed")]
mod embed_impl { /* rust_embed handler + embedded SPA fallback */ }
```

Enabling neither is fine — the static-serve module is then absent (used by `cargo test`).
M1 wires both paths and exercises them via the manual acceptance test; the frontend bundle
is real, not a stub.

### 4.3 Embed crate choice

**`rust-embed`** as default. `include_dir` is a fallback if `rust-embed` causes friction (rare). Picked for the friendlier API (`#[derive(RustEmbed)]` → `Files::get(path)` → `EmbeddedFile`).

### 4.4 Build pipeline order (CI + Dockerfile)

Strict: **frontend build → backend build → runtime image**. Dockerfile encodes this in three stages (`frontend-builder` → `backend-builder` → `runtime`); CI's M3 jobs do the same with artifacts passed between jobs.

---

## 5. M1 walking skeleton

The smallest deployable artifact that proves Rust → kube-rs → real cluster + SQLite migrations + the Helm install pipeline.

### 5.1 Backend layout

```
backend/
├── Cargo.toml              (axum 0.8, tokio "full", tower-http (trace, compression-gzip;
│                            +fs via the `serve-dir` feature), kube 3.x (client, rustls-tls,
│                            ring), k8s-openapi (pinned to v1_31+), sqlx (sqlite,
│                            runtime-tokio, migrate, chrono, macros), serde, serde_json,
│                            chrono, tracing, tracing-subscriber, anyhow, thiserror,
│                            rust-embed [optional], mime_guess [optional])
├── migrations/
│   └── 0001_init.sql       (servers + audit_log per §3)
├── src/
│   ├── main.rs             (tokio runtime, axum Server::bind, graceful shutdown on SIGTERM)
│   ├── config.rs           (M1 env: ANVIL_BIND_ADDR, ANVIL_DATABASE_URL,
│   │                        ANVIL_MC_NAMESPACE, ANVIL_LOG_LEVEL.
│   │                        M2 adds: ANVIL_DEFAULT_MC_VER, ANVIL_MC_STORAGE_CLASS,
│   │                        ANVIL_MC_SVC_TYPE.)
│   ├── error.rs            (AppError enum, IntoResponse impl, mapping to §2.5 codes)
│   ├── k8s.rs              (kube::Config::infer; list_statefulsets in mc namespace)
│   ├── db.rs               (SqlitePool, run migrations on startup)
│   └── routes/
│       ├── mod.rs          (Router + TraceLayer + ServeDir(frontend/out) + SPA fallback)
│       ├── health.rs       (GET /api/health)
│       └── servers.rs      (GET /api/servers — list only; POST/PATCH/DELETE land in M2)
└── tests/
    └── health.rs           (spawn app, GET /api/health, assert 200)
```

In M1 the `servers` table exists but no rows are written. `GET /api/servers` lists `StatefulSet`s in the `mc` namespace and returns them as the `{servers: []}` shape from §2.2.

### 5.2 Helm chart

`deploy/templates/`:

```
serviceaccount.yaml      (anvil's SA, in {{ .Release.Namespace }})
role.yaml                (in {{ .Values.mc.namespace }}: get/list/watch on apps/statefulsets)
rolebinding.yaml         (binds SA → Role across namespaces via RoleBinding namespace=mc.namespace)
statefulset.yaml         (anvil pod, replicas: 1, volumeClaimTemplates for SQLite at
                          /var/lib/anvil/anvil.db)
service.yaml             (type from .Values.service.type; default LoadBalancer)
configmap.yaml           (env: ANVIL_BIND_ADDR, ANVIL_DATABASE_URL, ANVIL_MC_NAMESPACE, ANVIL_LOG_LEVEL)
```

**Why a Deployment (not StatefulSet) for anvil itself:** simpler — one PVC + a single-replica Deployment with `strategy: Recreate` (so the old Pod releases the RWO volume before the new one binds it) covers the same need without the StatefulSet ceremony.

**Panel `Service` is `ClusterIP`**; the panel is fronted by an Ingress (`traefik` per the cluster profile). cert-manager TLS is gated behind `ingress.tls.enabled` — install cert-manager in the cluster first (M3 prereq).

The chart's RBAC ships M2-ready: full read/write verbs on `apps/statefulsets`, `apps/statefulsets/scale`, `core/services`, `core/persistentvolumeclaims`, `core/pods`, `core/pods/log`, `core/secrets`. Surfaced now to keep the chart contract stable through the lifecycle handlers.

### 5.3 Acceptance test (manual)

```bash
helm install anvil ./deploy -n anvil --create-namespace \
  --set image.tag=<commit-sha> \
  --set mcDefaults.storageClassName=tank
# mc namespace already exists via FluxCD (commit e244686).

kubectl port-forward -n anvil svc/anvil 8080:8080 &
curl http://localhost:8080/api/health        # → {"ok":true,"version":"0.1.0"}
curl http://localhost:8080/api/servers       # → {"servers":[]}
kubectl logs -n anvil deploy/anvil           # observe startup, migration logs, kube client init
```

### 5.4 CI

In `.gitlab-ci.yml`, M1 ships:
- `rust-fmt` (`cargo fmt --all -- --check`)
- `rust-clippy` (two passes: `--features serve-dir` and `--features embed`; `--all-features` is a hard `compile_error!` due to the static-serve mutex)
- `rust-test` (`cargo test --features serve-dir --locked`)
- `pnpm-typecheck-lint` (`pnpm typecheck && pnpm lint`)
- `pnpm-build` (artifacts: `frontend/out/`)
- `cargo-build-release` (`--features embed`, gated to `main`, needs `pnpm-build`)
- `docker-build` (gated to `main`, push `:$CI_COMMIT_SHA` and `:latest`)

---

## 6. M2 + M3 outline (driving M1's API surface; not implementation-ready)

**M2 (lifecycle):** wires up `POST/PATCH/DELETE /api/servers`, `start`, `stop`, `audit`, and the `logs` WebSocket against the schema in §3 and the API in §2. Helm chart's `Role` expands to the write verbs listed in §5.2.

**M3 (frontend):** Next.js App Router code, dense table on `/`, modal-A create flow, full-page detail at `/servers/[name]` with the four tabs from §1.3. Single-binary embed via `rust-embed` activated. SPA fallback verified end-to-end in the container.

M4 (auth) and M5 (modpacks/RCON) follow `docs/milestones.md`.

---

## 7. Cluster prereqs — status

| Prereq | Status | Action |
|---|---|---|
| `mc` namespace | **DONE** | Created via FluxCD: commit `e244686` in `homelab-k8s-fluxcd` (`apps/anvil/namespace.yaml`). Flux will reconcile shortly after the push. |
| `kube::Config::infer()` resolves | DONE | Local `~/.kube/config` already points at `homelab`. |
| LB provider for managed servers | DONE | Cilium LB IPAM, `default-pool`, 236 IPs free. |
| Default `StorageClass` | DONE | `tank` (zfs.csi.openebs.io). |
| `IngressClass` | DONE | `traefik`. |
| Authentik for M4 | DONE | Already running; M4 will add an Authentik application + drop client_id/secret into Helm values. |
| `cert-manager` | **PENDING** | Decide before M3: install now (recommended — one shot, useful for other panels) or defer and run M3 with LB-only / no TLS. |
| `VolumeSnapshotClass` for `zfs.csi.openebs.io` | Not needed | Backups are out of Anvil's scope (cluster ops). Optional cluster-side improvement, not a blocker. |

**Decommissioning `craftycontroller`** is a separate task. Anvil can coexist on the cluster; new managed servers will get fresh IPs from the same Cilium pool.

---

## 8. Out of v1 (explicit list)

- File browsing (deferred decision: integrated mini-browser vs FileBrowser; revisit in M3 close-out).
- Players tab content (RCON-dependent; M5 stretch).
- Console command-input field (RCON-dependent; M5 stretch).
- Per-user ACLs (Authentik group membership is the only access control).
- Multi-cluster support.
- Snapshots / backups / scheduled tasks.
- Operator pattern / CRDs (rejected by ADR 0001).
- Multi-replica MC (rejected by ADR 0002).
