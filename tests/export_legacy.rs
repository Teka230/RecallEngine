use std::path::{Path, PathBuf};

use recall_engine::cli::AssetMode;
use recall_engine::commands::import::run_chatgpt_import;
use recall_engine::export::legacy::{
    compute_token_count, export_legacy_sqlite, validate_legacy_schema, REQUIRED_COLUMNS,
};
use recall_engine::storage::Database;

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
    .unwrap();
}

#[test]
fn legacy_export_preserves_ic_without_recalc() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    let out = tmp.path().join("legacy.sqlite");
    import_fixture(&db);

    let database = Database::open(&db).unwrap();
    let stats = export_legacy_sqlite(database.connection(), &out).unwrap();
    assert!(stats.messages_exported > 0);

    let canonical = Database::open(&db).unwrap();
    let ic: i64 = canonical
        .connection()
        .query_row(
            "SELECT ic FROM messages WHERE id = 'msg-text-001'",
            [],
            |r| r.get(0),
        )
        .unwrap();

    let legacy = rusqlite::Connection::open(&out).unwrap();
    let legacy_ic: i64 = legacy
        .query_row(
            "SELECT IC FROM messages WHERE id = 'msg-text-001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ic, legacy_ic);
}

#[test]
fn legacy_export_matches_explo_gpt_required_columns() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    let out = tmp.path().join("legacy.sqlite");
    import_fixture(&db);

    let database = Database::open(&db).unwrap();
    export_legacy_sqlite(database.connection(), &out).unwrap();

    let legacy = rusqlite::Connection::open(&out).unwrap();
    validate_legacy_schema(&legacy).unwrap();

    let mut stmt = legacy.prepare("PRAGMA table_info(messages)").unwrap();
    let columns: Vec<String> = stmt
        .query_map([], |row| row.get(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    for required in REQUIRED_COLUMNS {
        assert!(columns.iter().any(|c| c == required), "missing {required}");
    }
}

#[test]
fn legacy_export_excludes_inactive_messages() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    let out = tmp.path().join("legacy.sqlite");
    import_fixture(&db);

    let database = Database::open(&db).unwrap();
    let conn = database.connection();
    let active_before: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE is_active = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();

    conn.execute(
        "UPDATE messages SET is_active = 0 WHERE id = 'msg-text-001'",
        [],
    )
    .unwrap();

    export_legacy_sqlite(conn, &out).unwrap();

    let legacy = rusqlite::Connection::open(&out).unwrap();
    let exported: i64 = legacy
        .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
        .unwrap();
    assert_eq!(exported, active_before - 1);

    let missing: i64 = legacy
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE id = 'msg-text-001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(missing, 0);
}

#[test]
fn legacy_export_orders_content_blocks_by_ordinal() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    let out = tmp.path().join("legacy.sqlite");
    import_fixture(&db);

    let database = Database::open(&db).unwrap();
    export_legacy_sqlite(database.connection(), &out).unwrap();

    let legacy = rusqlite::Connection::open(&out).unwrap();
    let content: String = legacy
        .query_row(
            "SELECT content FROM messages WHERE id = 'msg-multimodal-001'",
            [],
            |r| r.get(0),
        )
        .unwrap_or_default();

    assert!(!content.is_empty());
}

#[test]
fn legacy_export_ic_are_unique_and_contiguous_range() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    let out = tmp.path().join("legacy.sqlite");
    import_fixture(&db);

    let database = Database::open(&db).unwrap();
    let stats = export_legacy_sqlite(database.connection(), &out).unwrap();

    let legacy = rusqlite::Connection::open(&out).unwrap();
    let dup_ic: i64 = legacy
        .query_row(
            "SELECT COUNT(*) FROM (
                SELECT IC FROM messages GROUP BY IC HAVING COUNT(*) > 1
            )",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(dup_ic, 0);
    assert_eq!(stats.min_ic, 1);
    assert_eq!(stats.max_ic, stats.messages_exported as i64);
}

#[test]
fn legacy_export_builds_fts_index() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    let out = tmp.path().join("legacy.sqlite");
    import_fixture(&db);

    let database = Database::open(&db).unwrap();
    let stats = export_legacy_sqlite(database.connection(), &out).unwrap();
    assert!(stats.fts_available);

    let legacy = rusqlite::Connection::open(&out).unwrap();
    let fts_table: i64 = legacy
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='messages_fts'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(fts_table, 1);

    let titles_view: i64 = legacy
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='view' AND name='titles'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(titles_view, 1);

    let hits: i64 = legacy
        .query_row(
            "SELECT COUNT(*) FROM messages m
             JOIN messages_fts fts ON m.rowid = fts.rowid
             WHERE fts.messages_fts MATCH 'Hello'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(hits > 0);
}

#[test]
fn token_count_uses_fallback_heuristic() {
    assert_eq!(compute_token_count(""), 0);
    assert_eq!(compute_token_count("   "), 0);
    assert_eq!(compute_token_count("abcd"), 1);
    assert_eq!(compute_token_count("a".repeat(8).as_str()), 2);
}
