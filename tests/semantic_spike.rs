use recall_engine::cli::AssetMode;
use recall_engine::commands::import::run_chatgpt_import;
use recall_engine::search::service::SearchService;
use recall_engine::search::{CountMode, SearchQuery, SearchSyntax};
use recall_engine::semantic::chunking::ChunkerOptions;
use recall_engine::semantic::embedding::{FastEmbedProvider, MockDeterministicProvider};
use recall_engine::semantic::spike::SemanticSpikeRunner;
use recall_engine::storage::Database;
use std::fs;
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
fn test_semantic_spike_with_mock_provider() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("history.sqlite");

    let fixture_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chatgpt-sanitized");
    import_fixture(&fixture_dir, &db_path);

    let db = Database::open(&db_path).unwrap();
    let runner = SemanticSpikeRunner::new(db.connection(), ChunkerOptions::default());

    let mut mock_embedder = MockDeterministicProvider::default();
    let (report, matches) = runner
        .run(&mut mock_embedder, "Hello world", 3)
        .expect("Spike runner failed with mock provider");

    assert!(report.total_content_blocks > 0);
    assert!(report.total_chunks >= report.total_content_blocks);
    assert!(
        report.ranking_top_1_match,
        "Top 1 match must agree between vec and blob"
    );
    assert!(report.ranking_top_5_overlap_pct >= 80.0);
    assert!(!matches.is_empty());

    // Verify canonical references are preserved
    for m in &matches {
        assert!(!m.message_id.is_empty());
        assert!(!m.chunk_hash.is_empty());
        assert!(m.chunk_id > 0);
        assert!(m.content_block_id > 0);
    }
}

#[test]
fn test_semantic_conceptual_retrieval_fastembed_vs_fts5() {
    let tmp = tempfile::tempdir().unwrap();
    let fixture_dir = tmp.path().join("fixture");
    fs::create_dir_all(&fixture_dir).unwrap();

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    fs::copy(
        manifest_dir.join("tests/fixtures/semantic_eval_fixture.json"),
        fixture_dir.join("conversations-000.json"),
    )
    .unwrap();

    let db_path = tmp.path().join("semantic_eval.sqlite");
    import_fixture(&fixture_dir, &db_path);

    let db = Database::open(&db_path).unwrap();

    // 1. Test FTS5 behavior on conceptual query
    let fts_repo = recall_engine::repository::search::Fts5SearchRepository::new(db.connection());
    let fts_service = SearchService::new(fts_repo);

    let conceptual_query =
        "Comment reguler les permissions de l'agent sans lui donner les pleins pouvoirs ?";
    let fts_result = fts_service
        .search(SearchQuery {
            text: conceptual_query.to_string(),
            syntax: SearchSyntax::Simple,
            count_mode: CountMode::Exact,
            limit: 10,
            offset: 0,
            role: None,
            ic_min: None,
            ic_max: None,
            date_min: None,
            date_max: None,
        })
        .unwrap();

    println!(
        "FTS5 exact matches on conceptual query: {}",
        fts_result.results.len()
    );
    // FTS5 fails to match because terms like "reguler", "permissions", "pouvoirs" are not in the text
    assert_eq!(
        fts_result.results.len(),
        0,
        "FTS5 should return 0 results because vocabulary differs completely"
    );

    // 2. Test FastEmbed E5 semantic retrieval on the same conceptual query
    let mut embedder = FastEmbedProvider::try_new(None).expect("FastEmbed initialization");
    let runner = SemanticSpikeRunner::new(db.connection(), ChunkerOptions::default());

    let (report, matches) = runner
        .run(&mut embedder, conceptual_query, 3)
        .expect("Semantic spike execution with FastEmbed");

    println!("Spike Report: {:#?}", report);
    assert!(!matches.is_empty());

    let best = &matches[0];
    println!("Best match snippet: {}", best.text);
    println!("Best match message ID: {}", best.message_id);
    println!("Best match IC: {:?}", best.ic);
    println!("Best match distance: {}", best.distance);

    // Rank 1 must be the exact conceptual match: agent proposing actions without direct authority
    assert_eq!(
        best.message_id, "00000000-0000-4000-8000-000000000101",
        "Target message UUID must match the user question on agent authority"
    );
    assert_eq!(best.ic, Some(1), "IC citation must resolve to IC:1");
    assert!(
        best.text
            .contains("agent intelligent propose des actions sans disposer de l'autorite"),
        "Snippet must contain the authority concept"
    );
    assert!(
        best.distance < 0.35,
        "Top match should have small cosine distance (< 0.35)"
    );
}
