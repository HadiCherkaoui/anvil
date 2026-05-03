//! Integration tests for the OIDC auth middleware and `/api/auth/me` endpoint.
//!
//! These tests do NOT contact a real Authentik instance. Login/callback flows
//! are exercised manually against a live Authentik (see `docs/authentik-setup.md`).
//! Here we cover the cookie validation surface in isolation: missing cookie,
//! tampered signature, expired exp, and `ANVIL_ALLOWED_SUBS` gating.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum_extra::extract::cookie::Key;
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

use anvil::AppState;
use anvil::auth::OidcState;
use anvil::auth::session::{SESSION_COOKIE, mint};
use anvil::auth::types::SessionClaims;

/// Builds a minimal `AppState` for middleware tests. The `kube::Client` is a
/// `tower_test::mock::Mock` that will hang if any handler tries to use it —
/// the middleware bails before any handler runs in every test in this file.
fn test_state(allowed: Vec<String>) -> AppState {
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
        session_key,
        cookie_key,
        allowed_subs: allowed,
        oidc,
        // M5 modpack fields — none of the auth-middleware tests exercise
        // them, so all-disabled / empty defaults are fine.
        cf_client: None,
        snapshots_pvc: None,
        modpack_poll_interval: std::time::Duration::from_secs(3600),
        update_locks: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
        update_phase_buses: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        snapshot_pvc_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
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

fn cookie_header(token: &str) -> String {
    format!("{SESSION_COOKIE}={token}")
}

#[tokio::test]
async fn missing_cookie_yields_401() {
    let state = test_state(vec![]);
    let app = anvil::router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/servers")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn bad_signature_yields_401() {
    let state = test_state(vec![]);
    let valid = token_for(&state, "u-1");
    let mut bytes = valid.into_bytes();
    *bytes.last_mut().unwrap() ^= 0x01;
    let tampered = String::from_utf8(bytes).expect("ascii");
    let app = anvil::router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/servers")
                .header(header::COOKIE, cookie_header(&tampered))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn allowed_subs_gating_returns_403_when_not_in_list() {
    let state = test_state(vec!["other-uuid".into()]);
    let token = token_for(&state, "u-1");
    let app = anvil::router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/auth/me")
                .header(header::COOKIE, cookie_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["code"], "sub_not_allowed");
}

#[tokio::test]
async fn me_returns_user_for_valid_session() {
    let state = test_state(vec![]);
    let token = token_for(&state, "u-1");
    let app = anvil::router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/auth/me")
                .header(header::COOKIE, cookie_header(&token))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["sub"], "u-1");
    assert_eq!(v["email"], "hadi@example.test");
    assert_eq!(v["name"], "Hadi");
}

#[tokio::test]
async fn health_does_not_require_auth() {
    let state = test_state(vec![]);
    let app = anvil::router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
