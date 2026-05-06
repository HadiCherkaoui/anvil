# Anvil — Bugs & small UX (Spec 1)

**Date:** 2026-05-06
**Author:** Hadi (with Claude as scribe)
**Status:** Brainstormed — awaiting user signoff
**Spec series:** 1 of 4 from the 2026-05-06 triage. Companions: Spec 2 (PVC resize · file-helper kill · status nuance), Spec 3 (MC version change for non-modpack servers), Spec 4 (mod deps · per-mod updates · paper plugin pre-select).

---

## 1. Context

User filed a list of ~16 observations on the running Anvil. Triage grouped them into:

- **A — answers, no work**: items #1, #9, #16 (partial). Verified in code, behaviour is correct or feature already exists.
- **B — bugs, well-defined**: #3, #5, #7, #11, #13a.
- **C — small UX gaps**: #2, #10, #12.
- **D — features needing their own designs**: #4, #6, #8, #13b, #14, #15, #16b. Deferred to specs 2–4.

This spec covers groups B and C — eight items totalling one implementation session.

---

## 2. Scope

**In scope:**

| # | Item | Type |
|---|---|---|
| B#3 | Memory env not patched on settings save | bug |
| B#5 + #9 | NeoForge install fails — also covers Forge picker | bug + feature |
| B#7 | Fabric runtime mods gate (visual lie) | bug |
| B#11 | Discard pending mod doesn't refresh view | bug |
| B#13a | Paper "mods" tab labelled "mods" | cosmetic |
| C#2 | Copy-IP button on Overview | UX |
| C#10 | Pre-picked mods on create not auto-applied | UX |
| C#12 | Picked-mods list visible during create | UX |

**Confirmed no-action (verified in §3.x):** #1 last-started timestamp (intentional locale-aware render), #9 Forge install env (works as-is, gets the picker treatment along with NeoForge), #16 mod removal (already wired).

**Out of scope (deferred):**

- MC version change for non-modpack servers (Spec 3).
- Mod dependency resolution, per-mod update notifications, paper plugin pre-select in create form (Spec 4).
- PVC resize, file-helper kill button, restart-loop status nuance (Spec 2).

---

## 3. Anti-overengineering guardrails

Per CLAUDE.md:

- **No new traits, no plugin abstractions.** The runtime version logic stays inline per-runtime — it's two lookups (Forge, NeoForge) sharing one HTTP path, not a registry.
- **No new top-level deps unless required.** This spec adds **`quick-xml`** to the backend (parses maven-metadata.xml from the loader sources). `reqwest` is already present.
- **No SQLite migration.** `loader_version` rides in the existing JSON-serialised `source_config` column. Existing rows decode with `loader_version: None`.
- **No frontend test runner introduced.** Manual repro for FE items.
- **YAGNI — no version filtering for Fabric.** Fabric covers basically every MC release; the existing version list is fine.
- **No auto-restart on memory change.** Patch the StatefulSet env, surface "applies on next start" — user owns lifecycle.

---

## 4. Cross-cutting refactors

Two small refactors enable several items.

### 4.1 Promote `patch_statefulset_env`

`backend/src/modpack/orchestrator.rs:435-465` defines a private helper that Strategic-Merge-patches the `mc` container's env on a StatefulSet. The settings handler (B#3) is the second caller.

**Action:** move to a new module `backend/src/k8s_patches.rs` and make it public. Existing signature is preserved:

```rust
pub async fn patch_statefulset_env(
    kube: &kube::Client,
    namespace: &str,
    server_id: &str,
    env: &[EnvVar],
) -> Result<(), kube::Error>
```

It hardcodes the `mc` container internally — that's fine, every caller targets the same container. Both call sites (orchestrator swap/rollback, settings handler) now import from `k8s_patches`.

### 4.2 Refreshable `ServerDetailContext`

`frontend/app/lib/server-detail-context.ts` exposes `ServerDetail | null`. Items B#11 and C#10 (frontend portion of the latter) need a post-mutation refresh.

**Action:** widen the context value to `{ detail, refresh } | null`. `ServerDetailView.tsx` already owns the fetch — extract the fetch into a `refresh` callback and pass it through the provider. Add a new hook:

```ts
export function useServerDetail(): { detail: ServerDetail; refresh: () => void };
```

Keep the existing `useServerDetailCtx()` returning just `detail` so the unchanged consumers (Overview, Console, Players, Files, Settings tabs) don't need updates. Mods/Plugins tabs migrate to `useServerDetail()`.

### 4.3 Memory env helper

Memory env construction is currently inlined per provider (`vanilla.rs:64-66`, `modded.rs:160-165`, `modrinth.rs:119-124`, `curseforge.rs:228-234`, `paper.rs:73-80`). Settings.rs needs the same logic for B#3.

**Action:** extract to `backend/src/modpack/memory.rs`:

```rust
pub fn build_memory_env(memory_mi: u32) -> Vec<EnvVar> {
    vec![
        env_kv("INIT_MEMORY", &format!("{}M", init_memory_mi(memory_mi))),
        env_kv("MAX_MEMORY", &format!("{}M", memory_mi)),
        env_kv("JVM_XX_OPTS", IDLE_GC_OPTS),
    ]
}

pub fn init_memory_mi(memory_mi: u32) -> u32 {
    (memory_mi / 4).max(1024)
}
```

Each provider calls this. Settings PATCH calls this. One source of truth.

---

## 5. Per-item design

### 5.1 B#3 — memory env applied on settings save

**File:** `backend/src/routes/servers/settings.rs:67-73`

Today the handler updates `memory_mi` in SQLite and returns. The running pod keeps the old `INIT_MEMORY` / `MAX_MEMORY`.

**Action:** after the SQLite update, build the **full** new env for the running runtime (memory env merges with the rest — Strategic Merge replaces the entire `env` array per `name` key, so the patch must include all current env vars or k8s removes the missing ones). Reuse the runtime's existing `extra_env()` / `build_env()` to construct the full env, then call `patch_statefulset_env(&state.kube, &state.mc_namespace, &id, &new_env)`.

Toast wording stays "settings saved · applies on next start" — env patch only takes effect when the pod is recreated.

If the StatefulSet doesn't exist yet (server created but never reconciled — shouldn't happen in normal flow but is possible), the patch returns `404` — log a warning, continue (SQLite already updated, next start picks up the value).

**Test:** `backend/tests/settings_memory.rs` — create test server, PATCH `/api/servers/{id}/settings` with new memory, fetch StatefulSet via kube client, assert env contains expected `INIT_MEMORY` / `MAX_MEMORY`.

### 5.2 B#5 + #9 — NeoForge & Forge version pickers

**Root cause:** Anvil currently passes only `TYPE=NEOFORGE` + `VERSION=<mc>` to itzg. itzg defaults `NEOFORGE_VERSION=LATEST` (newest non-beta NeoForge **for the requested MC**). NeoForge doesn't release for every MC version (no 1.20.3, no 1.20.5, no 1.21.2, etc.). When the user picks an MC version NeoForge skipped, itzg's `install-neoforge` fails to find a matching loader. Forge has the same theoretical problem but covers far more MC versions, so the user hasn't hit it.

The fix is **not** to set `NEOFORGE_VERSION=LATEST` explicitly (that doesn't change the default behaviour) — the fix is to **let the user pick a real version that exists**.

#### 5.2.1 Backend — loader version endpoint

**New file:** `backend/src/routes/runtimes.rs`

```
GET /api/runtimes/{runtime}/versions
```

`runtime ∈ {forge, neoforge}` — fabric returns 404 (no list needed).

Response:

```json
{
  "mc_versions": ["1.21.4", "1.21.1", "1.20.6", "1.20.4", "1.20.2", "1.20.1"],
  "by_mc": {
    "1.21.4": ["21.4.81", "21.4.80", "21.4.79", ...],
    "1.21.1": ["21.1.182", ...]
  }
}
```

`mc_versions` is sorted descending (newest first) and deduplicated. Each `by_mc` value is sorted descending.

**Sources (parsed from maven-metadata.xml):**

- NeoForge: `https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml`
- Forge: `https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml`

**Parsing rules:**

- NeoForge `<version>21.4.81</version>` → MC `1.21.4`. Rule: split on `.`, take first two as `1.<a>.<b>`. Skip `*-beta` versions for the default list (still include them under a separate `betas` field if needed — start without).
- Forge `<version>1.21.4-54.1.0</version>` → MC `1.21.4`, loader `54.1.0`. Rule: split on `-`, prefix is the MC version, suffix is the loader. (We pass the **full** Forge version string to itzg, including the MC prefix, since that's what `FORGE_VERSION` accepts.)

**Caching:** in-memory `RwLock<Option<(Instant, ParsedVersions)>>` per runtime, TTL 1 hour. On miss, fetch + parse + write. On parse failure, return cached value if any, else 503.

**Caching home:** new struct `LoaderVersionCache` lives in `backend/src/state.rs::AppState` (existing struct).

**Dependencies:** add `quick-xml = "0.36"` (or current) to `backend/Cargo.toml`. Use `reqwest` (already present).

#### 5.2.2 Backend — env passing

**File:** `backend/src/modpack/modded.rs`

`ModdedConfig` gains:

```rust
pub struct ModdedConfig {
    pub runtime: Runtime,
    pub mc_version: String,
    pub loader_version: Option<String>, // NEW
    pub mods: Vec<ModEntry>,
    pub pending: Vec<PendingOp>,
}
```

In `extra_env()`:

```rust
match self.config.runtime {
    Runtime::Fabric => {} // no extra
    Runtime::Forge => env.push(env_kv(
        "FORGE_VERSION",
        self.config.loader_version.as_deref().unwrap_or("LATEST"),
    )),
    Runtime::NeoForge => env.push(env_kv(
        "NEOFORGE_VERSION",
        self.config.loader_version.as_deref().unwrap_or("LATEST"),
    )),
}
```

Existing servers in SQLite have `loader_version: None` (serde default) → behaviour unchanged (LATEST).

#### 5.2.3 Backend — create accepts loader version

**File:** `backend/src/routes/servers/create.rs`

`CreateServerRequest.modded` gains `loader_version: Option<String>`. Validation: if `runtime ∈ {forge, neoforge}` and `loader_version` is `Some`, accept any non-empty string (don't validate against the upstream list — that's a soft check best done in the UI; the worst case is itzg errors).

#### 5.2.4 Frontend — cascading pickers

**File:** `frontend/app/servers/new/page.tsx`

When `draft.type === "modded"`:

- runtime selector (existing).
- if `runtime ∈ {forge, neoforge}`:
  - **MC picker** sourced from `loaderVersions.mc_versions`.
  - **Loader version picker** showing `loaderVersions.by_mc[draft.mc_version]`. Default = first (newest).
- if `runtime === "fabric"`: existing MC picker, no loader picker.

New API client function in `frontend/app/lib/api.ts`:

```ts
export const fetchLoaderVersions = (
  runtime: "forge" | "neoforge",
  signal?: AbortSignal,
): Promise<LoaderVersions>;
```

with Zod schema `loaderVersionsSchema`.

New hook `frontend/app/lib/use-loader-versions.ts` mirroring `useMcVersions` — fetches lazily on first read, caches in module scope.

`CreateDraft` gains `loader_version: string | null`. Sent in `CreateServerRequest.modded.loader_version`.

**Switching runtime** — clears `loader_version` (matches existing `initial_mods` clear).

**Switching MC version** — clears `loader_version`, then the loader picker auto-selects the newest for the new MC.

**Fallback when endpoint unreachable** — show a one-line "loader list unavailable, using LATEST" notice; submit with `loader_version: null`.

#### 5.2.5 Tests

- `backend/src/routes/runtimes.rs` — fixture maven-metadata.xml in `backend/tests/fixtures/`, assert grouping & sort order for both runtimes.
- `backend/src/modpack/modded.rs` — unit test: `extra_env()` for each runtime sets the expected version env (FABRIC: none, FORGE: `FORGE_VERSION`, NEOFORGE: `NEOFORGE_VERSION`). Assert `loader_version: Some("...")` is forwarded verbatim.

### 5.3 B#7 — fabric runtime gate

**File:** `frontend/app/servers/new/page.tsx`

The `SegmentedControl` at `:358` reads `value={draft.runtime ?? "fabric"}` — visually highlights fabric when state is `null`. User assumes selection is valid, the "+ pre-pick mods" button at `:374-388` is gated on `draft.runtime !== null` so it stays disabled.

**Action:**

1. In the type onChange (`:322-333`), when `v === "modded"` and `draft.runtime === null`, set `runtime = "fabric"` (default). State now matches the visual.
2. Remove the `?? "fabric"` fallback at `:358` — since runtime is always set when type is modded, the segmented control gets `value={draft.runtime}`.

**Acceptance:** select type=modded → runtime is fabric in state → pick MC version → "+ pre-pick mods" button is enabled.

### 5.4 B#11 — discard pending mod refreshes view

**File:** `frontend/app/servers/tabs/ModsBody.tsx:122-133`

In `addPendingMod`, `removePendingMod`, `discardPending`: after the promise resolves, call `refresh()` (from §4.2's new context) before pushing the success toast. Same fix in the `PaperPluginsBody` equivalents.

### 5.5 B#13a — paper tab label

**File:** `frontend/app/servers/ServerDetailView.tsx:177-182`

```ts
{
  id: "mods",
  label: detail.source_kind === "paper" ? "plugins" : "mods",
  href: tabHref("mods"),
  ...(detail.update_available ? { mark: true } : {}),
}
```

`id` stays `"mods"` so URL routing and tab state are unchanged.

### 5.6 C#2 — copy-IP button

**File:** `frontend/app/servers/tabs/OverviewBody.tsx:79-86`

Wrap the connection block:

```tsx
<Card header="connection">
  <div className="flex items-center gap-2">
    <pre className="font-mono text-[12px] text-text-body">
      {detail.endpoint
        ? `${detail.endpoint.host}:${detail.endpoint.port.toString()}`
        : "address pending…"}
    </pre>
    {detail.endpoint && (
      <button
        type="button"
        onClick={() => {
          const addr = `${detail.endpoint!.host}:${detail.endpoint!.port}`;
          void navigator.clipboard.writeText(addr).then(() => {
            toast.push("copied", "success");
          });
        }}
        aria-label="copy address"
        className="..."  // matches IconButton primitive style
      >
        <CopyIcon />
      </button>
    )}
  </div>
</Card>
```

`CopyIcon` is a small inline SVG component in `frontend/app/components/icons/Copy.tsx` (new). No new deps.

### 5.7 C#10 — auto-apply mods on create

**File:** `backend/src/routes/servers/create.rs:507-513`

After the StatefulSet is created and SQLite is written, if `cfg.initial_mods` is non-empty:

1. Acquire an `UpdateGuard` (same guard the manual apply route uses, in `routes/servers/mods.rs:170-179`). On a freshly-created server this should always succeed.
2. Spawn `mods_apply::run(state.clone(), id.clone(), guard, SyncTarget::Mods)` — the same task the manual apply route spawns.

This mirrors the manual `POST /api/servers/{id}/mods/apply` flow without going through the HTTP handler.

**Failure handling:** if the apply Job fails to schedule, the create response still succeeds (StatefulSet exists, server row written). The audit log captures the failure. The pending ops remain in `source_config.pending` so the user can manually trigger apply from the Mods tab.

**Frontend:** no change. The detail page surfaces apply progress via the existing `ApplySheet` flow when the user lands there post-create.

**Acceptance:** create modded server with 3 picked mods → server appears in list → apply Job runs → mods land in PVC → `pending` is empty in `source_config` → `mods` lists the three.

### 5.8 C#12 — picked-mods list during create

**File:** `frontend/app/servers/new/page.tsx:373-388`

Below the "+ pre-pick mods" button, when `draft.initial_mods.length > 0`, render a list:

```tsx
{draft.initial_mods.length > 0 && (
  <ul className="mt-2 flex flex-col gap-1">
    {draft.initial_mods.map((m, i) => (
      <li key={`${m.provider}:${m.version_id}`} className="flex items-center gap-2 ...">
        <span className="font-mono text-[12px] text-text-body">{m.project_name}</span>
        <span className="font-mono text-[11px] text-text-faint">{m.version_name}</span>
        <button
          type="button"
          onClick={() => {
            set("initial_mods", draft.initial_mods.filter((_, j) => j !== i));
          }}
          aria-label={`remove ${m.project_name}`}
          className="ml-auto ..."
        >
          ×
        </button>
      </li>
    ))}
  </ul>
)}
```

`BuildSlip.tsx:93-94` keeps the count.

---

## 6. Data flow / deployment

- **New backend dep:** `quick-xml`. Run `cargo add quick-xml --features serialize` in `backend/`.
- **No SQLite migration.** `loader_version` rides in JSON-serialised `source_config`.
- **No Helm change.** No new env vars, no new ports.
- **Outbound network:** the backend now calls `maven.neoforged.net` and `maven.minecraftforge.net` for loader versions. Egress is allowed in the cluster (already allowed for Modrinth/CurseForge).

---

## 7. Error handling

| Path | Failure | Behaviour |
|---|---|---|
| Settings PATCH memory | StatefulSet patch fails | Return 500 `statefulset_patch_failed`. SQLite is already updated; document this in the audit log entry so the inconsistency is visible. Toast surfaces the kube error. |
| Loader versions endpoint | Upstream maven 4xx/5xx | If cache has a previous successful value, serve it (stale-but-usable). Otherwise return 503 `loader_versions_unreachable`. |
| Loader versions endpoint | Upstream parse failure | Same — fall back to cache, else 503. |
| Auto-apply on create | Apply Job schedule fails | Log + audit. Server is created. Pending ops remain. Frontend mods tab shows them. |
| Frontend loader fetch | Endpoint 503 | Show "loader list unavailable, using LATEST" inline notice; submit with `loader_version: null`. |
| Discard pending | Refresh fails | Pre-existing behaviour: show error toast. The mutation already succeeded server-side; user can reload. |

---

## 8. Testing

| Item | Test |
|---|---|
| §4.1 patch helper | Reuse existing orchestrator tests; the helper signature change is mechanical |
| §4.3 memory helper | Unit: `build_memory_env(4096)` returns expected env list |
| §5.1 B#3 | Integration: `backend/tests/settings_memory.rs` — PATCH settings with new memory → assert StatefulSet env updated |
| §5.2 B#5 + #9 | Unit: parse fixture `maven-metadata.xml` for both runtimes, assert grouping; unit: `extra_env()` per runtime |
| §5.3 B#7 | Manual repro |
| §5.4 B#11 | Manual repro |
| §5.5 B#13a | Manual repro |
| §5.6 C#2 | Manual repro |
| §5.7 C#10 | Integration: POST create with `initial_mods` → assert apply Job exists in cluster within 5s |
| §5.8 C#12 | Manual repro |

Manual repro suffices for the FE items because there is no FE test runner today (deferred — see existing v2 specs).

---

## 9. Open questions

None. Pickers locked in for both Forge and NeoForge per user signoff. C#10 backend auto-apply locked. Fabric default (B#7) locked.

---

## 10. Future work (notes for the next specs)

- **Spec 2:** PVC resize (grow), file-helper kill button when server stopped, restart-loop status nuance (extend `derive_status` to surface CrashLoopBackOff sooner).
- **Spec 3:** MC version change for vanilla / paper / modded — re-uses §5.2's loader version endpoint for runtime-aware version picking, plus the §4.1 patch helper for env application.
- **Spec 4:** Mod dependency resolution (deserialize Modrinth `dependencies` field), per-mod update notifications (extend the existing modpack poller), paper plugin pre-select in create form (symmetric to mods).

---

## 11. Implementation prompt (for paste into a fresh Claude Code session)

The implementation prompt for this spec is generated by the writing-plans skill in the next workflow step. It will be saved alongside this spec and reference the spec by path.
