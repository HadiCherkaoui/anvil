// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Integration tests for `PATCH /api/servers/:id/version`.
//!
//! These exercise the synchronous validation path of the route. The FSM is
//! spawned on the success path; the kube client is a tower-test mock so the
//! background task fails fast — but only AFTER the 202 response has been
//! sent, which is what we assert.

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
        mc_alpine_image: "alpine@sha256:test".to_owned(),
        mc_timezone: "Etc/UTC".to_owned(),
        mc_itzg_image: "itzg/minecraft-server:test".to_owned(),
        mc_busybox_image: "busybox:test".to_owned(),
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

async fn seed_modded_server(
    state: &AppState,
    id: &str,
    name: &str,
    runtime: &str,
    mc: &str,
    loader: Option<&str>,
) {
    let cfg = serde_json::json!({
        "runtime": runtime,
        "mc_version": mc,
        "loader_version": loader,
        "mods": [],
        "pending": [],
    });
    sqlx::query(
        "INSERT INTO servers (
            id, name, mc_version, memory_mi, source_kind, exposure_mode,
            storage_size_gi, source_config, created_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(name)
    .bind(mc)
    .bind(4096_i64)
    .bind("modded")
    .bind("clusterip")
    .bind(10_i64)
    .bind(cfg.to_string())
    .bind(0_i64)
    .execute(&state.pool)
    .await
    .expect("seed modded");
}

async fn seed_modrinth_server(state: &AppState, id: &str, name: &str) {
    let cfg = serde_json::json!({
        "project_id": "AANobbMI",
        "channel": "release",
        "version_skip": [],
        "current_version_id": "abc",
        "current_version_name": "ATM-9 4.4",
        "auto_update_mode": "notify",
    });
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
    .bind("modrinth")
    .bind("clusterip")
    .bind(10_i64)
    .bind(cfg.to_string())
    .bind(0_i64)
    .execute(&state.pool)
    .await
    .expect("seed modrinth");
}

fn patch_version_request(token: &str, id: &str, body: &serde_json::Value) -> Request<Body> {
    Request::builder()
        .method(Method::PATCH)
        .uri(format!("/api/servers/{id}/version"))
        .header(header::COOKIE, format!("{SESSION_COOKIE}={token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn patch_version_modpack_rejected() {
    let state = test_state();
    seed_modrinth_server(&state, "ts-mp", "smp").await;
    let token = token_for(&state, "u");
    let app = anvil::router(state);
    let resp = app
        .oneshot(patch_version_request(
            &token,
            "ts-mp",
            &serde_json::json!({"mc_version": "1.21.4"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["code"], "version_change_unsupported");
}

#[tokio::test]
async fn patch_version_neoforge_requires_loader() {
    let state = test_state();
    seed_modded_server(&state, "ts-nf", "nf", "neoforge", "1.21.4", Some("21.4.81")).await;
    let token = token_for(&state, "u");
    let app = anvil::router(state);
    let resp = app
        .oneshot(patch_version_request(
            &token,
            "ts-nf",
            &serde_json::json!({"mc_version": "1.21.3"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["code"], "loader_version_required");
}

#[tokio::test]
async fn patch_version_fabric_does_not_require_loader() {
    let state = test_state();
    seed_modded_server(&state, "ts-fb", "fb", "fabric", "1.21.4", None).await;
    let token = token_for(&state, "u");
    let app = anvil::router(state);
    let resp = app
        .oneshot(patch_version_request(
            &token,
            "ts-fb",
            &serde_json::json!({"mc_version": "1.21.3"}),
        ))
        .await
        .unwrap();
    // 202: validation passes, FSM spawned (will fail in background against
    // mock kube — does not affect the synchronous response).
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["status"], "started");
    assert_eq!(v["server_id"], "ts-fb");
}

#[tokio::test]
async fn patch_version_vanilla_starts_fsm() {
    let state = test_state();
    seed_vanilla_server(&state, "ts-v", "v").await;
    let token = token_for(&state, "u");
    let app = anvil::router(state);
    let resp = app
        .oneshot(patch_version_request(
            &token,
            "ts-v",
            &serde_json::json!({"mc_version": "1.21.3"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["status"], "started");
}

#[tokio::test]
async fn patch_version_unknown_mc_rejected() {
    let state = test_state();
    seed_vanilla_server(&state, "ts-u", "u").await;
    let token = token_for(&state, "u");
    let app = anvil::router(state);
    let resp = app
        .oneshot(patch_version_request(
            &token,
            "ts-u",
            &serde_json::json!({"mc_version": "9.9.9"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["code"], "mc_version_unknown");
}

#[tokio::test]
async fn patch_version_no_op_rejected() {
    let state = test_state();
    seed_vanilla_server(&state, "ts-n", "n").await;
    let token = token_for(&state, "u");
    let app = anvil::router(state);
    let resp = app
        .oneshot(patch_version_request(
            &token,
            "ts-n",
            &serde_json::json!({"mc_version": "1.21.4"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["code"], "nothing_to_change");
}

#[tokio::test]
async fn patch_version_unknown_server_404() {
    let state = test_state();
    let token = token_for(&state, "u");
    let app = anvil::router(state);
    let resp = app
        .oneshot(patch_version_request(
            &token,
            "missing",
            &serde_json::json!({"mc_version": "1.21.3"}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
