use std::path::{Path, PathBuf};

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use recall_engine::{
    cli::AssetMode,
    commands::{import::run_chatgpt_import, serve},
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

async fn get(app: axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = serde_json::from_slice(&body)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into_owned()));
    (status, value)
}

fn path_encode(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'.' | b'_' | b'~') {
                char::from(*byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

#[tokio::test]
async fn meta_exposes_an_explicit_read_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    import_fixture(&db);

    let (status, body) = get(serve::app(db.clone(), None), "/v1/meta").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["api_schema_version"], "1");
    assert_eq!(body["database_schema_version"], 2);
    assert_eq!(body["read_only"], true);
    assert!(body["snapshot_id"]
        .as_str()
        .is_some_and(|value| value.starts_with("import-run:")));
    assert_eq!(body["database_path"], db.display().to_string());
}

#[tokio::test]
async fn conversations_and_messages_use_cursor_pages_and_explicit_views() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    import_fixture(&db);

    let (status, first_body) = get(serve::app(db.clone(), None), "/v1/conversations?limit=1").await;
    assert_eq!(status, StatusCode::OK);
    let first = first_body["items"].as_array().unwrap();
    assert_eq!(first.len(), 1);
    let conversation_id = first[0]["conversation_id"].as_str().unwrap().to_owned();
    assert!(first[0].get("default_model_slug").is_some());
    let next_cursor = first_body["next_cursor"].as_str().unwrap();

    let (status, second_body) = get(
        serve::app(db.clone(), None),
        &format!(
            "/v1/conversations?limit=1&cursor={}",
            path_encode(next_cursor)
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(
        second_body["items"][0]["conversation_id"],
        Value::String(conversation_id.clone())
    );

    let (status, missing_view) = get(
        serve::app(db.clone(), None),
        &format!("/v1/conversations/{conversation_id}/messages?limit=2"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(missing_view.as_str().unwrap_or_default().contains("view"));

    let (status, messages_body) = get(
        serve::app(db.clone(), None),
        &format!("/v1/conversations/{conversation_id}/messages?view=current-thread&limit=2"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let messages = messages_body["items"].as_array().unwrap();
    assert!(!messages.is_empty());
    let message_id = messages[0]["message_id"].as_str().unwrap().to_owned();
    assert!(messages[0].get("model_slug").is_some());
    assert!(messages[0].get("technical").is_some());
    assert!(messages[0].get("visible").is_some());
    assert!(messages[0]["branch"].get("node_id").is_some());

    let (status, all_active_body) = get(
        serve::app(db.clone(), None),
        &format!("/v1/conversations/{conversation_id}/messages?view=all-active&limit=2"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!all_active_body["items"].as_array().unwrap().is_empty());

    let (status, message_body) = get(
        serve::app(db.clone(), None),
        &format!("/v1/messages/{message_id}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(message_body["message_id"], message_id);
}

#[tokio::test]
async fn search_context_and_reference_resolution_share_the_same_read_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    import_fixture(&db);

    let (status, search_body) =
        get(serve::app(db.clone(), None), "/v1/search?q=Hello&limit=1").await;
    assert_eq!(status, StatusCode::OK);
    let hit = &search_body["items"][0];
    let message_id = hit["message_id"].as_str().unwrap();
    assert!(hit.get("model_slug").is_some());
    assert!(hit["branch"].get("parent_id").is_some());

    let (status, context_body) = get(
        serve::app(db.clone(), None),
        &format!("/v1/messages/{message_id}/context?view=all-active&before=1&after=1"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(context_body["view"], "all-active");
    assert_eq!(context_body["center"]["message_id"], message_id);

    let stable_reference = hit["stable_reference"].as_str().unwrap();
    let (status, reference_body) = get(
        serve::app(db, None),
        &format!("/v1/references/{}", path_encode(stable_reference)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reference_body["message"]["message_id"], message_id);
    assert_eq!(reference_body["stable_reference"], stable_reference);
}

#[tokio::test]
async fn search_supports_modes_and_graceful_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    import_fixture(&db);

    // 1. Explicit mode=lexical works as default
    let (status, lexical_body) = get(
        serve::app(db.clone(), None),
        "/v1/search?q=Hello&mode=lexical&limit=5",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!lexical_body["items"].as_array().unwrap().is_empty());

    // 2. mode=hybrid on unindexed db gracefully falls back to FTS5
    let (status, hybrid_body) = get(
        serve::app(db.clone(), None),
        "/v1/search?q=Hello&mode=hybrid&limit=5",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!hybrid_body["items"].as_array().unwrap().is_empty());

    // 3. mode=semantic on unindexed db returns 400 Bad Request
    let (status, sem_err) = get(
        serve::app(db.clone(), None),
        "/v1/search?q=Hello&mode=semantic&limit=5",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(sem_err
        .as_str()
        .unwrap_or_default()
        .contains("Semantic search index not found"));

    // 4. Invalid mode returns 400 Bad Request
    let (status, invalid_err) =
        get(serve::app(db, None), "/v1/search?q=Hello&mode=unknown_mode").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(invalid_err
        .as_str()
        .unwrap_or_default()
        .contains("unsupported mode"));
}
