use recall_engine::cli::AssetMode;
use recall_engine::commands::import::run_chatgpt_import;
use recall_engine::search::{CountMode, SearchQuery, SearchRole, SearchSyntax};
use recall_engine::semantic::embedding::{
    is_e5_small_cached, FastEmbedProvider, MockDeterministicProvider,
};
use recall_engine::semantic::hybrid::HybridSearchService;
use recall_engine::semantic::indexer::SemanticIndexer;
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
fn test_hybrid_fallback_fts5_when_unindexed() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("unindexed.sqlite");
    let fixture_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chatgpt-sanitized");
    import_fixture(&fixture_dir, &db_path);

    let db = Database::open(&db_path).expect("open database");
    let hybrid = HybridSearchService::new(db.connection());

    // Vector tables are not populated yet
    assert!(!hybrid.is_vector_index_available());

    let query = SearchQuery {
        text: "world".to_string(),
        syntax: SearchSyntax::Simple,
        mode: recall_engine::search::SearchMode::Hybrid,
        count_mode: CountMode::Exact,
        limit: 5,
        offset: 0,
        role: None,
        ic_min: None,
        ic_max: None,
        date_min: None,
        date_max: None,
    };

    let result = hybrid
        .search(None, &query)
        .expect("graceful fallback to fts5");

    assert_eq!(result.mode, "hybrid-fallback-fts5");
    assert!(!result.results.is_empty());
    assert_eq!(result.total, Some(result.results.len() as u64));
    assert!(!result.has_more);
}

#[test]
fn test_hybrid_search_rrf_with_mock_embedder() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("mock_hybrid.sqlite");
    let fixture_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chatgpt-sanitized");
    import_fixture(&fixture_dir, &db_path);

    let db = Database::open(&db_path).expect("open database");
    let indexer = SemanticIndexer::new(db.connection());

    let mut mock = MockDeterministicProvider::default();
    let stats = indexer
        .index_all(&mut mock)
        .expect("index with mock provider");
    assert!(stats.total_chunks > 0);

    let hybrid = HybridSearchService::new(db.connection());
    assert!(hybrid.is_vector_index_available());

    let query = SearchQuery {
        text: "agent".to_string(),
        syntax: SearchSyntax::Simple,
        mode: recall_engine::search::SearchMode::Hybrid,
        count_mode: CountMode::Exact,
        limit: 10,
        offset: 0,
        role: None,
        ic_min: None,
        ic_max: None,
        date_min: None,
        date_max: None,
    };

    let result = hybrid
        .search(Some(&mut mock), &query)
        .expect("hybrid search succeeds");

    assert_eq!(result.mode, "hybrid");
    assert!(!result.results.is_empty());
    assert!(result.total.is_some());

    // Top result rank should be positive RRF score
    assert!(result.results[0].rank > 0.0);
}

#[test]
fn test_hybrid_search_role_and_ic_filtering() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("filtering.sqlite");
    let fixture_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chatgpt-sanitized");
    import_fixture(&fixture_dir, &db_path);

    let db = Database::open(&db_path).expect("open database");
    let indexer = SemanticIndexer::new(db.connection());

    let mut mock = MockDeterministicProvider::default();
    indexer.index_all(&mut mock).expect("index all");

    let hybrid = HybridSearchService::new(db.connection());

    // 1. Role filter: User only
    let user_query = SearchQuery {
        text: "agent".to_string(),
        syntax: SearchSyntax::Simple,
        mode: recall_engine::search::SearchMode::Hybrid,
        count_mode: CountMode::Exact,
        limit: 10,
        offset: 0,
        role: Some(SearchRole::User),
        ic_min: None,
        ic_max: None,
        date_min: None,
        date_max: None,
    };
    let user_res = hybrid
        .search(Some(&mut mock), &user_query)
        .expect("search user");
    for item in &user_res.results {
        assert_eq!(item.role, "user", "All filtered items must be user role");
    }

    // 2. IC min/max filter
    let ic_query = SearchQuery {
        text: "agent".to_string(),
        syntax: SearchSyntax::Simple,
        mode: recall_engine::search::SearchMode::Hybrid,
        count_mode: CountMode::Exact,
        limit: 10,
        offset: 0,
        role: None,
        ic_min: Some(1),
        ic_max: Some(1),
        date_min: None,
        date_max: None,
    };
    let ic_res = hybrid
        .search(Some(&mut mock), &ic_query)
        .expect("search ic range");
    for item in &ic_res.results {
        assert_eq!(item.ic, Some(1), "All filtered items must have IC=1");
    }
}

#[test]
fn test_hybrid_conceptual_retrieval_fastembed() {
    if !is_e5_small_cached(None) {
        println!("Skipping FastEmbed hybrid test: cache absent");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let fixture_dir = tmp.path().join("fixture");
    fs::create_dir_all(&fixture_dir).unwrap();

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    fs::copy(
        manifest_dir.join("tests/fixtures/semantic_eval_fixture.json"),
        fixture_dir.join("conversations-000.json"),
    )
    .unwrap();

    let db_path = tmp.path().join("hybrid_fastembed.sqlite");
    import_fixture(&fixture_dir, &db_path);

    let db = Database::open(&db_path).expect("open database");
    let indexer = SemanticIndexer::new(db.connection());

    let mut embedder = FastEmbedProvider::try_new_offline(None).expect("FastEmbed offline");
    let stats = indexer.index_all(&mut embedder).expect("index all");
    assert!(stats.total_chunks > 0);

    let hybrid = HybridSearchService::new(db.connection());

    // Conceptual query where FTS5 has 0 matches
    let conceptual_query = SearchQuery {
        text: "Comment reguler les permissions de l'agent sans lui donner les pleins pouvoirs ?"
            .to_string(),
        syntax: SearchSyntax::Simple,
        mode: recall_engine::search::SearchMode::Hybrid,
        count_mode: CountMode::Exact,
        limit: 5,
        offset: 0,
        role: None,
        ic_min: None,
        ic_max: None,
        date_min: None,
        date_max: None,
    };

    let result = hybrid
        .search(Some(&mut embedder), &conceptual_query)
        .expect("conceptual hybrid search");

    assert_eq!(result.mode, "hybrid");
    assert!(
        !result.results.is_empty(),
        "Hybrid search must retrieve results through semantic vector arm"
    );

    // Top match should be the agent authority message (IC 1)
    let top = &result.results[0];
    assert_eq!(top.ic, Some(1), "Top conceptual match must be IC:1");
    assert_eq!(top.message_id, "00000000-0000-4000-8000-000000000101");
}
