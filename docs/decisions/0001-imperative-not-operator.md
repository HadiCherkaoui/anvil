<!--
SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

# ADR 0001: Imperative Panel, Not a Kubernetes Operator

**Date:** 2026-05-02
**Status:** Accepted

## Context

Anvil needs to manage Minecraft server lifecycle (create, start, stop, delete) on a k0s
cluster. Two approaches were considered:

1. **Operator pattern** — define a `MinecraftServer` CRD; run a controller with a
   reconciliation loop that continuously converges actual state to desired state.
2. **Imperative panel** — web UI issues REST calls to a backend that directly calls the k8s
   API on each user action. No loop. No CRDs.

## Decision

Imperative panel (option 2).

## Rationale

Servers start and stop on **human demand**. Between user actions there is nothing to
reconcile. The operator pattern solves the problem of autonomous self-healing, which we
don't have.

Concrete costs that option 1 incurs and option 2 avoids:

- CRD boilerplate: schema, versioning, RBAC, status subresource
- A reconciliation loop (kube-rs `Controller`) and its leader-election machinery
- Status conditions, requeueing, back-off
- Debugging complexity: with imperative code the audit log shows exactly what API call
  was made and when

`kube-rs` typed APIs (`kube::Api<StatefulSet>`) give compile-time safety equivalent to CRDs
without any of the overhead.

## Consequences

- No CRDs, no controller-runtime, no reconciliation loop.
- The k8s API is the authoritative runtime state store. SQLite holds metadata only (server
  records, audit log).
- Every user action maps directly to one or more k8s API calls. This is intentional and
  testable.
- If the Anvil pod restarts, no runtime state is lost — k8s resources are the truth.
- **Trade-off:** if a user action partially fails (StatefulSet created but PVC bind fails),
  Anvil must surface that explicitly via audit log + error response. There's no controller
  to retry — the human user retries via the UI.
