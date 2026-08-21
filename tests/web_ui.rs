use std::path::PathBuf;

use axum::{
    body::{to_bytes, Body},
    http::{header, HeaderMap, Request, StatusCode},
};
use recall_engine::commands::web_ui;
use tower::ServiceExt;

async fn get(uri: &str) -> (StatusCode, HeaderMap, String) {
    let response = web_ui::app(PathBuf::from("/definitely/missing/recall-web-test.sqlite"), None)
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, headers, String::from_utf8(body.to_vec()).unwrap())
}

#[tokio::test]
async fn web_explorer_is_served_without_opening_the_database() {
    let (status, headers, body) = get("/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok()),
        Some("text/html; charset=utf-8")
    );
    assert!(body.contains("RecallEngine"));
    assert!(body.contains("Read-only memory explorer"));
    assert!(body.contains("data-read-only=\"true\""));
}

#[tokio::test]
async fn web_client_uses_only_the_versioned_read_contract() {
    let (status, headers, script) = get("/recall.js").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok()),
        Some("text/javascript; charset=utf-8")
    );
    assert!(script.contains("const API_BASE = \"/v1\""));
    assert!(script.contains("current-thread"));
    assert!(script.contains("all-active"));
    assert!(!script.contains("/api/"));
    assert!(!script.contains("method: \"POST\""));
    assert!(!script.contains("method: \"PUT\""));
    assert!(!script.contains("method: \"DELETE\""));
}

#[tokio::test]
async fn web_assets_ship_with_local_security_headers() {
    let (status, headers, _) = get("/recall.css").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get(header::CACHE_CONTROL).and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    assert_eq!(
        headers
            .get("x-content-type-options")
            .and_then(|value| value.to_str().ok()),
        Some("nosniff")
    );
    assert!(headers.get("content-security-policy").is_some());
}
