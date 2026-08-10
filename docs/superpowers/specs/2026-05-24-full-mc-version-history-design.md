<!--
SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

# Full Minecraft Version History — Design

**Status:** Approved
**Date:** 2026-05-24
**Author:** Hadi + Claude

## Problem

The server-create form's Minecraft version dropdown only shows the most
recent ~20 releases (≈ 1.20.4 → 1.21.x). Users who want to run legacy
servers (1.8, 1.12.2, 1.16.5, etc.) cannot select them, even though the
underlying itzg/minecraft-server image supports those versions and
auto-selects a compatible Java runtime.

## Goal

Surface every official Minecraft *release* version (1.0 → latest) in the
vanilla and Paper dropdowns. Backend validation accepts any release.
Snapshots, pre-releases, and betas stay out of scope.

## Non-Goals

- Snapshots / pre-releases / `*-rc*` / `24wXXa` weekly builds.
- Free-text version input.
- Pre-1.0 (`b1.x`, alpha) versions — even if itzg supports them, the
  Mojang manifest doesn't list them as `kind == "release"`.
- Any frontend rework beyond rendering the longer list.

## Changes

Three files. No schema changes, no new endpoints, no API contract change.

### 1. `backend/src/routes/mc_versions.rs`

- Remove `MAX_VERSIONS = 20` cap. The `.take(MAX_VERSIONS)` line in
  `parse_manifest` goes away.
- Keep the `kind == "release"` filter — that's what excludes snapshots.
- Keep the `McVersionsResponse { versions, source }` shape unchanged.
- The cache slot still holds `Vec<String>`; it just holds more entries.

The Mojang manifest is sorted newest-first by upstream contract. We
preserve that ordering (no re-sort), so the dropdown still shows
1.21.x at top.

### 2. `backend/src/routes/papermc.rs`

- Remove `MAX_VERSIONS = 25` cap. The `out.truncate(MAX_VERSIONS)` in
  `parse_project` goes away.
- Paper's API returns ascending; the existing `out.reverse()` already
  flips it to newest-first.
- `FALLBACK_VERSIONS` baseline stays as-is (only used when *both*
  upstream and cache are unavailable).

### 3. `backend/src/validation.rs`

- `validate_mc_version` is unchanged logically. It already does:
  cache hit → accept, else offline floor → accept, else reject. With
  the cap removed, the cache will contain the full list, so any real
  release a user picks via the dropdown will pass.
- Extend `KNOWN_MC_VERSIONS` to add popular legacy anchors so a cold
  cache + Mojang outage doesn't lock the user out of common builds.
  Add: `1.8.9`, `1.12.2`, `1.16.5`, `1.18.2`, `1.19.2`, `1.20.1` (six
  new anchors). Keep existing entries. Final list (12 entries):
  ```rust
  &["1.8.9", "1.12.2", "1.16.5", "1.18.2", "1.19.2",
    "1.20.1", "1.20.4", "1.20.6", "1.21.0", "1.21.1", "1.21.3", "1.21.4"]
  ```

## Tests

### `mc_versions.rs`

- Replace `parses_release_versions_capped`: assert all releases are
  returned (snapshots filtered, no length cap). Use a 3-release fixture
  to verify ordering + filtering.
- Delete `cap_enforced_at_max_versions`. Replace with a "no cap" test
  that proves a 100-version manifest produces a 100-version result
  with snapshots filtered.
- `empty_versions_is_ok` and `invalid_json_errors` stay as-is.

### `papermc.rs`

- Mirror treatment: existing cap tests get replaced with no-cap
  versions. Make sure newest-first ordering is still asserted.

### `validation.rs`

- Add cases in `offline_versions_pass`: `1.8.9`, `1.16.5`, etc.
- `offline_unknown_fails` stays — `1.7.10` still doesn't pass the
  offline floor. (If the cache is hot and Mojang lists 1.7.10, that's
  fine — but the offline-only test is testing the floor specifically.)
  Actually: Mojang does NOT list `1.7.10` as `kind == "release"` in
  recent manifests — it's not present. The test stays correct.

## Frontend Impact

None. `McVersionPicker` (`frontend/app/servers/new/page.tsx:908`)
renders `versions.map(v => <option>)` against whatever the backend
sends. A native `<select>` scrolls fine at ~75 entries. Same for
`VersionChangeSheet.tsx` and `SettingsBody.tsx`.

## Modded Servers

No change. Forge / NeoForge surface MC versions via their own maven
metadata (`backend/src/routes/runtimes.rs`); Fabric runs on every MC
release. Old MC versions automatically show up whenever the upstream
loader supports them.

## Risks & Trade-offs

| Risk | Mitigation |
|------|-----------|
| Dropdown becomes long (~75 entries). | Native `<select>` handles scroll; the user explicitly wants this. |
| Mojang manifest occasionally drops very old entries. | The 24h cache + offline floor (now broader) cover transients. |
| itzg image fails to launch on a chosen old version. | Out of scope — itzg surface error propagates to logs as it does today. |

## Rollout

Single commit on `main`. No migration, no Helm change, no config flag.
Container rebuild + helm release picks it up.
