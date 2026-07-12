use std::collections::HashMap;
use std::path::PathBuf;

use recall_engine::cli::AssetMode;
use recall_engine::commands::import::run_chatgpt_import;
use recall_engine::domain::ic::seed_legacy_ic_map;
use recall_engine::storage::Database;
use rusqlite::Connection;

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chatgpt-sanitized")
}

#[test]
fn seed_legacy_ic_preserves_existing_numbers() {
    let tmp = tempfile::tempdir().unwrap();
    let legacy_path = tmp.path().join("legacy.sqlite");
    let legacy = Connection::open(&legacy_path).unwrap();
    legacy
        .execute_batch(
            "CREATE TABLE messages (id TEXT PRIMARY KEY, IC INTEGER);
             INSERT INTO messages VALUES ('msg-text-001', 42);",
        )
        .unwrap();

    let db = tmp.path().join("history.sqlite");
    run_chatgpt_import(
        fixture_root(),
        db.clone(),
        AssetMode::External,
        None,
        false,
        Some(legacy_path),
    )
    .unwrap();

    let database = Database::open(&db).unwrap();
    let ic: i64 = database
        .connection()
        .query_row(
            "SELECT ic FROM messages WHERE id = 'msg-text-001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ic, 42);
}

#[test]
fn seed_legacy_ic_stats() {
    let tmp = tempfile::tempdir().unwrap();
    let legacy_path = tmp.path().join("legacy.sqlite");
    let legacy = Connection::open(&legacy_path).unwrap();
    legacy
        .execute_batch(
            "CREATE TABLE messages (id TEXT PRIMARY KEY, IC INTEGER);
             INSERT INTO messages VALUES ('msg-text-001', 42);
             INSERT INTO messages VALUES ('msg-only-legacy', 99);",
        )
        .unwrap();

    let seed = seed_legacy_ic_map(&legacy_path).unwrap();
    assert_eq!(seed.map.get("msg-text-001"), Some(&42));
    assert_eq!(seed.map.len(), 2);

    let db = tmp.path().join("history.sqlite");
    run_chatgpt_import(
        fixture_root(),
        db.clone(),
        AssetMode::External,
        None,
        false,
        Some(legacy_path),
    )
    .unwrap();

    let database = Database::open(&db).unwrap();
    let stats: String = database
        .connection()
        .query_row(
            "SELECT stats_json FROM import_runs ORDER BY started_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(stats.contains("legacy_ic_matched"));
}

#[test]
fn enriched_export_assigns_new_ic_after_max() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    run_chatgpt_import(
        fixture_root(),
        db.clone(),
        AssetMode::External,
        None,
        false,
        None,
    )
    .unwrap();

    let database = Database::open(&db).unwrap();
    let max_before: i64 = database
        .connection()
        .query_row("SELECT MAX(ic) FROM messages", [], |r| r.get(0))
        .unwrap();

    run_chatgpt_import(
        fixture_root(),
        db.clone(),
        AssetMode::External,
        None,
        false,
        None,
    )
    .unwrap();

    let max_after: i64 = database
        .connection()
        .query_row("SELECT MAX(ic) FROM messages", [], |r| r.get(0))
        .unwrap();
    assert_eq!(max_before, max_after);
}

#[allow(dead_code)]
fn _map() -> HashMap<String, i64> {
    HashMap::new()
}
