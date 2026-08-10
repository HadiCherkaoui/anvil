<!--
SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>

SPDX-License-Identifier: AGPL-3.0-or-later
-->

# Server Properties Tab — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface 16 commonly tweaked Minecraft `server.properties` fields through a new "Properties" tab on the server detail, plus a small World section (difficulty, hardcore, gamemode) on the create form. Apply on next start, matching the memory pattern.

**Architecture:** A typed `ServerProperties` Rust struct stored in a new SQLite JSON column, layered onto the StatefulSet env after the provider env. Settings PATCH writes the JSON + strategic-merges the StatefulSet env. Frontend renders 4 cards with typed inputs and `(i)` icons linking to `minecraft.wiki/w/Server.properties` anchors.

**Tech Stack:** Rust 1.83 / axum 0.8 / sqlx (SQLite) / kube-rs / Next.js 16 / TypeScript / Tailwind / Zod.

**Spec:** `docs/superpowers/specs/2026-05-06-server-properties-design.md`

---

## File Structure

**Backend — new files:**
- `backend/migrations/0009_server_properties.sql` — column migration
- `backend/src/server_properties.rs` — typed struct, defaults, validation, env emission
- `backend/tests/server_properties_e2e.rs` — integration test

**Backend — modified files:**
- `backend/src/lib.rs` — register `server_properties` module
- `backend/src/routes/servers/create.rs` — accept `properties`, persist + emit env
- `backend/src/routes/servers/settings.rs` — accept `properties`, persist + emit env
- `backend/src/routes/servers/get.rs` — read column, expose in `ServerDetail`

**Frontend — new files:**
- `frontend/app/servers/tabs/PropertiesBody.tsx` — the new tab body

**Frontend — modified files:**
- `frontend/app/lib/api.ts` — schemas, types, `DEFAULT_PROPERTIES`
- `frontend/app/servers/new/page.tsx` — Section 06 World
- `frontend/app/servers/ServerDetailView.tsx` — register tab

---

## Task 1: Migration + DB column

**Files:**
- Create: `backend/migrations/0009_server_properties.sql`

- [ ] **Step 1: Write the migration**

```sql
-- Per-server tunable Minecraft server.properties values, applied via itzg's
-- env-vars-to-server.properties overlay on every pod start. JSON object
-- keyed by server.properties field name; empty `{}` decodes to vanilla
-- defaults via the typed struct's `#[serde(default)]` per-field.

ALTER TABLE servers ADD COLUMN properties TEXT NOT NULL DEFAULT '{}';
```

- [ ] **Step 2: Regenerate sqlx offline metadata**

Run from `backend/`:
```
DATABASE_URL=sqlite::memory: cargo sqlx prepare --merged 2>/dev/null || true
```
(If `cargo sqlx prepare` complains, ignore for now — dependent queries will be added in later tasks; we'll re-prepare at the end.)

- [ ] **Step 3: Verify migration applies**

Run `cargo test --package anvil --lib db::tests -- --nocapture` to confirm the migration runner picks up `0009_*` without error.

- [ ] **Step 4: Commit**

```
git add backend/migrations/0009_server_properties.sql
git commit -m "feat(db): add servers.properties JSON column"
```

---

## Task 2: `ServerProperties` typed struct + Default + Serde

**Files:**
- Create: `backend/src/server_properties.rs`
- Modify: `backend/src/lib.rs`

- [ ] **Step 1: Write failing tests for `Default` and Serde round-trip**

Create `backend/src/server_properties.rs` with the test module:

```rust
//! Typed wrapper over the subset of itzg's server.properties env vars
//! Anvil exposes through the Properties tab. Stored as JSON in
//! `servers.properties`; deserialized through `#[serde(default)]` so old
//! rows with `'{}'` decode cleanly to vanilla MC defaults.

use k8s_openapi::api::core::v1::EnvVar;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Difficulty {
    Peaceful,
    Easy,
    Normal,
    Hard,
}

impl Difficulty {
    fn as_env(self) -> &'static str {
        match self {
            Self::Peaceful => "peaceful",
            Self::Easy => "easy",
            Self::Normal => "normal",
            Self::Hard => "hard",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Gamemode {
    Survival,
    Creative,
    Adventure,
    Spectator,
}

impl Gamemode {
    fn as_env(self) -> &'static str {
        match self {
            Self::Survival => "survival",
            Self::Creative => "creative",
            Self::Adventure => "adventure",
            Self::Spectator => "spectator",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerProperties {
    pub difficulty: Difficulty,
    pub hardcore: bool,
    pub gamemode: Gamemode,
    pub force_gamemode: bool,
    pub max_players: u32,
    pub view_distance: u32,
    pub simulation_distance: u32,
    pub pvp: bool,
    pub white_list: bool,
    pub spawn_protection: u32,
    pub spawn_animals: bool,
    pub spawn_monsters: bool,
    pub spawn_npcs: bool,
    pub allow_flight: bool,
    pub allow_nether: bool,
    pub enable_command_block: bool,
}

impl Default for ServerProperties {
    fn default() -> Self {
        Self {
            difficulty: Difficulty::Normal,
            hardcore: false,
            gamemode: Gamemode::Survival,
            force_gamemode: false,
            max_players: 20,
            view_distance: 10,
            simulation_distance: 10,
            pvp: true,
            white_list: false,
            spawn_protection: 16,
            spawn_animals: true,
            spawn_monsters: true,
            spawn_npcs: true,
            allow_flight: false,
            allow_nether: true,
            enable_command_block: false,
        }
    }
}

impl ServerProperties {
    /// Validates field ranges (enums are covered by Serde at deserialize time).
    ///
    /// # Errors
    ///
    /// Returns [`AppError::BadRequest`] with code `properties_<field>_invalid`
    /// when an integer field is out of its documented range.
    pub fn validate(&self) -> Result<(), AppError> {
        if self.max_players == 0 || self.max_players > 200 {
            return Err(AppError::BadRequest {
                code: "properties_max_players_invalid",
                message: "max_players must be 1..=200".to_owned(),
            });
        }
        if self.view_distance < 3 || self.view_distance > 32 {
            return Err(AppError::BadRequest {
                code: "properties_view_distance_invalid",
                message: "view_distance must be 3..=32".to_owned(),
            });
        }
        if self.simulation_distance < 3 || self.simulation_distance > 32 {
            return Err(AppError::BadRequest {
                code: "properties_simulation_distance_invalid",
                message: "simulation_distance must be 3..=32".to_owned(),
            });
        }
        if self.spawn_protection > 256 {
            return Err(AppError::BadRequest {
                code: "properties_spawn_protection_invalid",
                message: "spawn_protection must be 0..=256".to_owned(),
            });
        }
        Ok(())
    }

    /// Emits all 16 env vars itzg consumes to populate server.properties.
    /// Always emits every field — the stored JSON is canonical.
    #[must_use]
    pub fn to_env(&self) -> Vec<EnvVar> {
        fn kv(name: &str, value: String) -> EnvVar {
            EnvVar {
                name: name.to_owned(),
                value: Some(value),
                value_from: None,
            }
        }
        fn bool_str(b: bool) -> String {
            (if b { "true" } else { "false" }).to_owned()
        }
        vec![
            kv("DIFFICULTY", self.difficulty.as_env().to_owned()),
            kv("HARDCORE", bool_str(self.hardcore)),
            kv("MODE", self.gamemode.as_env().to_owned()),
            kv("FORCE_GAMEMODE", bool_str(self.force_gamemode)),
            kv("MAX_PLAYERS", self.max_players.to_string()),
            kv("VIEW_DISTANCE", self.view_distance.to_string()),
            kv("SIMULATION_DISTANCE", self.simulation_distance.to_string()),
            kv("PVP", bool_str(self.pvp)),
            kv("WHITE_LIST", bool_str(self.white_list)),
            kv("SPAWN_PROTECTION", self.spawn_protection.to_string()),
            kv("SPAWN_ANIMALS", bool_str(self.spawn_animals)),
            kv("SPAWN_MONSTERS", bool_str(self.spawn_monsters)),
            kv("SPAWN_NPCS", bool_str(self.spawn_npcs)),
            kv("ALLOW_FLIGHT", bool_str(self.allow_flight)),
            kv("ALLOW_NETHER", bool_str(self.allow_nether)),
            kv("ENABLE_COMMAND_BLOCK", bool_str(self.enable_command_block)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_vanilla_mc_defaults() {
        let d = ServerProperties::default();
        assert_eq!(d.difficulty, Difficulty::Normal);
        assert!(!d.hardcore);
        assert_eq!(d.gamemode, Gamemode::Survival);
        assert_eq!(d.max_players, 20);
        assert_eq!(d.view_distance, 10);
        assert!(d.pvp);
        assert!(!d.white_list);
    }

    #[test]
    fn empty_object_decodes_to_default() {
        let p: ServerProperties = serde_json::from_str("{}").unwrap();
        assert_eq!(p, ServerProperties::default());
    }

    #[test]
    fn round_trip_serde_preserves_state() {
        let mut p = ServerProperties::default();
        p.difficulty = Difficulty::Hard;
        p.hardcore = true;
        p.max_players = 50;
        let s = serde_json::to_string(&p).unwrap();
        let back: ServerProperties = serde_json::from_str(&s).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn deny_unknown_fields_rejects_typo() {
        let r: Result<ServerProperties, _> =
            serde_json::from_str(r#"{"difficultyy":"hard"}"#);
        assert!(r.is_err());
    }

    #[test]
    fn to_env_emits_sixteen_vars() {
        let env = ServerProperties::default().to_env();
        assert_eq!(env.len(), 16);
    }

    #[test]
    fn to_env_stringifies_booleans_lowercase() {
        let mut p = ServerProperties::default();
        p.pvp = false;
        let env = p.to_env();
        let pvp = env.iter().find(|e| e.name == "PVP").unwrap();
        assert_eq!(pvp.value.as_deref(), Some("false"));
        let wl = env.iter().find(|e| e.name == "WHITE_LIST").unwrap();
        assert_eq!(wl.value.as_deref(), Some("false"));
    }

    #[test]
    fn to_env_stringifies_enums_lowercase() {
        let mut p = ServerProperties::default();
        p.difficulty = Difficulty::Hard;
        p.gamemode = Gamemode::Creative;
        let env = p.to_env();
        let d = env.iter().find(|e| e.name == "DIFFICULTY").unwrap();
        assert_eq!(d.value.as_deref(), Some("hard"));
        let g = env.iter().find(|e| e.name == "MODE").unwrap();
        assert_eq!(g.value.as_deref(), Some("creative"));
    }

    #[test]
    fn to_env_stringifies_integers() {
        let mut p = ServerProperties::default();
        p.max_players = 50;
        p.view_distance = 16;
        let env = p.to_env();
        assert_eq!(
            env.iter()
                .find(|e| e.name == "MAX_PLAYERS")
                .and_then(|e| e.value.as_deref()),
            Some("50"),
        );
        assert_eq!(
            env.iter()
                .find(|e| e.name == "VIEW_DISTANCE")
                .and_then(|e| e.value.as_deref()),
            Some("16"),
        );
    }

    #[test]
    fn validate_rejects_max_players_zero() {
        let mut p = ServerProperties::default();
        p.max_players = 0;
        let err = p.validate().unwrap_err();
        match err {
            AppError::BadRequest { code, .. } => {
                assert_eq!(code, "properties_max_players_invalid")
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_max_players_over_200() {
        let mut p = ServerProperties::default();
        p.max_players = 201;
        assert!(p.validate().is_err());
    }

    #[test]
    fn validate_rejects_view_distance_below_three() {
        let mut p = ServerProperties::default();
        p.view_distance = 2;
        let err = p.validate().unwrap_err();
        match err {
            AppError::BadRequest { code, .. } => {
                assert_eq!(code, "properties_view_distance_invalid")
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_view_distance_above_32() {
        let mut p = ServerProperties::default();
        p.view_distance = 33;
        assert!(p.validate().is_err());
    }

    #[test]
    fn validate_rejects_simulation_distance_out_of_range() {
        let mut p = ServerProperties::default();
        p.simulation_distance = 2;
        assert!(p.validate().is_err());
        p.simulation_distance = 33;
        assert!(p.validate().is_err());
    }

    #[test]
    fn validate_rejects_spawn_protection_above_256() {
        let mut p = ServerProperties::default();
        p.spawn_protection = 257;
        let err = p.validate().unwrap_err();
        match err {
            AppError::BadRequest { code, .. } => {
                assert_eq!(code, "properties_spawn_protection_invalid")
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn validate_accepts_default() {
        ServerProperties::default().validate().expect("default valid");
    }
}
```

- [ ] **Step 2: Register the module**

Edit `backend/src/lib.rs` — insert in the alphabetically-sorted module block (between `routes` and `static_serve`):

```rust
pub mod server_properties;
```

- [ ] **Step 3: Run tests, expect passes**

```
cd backend && cargo test --lib server_properties:: -- --nocapture
```

Expected: 13 passing tests.

- [ ] **Step 4: Lint clean**

```
cd backend && cargo clippy --all-targets --features serve-dir -- -D warnings
```

If clippy flags `clippy::needless_pass_by_value` on `as_env(self)` (Copy enum), add `#[allow(...)]` or take `&self` — pick whichever clippy is happy with. Likewise for any other warnings. Fix all warnings; do not suppress lint discipline.

- [ ] **Step 5: Commit**

```
git add backend/src/server_properties.rs backend/src/lib.rs
git commit -m "feat: typed ServerProperties with serde + validation + env"
```

---

## Task 3: Wire properties through GET /api/servers/:id

**Files:**
- Modify: `backend/src/routes/servers/get.rs`

- [ ] **Step 1: Extend `ServerDetail` and `ServerRow` to carry `properties`**

In `backend/src/routes/servers/get.rs`, add the import at the top of the imports block:

```rust
use crate::server_properties::ServerProperties;
```

Add to the `ServerDetail` struct (after `mod_updates`):

```rust
    /// User-tunable subset of server.properties values applied via env on
    /// next pod start. Defaults to `ServerProperties::default()` for legacy
    /// rows whose JSON is `{}`.
    pub properties: ServerProperties,
```

Add to `ServerRow` struct (after `last_started_at`):

```rust
    pub properties: ServerProperties,
```

Update `ServerRowTuple`:

Currently 10-tuple ending with `Option<i64>` (last_started_at). Append `String` for the JSON column:

```rust
type ServerRowTuple = (
    String,
    String,
    String,
    i64,
    String,
    Option<String>,
    i64,
    Option<i64>,
    i64,
    Option<i64>,
    String,
);
```

- [ ] **Step 2: Update the SELECT and destructuring**

Change the SQL string in `fetch_server_row`:

```rust
    let opt: Option<ServerRowTuple> = sqlx::query_as(
        "SELECT id, name, mc_version, memory_mi, exposure_mode,
                storage_class, storage_size_gi, nodeport, created_at, last_started_at,
                properties
         FROM servers WHERE id = ?",
    )
```

Update the `Some(...)` destructuring + struct construction:

```rust
        Some((
            id,
            name,
            mc_version,
            memory_mi,
            exposure_mode,
            storage_class,
            storage_size_gi,
            nodeport,
            created_at,
            last_started_at,
            properties_json,
        )) => Ok(ServerRow {
            id,
            name,
            mc_version,
            memory_mi,
            exposure_mode,
            storage_class,
            storage_size_gi,
            nodeport: nodeport.and_then(|n| i32::try_from(n).ok()),
            created_at,
            last_started_at,
            properties: serde_json::from_str(&properties_json).unwrap_or_default(),
        }),
```

(The `unwrap_or_default()` ensures a corrupt JSON row doesn't 500 the whole detail fetch — the panel still renders with defaults.)

- [ ] **Step 3: Pass `properties` from row to detail**

In `fetch_detail`, when constructing `ServerDetail` (the bottom of the function), add after `mod_updates`:

```rust
        properties: row.properties,
```

Note: `row` is moved into the struct — make sure `row.properties` is read before any move-ing field below it. Easiest: clone `properties` to a local before the move, OR keep destructuring order so `row.properties` is the last field accessed.

Cleanest: `let properties = row.properties.clone();` immediately after fetch, then use `properties` in the struct literal. Since `ServerProperties` is `Clone`, no cost to readability.

- [ ] **Step 4: Run tests; expect existing detail tests still pass**

```
cd backend && cargo test --package anvil --test '*' -- --nocapture 2>&1 | tail -50
```

Expected: no regressions. `properties` field defaults won't be tested yet here — that's Task 7.

- [ ] **Step 5: Lint clean**

```
cd backend && cargo clippy --all-targets --features serve-dir -- -D warnings
```

- [ ] **Step 6: Commit**

```
git add backend/src/routes/servers/get.rs
git commit -m "feat(api): expose properties on GET /api/servers/:id"
```

---

## Task 4: Accept `properties` on POST /api/servers

**Files:**
- Modify: `backend/src/routes/servers/create.rs`

- [ ] **Step 1: Add `properties` to `CreateRequest` and import**

Top of `create.rs`, in the imports block:

```rust
use crate::server_properties::ServerProperties;
```

In `CreateRequest`, after `paper`:

```rust
    /// Optional server.properties overrides. Missing => vanilla defaults.
    #[serde(default)]
    pub properties: Option<ServerProperties>,
```

In the `let CreateRequest { ... } = request;` destructure, add `properties` to the list.

- [ ] **Step 2: Validate and persist properties JSON**

Right after the existing `validate_storage_size_gi(...)` call, add:

```rust
    let properties = properties.unwrap_or_default();
    properties.validate()?;
    let properties_json =
        serde_json::to_string(&properties).map_err(|e| AppError::Internal(e.into()))?;
```

- [ ] **Step 3: Persist on insert**

Update `insert_server` to accept the JSON, and pass it from the call site. Change signature:

```rust
#[allow(clippy::too_many_arguments)]
async fn insert_server(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    mc_version: &str,
    memory_mi: i64,
    source_kind: &str,
    exposure_mode: &str,
    storage_class: Option<&str>,
    storage_size_gi: i64,
    source_config: &str,
    properties_json: &str,
    nodeport: Option<i32>,
    created_at: i64,
) -> Result<(), AppError> {
```

SQL becomes:

```rust
    let result = sqlx::query(
        "INSERT INTO servers (
            id, name, mc_version, memory_mi,
            exposure_mode, storage_class, storage_size_gi, source_config,
            source_kind, properties, nodeport, created_at, last_started_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
    )
    .bind(id)
    .bind(name)
    .bind(mc_version)
    .bind(memory_mi)
    .bind(exposure_mode)
    .bind(storage_class)
    .bind(storage_size_gi)
    .bind(source_config)
    .bind(source_kind)
    .bind(properties_json)
    .bind(nodeport.map(i64::from))
    .bind(created_at)
    .execute(pool)
    .await;
```

Update the call site in `handle`:

```rust
    insert_server(
        &state.pool,
        &id,
        &name,
        &resolved.mc_version,
        memory_mi,
        resolved.source_kind,
        &exposure_mode,
        storage_class.as_deref(),
        storage_size_gi,
        &resolved.source_config,
        &properties_json,
        nodeport,
        now,
    )
    .await?;
```

Update the test helpers `insert_dummy(...)` calls in the existing `mod tests` block — pass `"{}"` for `properties_json`. The signature change ripples through; fix the test calls accordingly.

- [ ] **Step 4: Layer `properties.to_env()` onto provider env**

Find where `extra_env` is computed:

```rust
    let extra_env = resolved.provider.extra_env(&ctx);
```

Change to:

```rust
    let mut extra_env = resolved.provider.extra_env(&ctx);
    extra_env.extend(properties.to_env());
```

- [ ] **Step 5: Add audit field**

In the `insert_audit` call, add to the JSON object:

```rust
            "properties": &properties,
```

(`serde_json::json!` accepts references.)

- [ ] **Step 6: Run tests, expect passes**

```
cd backend && cargo test --package anvil --lib routes::servers::create:: -- --nocapture
```

Expected: existing 5 tests still pass with `"{}"` passed in for properties_json.

- [ ] **Step 7: Lint clean**

```
cd backend && cargo clippy --all-targets --features serve-dir -- -D warnings
```

- [ ] **Step 8: Commit**

```
git add backend/src/routes/servers/create.rs
git commit -m "feat(api): persist + emit env from properties on create"
```

---

## Task 5: Accept `properties` on PATCH /api/servers/:id/settings

**Files:**
- Modify: `backend/src/routes/servers/settings.rs`

- [ ] **Step 1: Add `properties` to `SettingsRequest`**

Top of `settings.rs`, in the imports:

```rust
use crate::server_properties::ServerProperties;
```

In `SettingsRequest`:

```rust
    /// Full-replacement when present. Validated and written verbatim to
    /// `servers.properties`; the StatefulSet env is rebuilt and patched in
    /// the same path as `memory_mi`.
    pub properties: Option<ServerProperties>,
```

- [ ] **Step 2: Wire the validation + UPDATE**

Find the block that validates `req.memory_mi` / `req.version_skip` / etc. Add after them:

```rust
    if let Some(p) = req.properties.as_ref() {
        p.validate()?;
    }
```

- [ ] **Step 3: Persist properties JSON when present**

Find the `if let Some(m) = req.memory_mi { ... }` block. Below it, add:

```rust
    if let Some(p) = req.properties.as_ref() {
        let raw = serde_json::to_string(p).map_err(|e| AppError::Internal(e.into()))?;
        sqlx::query("UPDATE servers SET properties = ? WHERE id = ?")
            .bind(&raw)
            .bind(&id)
            .execute(&state.pool)
            .await?;
    }
```

- [ ] **Step 4: Rebuild env when properties OR memory changed**

Currently the patch_statefulset_env call only fires inside `if let Some(m) = req.memory_mi`. Lift the env rebuild out so it fires when *either* memory OR properties changed.

Replace the `if let Some(m) = req.memory_mi { ... patch_statefulset_env ... }` block with:

```rust
    if req.memory_mi.is_some() || req.properties.is_some() {
        // Re-read the canonical memory + properties from the DB after the
        // updates above so the rebuilt env reflects the merged final state.
        let new_env = build_full_env_for_running_runtime(&state.pool, &id).await?;
        if let Err(e) =
            patch_statefulset_env(&state.kube, &state.mc_namespace, &id, &new_env).await
        {
            tracing::warn!(
                server.id = %id,
                error = %e,
                "settings PATCH wrote SQLite but failed to patch StatefulSet env",
            );
        }
    }
```

(Note: we drop the `m` argument from `build_full_env_for_running_runtime` — it now reads memory from the row. Refactor is in Step 5.)

- [ ] **Step 5: Refactor `build_full_env_for_running_runtime` to read everything from DB**

Change its signature and body:

```rust
async fn build_full_env_for_running_runtime(
    pool: &sqlx::SqlitePool,
    server_id: &str,
) -> Result<Vec<EnvVar>, AppError> {
    let row: (String, String, String, i64, String) = sqlx::query_as(
        "SELECT source_kind, source_config, mc_version, memory_mi, properties
         FROM servers WHERE id = ?",
    )
    .bind(server_id)
    .fetch_one(pool)
    .await?;
    let (source_kind, source_config, mc_version, memory_mi, properties_json) = row;
    let properties: ServerProperties =
        serde_json::from_str(&properties_json).unwrap_or_default();
    let mut env = if source_kind == "vanilla" {
        VanillaProvider::build_env(server_id, &mc_version, memory_mi)
    } else {
        let provider = from_db(&source_kind, &source_config)
            .map_err(|e| AppError::Internal(anyhow::anyhow!("rebuild provider: {e}")))?;
        provider.extra_env(&ProviderContext {
            server_id,
            memory_mi,
        })
    };
    env.extend(properties.to_env());
    Ok(env)
}
```

- [ ] **Step 6: Update audit and tests**

In the `audit` block, when properties was set, include it. Find:

```rust
    if let Some(m) = req.memory_mi
        && let Some(obj) = audit.as_object_mut()
    {
        obj.insert("memory_mi".into(), serde_json::json!(m));
    }
```

Below that, add:

```rust
    if let Some(p) = req.properties.as_ref()
        && let Some(obj) = audit.as_object_mut()
    {
        obj.insert("properties".into(), serde_json::json!(p));
    }
```

Existing tests at the bottom of `settings.rs` call `build_full_env_for_running_runtime(&pool, "v1", 8192)` — change to `build_full_env_for_running_runtime(&pool, "v1")` and update the seed `insert_server` test helper to pass the desired memory in the row directly. The signature of the test helper already accepts `memory_mi: i64`; just remove the override argument.

Also update `insert_server` test helper: the columns inserted don't include `properties`. SQLite's column DEFAULT covers it — no helper change needed for that.

- [ ] **Step 7: Add a test that properties round-trip through env rebuild**

Append to the `tests` mod in `settings.rs`:

```rust
    #[tokio::test]
    async fn rebuild_env_includes_properties_overrides() {
        let pool = seed_pool().await;
        insert_server(&pool, "v1", "vanilla", "{}", "1.21.4", 4096).await;
        // Poke the properties column directly to simulate a prior PATCH.
        sqlx::query("UPDATE servers SET properties = ? WHERE id = ?")
            .bind(r#"{"difficulty":"hard","max_players":50}"#)
            .bind("v1")
            .execute(&pool)
            .await
            .unwrap();

        let env = build_full_env_for_running_runtime(&pool, "v1")
            .await
            .expect("rebuild");

        assert_eq!(env_value(&env, "DIFFICULTY"), Some("hard"));
        assert_eq!(env_value(&env, "MAX_PLAYERS"), Some("50"));
        // Provider env still present.
        assert_eq!(env_value(&env, "MAX_MEMORY"), Some("4096M"));
    }
```

- [ ] **Step 8: Run the full test suite**

```
cd backend && cargo test -- --nocapture 2>&1 | tail -80
```

Expected: all tests pass, including the new `rebuild_env_includes_properties_overrides`.

- [ ] **Step 9: Lint clean**

```
cd backend && cargo clippy --all-targets --features serve-dir -- -D warnings
```

- [ ] **Step 10: Commit**

```
git add backend/src/routes/servers/settings.rs
git commit -m "feat(api): accept properties on PATCH /settings, rebuild env"
```

---

## Task 6: SQLx prepare + embed feature build

**Files:**
- Modify: `backend/.sqlx/*` (regenerated by `cargo sqlx prepare`)

- [ ] **Step 1: Regenerate sqlx offline metadata**

```
cd backend && DATABASE_URL=sqlite::memory: cargo sqlx prepare --workspace
```

If `cargo sqlx prepare` complains about unused or stale cache files, run:

```
cd backend && rm -rf .sqlx && DATABASE_URL=sqlite::memory: cargo sqlx prepare --workspace
```

- [ ] **Step 2: Verify the embed feature compiles**

```
cd backend && cargo clippy --all-targets --features embed -- -D warnings
```

(Both feature flavors must be checked — running both `serve-dir` and `embed` lints catches any feature-gated drift.)

- [ ] **Step 3: Commit**

```
git add backend/.sqlx
git commit -m "chore(sqlx): refresh offline metadata for properties column"
```

---

## Task 7: Backend integration test

**Files:**
- Create: `backend/tests/server_properties_e2e.rs`

- [ ] **Step 1: Write the integration test**

```rust
//! E2E sanity for the new properties column: round-trips through the DB
//! and the env-rebuild helper. No k8s API in scope — these are pool-only
//! tests using the same in-memory SQLite the other test files use.

use anvil::server_properties::{Difficulty, Gamemode, ServerProperties};

#[tokio::test]
async fn properties_default_serialises_to_full_env() {
    let p = ServerProperties::default();
    let env = p.to_env();
    let names: Vec<&str> = env.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"DIFFICULTY"));
    assert!(names.contains(&"HARDCORE"));
    assert!(names.contains(&"MODE"));
    assert!(names.contains(&"MAX_PLAYERS"));
    assert!(names.contains(&"VIEW_DISTANCE"));
    assert!(names.contains(&"PVP"));
    assert!(names.contains(&"WHITE_LIST"));
}

#[tokio::test]
async fn properties_round_trip_via_db_column() {
    let pool = anvil::db::init("sqlite::memory:").await.unwrap();

    sqlx::query(
        "INSERT INTO servers (
            id, name, mc_version, memory_mi, exposure_mode, storage_class,
            storage_size_gi, source_config, source_kind, properties,
            nodeport, created_at, last_started_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
    )
    .bind("srv-1")
    .bind("test")
    .bind("1.21.4")
    .bind(4096_i64)
    .bind("clusterip")
    .bind(Option::<String>::None)
    .bind(10_i64)
    .bind("{}")
    .bind("vanilla")
    .bind(r#"{"difficulty":"hard","hardcore":true,"max_players":42}"#)
    .bind(Option::<i64>::None)
    .bind(0_i64)
    .execute(&pool)
    .await
    .expect("insert");

    let raw: String = sqlx::query_scalar("SELECT properties FROM servers WHERE id = ?")
        .bind("srv-1")
        .fetch_one(&pool)
        .await
        .unwrap();
    let p: ServerProperties = serde_json::from_str(&raw).unwrap();
    assert_eq!(p.difficulty, Difficulty::Hard);
    assert!(p.hardcore);
    assert_eq!(p.gamemode, Gamemode::Survival); // default
    assert_eq!(p.max_players, 42);
    assert!(p.pvp); // default
}

#[tokio::test]
async fn legacy_empty_object_decodes_to_default() {
    let pool = anvil::db::init("sqlite::memory:").await.unwrap();
    sqlx::query(
        "INSERT INTO servers (
            id, name, mc_version, memory_mi, exposure_mode, storage_class,
            storage_size_gi, source_config, source_kind, properties,
            nodeport, created_at, last_started_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
    )
    .bind("srv-2")
    .bind("legacy")
    .bind("1.21.4")
    .bind(4096_i64)
    .bind("clusterip")
    .bind(Option::<String>::None)
    .bind(10_i64)
    .bind("{}")
    .bind("vanilla")
    .bind("{}") // legacy default
    .bind(Option::<i64>::None)
    .bind(0_i64)
    .execute(&pool)
    .await
    .expect("insert");

    let raw: String = sqlx::query_scalar("SELECT properties FROM servers WHERE id = ?")
        .bind("srv-2")
        .fetch_one(&pool)
        .await
        .unwrap();
    let p: ServerProperties = serde_json::from_str(&raw).unwrap();
    assert_eq!(p, ServerProperties::default());
}
```

- [ ] **Step 2: Run integration tests**

```
cd backend && cargo test --test server_properties_e2e -- --nocapture
```

Expected: 3 passing tests.

- [ ] **Step 3: Commit**

```
git add backend/tests/server_properties_e2e.rs
git commit -m "test: round-trip properties column through SQLite"
```

---

## Task 8: Frontend types — Zod schema + DEFAULT_PROPERTIES

**Files:**
- Modify: `frontend/app/lib/api.ts`

- [ ] **Step 1: Add the schemas + types**

Insert just after `serverStatusSchema` (around line 21) — keeping all schema declarations in one block at the top of the file:

```ts
// --- server.properties subset --------------------------------------------

export const difficultySchema = z.enum([
	"peaceful",
	"easy",
	"normal",
	"hard",
]);
export const gamemodeSchema = z.enum([
	"survival",
	"creative",
	"adventure",
	"spectator",
]);

export const serverPropertiesSchema = z.object({
	difficulty: difficultySchema.default("normal"),
	hardcore: z.boolean().default(false),
	gamemode: gamemodeSchema.default("survival"),
	force_gamemode: z.boolean().default(false),
	max_players: z.number().int().min(1).max(200).default(20),
	view_distance: z.number().int().min(3).max(32).default(10),
	simulation_distance: z.number().int().min(3).max(32).default(10),
	pvp: z.boolean().default(true),
	white_list: z.boolean().default(false),
	spawn_protection: z.number().int().min(0).max(256).default(16),
	spawn_animals: z.boolean().default(true),
	spawn_monsters: z.boolean().default(true),
	spawn_npcs: z.boolean().default(true),
	allow_flight: z.boolean().default(false),
	allow_nether: z.boolean().default(true),
	enable_command_block: z.boolean().default(false),
});

export type Difficulty = z.infer<typeof difficultySchema>;
export type Gamemode = z.infer<typeof gamemodeSchema>;
export type ServerProperties = z.infer<typeof serverPropertiesSchema>;

export const DEFAULT_PROPERTIES: ServerProperties = serverPropertiesSchema.parse({});
```

- [ ] **Step 2: Add `properties` to `serverDetailSchema`**

Locate `serverDetailSchema = serverSummarySchema.extend({...})`. Append a `properties` field:

```ts
	properties: serverPropertiesSchema.default(DEFAULT_PROPERTIES),
```

(The default protects against backend-deploy drift where the field is missing.)

- [ ] **Step 3: Add `properties` to `createServerRequestSchema`**

In `createServerRequestSchema`, append:

```ts
	properties: serverPropertiesSchema.optional(),
```

- [ ] **Step 4: Add `properties` to `settingsRequestSchema`**

In `settingsRequestSchema`, append:

```ts
	properties: serverPropertiesSchema.optional(),
```

- [ ] **Step 5: Type-check**

```
cd frontend && pnpm typecheck
```

Expected: no new errors.

- [ ] **Step 6: Lint**

```
cd frontend && pnpm lint
```

Expected: clean.

- [ ] **Step 7: Commit**

```
git add frontend/app/lib/api.ts
git commit -m "feat(api): typescript types + Zod schemas for ServerProperties"
```

---

## Task 9: Create form — Section 06 World

**Files:**
- Modify: `frontend/app/servers/new/page.tsx`

- [ ] **Step 1: Add World section state**

Update imports at top of the file:

```tsx
import {
	ApiError,
	createServer,
	DEFAULT_PROPERTIES,
	fetchCapabilities,
	type CfChannel,
	type ClusterCapabilities,
	type CreateServerRequest,
	type Difficulty,
	type ExposureMode,
	type Gamemode,
	type ModEntry,
	type Runtime,
	type ServerProperties,
} from "../../lib/api";
```

- [ ] **Step 2: Extend `CreateDraft` with `properties`**

In `frontend/app/components/BuildSlip.tsx` (where `CreateDraft` is declared), add:

```ts
properties: ServerProperties;
```

(This is a controlled state object for the World fields.)

In `frontend/app/servers/new/page.tsx`, update `INITIAL`:

```tsx
const INITIAL: CreateDraft = {
	name: "",
	type: "vanilla",
	mc_version: null,
	memory_mi: 4096,
	storage_size_gi: 20,
	storage_class: null,
	exposure_mode: "loadbalancer",
	curseforge: null,
	modrinth: null,
	runtime: null,
	initial_mods: [],
	initial_plugins: [],
	loader_version: null,
	properties: DEFAULT_PROPERTIES,
};
```

- [ ] **Step 3: Define option lists**

Above `INITIAL`, add:

```tsx
const DIFFICULTY_OPTIONS: ReadonlyArray<{ value: Difficulty; label: string }> = [
	{ value: "peaceful", label: "peaceful" },
	{ value: "easy", label: "easy" },
	{ value: "normal", label: "normal" },
	{ value: "hard", label: "hard" },
];
const GAMEMODE_OPTIONS: ReadonlyArray<{ value: Gamemode; label: string }> = [
	{ value: "survival", label: "survival" },
	{ value: "creative", label: "creative" },
	{ value: "adventure", label: "adventure" },
	{ value: "spectator", label: "spectator" },
];
const HARDCORE_OPTIONS: ReadonlyArray<{ value: "off" | "on"; label: string }> = [
	{ value: "off", label: "off" },
	{ value: "on", label: "on" },
];
```

- [ ] **Step 4: Render Section 06**

Find the closing `</Section>` of section "05" titled "storage". Immediately after it, add:

```tsx
<Section number="06" title="world">
	<Card>
		<div className="flex flex-col gap-4">
			<div>
				<label className="mb-1 block font-mono text-[11px] uppercase tracking-wider text-text-muted">
					difficulty
				</label>
				<SegmentedControl
					ariaLabel="difficulty"
					value={draft.properties.difficulty}
					onChange={(v) => {
						set("properties", { ...draft.properties, difficulty: v });
					}}
					options={DIFFICULTY_OPTIONS}
				/>
			</div>
			<div>
				<label className="mb-1 block font-mono text-[11px] uppercase tracking-wider text-text-muted">
					gamemode
				</label>
				<SegmentedControl
					ariaLabel="gamemode"
					value={draft.properties.gamemode}
					onChange={(v) => {
						set("properties", { ...draft.properties, gamemode: v });
					}}
					options={GAMEMODE_OPTIONS}
				/>
			</div>
			<div>
				<label className="mb-1 block font-mono text-[11px] uppercase tracking-wider text-text-muted">
					hardcore
				</label>
				<SegmentedControl
					ariaLabel="hardcore"
					value={draft.properties.hardcore ? "on" : "off"}
					onChange={(v) => {
						set("properties", {
							...draft.properties,
							hardcore: v === "on",
						});
					}}
					options={HARDCORE_OPTIONS}
				/>
				<p className="mt-1 font-mono text-[11px] text-text-faint">
					hardcore bans players on death — only meaningful from a fresh world
				</p>
			</div>
		</div>
	</Card>
</Section>
```

- [ ] **Step 5: Submit `properties` on create**

In `submit()`, in the `request: CreateServerRequest = {...}` literal, append:

```tsx
				properties: draft.properties,
```

(This always sends the full `ServerProperties` object — defaults included for fields the form doesn't expose.)

- [ ] **Step 6: Type-check + lint**

```
cd frontend && pnpm typecheck && pnpm lint
```

Expected: clean.

- [ ] **Step 7: Commit**

```
git add frontend/app/servers/new/page.tsx frontend/app/components/BuildSlip.tsx
git commit -m "feat(ui): create form World section (difficulty, gamemode, hardcore)"
```

---

## Task 10: Properties tab body

**Files:**
- Create: `frontend/app/servers/tabs/PropertiesBody.tsx`

- [ ] **Step 1: Write the component**

```tsx
"use client";

import { useState, type ChangeEvent, type ReactElement, type ReactNode } from "react";

import {
	ApiError,
	updateServerSettings,
	type Difficulty,
	type Gamemode,
	type ServerProperties,
} from "../../lib/api";
import { useServerDetail } from "../../lib/server-detail-context";

import { Button } from "../../components/Button";
import { Card } from "../../components/Card";
import { SegmentedControl } from "../../components/SegmentedControl";
import { Tooltip } from "../../components/Tooltip";
import { useToast } from "../../components/Toast";

const DIFFICULTY_OPTIONS: ReadonlyArray<{ value: Difficulty; label: string }> = [
	{ value: "peaceful", label: "peaceful" },
	{ value: "easy", label: "easy" },
	{ value: "normal", label: "normal" },
	{ value: "hard", label: "hard" },
];
const GAMEMODE_OPTIONS: ReadonlyArray<{ value: Gamemode; label: string }> = [
	{ value: "survival", label: "survival" },
	{ value: "creative", label: "creative" },
	{ value: "adventure", label: "adventure" },
	{ value: "spectator", label: "spectator" },
];
const TOGGLE_OPTIONS: ReadonlyArray<{ value: "off" | "on"; label: string }> = [
	{ value: "off", label: "off" },
	{ value: "on", label: "on" },
];

interface FieldHelp {
	tip: string;
	wikiAnchor: string;
}

const FIELD_HELP: Record<keyof ServerProperties, FieldHelp> = {
	difficulty: {
		tip: "controls hostile mob damage and spawning",
		wikiAnchor: "difficulty",
	},
	hardcore: {
		tip: "bans players on death; only meaningful from a fresh world",
		wikiAnchor: "hardcore",
	},
	gamemode: {
		tip: "default gamemode for new players",
		wikiAnchor: "gamemode",
	},
	force_gamemode: {
		tip: "forces all players back to the default gamemode on join",
		wikiAnchor: "force-gamemode",
	},
	max_players: {
		tip: "maximum concurrent players",
		wikiAnchor: "max-players",
	},
	view_distance: {
		tip: "chunks visible per player; 32 is max",
		wikiAnchor: "view-distance",
	},
	simulation_distance: {
		tip: "chunks ticking per player; usually <= view-distance",
		wikiAnchor: "simulation-distance",
	},
	pvp: {
		tip: "allow player-vs-player damage",
		wikiAnchor: "pvp",
	},
	white_list: {
		tip: "enforce whitelist; manage names in the players tab",
		wikiAnchor: "white-list",
	},
	spawn_protection: {
		tip: "blocks of spawn radius non-ops cannot modify; 0 disables",
		wikiAnchor: "spawn-protection",
	},
	spawn_animals: {
		tip: "passive mobs (cows, sheep, …) spawn",
		wikiAnchor: "spawn-animals",
	},
	spawn_monsters: {
		tip: "hostile mobs spawn",
		wikiAnchor: "spawn-monsters",
	},
	spawn_npcs: {
		tip: "villagers spawn",
		wikiAnchor: "spawn-npcs",
	},
	allow_flight: {
		tip: "lets clients fly (mods/creative); kicks otherwise",
		wikiAnchor: "allow-flight",
	},
	allow_nether: {
		tip: "permits nether portals",
		wikiAnchor: "allow-nether",
	},
	enable_command_block: {
		tip: "command blocks tickable by ops",
		wikiAnchor: "enable-command-block",
	},
};

const WIKI_BASE = "https://minecraft.wiki/w/Server.properties#";

function InfoLink({ field }: { field: keyof ServerProperties }): ReactElement {
	const help = FIELD_HELP[field];
	return (
		<Tooltip label={help.tip}>
			<a
				href={`${WIKI_BASE}${help.wikiAnchor}`}
				target="_blank"
				rel="noopener noreferrer"
				aria-label={`${field} on the minecraft wiki`}
				className="ml-1 inline-flex h-4 w-4 items-center justify-center rounded-full border border-border text-[10px] text-text-muted hover:text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
			>
				i
			</a>
		</Tooltip>
	);
}

interface RowProps {
	field: keyof ServerProperties;
	label: string;
	children: ReactNode;
}

function Row({ field, label, children }: RowProps): ReactElement {
	return (
		<div className="flex items-center justify-between gap-3">
			<div className="flex items-center font-mono text-[11px] uppercase tracking-wider text-text-muted">
				<span>{label}</span>
				<InfoLink field={field} />
			</div>
			<div>{children}</div>
		</div>
	);
}

interface ToggleRowProps {
	field: keyof ServerProperties;
	label: string;
	value: boolean;
	onChange: (next: boolean) => void;
}

function ToggleRow({ field, label, value, onChange }: ToggleRowProps): ReactElement {
	return (
		<Row field={field} label={label}>
			<SegmentedControl
				ariaLabel={label}
				value={value ? "on" : "off"}
				options={TOGGLE_OPTIONS}
				onChange={(v) => {
					onChange(v === "on");
				}}
			/>
		</Row>
	);
}

interface NumberRowProps {
	field: keyof ServerProperties;
	label: string;
	value: number;
	min: number;
	max: number;
	onChange: (next: number) => void;
}

function NumberRow({
	field,
	label,
	value,
	min,
	max,
	onChange,
}: NumberRowProps): ReactElement {
	return (
		<Row field={field} label={label}>
			<input
				type="number"
				min={min}
				max={max}
				value={value}
				onChange={(e: ChangeEvent<HTMLInputElement>) => {
					const n = Number.parseInt(e.target.value, 10);
					if (Number.isFinite(n)) onChange(n);
				}}
				className="w-20 rounded-md border border-border bg-bg px-2 py-1 text-right font-mono text-[12px] text-text-body focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
			/>
		</Row>
	);
}

function shallowEqual<T extends Record<string, unknown>>(a: T, b: T): boolean {
	const ak = Object.keys(a);
	const bk = Object.keys(b);
	if (ak.length !== bk.length) return false;
	return ak.every((k) => a[k] === b[k]);
}

export function PropertiesBody(): ReactElement {
	const { detail, refresh } = useServerDetail();
	const toast = useToast();
	const [props, setProps] = useState<ServerProperties>(detail.properties);
	const [busy, setBusy] = useState(false);
	const dirty = !shallowEqual(props, detail.properties);

	const set = <K extends keyof ServerProperties>(
		k: K,
		v: ServerProperties[K],
	): void => {
		setProps((p) => ({ ...p, [k]: v }));
	};

	const save = (): void => {
		setBusy(true);
		updateServerSettings(detail.id, { properties: props })
			.then(() => {
				toast.push("settings saved · applies on next start", "success");
				refresh();
			})
			.catch((err: unknown) => {
				const msg =
					err instanceof ApiError
						? `${err.code}: ${err.message}`
						: err instanceof Error
							? err.message
							: "unknown error";
				toast.push(`save failed · ${msg}`, "error");
			})
			.finally(() => {
				setBusy(false);
			});
	};

	return (
		<div className="flex max-w-2xl flex-col gap-4">
			<Card header="world">
				<div className="flex flex-col gap-3">
					<Row field="difficulty" label="difficulty">
						<SegmentedControl
							ariaLabel="difficulty"
							value={props.difficulty}
							options={DIFFICULTY_OPTIONS}
							onChange={(v) => {
								set("difficulty", v);
							}}
						/>
					</Row>
					<Row field="gamemode" label="gamemode">
						<SegmentedControl
							ariaLabel="gamemode"
							value={props.gamemode}
							options={GAMEMODE_OPTIONS}
							onChange={(v) => {
								set("gamemode", v);
							}}
						/>
					</Row>
					<ToggleRow
						field="hardcore"
						label="hardcore"
						value={props.hardcore}
						onChange={(v) => {
							set("hardcore", v);
						}}
					/>
					<ToggleRow
						field="force_gamemode"
						label="force gamemode"
						value={props.force_gamemode}
						onChange={(v) => {
							set("force_gamemode", v);
						}}
					/>
				</div>
			</Card>

			<Card header="players">
				<div className="flex flex-col gap-3">
					<NumberRow
						field="max_players"
						label="max players"
						value={props.max_players}
						min={1}
						max={200}
						onChange={(v) => {
							set("max_players", v);
						}}
					/>
					<NumberRow
						field="view_distance"
						label="view distance"
						value={props.view_distance}
						min={3}
						max={32}
						onChange={(v) => {
							set("view_distance", v);
						}}
					/>
					<NumberRow
						field="simulation_distance"
						label="simulation distance"
						value={props.simulation_distance}
						min={3}
						max={32}
						onChange={(v) => {
							set("simulation_distance", v);
						}}
					/>
					<ToggleRow
						field="pvp"
						label="pvp"
						value={props.pvp}
						onChange={(v) => {
							set("pvp", v);
						}}
					/>
					<ToggleRow
						field="white_list"
						label="whitelist enforced"
						value={props.white_list}
						onChange={(v) => {
							set("white_list", v);
						}}
					/>
				</div>
			</Card>

			<Card header="spawn">
				<div className="flex flex-col gap-3">
					<NumberRow
						field="spawn_protection"
						label="spawn protection"
						value={props.spawn_protection}
						min={0}
						max={256}
						onChange={(v) => {
							set("spawn_protection", v);
						}}
					/>
					<ToggleRow
						field="spawn_animals"
						label="spawn animals"
						value={props.spawn_animals}
						onChange={(v) => {
							set("spawn_animals", v);
						}}
					/>
					<ToggleRow
						field="spawn_monsters"
						label="spawn monsters"
						value={props.spawn_monsters}
						onChange={(v) => {
							set("spawn_monsters", v);
						}}
					/>
					<ToggleRow
						field="spawn_npcs"
						label="spawn npcs"
						value={props.spawn_npcs}
						onChange={(v) => {
							set("spawn_npcs", v);
						}}
					/>
				</div>
			</Card>

			<Card header="features">
				<div className="flex flex-col gap-3">
					<ToggleRow
						field="allow_flight"
						label="allow flight"
						value={props.allow_flight}
						onChange={(v) => {
							set("allow_flight", v);
						}}
					/>
					<ToggleRow
						field="allow_nether"
						label="allow nether"
						value={props.allow_nether}
						onChange={(v) => {
							set("allow_nether", v);
						}}
					/>
					<ToggleRow
						field="enable_command_block"
						label="command blocks"
						value={props.enable_command_block}
						onChange={(v) => {
							set("enable_command_block", v);
						}}
					/>
				</div>
			</Card>

			<div className="flex justify-end gap-2">
				<Button variant="primary" disabled={!dirty || busy} onClick={save}>
					save
				</Button>
			</div>
		</div>
	);
}
```

- [ ] **Step 2: Type-check + lint**

```
cd frontend && pnpm typecheck && pnpm lint
```

Expected: clean.

- [ ] **Step 3: Commit**

```
git add frontend/app/servers/tabs/PropertiesBody.tsx
git commit -m "feat(ui): properties tab with 16 typed fields and wiki links"
```

---

## Task 11: Register Properties tab in detail view

**Files:**
- Modify: `frontend/app/servers/ServerDetailView.tsx`

- [ ] **Step 1: Import the body**

Top of file, in the tab imports block, add (alphabetically) just before `SettingsBody`:

```tsx
import { PropertiesBody } from "./tabs/PropertiesBody";
```

- [ ] **Step 2: Extend `TabId` and `TAB_IDS`**

Update the union:

```tsx
type TabId =
	| "overview"
	| "console"
	| "mods"
	| "players"
	| "backups"
	| "files"
	| "properties"
	| "settings";
```

And the array — `properties` between `files` and `settings`:

```tsx
const TAB_IDS: ReadonlyArray<TabId> = [
	"overview",
	"console",
	"mods",
	"players",
	"backups",
	"files",
	"properties",
	"settings",
];
```

- [ ] **Step 3: Render the body**

Find the switch / map that renders `<TabBody />` based on `tab`. Add a case (or array entry) for `properties`:

```tsx
case "properties":
	return <PropertiesBody />;
```

(If the tabs are rendered via a map of `{id, label, body}`, mirror that shape.)

The exact insertion shape depends on the file's current pattern — read 60-100 lines around the existing tab switch and follow it.

- [ ] **Step 4: Type-check + lint**

```
cd frontend && pnpm typecheck && pnpm lint
```

Expected: clean.

- [ ] **Step 5: Build the frontend**

```
cd frontend && pnpm build
```

Expected: build succeeds. Static export goes to `frontend/out/`.

- [ ] **Step 6: Commit**

```
git add frontend/app/servers/ServerDetailView.tsx
git commit -m "feat(ui): wire properties tab between files and settings"
```

---

## Task 12: Final verification

- [ ] **Step 1: Run full backend test suite**

```
cd backend && cargo test --all -- --nocapture 2>&1 | tail -30
```

Expected: all tests pass.

- [ ] **Step 2: Lint both feature flavours**

```
cd backend && cargo clippy --all-targets --features serve-dir -- -D warnings
cd backend && cargo clippy --all-targets --features embed -- -D warnings
```

Expected: clean both ways.

- [ ] **Step 3: Format**

```
cd backend && cargo fmt --all
cd frontend && pnpm lint
```

- [ ] **Step 4: Frontend build**

```
cd frontend && pnpm build
```

Expected: clean static export.

- [ ] **Step 5: Smoke run (manual)**

Build the binary with embed feature:

```
cd frontend && pnpm build
cd ../backend && cargo build --release --features embed
```

Confirm the binary builds. (No live cluster smoke test — that's done by the operator in their homelab.)

- [ ] **Step 6: Commit any formatting drift, push nothing**

```
git status
# If anything was reformatted:
git add -u
git commit -m "chore: cargo fmt"
```

---

## Self-review checklist

Run after the plan is fully written:

1. **Spec coverage** — every section of `2026-05-06-server-properties-design.md` maps to a task:
   - Schema → Task 1 ✓
   - `ServerProperties` struct + Default + validate + to_env → Task 2 ✓
   - Layering into providers → Tasks 4, 5 ✓
   - POST /api/servers → Task 4 ✓
   - PATCH /settings → Task 5 ✓
   - GET detail → Task 3 ✓
   - Frontend types → Task 8 ✓
   - Create form Section 06 → Task 9 ✓
   - PropertiesBody → Task 10 ✓
   - Tab registration → Task 11 ✓
   - Tests → Tasks 2, 5, 7 ✓

2. **Placeholder scan** — no "TBD"/"TODO"/"add appropriate"/"similar to". Every step has concrete code.

3. **Type consistency** — `ServerProperties` field names match across Rust, Zod, TS, SQL JSON. The env names (`MODE` for gamemode, etc.) match the itzg docs. The detail GET, settings PATCH, and create POST all serialize the same shape.

4. **Risk** — `MODE` env var name (not `GAMEMODE`) is itzg's convention; verified against
   `docker-minecraft-server.readthedocs.io/en/latest/configuration/server-properties/#mode`.
   Recorded in the plan inline; backend tests will catch a misspelling.
