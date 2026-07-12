use std::path::{Path, PathBuf};

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use recall_engine::{
    cli::AssetMode,
    commands::{import::run_chatgpt_import, serve},
    read_model::ReadRepository,
};
use serde_json::Value;
use tower::ServiceExt;

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chatgpt-sanitized")
}

fn import_fixture(db: &Path) {
    run_chatgpt_import(
        fixture_root(),
        db.to_path_buf(),
        AssetMode::External,
        None,
        false,
        None,
    )
    .expect("import fixture");
}

async fn get(app: axum::Router, uri: &str) -> (StatusCode, String) {
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

#[tokio::test]
async fn legacy_search_keeps_substring_fallback_and_caps_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    import_fixture(&db);

    let (status, body) = get(serve::app(db, None), "/api/search?q=ell&limit=9999").await;
    assert_eq!(status, StatusCode::OK);
    let rows: Vec<Value> = serde_json::from_str(&body).unwrap();
    assert!(!rows.is_empty());
    assert!(rows.len() <= 500);
    assert!(rows[0].get("messageId").is_some());
}

#[tokio::test]
async fn message_routes_resolve_same_pair_without_breaking_legacy_id() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    import_fixture(&db);
    let repository = ReadRepository::open_read_only(&db).unwrap();
    let hit = repository.search("Hello", 1).unwrap().remove(0);
    let ic = hit.ic.unwrap();

    let (status, body) = get(
        serve::app(db.clone(), None),
        &format!("/api/messages/by-ic/{ic}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let by_ic: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(by_ic["id"], hit.message_id);
    assert_eq!(by_ic["messageId"], by_ic["id"]);
    assert_eq!(
        by_ic["reference"],
        format!("[IC:{ic} | msg:{}]", hit.message_id)
    );

    let (status, body) = get(
        serve::app(db.clone(), None),
        &format!("/api/messages/by-message-id/{}", hit.message_id),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let by_id: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(by_id, by_ic);

    let token = format!("ref:ic/{ic}/uuid/{}", hit.message_id);
    let (status, body) = get(
        serve::app(db, None),
        &format!("/api/messages/by-reference?ref={token}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let by_reference: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(by_reference, by_ic);
}

#[tokio::test]
async fn invalid_missing_and_mismatched_references_are_explicit() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    import_fixture(&db);
    let repository = ReadRepository::open_read_only(&db).unwrap();
    let hit = repository.search("Hello", 1).unwrap().remove(0);
    let ic = hit.ic.unwrap();

    let (status, _) = get(
        serve::app(db.clone(), None),
        "/api/messages/by-reference?ref=invalid",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let mismatched = format!("ref:ic/{}/uuid/{}", ic + 1, hit.message_id);
    let (status, _) = get(
        serve::app(db.clone(), None),
        &format!("/api/messages/by-reference?ref={mismatched}"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = get(
        serve::app(db, None),
        "/api/messages/by-message-id/does-not-exist",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
