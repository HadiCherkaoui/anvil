//! Typed wrapper over the subset of itzg's server.properties env vars
//! Anvil exposes through the Properties tab.
//!
//! Stored as JSON in `servers.properties`; deserialized through
//! `#[serde(default)]` so legacy rows persisted before the column existed
//! (now backfilled to `'{}'`) decode cleanly to vanilla MC defaults.
//!
//! itzg's image overlays env vars onto `server.properties` on every boot,
//! so [`ServerProperties::to_env`] always emits every entry — the
//! stored JSON is canonical and the next pod start picks the values up.

use k8s_openapi::api::core::v1::EnvVar;
use serde::{Deserialize, Serialize};

use crate::error::AppError;

/// Vanilla Minecraft world difficulty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Difficulty {
    Peaceful,
    Easy,
    Normal,
    Hard,
}

impl Difficulty {
    /// Returns the lowercase token Mojang's server.properties expects.
    fn as_env(self) -> &'static str {
        match self {
            Self::Peaceful => "peaceful",
            Self::Easy => "easy",
            Self::Normal => "normal",
            Self::Hard => "hard",
        }
    }
}

/// Vanilla Minecraft default gamemode for new players.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Gamemode {
    Survival,
    Creative,
    Adventure,
    Spectator,
}

impl Gamemode {
    /// Returns the lowercase token Mojang's server.properties expects.
    fn as_env(self) -> &'static str {
        match self {
            Self::Survival => "survival",
            Self::Creative => "creative",
            Self::Adventure => "adventure",
            Self::Spectator => "spectator",
        }
    }
}

/// Curated subset of `server.properties` keys the panel exposes.
///
/// Every field has a `#[serde(default)]` via the struct-level attribute, so
/// a `'{}'` JSON value decodes to [`ServerProperties::default`] (which
/// matches vanilla MC defaults). New fields can be added without a schema
/// migration: existing rows fall back to the new `Default` value.
///
/// The companion [`ServerProperties::to_env`] always emits every entry,
/// matching itzg's `KEY=VALUE` overlay convention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, deny_unknown_fields)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "mirrors itzg's flat KEY=bool env contract; a state machine would obscure the 1:1 mapping"
)]
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
    /// World seed (itzg `SEED`, vanilla `level-seed`). Empty = random.
    /// Only meaningful at world generation; existing worlds keep their
    /// `level.dat` seed regardless of what is written here on restart.
    pub seed: String,
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
            seed: String::new(),
        }
    }
}

impl ServerProperties {
    /// Validates the integer field ranges.
    ///
    /// Enum fields are infallible via Serde — they fail at deserialize time
    /// with a useful message before reaching this method.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::BadRequest`] with code `properties_<field>_invalid`
    /// when a numeric field falls outside its documented vanilla range.
    pub fn validate(&self) -> Result<(), AppError> {
        if self.max_players == 0 || self.max_players > 200 {
            return Err(AppError::BadRequest {
                code: "properties_max_players_invalid",
                message: "max_players must be 1..=200".to_owned(),
            });
        }
        if !(3..=32).contains(&self.view_distance) {
            return Err(AppError::BadRequest {
                code: "properties_view_distance_invalid",
                message: "view_distance must be 3..=32".to_owned(),
            });
        }
        if !(3..=32).contains(&self.simulation_distance) {
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
        // Cap at 256 bytes (any usable seed fits) and forbid control chars
        // because itzg renders SEED into the line-based server.properties
        // file as `level-seed=<value>` — a stray newline would corrupt it.
        if self.seed.len() > 256 {
            return Err(AppError::BadRequest {
                code: "properties_seed_invalid",
                message: "seed must be ≤ 256 bytes".to_owned(),
            });
        }
        if self
            .seed
            .chars()
            .any(|c| (c as u32) < 0x20 || c == '\u{7f}')
        {
            return Err(AppError::BadRequest {
                code: "properties_seed_invalid",
                message: "seed must not contain control characters".to_owned(),
            });
        }
        Ok(())
    }

    /// Emits the env vars itzg consumes to populate server.properties.
    ///
    /// Every field is always emitted: the stored JSON is the canonical
    /// state, so re-applying it on each pod start keeps the live world in
    /// sync without needing to track which fields changed. `SEED` is
    /// emitted with an empty value when no seed is set — itzg interprets
    /// that the same as unset (random world generation).
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
        // itzg's env name for the gamemode is `MODE`, not `GAMEMODE`
        // (docker-minecraft-server.readthedocs.io configuration/server-properties#mode).
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
            kv("SEED", self.seed.clone()),
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
        assert_eq!(d.spawn_protection, 16);
        assert!(d.allow_nether);
        assert!(!d.allow_flight);
        assert!(d.seed.is_empty());
    }

    #[test]
    fn empty_object_decodes_to_default() {
        let p: ServerProperties = serde_json::from_str("{}").unwrap();
        assert_eq!(p, ServerProperties::default());
    }

    #[test]
    fn round_trip_serde_preserves_state() {
        let p = ServerProperties {
            difficulty: Difficulty::Hard,
            hardcore: true,
            max_players: 50,
            ..ServerProperties::default()
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: ServerProperties = serde_json::from_str(&s).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn deny_unknown_fields_rejects_typo() {
        let r: Result<ServerProperties, _> = serde_json::from_str(r#"{"difficultyy":"hard"}"#);
        assert!(r.is_err());
    }

    #[test]
    fn to_env_emits_seventeen_vars() {
        let env = ServerProperties::default().to_env();
        assert_eq!(env.len(), 17);
    }

    #[test]
    fn to_env_emits_seed_empty_by_default() {
        let env = ServerProperties::default().to_env();
        let seed = env.iter().find(|e| e.name == "SEED").unwrap();
        assert_eq!(seed.value.as_deref(), Some(""));
    }

    #[test]
    fn to_env_emits_seed_when_set() {
        let p = ServerProperties {
            seed: "1234567890".to_owned(),
            ..ServerProperties::default()
        };
        let env = p.to_env();
        let seed = env.iter().find(|e| e.name == "SEED").unwrap();
        assert_eq!(seed.value.as_deref(), Some("1234567890"));
    }

    #[test]
    fn to_env_stringifies_booleans_lowercase() {
        let p = ServerProperties {
            pvp: false,
            ..ServerProperties::default()
        };
        let env = p.to_env();
        let pvp = env.iter().find(|e| e.name == "PVP").unwrap();
        assert_eq!(pvp.value.as_deref(), Some("false"));
        let wl = env.iter().find(|e| e.name == "WHITE_LIST").unwrap();
        assert_eq!(wl.value.as_deref(), Some("false"));
    }

    #[test]
    fn to_env_stringifies_enums_lowercase() {
        let p = ServerProperties {
            difficulty: Difficulty::Hard,
            gamemode: Gamemode::Creative,
            ..ServerProperties::default()
        };
        let env = p.to_env();
        let d = env.iter().find(|e| e.name == "DIFFICULTY").unwrap();
        assert_eq!(d.value.as_deref(), Some("hard"));
        let g = env.iter().find(|e| e.name == "MODE").unwrap();
        assert_eq!(g.value.as_deref(), Some("creative"));
    }

    #[test]
    fn to_env_stringifies_integers() {
        let p = ServerProperties {
            max_players: 50,
            view_distance: 16,
            ..ServerProperties::default()
        };
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
        let p = ServerProperties {
            max_players: 0,
            ..ServerProperties::default()
        };
        match p.validate().unwrap_err() {
            AppError::BadRequest { code, .. } => {
                assert_eq!(code, "properties_max_players_invalid");
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_max_players_over_200() {
        let p = ServerProperties {
            max_players: 201,
            ..ServerProperties::default()
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn validate_rejects_view_distance_below_three() {
        let p = ServerProperties {
            view_distance: 2,
            ..ServerProperties::default()
        };
        match p.validate().unwrap_err() {
            AppError::BadRequest { code, .. } => {
                assert_eq!(code, "properties_view_distance_invalid");
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_view_distance_above_32() {
        let p = ServerProperties {
            view_distance: 33,
            ..ServerProperties::default()
        };
        assert!(p.validate().is_err());
    }

    #[test]
    fn validate_rejects_simulation_distance_out_of_range() {
        let mut p = ServerProperties {
            simulation_distance: 2,
            ..ServerProperties::default()
        };
        assert!(p.validate().is_err());
        p.simulation_distance = 33;
        assert!(p.validate().is_err());
    }

    #[test]
    fn validate_rejects_spawn_protection_above_256() {
        let p = ServerProperties {
            spawn_protection: 257,
            ..ServerProperties::default()
        };
        match p.validate().unwrap_err() {
            AppError::BadRequest { code, .. } => {
                assert_eq!(code, "properties_spawn_protection_invalid");
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn validate_accepts_default() {
        ServerProperties::default()
            .validate()
            .expect("default valid");
    }

    #[test]
    fn validate_accepts_empty_and_typical_seeds() {
        for s in ["", "0", "-1", "1234567890", "my custom text seed"] {
            let p = ServerProperties {
                seed: s.to_owned(),
                ..ServerProperties::default()
            };
            assert!(p.validate().is_ok(), "expected {s:?} to pass");
        }
    }

    #[test]
    fn validate_rejects_seed_over_256_bytes() {
        let p = ServerProperties {
            seed: "a".repeat(257),
            ..ServerProperties::default()
        };
        match p.validate().unwrap_err() {
            AppError::BadRequest { code, .. } => {
                assert_eq!(code, "properties_seed_invalid");
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn validate_rejects_seed_with_control_chars() {
        for bad in ["with\nnewline", "with\rcarriage", "with\ttab", "with\0null"] {
            let p = ServerProperties {
                seed: bad.to_owned(),
                ..ServerProperties::default()
            };
            assert!(p.validate().is_err(), "expected {bad:?} to fail");
        }
    }
}
