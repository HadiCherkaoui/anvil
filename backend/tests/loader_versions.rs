//! Integration tests for `GET /api/runtimes/{runtime}/versions`.
//!
//! Upstream maven is mocked by priming the in-memory cache directly — the
//! handler reads the cache before going to the network, so as long as the
//! cache is populated the request never touches the upstream.

use std::collections::BTreeMap;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum_extra::extract::cookie::Key;
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

use anvil::AppState;
use anvil::auth::OidcState;
use anvil::auth::session::{SESSION_COOKIE, mint};
use anvil::auth::types::SessionClaims;
use anvil::routes::runtimes::{LoaderVersions, prime_cache};

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

fn neoforge_fixture() -> LoaderVersions {
    let mut by_mc: BTreeMap<String, Vec<String>> = BTreeMap::new();
    by_mc.insert(
        "1.21.4".to_owned(),
        vec!["21.4.81".to_owned(), "21.4.80".to_owned()],
    );
    by_mc.insert("1.21.1".to_owned(), vec!["21.1.182".to_owned()]);
    LoaderVersions {
        mc_versions: vec!["1.21.4".to_owned(), "1.21.1".to_owned()],
        by_mc,
    }
}

#[tokio::test]
async fn loader_versions_neoforge_returns_grouping() {
    let state = test_state();
    prime_cache(&state.loader_version_cache, "neoforge", neoforge_fixture());

    let token = token_for(&state, "u-1");
    let app = anvil::router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/runtimes/neoforge/versions")
                .header(header::COOKIE, format!("{SESSION_COOKIE}={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(!v["mc_versions"].as_array().unwrap().is_empty());
    assert!(v["by_mc"]["1.21.4"].as_array().is_some());
}

#[tokio::test]
async fn loader_versions_unknown_runtime_404() {
    let state = test_state();
    let token = token_for(&state, "u-1");
    let app = anvil::router(state);
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/runtimes/fabric/versions")
                .header(header::COOKIE, format!("{SESSION_COOKIE}={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
