# ADR 0002: StatefulSet Replicas as Lifecycle Primitive

**Date:** 2026-05-02
**Status:** Accepted

## Context

Each Minecraft server needs:
- One JVM process at a time (compute)
- Persistent world data (storage)
- A "stopped" state with zero compute cost but data preserved

Anvil maps user actions (start / stop) to k8s primitives. We need a primitive that represents
both running and stopped states cleanly.

Options:

1. **Bare Pod** — simple YAML; Pods are ephemeral by design. "Stopping" means deleting and
   recreating, which complicates PVC handling.
2. **Deployment** — manages Pod replicas, but pairs poorly with PVCs (no
   `volumeClaimTemplates`); manual PVC binding is fragile.
3. **StatefulSet (replicas: 0 or 1)** — stable Pod name, `volumeClaimTemplates` tightly
   couples PVC lifecycle, native scale-to-zero.

## Decision

One **StatefulSet per server** with `replicas` ∈ {0, 1}.

- **Start** = patch `spec.replicas = 1`
- **Stop** = patch `spec.replicas = 0`
- **Delete** = ordered teardown (`Service` → `StatefulSet` → `PVC`); guard requires
  `replicas == 0` first.

## Rationale

- `kubectl scale statefulset <name> --replicas=0` is the native k8s expression of "stop
  without losing data". The PVC remains bound; pod is gone; storage cost continues, compute
  cost is zero.
- StatefulSet pod names are stable (`<name>-0`), making log tailing and `exec` deterministic.
- `volumeClaimTemplates` couples PVC lifecycle to the StatefulSet — one resource definition
  for compute + storage instead of three.
- Bare Pods can't represent "stopped" cleanly: stopping requires deletion, which complicates
  re-creation if the PVC binding wasn't carefully handled.
- Deployments don't pair well with PVCs.

## Consequences

- Pod name for each server is always `<server-name>-0`. Safe to hardcode in log/exec paths.
- **No `replicas > 1` logic anywhere.** One MC JVM per server, always. If we ever need
  multi-replica MC (e.g., proxy + backends), that's a different abstraction — not a
  generalization of this one.
- Transient state (`replicas: 1` requested but Pod is `Pending` while pulling the image) is
  observable through the k8s API but not stored in SQLite. The frontend handles it as
  "Starting…" derived from `status.readyReplicas`.
- Delete must be **ordered and explicit** in handler code. The `StatefulSet`'s
  `volumeClaimTemplates` does NOT cascade-delete PVCs by default; we delete the PVC
  ourselves to be intentional about data destruction.
