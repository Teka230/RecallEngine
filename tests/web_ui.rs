use std::{fs, path::PathBuf};

use axum::{
    body::{to_bytes, Body},
    http::{header, HeaderMap, Request, StatusCode},
};
use recall_engine::commands::web_ui;
use tower::ServiceExt;

fn built_dist() -> tempfile::TempDir {
    let dist = tempfile::tempdir().unwrap();
    fs::create_dir_all(dist.path().join("assets")).unwrap();
    fs::write(
        dist.path().join("index.html"),
        "<!doctype html><html><body><div id=\"root\">RecallEngine React UI</div><script type=\"module\" src=\"/assets/app.js\"></script></body></html>",
    )
    .unwrap();
    fs::write(dist.path().join("assets/app.js"), "console.log('react build');").unwrap();
    fs::write(dist.path().join("assets/app.css"), "body { margin: 0; }").unwrap();
    dist
}

async fn get(uri: &str) -> (StatusCode, HeaderMap, String) {
    let dist = built_dist();
    let response = web_ui::app_with_dist(
        PathBuf::from("/definitely/missing/recall-web-test.sqlite"),
        None,
        dist.path().to_path_buf(),
    )
    .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
    .await
    .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, headers, String::from_utf8(body.to_vec()).unwrap())
}

#[tokio::test]
async fn built_web_ui_is_served_without_opening_the_database() {
    let (status, headers, body) = get("/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers.get(header::CONTENT_TYPE).and_then(|value| value.to_str().ok()),
        Some("text/html; charset=utf-8")
    );
    assert!(body.contains("RecallEngine React UI"));
}

#[tokio::test]
async fn web_client_source_uses_only_the_versioned_read_contract() {
    let api = include_str!("../web/src/api.ts");
    let app = include_str!("../web/src/App.tsx");
    assert!(api.contains("export const API_BASE = \"/v1\""));
    assert!(api.contains("/messages/by-ic/"));
    assert!(app.contains("current-thread"));
    assert!(app.contains("all-active"));
    assert!(!api.contains("/api/"));
    assert!(!app.contains("/api/"));
    for method in ["POST", "PUT", "PATCH", "DELETE"] {
        assert!(!api.contains(&format!("method: \"{method}\"")));
    }
}

#[tokio::test]
async fn web_assets_ship_with_local_security_headers() {
    let (status, headers, _) = get("/assets/app.css").await;
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

#[tokio::test]
async fn missing_web_build_is_explicit() {
    let missing = tempfile::tempdir().unwrap().path().join("missing-dist");
    let response = web_ui::app_with_dist(
        PathBuf::from("/definitely/missing/recall-web-test.sqlite"),
        None,
        missing,
    )
    .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}
