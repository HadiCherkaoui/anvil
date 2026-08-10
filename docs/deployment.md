<!--
SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

# Deployment Guide

This document is the operator's guide to running Anvil on a Kubernetes
cluster. For what Anvil *is* and how it's built, see
[`architecture.md`](architecture.md).

## Contents

- [Cluster prerequisites](#cluster-prerequisites)
- [Quickstart](#quickstart)
- [Helm values reference](#helm-values-reference)
- [RBAC](#rbac)
- [Persistence and sizing](#persistence-and-sizing)
- [Ingress and TLS](#ingress-and-tls)
- [OIDC (Authentik)](#oidc-authentik)
- [Modpack support (CurseForge / Modrinth)](#modpack-support-curseforge--modrinth)
- [File browser](#file-browser)
- [Upgrades](#upgrades)
- [Troubleshooting](#troubleshooting)

## Cluster prerequisites

| Capability | Required for | Notes |
|---|---|---|
| **Kubernetes 1.31+** | core | `kube-rs` 3.x targets this. Older clusters will probably work but aren't tested. |
| **A namespace for managed servers** | core | Default name `mc`. The Helm chart does **not** create it — provision via Flux/Argo/`kubectl` first. |
| **A default `StorageClass`** | core | Used for managed-server PVCs. Listed in `mcDefaults.storageClassName` (required value). Anvil also needs a class for its own SQLite PVC (`persistence.storageClassName`, optional). |
| **Volume expansion** | grow disk feature | The chosen StorageClass must have `allowVolumeExpansion: true` for `PATCH /api/servers/{id}/storage` to work. |
| **A `LoadBalancer` provider** | recommended | MetalLB, Cilium LB IPAM, cloud LB, etc. If absent, set `mcDefaults.loadbalancerSupported=false` and use `NodePort` mode for managed servers. |
| **Authentik (or another OIDC provider)** | auth | Anvil only ships an Authentik runbook. Other providers should work as long as they support Authorization Code + PKCE and expose `end_session_endpoint` in discovery. |
| **A snapshots PVC** | modpack updates | Anvil needs a shared PVC named `mc-snapshots` in the `mc` namespace for tar backups. The chart provisions it when `modpack.snapshotsPvc.enabled=true`. |
| **`metrics-server`** | per-server CPU/memory chart | Optional; without it, `/api/servers/{id}/metrics` returns nulls and the UI hides the chart. |
| **Cluster-scoped RBAC** | core | The panel needs read access to `StorageClass` and `Node` cluster-wide. See [RBAC](#rbac). |
| **`cert-manager`** | TLS | Only if you turn on `ingress.tls.enabled`. Otherwise skip. |

## Quickstart

The chart is published as an OCI artifact in the project's container
registry. There is no public release on Helm Hub.

```bash
# 1. Pull the chart
helm pull oci://gitlab.cherkaoui.ch/hadicherkaoui/anvil/anvil --version 1.0.10

# 2. Create the managed-servers namespace (chart does not create it)
kubectl create ns mc

# 3. Provision a session key (≥ 32 bytes, base64)
SESSION_KEY=$(openssl rand -base64 32)

# 4. Install
helm install anvil anvil-1.0.10.tgz \
  --namespace anvil --create-namespace \
  --set mcNamespace=mc \
  --set mcDefaults.storageClassName=tank \
  --set mcDefaults.loadbalancerSupported=true \
  --set mcDefaults.alpineImage="alpine@sha256:<digest>" \
  --set persistence.size=1Gi \
  --set oidc.enabled=true \
  --set oidc.issuerUrl="https://authentik.example.com/application/o/anvil/" \
  --set oidc.clientId="<authentik-client-id>" \
  --set oidc.clientSecret="<authentik-client-secret>" \
  --set oidc.redirectUrl="https://anvil.example.com/api/auth/callback" \
  --set oidc.sessionKey="$SESSION_KEY" \
  --set ingress.enabled=true \
  --set ingress.host="anvil.example.com" \
  --set ingress.tls.enabled=true \
  --set modpack.snapshotsPvc.enabled=true \
  --set modpack.snapshotsPvc.size=100Gi
```

Verify the pod comes up cleanly:

```bash
kubectl -n anvil get pods
kubectl -n anvil logs deploy/anvil
```

A successful boot logs `anvil.startup` with the bind address and the MC
namespace. Then the `/api/health` probe should return:

```bash
kubectl -n anvil port-forward svc/anvil 8080:8080
curl http://localhost:8080/api/health   # → {"ok":true,"version":"1.0.0"}
```

## Helm values reference

Full default values: [`deploy/values.yaml`](../deploy/values.yaml). The
table below documents every knob you'd actually set.

### Image

| Path | Default | Notes |
|---|---|---|
| `image.repo` | `gitlab.cherkaoui.ch/hadicherkaoui/anvil` | Override to point at your own registry. |
| `image.tag` | `""` | Empty falls back to `.Chart.AppVersion`. |
| `image.pullPolicy` | `IfNotPresent` | |

### Managed-server defaults (`mcDefaults`)

These are projected into the panel's environment as `ANVIL_*` vars and
also appear as defaults in the New Server form.

| Path | Required | Default | Notes |
|---|---|---|---|
| `mcNamespace` | yes | `mc` | Namespace where the panel reads/writes managed StatefulSets/Services/Secrets. |
| `mcDefaults.storageClassName` | **yes** | *(empty — chart fails to render)* | Default StorageClass for managed-server PVCs. Forces the operator to confirm a deliberate value. |
| `mcDefaults.serviceType` | no | `LoadBalancer` | Default exposure mode (`LoadBalancer` \| `NodePort` \| `ClusterIP`). Per-server overridable from the UI. |
| `mcDefaults.nodeHost` | no | `""` | External IP/hostname displayed for `NodePort` servers. Empty shows `<unset>`. |
| `mcDefaults.loadbalancerSupported` | no | `true` | When `false`, requests for `exposure_mode=loadbalancer` are rejected `502 lb_unavailable`. |
| `mcDefaults.alpineImage` | yes | `alpine:3.20` | Alpine image shared by the file-browser helper Pod (sub-project D) and the mod-sync Job (M5 — `apk add curl`). **Pin by digest in production** — both workloads mount the data PVC, so a tag-mutation supply-chain attack would land on MC server data. |
| `mcDefaults.timezone` | yes | `Etc/UTC` | IANA timezone written into the `TZ` env of every managed MC pod so log timestamps line up with the operator's locale. The homelab override is `Europe/Zurich`. |
| `mcDefaults.itzgImage` | yes | `itzg/minecraft-server:java25` | Container image used by every managed MC `StatefulSet` (vanilla, modded, modrinth, paper, curseforge). Pin by digest in production. |
| `mcDefaults.busyboxImage` | yes | `busybox:1.36` | Image used by the backup, restore, and snapshot-cleanup Jobs (busybox ships `tar` + `sh`). Pin by digest in production. |

### Persistence

The panel itself needs a small PVC for SQLite.

| Path | Default | Notes |
|---|---|---|
| `persistence.enabled` | `true` | `false` puts SQLite in an emptyDir — fine for testing only. |
| `persistence.storageClassName` | `""` | Empty = cluster default. |
| `persistence.size` | `1Gi` | More than enough for the audit log. |

### Resources

```yaml
resources:
  requests: { cpu: 100m, memory: 128Mi }
  limits:   { cpu: 500m, memory: 512Mi }
```

Defaults are tuned for ~5 servers. Bump `limits.memory` if you have more
servers or run very chatty mods.

### Ingress (Traefik)

The chart writes a Traefik `IngressRoute` (not a vanilla `Ingress`) — the
homelab uses Traefik. Adapt the template if you use NGINX/HAProxy.

| Path | Default | Notes |
|---|---|---|
| `ingress.enabled` | `false` | Off by default; set to `true` once DNS resolves. |
| `ingress.entryPoint` | `websecure` | Traefik entryPoint name. `websecure` = TLS, `web` = plain HTTP. |
| `ingress.host` | `anvil.example.local` | FQDN. |
| `ingress.middlewares` | `[]` | Optional refs, e.g. `[{ name: security-headers, namespace: traefik }]`. |
| `ingress.tls.enabled` | `false` | Required when `oidc.enabled=true`. |
| `ingress.tls.certResolver` | `letsencrypt` | Traefik certResolver name. |

### OIDC (M4)

```yaml
oidc:
  enabled: true
  issuerUrl: "https://authentik.example.com/application/o/anvil/"
  clientId: "..."
  clientSecret: "..."             # or use `existingSecret`
  redirectUrl: "https://anvil.example.com/api/auth/callback"
  sessionKey: "<base64-32-bytes>" # `openssl rand -base64 32`
  allowedSubs: ""                 # optional CSV of Authentik subject UUIDs
  existingSecret: ""              # name of a pre-existing Secret carrying the
                                  # ANVIL_OIDC_CLIENT_SECRET + ANVIL_SESSION_KEY
                                  # keys (preferred for production)
```

The chart enforces two invariants at render time:

1. **`oidc.enabled` requires `ingress.tls.enabled=true`** — cookie security
   flags (`Secure; SameSite=Lax`) are pointless without HTTPS.
2. **`mcDefaults.storageClassName` must be set** — fails clearly instead of
   producing a half-baked install.

For the Authentik provisioning steps (Provider, Application, Outpost
binding), see [`authentik-setup.md`](authentik-setup.md).

### Modpack support (M5)

```yaml
secrets:
  cfApiKey: ""                     # CurseForge API key (or use existingSecret)
  cfApiKeyExistingSecret: ""

modpack:
  pollIntervalMinutes: 60
  snapshotsPvc:
    enabled: true
    storageClass: ""               # empty = cluster default
    size: 100Gi
```

When `secrets.cfApiKey` is set inline, the chart provisions it as **two
Secrets**:

1. `<release>-cf` in the release namespace (panel reads via `envFrom`).
2. `cf-api-key` in `mcNamespace` (per-server `itzg/minecraft-server` pods
   reference it via `secretKeyRef` so `TYPE=AUTO_CURSEFORGE` can download
   pack files).

When you use `cfApiKeyExistingSecret`, the chart only manages the panel
Secret. **You must provision the `cf-api-key` Secret in `mcNamespace`
yourself** — typically as a SOPS/sealed-secret with
`metadata.name: cf-api-key`, `metadata.namespace: <mcNamespace>`,
`stringData.CF_API_KEY: <key>`. Without it, modpack pods fail with
`CreateContainerConfigError: secret "cf-api-key" not found`.

Modrinth requires no API key; it works as long as `modpack.snapshotsPvc`
is enabled.

### Logging

| Path | Default | Notes |
|---|---|---|
| `logLevel` | `info` | `tracing` filter directive. Examples: `debug`, `info,kube=warn`. |

## RBAC

Anvil ships with two RBAC bundles, both gated on `rbac.create=true`
(default).

**Namespace-scoped `Role` in `mcNamespace`** (full read/write on the
managed-server resources):

- `apps/statefulsets`, `apps/statefulsets/scale` — create/patch/scale
- `core/persistentvolumeclaims`, `core/services` — full CRUD
- `core/pods` — `get/list/watch/create/delete` (the `create+delete` is for
  the files-helper Pod)
- `core/pods/log` — read
- `core/pods/exec` — `create+get` (both verbs needed: WS exec uses
  `get+upgrade`, SPDY exec uses `create`)
- `core/secrets` — full CRUD (RCON passwords, CF API key)
- `batch/jobs` — `get/list/watch/create/delete` (backup/restore jobs)
- `metrics.k8s.io/pods` — `get`

**Cluster-scoped `ClusterRole`** (read-only, used by
`/api/cluster/capabilities`):

- `storage.k8s.io/storageclasses` — `get/list/watch`
- `core/nodes` — `get/list/watch` (CPU-core total)

The cluster role is the reason a namespace-scoped install isn't enough —
whoever installs Anvil must be able to bind a `ClusterRole`. See
[ADR 0005](decisions/0005-storage-class-runtime-discovery.md) for why this
is acceptable.

## Persistence and sizing

| Volume | Where | Default | When to bump |
|---|---|---|---|
| Panel SQLite | `persistence.size` | 1 GiB | If you have hundreds of servers and want years of audit log retention. |
| Per-server data | `storage_size_gi` (per server) | 10 GiB | Set per server in the UI; vanilla worlds rarely need more, modded packs often want 20–40. |
| `mc-snapshots` PVC | `modpack.snapshotsPvc.size` | 100 GiB | Sized for ~5 servers × 3 retained backups × ~7 GB. Resize as you add servers. |

The snapshots PVC is RWO. Anvil serializes Job mounts via an in-process
mutex, so multi-server backup contention is fine — they queue and run
sequentially.

## Ingress and TLS

The shipped template is a Traefik `IngressRoute`. To use NGINX or HAProxy,
fork the chart and rewrite
[`deploy/templates/ingressroute.yaml`](../deploy/templates/ingressroute.yaml)
as a vanilla `networking.k8s.io/v1 Ingress`.

If `oidc.enabled=true`, you **must** enable TLS — the session cookie is
flagged `Secure`, which browsers ignore over plain HTTP. The chart fails
the render if you misconfigure this.

You can also expose the panel without ingress: `kubectl port-forward svc/
anvil 8080:8080` for local testing, or set `service.type=LoadBalancer` to
front the Service directly. Production should use ingress + TLS.

## OIDC (Authentik)

End-to-end Authentik provisioning is documented in
[`authentik-setup.md`](authentik-setup.md). The summary:

1. In Authentik, create an **OAuth2/OpenID Provider** with:
   - Client type: Confidential
   - Redirect URIs: `https://anvil.example.com/api/auth/callback` (exact match)
   - Signing key: any RSA key
   - Subject mode: `Based on the User's hashed ID`
2. Bind it to an **Application** with slug `anvil`.
3. Bind users (or a Group) to that Application.
4. Copy the issuer URL (the Application's `o/anvil/` page), client id, and
   client secret into the Helm values.
5. (Optional, recommended for production) Pre-create a Kubernetes Secret
   carrying `ANVIL_OIDC_CLIENT_SECRET` and `ANVIL_SESSION_KEY`, then point
   `oidc.existingSecret` at it.

To allow only specific Authentik users (instead of all members of the
Application), set `oidc.allowedSubs` to a comma-separated list of subject
UUIDs (visible in Authentik's user detail page).

## Modpack support (CurseForge / Modrinth)

CurseForge requires an API key from
<https://console.curseforge.com/>. Modrinth doesn't.

When `secrets.cfApiKey` is set:

- The CurseForge provider becomes available in the New Server form.
- The hourly poller queries CF for new ServerFiles versions of every
  CF-backed server.
- The update orchestrator can apply CF version changes (announce → stop →
  tar backup → swap pack → start → verify boot, with rollback on failure).

When `secrets.cfApiKey` is unset:

- The New Server form hides the CurseForge option.
- Modrinth, modded (Fabric/Forge/NeoForge), and Paper still work.
- The poller skips CF rows in-loop without errors.

The shared `mc-snapshots` PVC must be enabled (`modpack.snapshotsPvc.
enabled=true`) for any update path to function — the orchestrator's first
step is always a tar backup.

## File browser

When a server is stopped and a user opens the Files tab, Anvil lazily
spawns a helper Pod (`mc-{id}-files`, `alpine@<digest> + sleep infinity`)
that mounts the data PVC. The helper is torn down on next start, or
manually via `DELETE /api/servers/{id}/files/helper`.

**Pin `mcDefaults.alpineImage` by digest in production.** The default
`alpine:3.20` resolves the latest tag at run time, which is fine for
homelabs but a supply-chain risk for production. The same image is also
used by the mod-sync Job (M5), so a tag-mutation attack would land code
in any Pod/Job that mounts a server's data volume (`pods/exec` access
plus mod download paths).

The upload size cap is hardcoded at 100 MiB. Larger files should be
uploaded over RCON-tooling or `kubectl cp` directly.

## Upgrades

```bash
helm pull oci://gitlab.cherkaoui.ch/hadicherkaoui/anvil/anvil --version <new>
helm upgrade anvil anvil-<new>.tgz -n anvil --reuse-values
```

The `Recreate` strategy means there is a few-second outage during
upgrade — the old pod releases the SQLite PVC, then the new one binds it.
Rolling updates would deadlock on the single-attach RWO volume.

Migrations run automatically on startup. `sqlx::migrate!` is forward-only
and embedded in the binary; rolling back a release that already ran a new
migration requires restoring SQLite from a snapshot.

## Troubleshooting

### Pod stuck `CreateContainerConfigError: secret "cf-api-key" not found`

You are using `cfApiKeyExistingSecret` and forgot to provision the
`cf-api-key` Secret in `mcNamespace`. Provision it manually (see
[Modpack support](#modpack-support-curseforge--modrinth)).

### `oidc.enabled=true` but `helm install` fails with `requireOidc`

The chart enforces `ingress.tls.enabled=true` when OIDC is on. Either
provide a TLS cert resolver or disable OIDC for testing.

### `502 lb_unavailable` when creating a server

Either the cluster has no LoadBalancer provider and you should set
`mcDefaults.loadbalancerSupported=false`, or the LoadBalancer provider is
broken — `kubectl describe svc -n mc` will show the failure reason.

### `409 expansion_unsupported` on `PATCH /storage`

The StorageClass does not have `allowVolumeExpansion: true`. This is a
property of the StorageClass, not Anvil — `kubectl patch storageclass
<name> -p '{"allowVolumeExpansion":true}'`.

### Files tab shows `409 server_transitioning`

The server is starting or stopping; the helper Pod can't safely attach
mid-transition. Wait for the status to settle.

### Backups Jobs stuck Pending

Inspect the snapshots PVC — it might not have provisioned, or the chosen
StorageClass might not support `ReadWriteOnce` (Anvil's only access mode).
`kubectl describe pvc mc-snapshots -n mc` will tell you.

### Modpack updates always end in `failed-rolled-back`

Common causes: pack downloads time out (CF/Modrinth slow, or your egress
rules), pack incompatible with the JVM heap (raise `memory_mi`), or
`Done (` never shows up in pod logs (modpack starts but fails — open the
log stream to see the actual error). The rollback is intentional safety,
not a bug.

### Where to look for logs

```bash
kubectl -n anvil logs deploy/anvil -f                  # panel
kubectl -n mc    logs mc-{id}-0 -f                     # MC server
kubectl -n mc    get jobs                              # backup/swap/restore jobs
kubectl -n mc    logs job/<job-name>                   # one-shot job logs
```

Inside the panel logs, look for structured events like `anvil.update.*`,
`anvil.request.error`, `anvil.startup`. Tracing levels are controlled by
`ANVIL_LOG_LEVEL` (default `info`). Bump to `debug` for kube-rs detail or
`info,kube=warn,sqlx=warn` to keep the logs quiet without losing your own
events.
