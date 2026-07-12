use std::fs;
use std::path::PathBuf;

use recall_engine::cli::AssetMode;
use recall_engine::commands::import::run_chatgpt_import;
use recall_engine::commands::verify;
use recall_engine::storage::Database;

#[test]
fn corrupt_fragment_on_first_import_leaves_no_messages() {
    let tmp = tempfile::tempdir().unwrap();
    let export = tmp.path().join("export");
    fs::create_dir_all(&export).unwrap();
    fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/chatgpt-sanitized/conversations-000.json"),
        export.join("conversations-000.json"),
    )
    .unwrap();
    fs::write(export.join("conversations-001.json"), "NOT VALID JSON").unwrap();

    let db = tmp.path().join("history.sqlite");
    run_chatgpt_import(export, db.clone(), AssetMode::External, None, false, None).unwrap();

    let database = Database::open(&db).unwrap();
    let conn = database.connection();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        count, 0,
        "partial first import must rollback canonical messages"
    );

    let partial: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM import_runs WHERE status = 'partial'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(partial, 1);
}

#[test]
fn verify_fails_on_altered_database() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chatgpt-sanitized");
    run_chatgpt_import(fixture, db.clone(), AssetMode::External, None, false, None).unwrap();

    let database = Database::open(&db).unwrap();
    let conn = database.connection();
    let (node_id,): (String,) = conn
        .query_row("SELECT node_id FROM messages LIMIT 1", [], |r| {
            Ok((r.get(0)?,))
        })
        .unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
    conn.execute("DELETE FROM nodes WHERE id = ?1", [&node_id])
        .unwrap();
    conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

    assert!(verify::run(db).is_err());
}

#[test]
fn legacy_export_reads_ic_without_recalc() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    let out = tmp.path().join("legacy.sqlite");
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chatgpt-sanitized");
    run_chatgpt_import(fixture, db.clone(), AssetMode::External, None, false, None).unwrap();

    recall_engine::commands::export::run_legacy_sqlite(db.clone(), out.clone()).unwrap();

    let database = Database::open(&db).unwrap();
    let conn = database.connection();
    let ic: i64 = conn
        .query_row(
            "SELECT ic FROM messages WHERE id = 'msg-text-001'",
            [],
            |r| r.get(0),
        )
        .unwrap();

    let legacy = rusqlite::Connection::open(out).unwrap();
    let legacy_ic: i64 = legacy
        .query_row(
            "SELECT IC FROM messages WHERE id = 'msg-text-001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ic, legacy_ic);
}
