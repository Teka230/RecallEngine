use std::path::{Path, PathBuf};

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use recall_engine::{
    cli::AssetMode,
    commands::{import::run_chatgpt_import, web_ui},
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

#[tokio::test]
async fn v1_ic_lookup_resolves_to_a_stable_message_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    import_fixture(&db);
    let repository = ReadRepository::open_read_only(&db).unwrap();
    let hit = repository.search("Hello", 1).unwrap().remove(0);
    let ic = hit.ic.expect("fixture search hit should be referenceable");

    let response = web_ui::app_with_dist(db, None, tmp.path().join("missing-web"))
        .oneshot(
            Request::builder()
                .uri(format!("/v1/messages/by-ic/{ic}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["ic"], ic);
    assert_eq!(value["message_id"], hit.message_id);
    assert_eq!(value["conversation_id"], hit.conversation_id);
    assert_eq!(
        value["stable_reference"],
        format!("ref:ic/{ic}/uuid/{}", hit.message_id)
    );
}
