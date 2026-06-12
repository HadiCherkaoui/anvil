//! In-memory `SQLite` tests for the servers/audit_log schema.

use anvil::db;
use sqlx::Row;

#[tokio::test]
async fn migration_runs_on_fresh_db() {
    let pool = db::init("sqlite::memory:").await.expect("migrate");
    let row =
        sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'servers'")
            .fetch_one(&pool)
            .await
            .expect("query");
    let name: String = row.try_get(0).expect("col");
    assert_eq!(name, "servers");

    // audit_log + the two indexes also need to exist.
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index'
         AND name IN ('idx_audit_server_ts', 'idx_servers_nodeport')",
    )
    .fetch_one(&pool)
    .await
    .expect("index count");
    assert_eq!(count, 2);
}

#[tokio::test]
async fn insert_and_query_round_trips() {
    let pool = db::init("sqlite::memory:").await.unwrap();
    let id = "9b0e0c8a-1234-5678-9abc-def012345678";
    sqlx::query(
        "INSERT INTO servers (
            id, name, mc_version, memory_mi, source_kind, exposure_mode,
            storage_class, storage_size_gi, source_config, nodeport,
            created_at, last_started_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind("smp")
    .bind("1.21.4")
    .bind(4096_i64)
    .bind("vanilla")
    .bind("loadbalancer")
    .bind(Option::<String>::None)
    .bind(10_i64)
    .bind("{}")
    .bind(Option::<i64>::None)
    .bind(1_700_000_000_i64)
    .bind(Option::<i64>::None)
    .execute(&pool)
    .await
    .expect("insert");

    let row = sqlx::query("SELECT name, memory_mi, nodeport FROM servers WHERE id = ?")
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("select");
    let name: String = row.try_get("name").unwrap();
    let memory: i64 = row.try_get("memory_mi").unwrap();
    let nodeport: Option<i64> = row.try_get("nodeport").unwrap();
    assert_eq!(name, "smp");
    assert_eq!(memory, 4096);
    assert_eq!(nodeport, None);
}

#[tokio::test]
async fn nodeport_stored_when_assigned() {
    let pool = db::init("sqlite::memory:").await.unwrap();
    sqlx::query(
        "INSERT INTO servers (
            id, name, mc_version, memory_mi, source_kind, exposure_mode,
            storage_size_gi, source_config, nodeport, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("id-np")
    .bind("nps")
    .bind("1.21.4")
    .bind(2048_i64)
    .bind("vanilla")
    .bind("nodeport")
    .bind(10_i64)
    .bind("{}")
    .bind(30_005_i64)
    .bind(0_i64)
    .execute(&pool)
    .await
    .unwrap();

    let nodeport: Option<i64> = sqlx::query_scalar("SELECT nodeport FROM servers WHERE id = ?")
        .bind("id-np")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(nodeport, Some(30_005));
}

#[tokio::test]
async fn name_unique_constraint() {
    let pool = db::init("sqlite::memory:").await.unwrap();
    let stmt = "INSERT INTO servers (
            id, name, mc_version, memory_mi, source_kind, exposure_mode,
            storage_size_gi, source_config, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)";
    let bind = |id: &'static str| {
        sqlx::query(stmt)
            .bind(id)
            .bind("smp")
            .bind("1.21.4")
            .bind(4096_i64)
            .bind("vanilla")
            .bind("loadbalancer")
            .bind(10_i64)
            .bind("{}")
            .bind(0_i64)
    };
    bind("id-1").execute(&pool).await.expect("first ok");
    let err = bind("id-2")
        .execute(&pool)
        .await
        .expect_err("must conflict");
    let s = format!("{err}").to_lowercase();
    assert!(
        s.contains("unique"),
        "expected UNIQUE-constraint error, got: {s}"
    );
}

#[tokio::test]
async fn audit_log_insert_and_order() {
    let pool = db::init("sqlite::memory:").await.unwrap();
    for ts in [10_i64, 20, 15] {
        sqlx::query(
            "INSERT INTO audit_log (ts, server_id, action, details, actor)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(ts)
        .bind("srv-1")
        .bind("started")
        .bind(Option::<String>::None)
        .bind(Option::<String>::None)
        .execute(&pool)
        .await
        .unwrap();
    }
    let rows = sqlx::query("SELECT ts FROM audit_log WHERE server_id = ? ORDER BY ts DESC")
        .bind("srv-1")
        .fetch_all(&pool)
        .await
        .unwrap();
    let timestamps: Vec<i64> = rows.iter().map(|r| r.try_get(0).unwrap()).collect();
    assert_eq!(timestamps, vec![20, 15, 10]);
}

#[tokio::test]
async fn last_started_at_nullable_then_set() {
    let pool = db::init("sqlite::memory:").await.unwrap();
    sqlx::query(
        "INSERT INTO servers (
            id, name, mc_version, memory_mi, source_kind, exposure_mode,
            storage_size_gi, source_config, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("id-ls")
    .bind("ls")
    .bind("1.21.4")
    .bind(2048_i64)
    .bind("vanilla")
    .bind("clusterip")
    .bind(10_i64)
    .bind("{}")
    .bind(0_i64)
    .execute(&pool)
    .await
    .unwrap();

    let initial: Option<i64> =
        sqlx::query_scalar("SELECT last_started_at FROM servers WHERE id = ?")
            .bind("id-ls")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(initial, None);

    sqlx::query("UPDATE servers SET last_started_at = ? WHERE id = ?")
        .bind(1_700_000_500_i64)
        .bind("id-ls")
        .execute(&pool)
        .await
        .unwrap();

    let updated: Option<i64> =
        sqlx::query_scalar("SELECT last_started_at FROM servers WHERE id = ?")
            .bind("id-ls")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(updated, Some(1_700_000_500));
}
