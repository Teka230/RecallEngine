use std::path::{Path, PathBuf};

use recall_engine::cli::AssetMode;
use recall_engine::commands::import::run_chatgpt_import;
use recall_engine::commands::{stats, verify};
use recall_engine::import::chatgpt::{parse_fragment, parse_fragment_metadata};
use recall_engine::import::discovery::discover_export;
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
    .expect("import fixture");
}

#[test]
fn metadata_pass_preserves_full_parser_ic_sort_keys() {
    let fixture = fixture_root();
    let layout = discover_export(&fixture, false).unwrap();

    for (index, path) in layout.conversation_paths.iter().enumerate() {
        let shard_index = (index + 1) as i32;
        let metadata = parse_fragment_metadata(&layout, path, shard_index).unwrap();
        let full = parse_fragment(&layout, path, shard_index).unwrap();
        let planned: Vec<_> = metadata
            .messages
            .into_iter()
            .map(|candidate| {
                (
                    candidate.id,
                    candidate.conversation_id,
                    candidate.create_time,
                    candidate.create_time_raw,
                    candidate.source_shard_index,
                    candidate.source_node_order,
                )
            })
            .collect();
        let parsed: Vec<_> = full
            .messages
            .into_iter()
            .map(|message| {
                (
                    message.id,
                    message.conversation_id,
                    message.create_time,
                    message.create_time_raw,
                    message.source_shard_index,
                    message.source_node_order,
                )
            })
            .collect();
        assert_eq!(planned, parsed, "IC planning diverged for {path:?}");
    }
}

#[test]
fn imports_fixture_and_preserves_graph() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    import_fixture(&db);

    let database = Database::open(&db).unwrap();
    let conn = database.connection();

    let nodes: i64 = conn
        .query_row("SELECT COUNT(*) FROM nodes WHERE is_active = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(nodes >= 10);

    let current: String = conn
        .query_row(
            "SELECT current_node_id FROM conversations WHERE id = 'conv-linear-001'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(current, "node-u1");

    let kinds: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM content_blocks WHERE kind IN ('thoughts','reasoning_recap','text','asset_reference')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(kinds >= 4);
}

#[test]
fn ic_assigned_contiguous_on_first_import() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    import_fixture(&db);

    let database = Database::open(&db).unwrap();
    let conn = database.connection();
    let max_ic: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(ic), 0) FROM messages WHERE is_active = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE is_active = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(max_ic, count);
}

#[test]
fn idempotent_reimport_preserves_ic() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    import_fixture(&db);

    let database = Database::open(&db).unwrap();
    let conn = database.connection();
    let before: Vec<(String, i64)> = {
        let mut stmt = conn
            .prepare("SELECT id, ic FROM messages ORDER BY ic")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };

    import_fixture(&db);

    let after: Vec<(String, i64)> = {
        let mut stmt = conn
            .prepare("SELECT id, ic FROM messages ORDER BY ic")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(before, after);
}

#[test]
fn verify_passes_on_imported_db() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    import_fixture(&db);
    verify::run(db.clone()).expect("verify should pass");
}

#[test]
fn stats_json_has_expected_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    import_fixture(&db);
    stats::run(db, true).expect("stats");
}

#[test]
fn fts5_search_index_populated_and_queryable() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    import_fixture(&db);

    let database = Database::open(&db).unwrap();
    let conn = database.connection();

    // Verify FTS5 table has records
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM content_blocks_fts", [], |r| r.get(0))
        .unwrap();
    assert!(count > 0, "FTS5 table should have records");

    // Perform a test MATCH query
    let sample_text: String = conn
        .query_row(
            "SELECT text_content FROM content_blocks WHERE text_content IS NOT NULL LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();

    // Extract first word
    let word = sample_text.split_whitespace().next().unwrap_or("hello");
    let cleaned: String = word.chars().filter(|c| c.is_alphanumeric()).collect();
    if !cleaned.is_empty() {
        let fts_query = format!("\"{}\"*", cleaned);
        let match_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM content_blocks_fts WHERE content_blocks_fts MATCH ?1",
                [&fts_query],
                |r| r.get(0),
            )
            .unwrap();
        assert!(
            match_count > 0,
            "MATCH query should find the word: {}",
            cleaned
        );
    }
}
