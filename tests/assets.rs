use std::path::PathBuf;

use recall_engine::cli::AssetMode;
use recall_engine::commands::import::run_chatgpt_import;
use recall_engine::storage::Database;

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chatgpt-sanitized")
}

#[test]
fn shared_asset_resolved_via_mapping() {
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
    let conn = database.connection();
    let links: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM message_assets WHERE asset_id = 'chatgpt:file-fixture-asset.dat'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(links >= 2, "two messages should share the asset");
}

#[test]
fn missing_asset_file_recorded_as_issue() {
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
    let conn = database.connection();
    let missing: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM import_issues WHERE code = 'MISSING_ASSET_FILE'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(missing >= 1);
}

#[test]
fn content_references_from_metadata() {
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
    let conn = database.connection();
    let refs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM content_references WHERE ref_source = 'metadata'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(refs >= 1);
}

#[test]
fn sidecars_imported() {
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
    let conn = database.connection();
    let feedback: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM feedback WHERE is_active = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let shared: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM shared_conversations WHERE is_active = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let library: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM library_files WHERE is_active = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(feedback, 1);
    assert_eq!(shared, 1);
    assert_eq!(library, 1);
}

#[test]
fn assets_are_copied_physically() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("export");
    let dest = tmp.path().join("db");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dest).unwrap();

    // Copy fixture files to temp src, and create dummy asset
    for entry in std::fs::read_dir(fixture_root()).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), src.join(entry.file_name())).unwrap();
        }
    }
    std::fs::write(src.join("file-fixture-asset.dat"), "dummy content").unwrap();

    let db = dest.join("history.sqlite");
    let assets_dir = dest.join("custom_assets");
    run_chatgpt_import(
        src,
        db.clone(),
        AssetMode::Copy,
        Some(assets_dir.clone()),
        false,
        None,
    )
    .unwrap();

    // Verify copied file exists at destination
    let copied_file = assets_dir.join("file-fixture-asset.dat");
    assert!(copied_file.exists());
    assert_eq!(
        std::fs::read_to_string(&copied_file).unwrap(),
        "dummy content"
    );

    let database = Database::open(&db).unwrap();
    let conn = database.connection();
    let exists_in_db: i64 = conn
        .query_row(
            "SELECT exists_locally FROM assets WHERE id = 'chatgpt:file-fixture-asset.dat'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(exists_in_db, 1);
}

#[test]
fn assets_are_symlinked_physically() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("export");
    let dest = tmp.path().join("db");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::create_dir_all(&dest).unwrap();

    for entry in std::fs::read_dir(fixture_root()).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), src.join(entry.file_name())).unwrap();
        }
    }
    std::fs::write(src.join("file-fixture-asset.dat"), "dummy link content").unwrap();

    let db = dest.join("history.sqlite");
    let assets_dir = dest.join("custom_assets");
    run_chatgpt_import(
        src,
        db.clone(),
        AssetMode::Symlink,
        Some(assets_dir.clone()),
        false,
        None,
    )
    .unwrap();

    let copied_file = assets_dir.join("file-fixture-asset.dat");
    assert!(copied_file.exists());
    assert_eq!(
        std::fs::read_to_string(&copied_file).unwrap(),
        "dummy link content"
    );

    // Check it's a symlink (or copy if platform does not support symlinks, but on macOS/Unix it does)
    let meta = std::fs::symlink_metadata(&copied_file).unwrap();
    #[cfg(unix)]
    assert!(meta.file_type().is_symlink());

    let database = Database::open(&db).unwrap();
    let conn = database.connection();
    let exists_in_db: i64 = conn
        .query_row(
            "SELECT exists_locally FROM assets WHERE id = 'chatgpt:file-fixture-asset.dat'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(exists_in_db, 1);
}
