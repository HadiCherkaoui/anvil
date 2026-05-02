# Anvil — CLAUDE.md

## What Is This

Anvil is a k8s-native web panel for managing Minecraft servers as `StatefulSet` + `PVC` +
`Service` triples on a homelab k0s cluster. It is an **imperative panel, not a Kubernetes
operator.** No CRDs, no reconciliation loop. Each user action maps to one or more direct
Kubernetes API calls.

Audience: one owner (Hadi) + ~3 friends. Single cluster, internal use, **NOT a SaaS**. Build
accordingly.

---

## Stack

| Layer       | Technology                                                       |
|-------------|------------------------------------------------------------------|
| Backend     | Rust 1.83+ · axum 0.8 · kube-rs · sqlx (SQLite, offline mode)    |
| Frontend    | Next.js 14+ (App Router, `output: 'export'`) · TypeScript · Tailwind |
| Build       | Frontend: `pnpm build` → `./frontend/out/` consumed by axum      |
| Serve       | Dev: `tower-http::services::ServeDir`. Release: `rust-embed`. Cargo feature-gated. |
| Storage     | SQLite (sqlx, offline migrations) for panel metadata + audit log |
| Auth        | None for v1. Authentik OIDC at M4.                               |
| Deploy      | Helm chart in `/deploy/` — installed via FluxCD `HelmRelease`    |

---

## Cluster Contract

Anvil runs on a single homelab k0s cluster (single node, ~5 users). **Cluster-specific values
are NOT hardcoded** — they are configured via Helm values and read by the backend at startup.
See `docs/cluster-profile.md` for the discovered baseline.

Configurable per-deployment (Helm values → backend env):

| Knob                   | Default (per cluster profile)                  | Notes                  |
|------------------------|-------------------------------------------------|------------------------|
| `mc.namespace`         | `mc`                                            | Where managed MC resources live. |
| `mc.storageClassName`  | `tank` (cluster default-marked SC, zfs.csi.openebs.io) | PVC class for managed servers. |
| `mc.serviceType`       | `LoadBalancer`                                  | Cluster-wide default; overridable per-server. |
| `mc.ingressClassName`  | `traefik`                                       | For the panel's own ingress (M3+). |

**If the cluster cannot fulfill a Service type** (e.g., `LoadBalancer` requested but no LB
provider), Anvil **surfaces this as a clear error**. It does NOT silently fall back.

---

## Architecture in One Sentence

User clicks → axum handler → kube-rs API call → StatefulSet/PVC/Service mutated.
SQLite stores panel metadata + audit log; the k8s API is the runtime state store.

---

## Hard Constraints

These are non-negotiable. If you think you have a reason to break one, ask first.

- Frontend is Next.js with App Router and `output: 'export'`. Static export only — **no API
  routes, no SSR, no middleware.** All data fetching is client-side against `/api/*`.
- Backend serves the static export. `tower-http::services::ServeDir` in dev (frontend
  rebuilds on disk, axum picks up); `rust-embed` in release for single-binary distribution.
  Feature-gated. SPA fallback: any unmatched non-`/api` GET returns `index.html`.
- Use **kube-rs typed APIs** (`kube::Api<StatefulSet>` etc.). No raw HTTP to k8s.
- **No CRDs, no controller-runtime, no reconciliation loop.**
- **SQLite for v1** via `sqlx`, offline migrations. Postgres deferred until asked.
- **Single user** for v1. OIDC is M4.
- StorageClass for managed PVCs is configurable. Default reads from `mc.storageClassName`,
  whose default in `values.yaml` is the cluster's default-marked SC at the time of writing.

---

## Anti-Overengineering Rules

This is the known failure mode for large models. Read this before writing code.

This is for **ONE cluster, ~5 users, internal use. NOT a SaaS.**

**Do NOT:**
- Add traits where there's one implementation. Trait-ify in M5 when the second provider exists.
- Build configuration systems for values that won't change.
- Create plugin/extension architectures.
- Make auth pluggable. M4 wires Authentik OIDC, full stop.
- Add background workers, queues, or event buses.
- Generate "preparedness" code or "future-proof" interfaces.

**Do:**
- YAGNI ruthlessly.
- Hardcode the namespace name `mc` (overridable via ONE env var, not a config system).
- Inline single-caller functions.
- Write the minimum that passes the test.

---

## Conventions

### Rust (backend)
- `cargo fmt --all` for formatting.
- `cargo clippy --all-targets --all-features -- -D warnings` for linting.
- `cargo test --all` for tests; integration tests live in `backend/tests/`.
- Follow Microsoft Rust Guidelines (skill `ms-rust`). Public APIs use M-CANONICAL-DOCS doc
  format (summary < 15 words, then Examples / Errors / Panics / Safety as applicable).
- Comments in American English; sparse — only for non-obvious WHY.
- Use `kube-rs` typed APIs. axum 0.8 path syntax: `{param}`, not `:param`.

### TypeScript / Next.js (frontend)
- `pnpm` is the package manager (lockfile: `pnpm-lock.yaml`).
- Strict `tsconfig.json`: `strict`, `noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`,
  `useUnknownInCatchVariables`, `noImplicitReturns`, `noFallthroughCasesInSwitch`.
  **`any` is a bug.**
- ESLint: flat config with `tseslint.configs.strictTypeChecked` + `stylisticTypeChecked`,
  plus `no-floating-promises`, `no-misused-promises`, `consistent-type-imports`.
- **Zod** for runtime validation at all network boundaries. Never trust raw JSON.
- Static export only: `output: 'export'`, `images: { unoptimized: true }`. No `app/api/*`,
  no `'use server'`, no middleware.

### Containers
- Multi-stage Dockerfile: frontend build → backend build → distroless runtime.
- Pin base image digests in production releases.

### Git
- Commits on `main`. No PRs for now (single-user homelab).
- Conventional commits style: `feat:`, `fix:`, `chore:`, `docs:`, `refactor:`.

---

## Run Commands

```bash
# Backend dev (from /backend)
cargo run                                     # API on :3000, ServeDir on ../frontend/out
cargo test --all                              # unit + integration
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings

# Frontend dev (from /frontend)
pnpm dev                                      # Next dev on :3001, proxies /api → :3000
pnpm build                                    # produces ./out/
pnpm typecheck                                # tsc --noEmit
pnpm lint

# Single-binary release build (from repo root)
cd frontend && pnpm build
cd ../backend && cargo build --release --features embed-frontend

# Container
docker build -t anvil:dev .
docker run --rm -p 3000:3000 anvil:dev
```

---

## Process Rules

- **Read-only first.** Inspect what exists before adding.
- **When uncertain, ask.** One clarifying question beats a wrong assumption.
- **Use installed skills** (`ms-rust`, `axum`, `nextjs-typescript`, `containers`, `ci-cd`,
  `premium-ui`, `superpowers:*`) before falling back to first principles.
- **End-of-milestone summary.** What was built, what was deferred, proposed next step.
  Wait for signoff before starting next milestone.
- **No application code in M0.** Bootstrap only.

---

## What Anvil Intentionally Does NOT Build

| Capability       | Why not                                | Where it lives              |
|------------------|----------------------------------------|-----------------------------|
| File management  | Already have FileBrowser               | files.cherkaoui.ch          |
| Auth UI / signup | Authentik handles it                   | Authentik (M4)              |
| Backups          | VolumeSnapshot CronJob (cluster infra) | Cluster ops, not in Anvil   |
| Multi-namespace  | One namespace is enough                | hardcoded `mc`, env-overridable |
| Operator / CRDs  | No reconciliation needed               | N/A — see ADR 0001          |

---

## Milestones

See `docs/milestones.md` for full breakdown.

- **M0** — Bootstrap: repo, docs, scaffold, brainstormed v1 spec ← *this session*
- **M1** — Walking skeleton: axum + kube-rs + SQLite, `/health` + `GET /api/servers`
- **M2** — Server management core: create/start/stop/delete via REST API
- **M3** — Frontend: Next.js UI; single-binary serving wired up via `rust-embed`
- **M4** — Auth: Authentik OIDC
- **M5** — Modpack support: Modrinth provider (first warranted trait abstraction)
