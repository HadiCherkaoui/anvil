# Cluster Profile

This document captures what `anvil`'s target cluster can and can't do. Cluster-specific
values in code go through Helm values + backend config — **do NOT bake findings here as
constants in code.**

Last verified: **2026-05-02** against cluster `homelab` (`kubectl` against context default).

## Topology

- **Distribution:** k0s v1.35.1
- **Nodes:** 1 (single control-plane node `homelab`, Ubuntu 24.04, internal IP 172.26.20.250)
- **HA:** No. If the node goes down, everything goes down. Acceptable for v1.

## Capability Matrix

| Capability                              | Present? | Notes                                                                                       |
|-----------------------------------------|----------|---------------------------------------------------------------------------------------------|
| `Service: LoadBalancer`                 | **Yes**  | Cilium LB IPAM via `ciliumloadbalancerippool/default-pool` (236 IPs free).                  |
| L2 advertisement                        | **Yes**  | `ciliuml2announcementpolicy/default-l2-policy`. Existing assignments: 172.26.20.17–.19.    |
| Default `StorageClass`                  | **Yes**  | `tank` — `zfs.csi.openebs.io`, marked `storageclass.kubernetes.io/is-default-class=true`. |
| Alternative `StorageClass`              | Yes      | `fast` (zfs.csi.openebs.io, pool `fast/k0s`). `openebs-hostpath` (openebs.io/local).        |
| `VolumeSnapshot` CRDs                   | Partial  | CRDs (`vsclass`, `vsc`, `vs`) installed, but **no `VolumeSnapshotClass` configured** and snapshotter pod not observed in `kube-system`. **Snapshots will not work without further setup.** |
| Default `IngressClass`                  | **Yes**  | `traefik` (only `IngressClass` present). Traefik runs in `traefik` namespace.              |
| `cert-manager`                          | **No**   | Namespace empty. **Required if/when M3 adds TLS to the panel ingress.**                     |
| OIDC provider for M4                    | **Yes**  | Authentik installed in `authentik` namespace.                                               |
| GitOps tooling                          | Yes      | FluxCD installed (`flux-system`).                                                           |
| Existing MC tooling                     | Yes      | `craftycontroller` running with LB IP `172.26.20.17` (ports 25565, 25566). **Anvil replaces this** — coexists fine until decommissioned. |
| `mc` namespace                          | **Yes**  | Flux-owned (`apps/anvil/namespace.yaml` in `homelab-k8s-fluxcd`, commit `e244686`). The Anvil chart does **not** create it. |

## Network Notes (informational, not for hardcoding)

- LoadBalancer pool advertises in the homelab subnet. Each MC server Service will get its own
  external IP from `default-pool`.
- Pod / service CIDRs are Cilium / k0s defaults; not relevant to anvil application code.

## Implications for `anvil`

1. **MC server `Service` defaults to `LoadBalancer`** — cluster supports it. Each server
   gets its own external IP.
2. **MC server PVCs default to `tank`** — ZFS-backed, thin-provisioned. `Helm values` exposes
   this; per-server override available in the panel UI.
3. **Snapshots are NOT a v1 feature.** Backups (M5+) require provisioning a
   `VolumeSnapshotClass` for `zfs.csi.openebs.io` first.
4. **Single-node** ⇒ no `topologySpreadConstraints` or anti-affinity needed. Don't add that
   complexity.
5. **`cert-manager` not installed.** If M3 adds TLS via Traefik ingress, install
   `cert-manager` first or skip ingress and rely on the LoadBalancer IP.
6. **Timezone defaults to `Etc/UTC`.** The chart's `mcDefaults.timezone` is the universal
   default; the homelab `HelmRelease` overrides to `Europe/Zurich` so MC / JVM / installer
   logs match the operator's wall clock. Other operators set their own IANA zone.
7. **Container images for managed servers are configurable.** `mcDefaults.itzgImage`
   (every MC `StatefulSet`) and `mcDefaults.busyboxImage` (backup/restore Jobs) default
   to tag-pinned upstream values; pin by digest at install time in production.

## Prereqs to Resolve Before M1

These are decisions for Hadi to make at the end of M0:

- [ ] **`cert-manager`** — install now (needed for M3 TLS on the panel ingress) or defer?
- [ ] **`VolumeSnapshotClass` for `zfs.csi.openebs.io`** — create now (enables future
      backup workflows) or defer to M5+?
- [x] **`mc` namespace** — Flux-owned, pre-existing on the cluster. Helm chart trusts the prereq and does NOT create it.
- [ ] **Existing `craftycontroller`** — coexistence is fine; anvil + crafty share the cluster
      until decommission.

## Re-running Discovery

```bash
kubectl get nodes -o wide
kubectl get storageclass -o wide
kubectl get ingressclass
kubectl get ciliumloadbalancerippool -A
kubectl get volumesnapshotclass
kubectl get pods -n cert-manager
kubectl api-resources | grep -iE 'volumesnapshot|loadbalancer'
```

If any of these change, update this document and the Helm chart's `values.yaml` defaults
(see ADR 0004).
