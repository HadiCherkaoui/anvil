# Anvil

k8s-native Minecraft server panel. Manages servers as `StatefulSet` + `PVC` + `Service` triples
on a homelab k0s cluster.

**Status:** Pre-alpha · M0 bootstrap

## What It Does

- Create, start, stop, and delete Minecraft servers on demand
- Each server runs as a StatefulSet (replicas: 1) backed by a PVC
- "Stopped" means `replicas: 0` — world data persists at zero compute cost
- Web panel: Rust/axum REST API + Next.js (static export) frontend, served as a single binary

## Stack

- **Backend:** Rust · axum · kube-rs · sqlx (SQLite)
- **Frontend:** Next.js (App Router, `output: 'export'`) · TypeScript · Tailwind
- **Cluster:** k0s · Cilium (CNI + LB IPAM) · Traefik · OpenEBS ZFS · Authentik (M4)

## Docs

- [CLAUDE.md](CLAUDE.md) — operating rules and conventions
- [docs/cluster-profile.md](docs/cluster-profile.md) — what this cluster can/can't do
- [docs/milestones.md](docs/milestones.md)
- [docs/decisions/](docs/decisions/) — architecture decision records

## Non-Goals

- Not an operator (no CRDs, no reconciliation loop)
- Not multi-tenant
- No built-in file management → [FileBrowser](https://files.cherkaoui.ch)
- No custom auth UI → Authentik OIDC (M4)
