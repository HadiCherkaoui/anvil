<!--
SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

# 0006 — Modpack provider trait + tar-to-PVC backups

Status: accepted (M5, 2026-05-03)

## Context

M5 adds CurseForge ServerFiles support alongside the M2 vanilla path. Two
shapes of decision needed locking:

1. **How to abstract over providers.** The brief explicitly asked for a
   `ModpackProvider` trait once the second implementation showed up. M2's
   anti-overengineering rule pushed the trait out of M2; M5 is when the
   trait *earns* its abstraction cost.

2. **How to back up server data before applying an update.** The plan
   outlined two paths: a `VolumeSnapshot` of the per-server PVC (instant,
   storage-driver native) or a Job that mounts the data PVC + a shared
   snapshots PVC and tars `/data` into it.

## Decision

### Trait shape

`#[async_trait]` boxed-trait dispatch (`Box<dyn ModpackProvider + Send +
Sync>`), reconstructed at every call site from the SQLite `source_kind` +
`source_config` columns. The provider is a *value* with no internal state
that needs to outlive a single request — it can be cheaply rebuilt by the
poller, the create handler, and the update orchestrator.

`async_trait` over RPITIT because the trait must be dyn-compatible: the
update orchestrator holds a `Box<dyn ModpackProvider>` for the lifetime of
the update task and can't name the concrete provider type at that site.
The one heap allocation per async call is irrelevant at homelab scale.

Enum dispatch was considered and rejected: the dispatch table would just
move into match arms at every call site, and the moment a third provider
appears (Modrinth in M6+) we'd revisit each match. The trait centralizes
the contract.

### Backup strategy

**Tar-to-shared-PVC only**, no VolumeSnapshot path in v1.

`docs/cluster-profile.md` (verified 2026-05-02) records that the homelab
cluster has the VolumeSnapshot CRDs installed but **no VolumeSnapshotClass
for `zfs.csi.openebs.io`** and no snapshotter pod in `kube-system`.
Snapshots wouldn't work without further cluster setup. Per the
anti-overengineering rule, building a runtime detect-or-fall-back path
when one of the two backends is unusable is one implementation too many.

The path:

- Backup Job: `busybox:1.36`, mounts the data PVC RO + shared `mc-snapshots`
  PVC RW, `tar czf /snap/mc-{id}/mc-{id}-{ts}.tgz -C /data .`.
- Swap Job: `alpine:3.20`, downloads the new ServerFiles zip, applies the
  preserve / wipe lists, re-merges panel-managed `server.properties` keys.
- Restore Job: same shape as backup, reversed; `find /data -mindepth 1
  -delete && tar xzf /snap/.../latest.tgz -C /data`.
- Concurrency: a single `snapshot_pvc_lock: Arc<Mutex<()>>` in `AppState`
  serializes Jobs panel-wide so the RWO mount on the shared PVC never
  contends. Per-server update lock prevents same-server concurrency.
- Retention: hardcoded last 3 archives, GC'd at the end of every successful
  backup Job (`ls -t | tail -n +4 | xargs -r rm -f`).

## Consequences

- The trait makes adding Modrinth (or any third provider) mechanical — one
  new file in `backend/src/modpack/`, one match arm in `from_db`.
- The tar path costs one extra Job and ~7 GB/server/backup × 3 = ~21
  GB/server on the snapshots PVC. The Helm chart defaults the PVC to
  100 GiB, comfortable for ~5 servers. Resize when the homelab grows.
- Migration to VolumeSnapshot is clean when the cluster grows a working
  `VolumeSnapshotClass`: add a snapshot Job builder, a config knob to
  pick, and the rest of the orchestrator stays the same. No data
  format change — the snapshot path would store `VolumeSnapshot` objects
  in lieu of `.tgz` files in the PVC.
- The `apply` auto-update mode fires updates immediately on detection;
  there's no maintenance window. For a 5-friend homelab this is fine;
  if it ever isn't, the poller can grow a "wait until 4am" gate without
  changing the orchestrator.

## Rejected alternatives

- **Runtime detection (snapshot if available, else tar).** Two
  implementations to maintain when only one path actually runs in
  production for the foreseeable future.
- **Helm-value backup-backend selector.** Same problem, with extra
  per-deployment configuration the operator has to think about.
- **Per-server snapshots PVC.** Cleaner namespacing but multiplies storage
  cost; one PVC per server eats the per-PVC overhead from `zfs.csi.openebs.io`
  for every modded server.
