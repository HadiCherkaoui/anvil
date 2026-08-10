// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Integration test for the health endpoint.
//!
//! Confirms the router responds to `GET /api/health` with 200 and a JSON body
//! whose `ok` field is `true`. Doesn't touch the cluster or the database.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

#[tokio::test]
async fn get_api_health_returns_ok_true() {
    let app = anvil::stateless_router();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/health")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = response
        .into_body()
        .collect()
        .await
        .expect("body should collect")
        .to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&body_bytes).expect("body should be JSON");

    assert_eq!(body.get("ok"), Some(&serde_json::Value::Bool(true)));
    assert!(
        body.get("version").is_some(),
        "response should expose `version` for client introspection"
    );
}
