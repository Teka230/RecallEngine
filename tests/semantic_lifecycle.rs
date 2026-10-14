use recall_engine::cli::AssetMode;
use recall_engine::commands::import::run_chatgpt_import;
use recall_engine::read_model::ReadRepository;
use recall_engine::semantic::embedding::MockDeterministicProvider;
use recall_engine::semantic::indexer::SemanticIndexer;
use recall_engine::storage::Database;
use std::path::{Path, PathBuf};

fn import_fixture(source: &Path, db: &Path) {
    recall_engine::semantic::store::ensure_sqlite_vec_registered();
    run_chatgpt_import(
        source.to_path_buf(),
        db.to_path_buf(),
        AssetMode::External,
        None,
        false,
        None,
    )
    .expect("import test fixture");
}

#[test]
fn test_sqlite_vec_registered_on_fresh_connections() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("fresh_conn.sqlite");

    let fixture_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chatgpt-sanitized");
    import_fixture(&fixture_dir, &db_path);

    // 1. Verify Database::open auto-registers sqlite-vec and can run vec_version()
    let db = Database::open(&db_path).expect("Open database");
    let version_rw: String = db
        .connection()
        .query_row("SELECT vec_version()", [], |r| r.get(0))
        .expect("Query vec_version on Database connection");
    assert!(!version_rw.is_empty(), "vec_version must not be empty");

    // Verify virtual table vec0 works on Database connection
    let items_count: i64 = db
        .connection()
        .query_row("SELECT COUNT(*) FROM semantic_vec_items", [], |r| r.get(0))
        .expect("Query semantic_vec_items on Database connection");
    assert_eq!(items_count, 0);

    // 2. Verify ReadRepository::open_read_only auto-registers sqlite-vec on read-only connection
    let read_repo = ReadRepository::open_read_only(&db_path).expect("Open ReadRepository");
    let version_ro: String = read_repo
        .connection()
        .query_row("SELECT vec_version()", [], |r| r.get(0))
        .expect("Query vec_version on ReadRepository read-only connection");
    assert_eq!(version_rw, version_ro);
}

#[test]
fn test_p2a_gate_semantic_indexer_pre_inference_idempotence() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("lifecycle.sqlite");

    let fixture_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chatgpt-sanitized");
    import_fixture(&fixture_dir, &db_path);

    let db = Database::open(&db_path).expect("Open database");
    let indexer = SemanticIndexer::new(db.connection());

    // ------------------------------------------------------------------------
    // RUN 1: Initial full indexing of the corpus
    // ------------------------------------------------------------------------
    let mut mock = MockDeterministicProvider::default();
    let stats1 = indexer.index_all(&mut mock).expect("Initial index_all run");

    println!("Run 1 Stats: {:?}", stats1);
    assert!(
        stats1.total_eligible_blocks > 0,
        "Fixture must contain eligible content blocks"
    );
    assert_eq!(
        stats1.indexed_blocks, stats1.total_eligible_blocks,
        "All blocks must be indexed on initial pass"
    );
    assert_eq!(
        stats1.up_to_date_blocks, 0,
        "No blocks should be up to date on fresh index"
    );
    assert!(
        stats1.embeddings_computed > 0,
        "Embeddings must be computed on first pass"
    );
    assert_eq!(mock.calls_count(), 1, "Exactly one batch inference call");
    assert_eq!(
        mock.embeddings_computed(),
        stats1.embeddings_computed,
        "Mock accounted embeddings must match reported stats"
    );
    assert_eq!(
        stats1.total_chunks, stats1.embeddings_computed,
        "Total chunks must match computed embeddings"
    );

    // ------------------------------------------------------------------------
    // RUN 2: Immediate re-index of the EXACT same corpus (IDEMPOTENCE GATE)
    // CRITICAL: Must result in 0 embedding calls and 0 embeddings computed!
    // ------------------------------------------------------------------------
    mock.reset_counters();
    let stats2 = indexer.index_all(&mut mock).expect("Second index_all run");

    println!("Run 2 Stats: {:?}", stats2);
    assert_eq!(
        stats2.total_eligible_blocks, stats1.total_eligible_blocks,
        "Eligible blocks count unchanged"
    );
    assert_eq!(
        stats2.up_to_date_blocks, stats1.total_eligible_blocks,
        "Every block must be detected as up-to-date via pre-inference fingerprints"
    );
    assert_eq!(stats2.indexed_blocks, 0, "Zero blocks should be re-indexed");
    assert_eq!(
        stats2.embeddings_computed, 0,
        "Zero embeddings computed in stats"
    );
    assert_eq!(
        mock.calls_count(),
        0,
        "P2-A GATE VIOLATION: Inactive/up-to-date corpus triggered embed_passages!"
    );
    assert_eq!(
        mock.embeddings_computed(),
        0,
        "P2-A GATE VIOLATION: Mock recorded vector calculations on up-to-date corpus!"
    );
    assert_eq!(
        stats2.total_chunks, stats1.total_chunks,
        "Total chunks preserved"
    );

    // ------------------------------------------------------------------------
    // RUN 3: Mutate exactly 1 content block
    // CRITICAL: Must compute embeddings ONLY for that 1 block's chunks!
    // ------------------------------------------------------------------------
    mock.reset_counters();
    let target_block_id: String = db
        .connection()
        .query_row(
            "SELECT id FROM content_blocks WHERE text_content IS NOT NULL AND LENGTH(TRIM(text_content)) > 0 LIMIT 1",
            [],
            |r| r.get(0),
        )
        .expect("Find target block");

    db.connection()
        .execute(
            "UPDATE content_blocks SET text_content = text_content || ' [modified payload for P2-A test]' WHERE id = ?1",
            [&target_block_id],
        )
        .expect("Mutate single content block");

    let stats3 = indexer.index_all(&mut mock).expect("Third index_all run");

    println!("Run 3 Stats: {:?}", stats3);
    assert_eq!(
        stats3.indexed_blocks, 1,
        "P2-A GATE: Exactly 1 block must be indexed"
    );
    assert_eq!(
        stats3.up_to_date_blocks,
        stats1.total_eligible_blocks - 1,
        "All other blocks must remain up to date without embedding"
    );
    assert!(
        stats3.embeddings_computed >= 1,
        "Embeddings computed only for mutated block"
    );
    assert_eq!(mock.calls_count(), 1, "Single batch call for mutated block");
    assert_eq!(
        mock.embeddings_computed(),
        stats3.embeddings_computed,
        "Mock accounted embeddings must match mutated block's chunk count"
    );

    // ------------------------------------------------------------------------
    // RUN 4: Semantic Search KNN verification
    // ------------------------------------------------------------------------
    let matches = indexer
        .search(&mut mock, "RecallEngine architecture graph", 3)
        .expect("KNN search");
    assert!(!matches.is_empty(), "Search should return top-k matches");
    assert!(matches[0].similarity > 0.0);
    assert!(!matches[0].content_block_id.is_empty());
    assert!(!matches[0].fingerprint.is_empty());

    // ------------------------------------------------------------------------
    // RUN 5: Ineligibility pruning test (role changed to 'system')
    // ------------------------------------------------------------------------
    mock.reset_counters();
    db.connection()
        .execute(
            "UPDATE messages SET role = 'system' WHERE id = (SELECT message_id FROM content_blocks WHERE id = ?1)",
            [&target_block_id],
        )
        .expect("Deactivate message to system role");

    let stats5 = indexer
        .index_all(&mut mock)
        .expect("Fifth index_all run after ineligibility change");

    println!("Run 5 Stats: {:?}", stats5);
    assert_eq!(
        stats5.pruned_blocks, 1,
        "Ineligible block must be pruned from vector tables"
    );
    assert_eq!(
        stats5.indexed_blocks, 0,
        "Zero blocks indexed after ineligibility pruning"
    );
    assert_eq!(mock.calls_count(), 0, "Zero embedding calls on pruning");
    assert_eq!(
        stats5.total_chunks,
        stats1.total_chunks - 1,
        "Total chunks decreased by 1"
    );

    // ------------------------------------------------------------------------
    // RUN 6: Verify cascade deletion trigger cleans virtual table items
    // ------------------------------------------------------------------------
    let another_block_id: String = db
        .connection()
        .query_row(
            "SELECT content_block_id FROM semantic_vec_chunks LIMIT 1",
            [],
            |r| r.get(0),
        )
        .expect("Find indexed block");

    let items_before: i64 = db
        .connection()
        .query_row("SELECT COUNT(*) FROM semantic_vec_items", [], |r| r.get(0))
        .unwrap();

    db.connection()
        .execute(
            "DELETE FROM content_blocks WHERE id = ?1",
            [&another_block_id],
        )
        .expect("Delete content block");

    let items_after: i64 = db
        .connection()
        .query_row("SELECT COUNT(*) FROM semantic_vec_items", [], |r| r.get(0))
        .unwrap();

    assert_eq!(
        items_after,
        items_before - 1,
        "Trigger trg_semantic_vec_chunks_delete must cascade-delete from semantic_vec_items"
    );
}
