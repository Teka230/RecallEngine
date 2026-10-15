use recall_engine::cli::AssetMode;
use recall_engine::commands::import::run_chatgpt_import;
use recall_engine::semantic::embedding::is_e5_small_cached;
use std::path::{Path, PathBuf};
use std::process::Command;

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
fn test_cli_help_embed_and_search_mode() {
    // 1. Embed help
    let output = Command::new(env!("CARGO_BIN_EXE_recall"))
        .args(["embed", "--help"])
        .output()
        .expect("run recall embed --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--db"));
    assert!(stdout.contains("--batch-size"));
    assert!(stdout.contains("--prune"));
    assert!(stdout.contains("--allow-download"));
    assert!(stdout.contains("--cache-dir"));
    assert!(stdout.contains("--json"));

    // 2. Search help shows --mode
    let output = Command::new(env!("CARGO_BIN_EXE_recall"))
        .args(["search", "--help"])
        .output()
        .expect("run recall search --help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--mode"));
    assert!(stdout.contains("lexical"));
    assert!(stdout.contains("semantic"));
    assert!(stdout.contains("hybrid"));
}

#[test]
fn test_cli_embed_nonexistent_db() {
    let output = Command::new(env!("CARGO_BIN_EXE_recall"))
        .args(["embed", "--db", "/path/does/not/exist/db.sqlite"])
        .output()
        .expect("run recall embed");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not found") || stderr.contains("Database file not found"),
        "stderr: {stderr}"
    );
}

#[test]
fn test_cli_semantic_search_fails_on_unindexed_db() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("unindexed.sqlite");
    let fixture_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chatgpt-sanitized");
    import_fixture(&fixture_dir, &db_path);

    let output = Command::new(env!("CARGO_BIN_EXE_recall"))
        .args([
            "search",
            "--db",
            db_path.to_str().unwrap(),
            "--mode",
            "semantic",
            "agent",
        ])
        .output()
        .expect("run recall search semantic on unindexed db");

    assert!(
        !output.status.success(),
        "Semantic search must fail when database has no semantic index"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Semantic search index not found or empty")
            || stderr.contains("recall embed"),
        "stderr: {stderr}"
    );
}

#[test]
fn test_cli_embed_and_hybrid_search_end_to_end() {
    if !is_e5_small_cached(None) {
        println!("Skipping end-to-end CLI embed test: model cache absent");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("cli_embed.sqlite");
    let fixture_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chatgpt-sanitized");
    import_fixture(&fixture_dir, &db_path);

    // 1. First embed run: should index all chunks and return JSON envelope
    let output = Command::new(env!("CARGO_BIN_EXE_recall"))
        .args([
            "embed",
            "--db",
            db_path.to_str().unwrap(),
            "--batch-size",
            "16",
            "--json",
        ])
        .output()
        .expect("run recall embed");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let val: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse embed JSON output");
    assert_eq!(val["schema_version"], "1");
    assert!(val["total_eligible_blocks"].as_u64().unwrap_or(0) > 0);
    assert_eq!(val["indexed_blocks"], val["total_eligible_blocks"]);
    assert_eq!(val["up_to_date_blocks"], 0);
    assert!(val["embeddings_computed"].as_u64().unwrap_or(0) > 0);

    // 2. Second embed run: Pre-inference idempotence check!
    let output2 = Command::new(env!("CARGO_BIN_EXE_recall"))
        .args(["embed", "--db", db_path.to_str().unwrap(), "--json"])
        .output()
        .expect("run recall embed second time");

    assert!(output2.status.success());
    let val2: serde_json::Value =
        serde_json::from_slice(&output2.stdout).expect("parse second embed JSON output");
    assert_eq!(val2["schema_version"], "1");
    assert_eq!(
        val2["indexed_blocks"], 0,
        "Zero blocks should be re-indexed on second run"
    );
    assert_eq!(
        val2["embeddings_computed"], 0,
        "Zero embeddings should be computed on second run (pre-inference idempotence)"
    );
    assert_eq!(val2["up_to_date_blocks"], val2["total_eligible_blocks"]);

    // 3. Search CLI with hybrid mode
    let search_output = Command::new(env!("CARGO_BIN_EXE_recall"))
        .args([
            "search",
            "--db",
            db_path.to_str().unwrap(),
            "--mode",
            "hybrid",
            "--json",
            "agent",
        ])
        .output()
        .expect("run recall search hybrid");

    assert!(
        search_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&search_output.stderr)
    );

    let search_val: serde_json::Value =
        serde_json::from_slice(&search_output.stdout).expect("parse hybrid search JSON");
    assert_eq!(search_val["schema_version"], "1");
    assert_eq!(search_val["mode"], "hybrid");
    assert!(!search_val["results"].as_array().unwrap().is_empty());

    // 4. Search CLI with semantic mode
    let semantic_output = Command::new(env!("CARGO_BIN_EXE_recall"))
        .args([
            "search",
            "--db",
            db_path.to_str().unwrap(),
            "--mode",
            "semantic",
            "--json",
            "agent",
        ])
        .output()
        .expect("run recall search semantic");

    assert!(
        semantic_output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&semantic_output.stderr)
    );

    let sem_val: serde_json::Value =
        serde_json::from_slice(&semantic_output.stdout).expect("parse semantic search JSON");
    assert_eq!(sem_val["schema_version"], "1");
    assert_eq!(sem_val["mode"], "semantic");
    assert!(!sem_val["results"].as_array().unwrap().is_empty());
}
