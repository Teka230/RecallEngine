use recall_engine::cli::AssetMode;
use recall_engine::commands::import::run_chatgpt_import;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chatgpt-sanitized")
}

fn import_search_db(dir: &Path) -> PathBuf {
    let db = dir.join("history.sqlite");
    run_chatgpt_import(
        fixture_root(),
        db.clone(),
        AssetMode::External,
        None,
        false,
        None,
    )
    .expect("import sanitized fixture");
    db
}

fn run_search(db: &Path, args: &[&str]) -> Value {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_recall"));
    cmd.arg("search")
        .arg("--db")
        .arg(db)
        .args(args)
        .arg("--json");
    let output = cmd.output().expect("Failed to execute search");
    assert!(
        output.status.success(),
        "Search failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("Valid JSON")
}

#[test]
fn test_search_simple() {
    let tmp = tempfile::tempdir().unwrap();
    let db = import_search_db(tmp.path());
    let json = run_search(&db, &["hello"]);

    assert_eq!(json["schema_version"], "1");
    assert_eq!(json["query"], "hello");
    assert_eq!(json["mode"], "fts5-simple");

    let results = json["results"].as_array().expect("results is array");
    assert!(!results.is_empty(), "Expected some results for hello");

    let first_result = &results[0];
    assert!(first_result["message_id"].is_string());
    assert!(first_result["snippet"]["text"].is_string());
}

#[test]
fn test_search_role_filter() {
    let tmp = tempfile::tempdir().unwrap();
    let db = import_search_db(tmp.path());
    let json = run_search(&db, &["hello", "--role", "user"]);

    let results = json["results"].as_array().unwrap();
    assert!(!results.is_empty());
    for res in results {
        assert_eq!(res["role"], "user");
    }
}

#[test]
fn test_search_count_exact() {
    let tmp = tempfile::tempdir().unwrap();
    let db = import_search_db(tmp.path());
    let json = run_search(&db, &["Path", "--count-mode", "exact"]);

    assert_eq!(json["total_is_exact"], true);
    assert!(json["total"].is_number());
    assert!(json["total"].as_u64().unwrap() >= 1);
}
