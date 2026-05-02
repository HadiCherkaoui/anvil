# ADR 0004: Cluster-Specific Values Surfaced via Helm Values, Not Hardcoded

**Date:** 2026-05-02
**Status:** Accepted

## Context

`docs/cluster-profile.md` enumerates what this cluster can and can't do (StorageClasses,
IngressClasses, LB provider, etc.). Anvil must work against this cluster today, but should
be portable enough to run on a slightly different cluster (different `StorageClass` name,
different ingress, different LB provider) without code changes.

Two ways to handle cluster-specific values:

1. **Hardcode** them in Rust constants. Fast to build. Painful if anything changes; impossible
   to test against a fake cluster.
2. **Surface them via configuration** — Helm values → `ConfigMap` / env → backend. Slightly
   more code, but everything cluster-specific lives in one place.

## Decision

All cluster-specific values are configured via the **Helm chart's `values.yaml`** and read by
the backend at startup as **environment variables**. Defaults in `values.yaml` are derived
from `docs/cluster-profile.md` but commented as overridable.

Initial set:

| Helm value path        | Backend env var          | Default (per cluster profile)                  | Per-server override?         |
|------------------------|--------------------------|-------------------------------------------------|------------------------------|
| `mc.namespace`         | `ANVIL_MC_NAMESPACE`     | `mc`                                            | No                           |
| `mc.storageClassName`  | `ANVIL_MC_STORAGE_CLASS` | `tank` (cluster-default ZFS SC)                | Yes (panel UI)               |
| `mc.serviceType`       | `ANVIL_MC_SVC_TYPE`      | `LoadBalancer`                                  | Yes (panel UI)               |
| `mc.defaultMcVersion`  | `ANVIL_DEFAULT_MC_VER`   | latest stable                                   | Yes (per-server)             |
| `panel.ingressClassName` | `ANVIL_INGRESS_CLASS`  | `traefik`                                       | n/a (panel-level)            |

The backend does **NOT** auto-discover the cluster's default StorageClass at runtime. The
Helm chart's defaults assume the operator (the human) has determined the right value via the
cluster profile.

## Rationale

- One place to change values (`values.yaml`) when porting to a different cluster — no
  recompile required.
- ConfigMap → env keeps the backend code unaware of Helm; testable by setting env vars
  directly in `cargo test`.
- Hardcoded defaults that read cluster state at runtime would create a hidden coupling
  between cluster discovery and app startup, complicating tests and breaking on permission
  errors.
- This stays YAGNI: **ONE env var per setting**, not a "cluster discovery" subsystem.

## Consequences

- The Helm chart's `values.yaml` is the canonical place for cluster-specific overrides.
- `docs/cluster-profile.md` is a *living* document. When a new milestone or reality forces a
  new config knob (e.g., `mc.ingressClassName` for M3), it gets added to both `values.yaml`
  and the table here.
- **No backwards-compat shims** for env vars that change. If we rename
  `ANVIL_MC_STORAGE_CLASS` → `ANVIL_MC_PVC_CLASS`, that's a clean break in a release; the
  Helm chart updates in the same commit.
- If the cluster cannot fulfill a request (e.g., `serviceType: LoadBalancer` but no LB
  provider), the backend surfaces the failure at server-create time — it does NOT silently
  fall back.
- Per-server overrides (StorageClass, Service type) live in the panel UI and override the
  cluster default; persisted in the `servers` SQLite row.
