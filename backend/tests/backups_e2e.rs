//! Integration tests for `/api/servers/:id/backups`.
//!
//! The kube client is a tower-test mock without a configured responder,
//! so any path that polls k8s blocks forever. We exercise the synchronous
//! validation paths (server / backup existence, name validation, the
//! `UpdateGuard` contention check, list ordering). The Job-running paths —
//! tar, restore, the synchronous delete Job — would hang against the
//! mock and need a real cluster to verify; those paths are covered by
//! the manual e2e in the implementation plan §verification.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use axum_extra::extract::cookie::Key;
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

use anvil::AppState;
use anvil::auth::OidcState;
use anvil::auth::session::{SESSION_COOKIE, mint};
use anvil::auth::types::SessionClaims;

fn test_state() -> AppState {
    use kube::client::Body as KubeBody;
    let (mock_service, _handle) =
        tower_test::mock::pair::<http::Request<KubeBody>, http::Response<KubeBody>>();
    let kube = kube::Client::new(mock_service, "default");
    let pool = futures::executor::block_on(anvil::db::init("sqlite::memory:"))
        .expect("init in-memory sqlite");
    let session_key = vec![0x42_u8; 32];
    let cookie_key = Key::derive_from(&session_key);
    let oidc = OidcState::new(
        "https://authentik.invalid/application/o/anvil/".into(),
        "anvil".into(),
        "secret".into(),
        "https://anvil.invalid/api/auth/callback".into(),
    )
    .expect("OIDC state constructible");
    AppState {
        kube,
        pool,
        mc_namespace: "mc".into(),
        mc_storage_class: "tank".into(),
        mc_svc_type: "LoadBalancer".into(),
        node_host: "host".into(),
        loadbalancer_supported: false,
        capabilities_cache: anvil::routes::cluster::new_cache(),
        mc_versions_cache: anvil::routes::mc_versions::new_cache(),
        loader_version_cache: anvil::routes::runtimes::new_cache(),
        papermc_cache: anvil::routes::papermc::new_cache(),
        session_key,
        cookie_key,
        allowed_subs: vec![],
        oidc,
        cf_client: None,
        mr_client: std::sync::Arc::new(
            anvil::modpack::ModrinthClient::new().expect("test Modrinth client"),
        ),
        snapshots_pvc: std::sync::Arc::new("mc-snapshots".to_owned()),
        modpack_poll_interval: std::time::Duration::from_hours(1),
        update_locks: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        update_phase_buses: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        update_errors: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        update_terminals: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        snapshot_pvc_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        files_helper_image: "alpine@sha256:test".to_owned(),
    }
}

fn token_for(state: &AppState, sub: &str) -> String {
    let now = jiff::Timestamp::now().as_second();
    mint(
        &state.session_key,
        &SessionClaims {
            sub: sub.into(),
            name: "Hadi".into(),
            email: "hadi@example.test".into(),
            picture: None,
            iat: now,
            exp: now + 3600,
        },
    )
    .expect("mint")
}

async fn seed_vanilla_server(state: &AppState, id: &str, name: &str) {
    sqlx::query(
        "INSERT INTO servers (
            id, name, mc_version, memory_mi, source_kind, exposure_mode,
            storage_size_gi, source_config, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(name)
    .bind("1.21.4")
    .bind(4096_i64)
    .bind("vanilla")
    .bind("clusterip")
    .bind(10_i64)
    .bind("{}")
    .bind(0_i64)
    .execute(&state.pool)
    .await
    .expect("seed vanilla");
}

async fn seed_backup_row(state: &AppState, server_id: &str, backup_id: &str, name: Option<&str>) {
    sqlx::query(
        "INSERT INTO backups
            (id, server_id, name, created_at, snapshot_path, mc_version, memory_mi,
             storage_size_gi, storage_class, exposure_mode, source_kind, source_config)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(backup_id)
    .bind(server_id)
    .bind(name)
    .bind(1_700_000_000_i64)
    .bind(format!("manual/{backup_id}.tgz"))
    .bind("1.21.4")
    .bind(4096_i64)
    .bind(10_i64)
    .bind(Option::<String>::None)
    .bind("clusterip")
    .bind("vanilla")
    .bind("{}")
    .execute(&state.pool)
    .await
    .expect("seed backup");
}

fn json_request(method: Method, uri: &str, token: &str, body: &serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, format!("{SESSION_COOKIE}={token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn empty_request(method: Method, uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, format!("{SESSION_COOKIE}={token}"))
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn create_backup_unknown_server_404() {
    let state = test_state();
    let token = token_for(&state, "u");
    let app = anvil::router(state);
    let resp = app
        .oneshot(json_request(
            Method::POST,
            "/api/servers/missing/backups",
            &token,
            &serde_json::json!({"name": "smoke"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn create_backup_invalid_name_400() {
    let state = test_state();
    seed_vanilla_server(&state, "ts-bk", "smoke").await;
    let token = token_for(&state, "u");
    let app = anvil::router(state);
    let body = serde_json::json!({ "name": "x".repeat(65) });
    let resp = app
        .oneshot(json_request(
            Method::POST,
            "/api/servers/ts-bk/backups",
            &token,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["code"], "invalid_name");
}

#[tokio::test]
async fn create_backup_succeeds_202() {
    let state = test_state();
    seed_vanilla_server(&state, "ts-bk", "smoke").await;
    let token = token_for(&state, "u");
    let app = anvil::router(state.clone());
    let resp = app
        .oneshot(json_request(
            Method::POST,
            "/api/servers/ts-bk/backups",
            &token,
            &serde_json::json!({"name": "pre-test"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["status"], "started");
    assert!(v["backup_id"].as_str().unwrap().starts_with("bk-"));
    // The spawned FSM will fail in the background against the mock kube
    // and clean up its own row; we don't assert on the row here because
    // it's racy. The acceptance signal is the 202 + backup_id shape.
    let _ = state;
}

#[tokio::test]
async fn create_backup_conflicts_with_running_update() {
    let state = test_state();
    seed_vanilla_server(&state, "ts-bk", "smoke").await;
    state
        .update_locks
        .lock()
        .unwrap()
        .insert("ts-bk".to_owned());
    let token = token_for(&state, "u");
    let app = anvil::router(state);
    let resp = app
        .oneshot(json_request(
            Method::POST,
            "/api/servers/ts-bk/backups",
            &token,
            &serde_json::json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["code"], "update_in_progress");
}

#[tokio::test]
async fn list_backups_returns_rows_desc_by_created_at() {
    let state = test_state();
    seed_vanilla_server(&state, "ts-bk", "smoke").await;
    // Two rows, second-inserted is older.
    sqlx::query(
        "INSERT INTO backups
            (id, server_id, name, created_at, snapshot_path, mc_version, memory_mi,
             storage_size_gi, storage_class, exposure_mode, source_kind, source_config)
         VALUES
            ('bk-newer', 'ts-bk', 'newer', 200, 'manual/bk-newer.tgz', '1.21.4', 4096, 10, NULL, 'clusterip', 'vanilla', '{}'),
            ('bk-older', 'ts-bk', 'older', 100, 'manual/bk-older.tgz', '1.21.4', 4096, 10, NULL, 'clusterip', 'vanilla', '{}')",
    )
    .execute(&state.pool)
    .await
    .unwrap();
    let token = token_for(&state, "u");
    let app = anvil::router(state);
    let resp = app
        .oneshot(empty_request(
            Method::GET,
            "/api/servers/ts-bk/backups",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["id"], "bk-newer");
    assert_eq!(arr[1]["id"], "bk-older");
}

#[tokio::test]
async fn restore_unknown_backup_404() {
    let state = test_state();
    seed_vanilla_server(&state, "ts-bk", "smoke").await;
    let token = token_for(&state, "u");
    let app = anvil::router(state);
    let resp = app
        .oneshot(empty_request(
            Method::POST,
            "/api/servers/ts-bk/backups/bk-missing/restore",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn restore_known_backup_starts_fsm_202() {
    let state = test_state();
    seed_vanilla_server(&state, "ts-bk", "smoke").await;
    seed_backup_row(&state, "ts-bk", "bk-x", Some("snap")).await;
    let token = token_for(&state, "u");
    let app = anvil::router(state);
    let resp = app
        .oneshot(empty_request(
            Method::POST,
            "/api/servers/ts-bk/backups/bk-x/restore",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["status"], "started");
}

#[tokio::test]
async fn delete_unknown_backup_404() {
    let state = test_state();
    seed_vanilla_server(&state, "ts-bk", "smoke").await;
    let token = token_for(&state, "u");
    let app = anvil::router(state);
    let resp = app
        .oneshot(empty_request(
            Method::DELETE,
            "/api/servers/ts-bk/backups/bk-missing",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn server_delete_cascade_drops_backup_rows() {
    let state = test_state();
    seed_vanilla_server(&state, "ts-bk", "smoke").await;
    seed_backup_row(&state, "ts-bk", "bk-1", Some("a")).await;
    seed_backup_row(&state, "ts-bk", "bk-2", Some("b")).await;
    // Manually run the cascade by deleting the servers row — we don't go
    // through the route handler because that hits the kube mock.
    sqlx::query("DELETE FROM servers WHERE id = ?")
        .bind("ts-bk")
        .execute(&state.pool)
        .await
        .unwrap();
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM backups WHERE server_id = ?")
        .bind("ts-bk")
        .fetch_one(&state.pool)
        .await
        .unwrap();
    assert_eq!(count.0, 0, "FK CASCADE should remove backup rows");
}
