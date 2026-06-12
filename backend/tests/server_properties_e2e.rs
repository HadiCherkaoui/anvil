//! E2E sanity for the new properties column: round-trips through the DB
//! and the public env-emission helper. No k8s API in scope — these are
//! pool-only tests using the same in-memory `SQLite` the other test files
//! use.

use anvil::server_properties::{Difficulty, Gamemode, ServerProperties};

#[tokio::test]
async fn properties_default_emits_full_env_block() {
    let p = ServerProperties::default();
    let env = p.to_env();
    let names: Vec<&str> = env.iter().map(|e| e.name.as_str()).collect();
    for k in [
        "DIFFICULTY",
        "HARDCORE",
        "MODE",
        "FORCE_GAMEMODE",
        "MAX_PLAYERS",
        "VIEW_DISTANCE",
        "SIMULATION_DISTANCE",
        "PVP",
        "WHITE_LIST",
        "SPAWN_PROTECTION",
        "SPAWN_ANIMALS",
        "SPAWN_MONSTERS",
        "SPAWN_NPCS",
        "ALLOW_FLIGHT",
        "ALLOW_NETHER",
        "ENABLE_COMMAND_BLOCK",
        "SEED",
    ] {
        assert!(names.contains(&k), "missing env var {k}");
    }
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
    assert_eq!(p.gamemode, Gamemode::Survival);
    assert_eq!(p.max_players, 42);
    assert!(p.pvp);
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
    .bind("{}")
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
