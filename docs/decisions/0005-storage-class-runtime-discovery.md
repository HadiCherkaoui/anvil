<!--
SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

# ADR 0005: StorageClass List via Runtime k8s API, LB Support via Helm

**Date:** 2026-05-02
**Status:** Accepted

## Context

ADR 0004 established that cluster-specific values are surfaced through Helm values and
read as env vars at startup; the backend does **not** auto-discover cluster state. M2's
new `GET /api/cluster/capabilities` endpoint stretches that stance.

The endpoint returns four pieces of information used by the "New server" modal:

1. Whether `LoadBalancer` Services work on this cluster.
2. Whether `NodePort` works.
3. Whether `ClusterIP` works.
4. The list of available `StorageClass`es and which one is annotated as default.

(1) is operationally static — a cluster either has a LoadBalancer provider or it doesn't,
and switching that is an infrastructure event the human knows about. (2) and (3) are
always available on a working k8s cluster. (4), in contrast, changes more often: the
operator adds a new tier (`fast`, `archive`), retires the openebs-hostpath fallback,
flips the default. Forcing the operator to maintain that list in `values.yaml` and
re-deploy the chart on every storage tier change is a bad UX.

## Decision

Hybrid sourcing for `GET /api/cluster/capabilities`:

| Field                         | Source                                                                                |
|------------------------------ |---------------------------------------------------------------------------------------|
| `loadbalancer`                | `ANVIL_LB_SUPPORTED` env (Helm `mcDefaults.loadbalancerSupported`, default `true`)    |
| `nodeport`, `clusterip`       | hardcoded `true` — k8s primitives                                                     |
| `available_storage_classes`   | `Api::<StorageClass>::all().list()` via kube-rs, cached 5 minutes in `AppState`        |
| `default_storage_class`       | the listed `StorageClass` annotated `storageclass.kubernetes.io/is-default-class=true`|

The runtime path requires a new `ClusterRole` granting `get/list/watch` on
`storage.k8s.io/storageclasses`. The chart adds it under `templates/cluster-role.yaml`
and `templates/cluster-role-binding.yaml`, gated by the existing `rbac.create`
toggle. Verbs are read-only.

This **deviates** from ADR 0004 for one specific resource type. ADR 0004's stance still
holds for everything else (namespace, default `StorageClass` for new servers, default
Service type, NodePort host, LB support). When tempted to add another runtime-discovery
endpoint, the bar is the same one applied here: does the value change often enough that
maintaining it in `values.yaml` is operationally painful? If not, ADR 0004 wins.

## Consequences

**Positive:**
- Operators see the live cluster's storage tiers in the panel without re-deploying.
- Adding a new `StorageClass` reflects in the UI within 5 minutes.
- The cache keeps load on the kube API server negligible (~one list every 5 min).

**Negative:**
- The chart now needs cluster-scoped RBAC. A namespace-scoped install is no longer
  enough; whoever installs Anvil must be able to bind a `ClusterRole`.
- The capabilities endpoint can fail with `502` (kube unreachable) instead of always
  returning a static value.

## References

- ADR 0004 — establishes the helm-config stance this ADR scoped-deviates from
- `backend/src/routes/cluster.rs` — the implementation
- `deploy/templates/cluster-role.yaml`, `deploy/templates/cluster-role-binding.yaml`
- `docs/cluster-profile.md` — homelab cluster reality (`tank`, `fast`, `openebs-hostpath`)
