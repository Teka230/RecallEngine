use std::path::{Path, PathBuf};

use recall_engine::{
    cli::AssetMode, commands::import::run_chatgpt_import, domain::reference::MessageReference,
    read_model::ReadRepository,
};

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
fn read_model_lists_searches_and_resolves_fixture() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    import_fixture(&db);

    let repository = ReadRepository::open_read_only(&db).unwrap();
    let conversations = repository.list_conversations("", 100).unwrap();
    assert!(!conversations.is_empty());

    let search = repository.search("Hello", 100).unwrap();
    assert!(!search.is_empty());
    let hit = &search[0];
    let thread = repository.thread_for_message(&hit.message_id).unwrap();
    assert!(thread.iter().any(|message| message.id == hit.message_id));

    let ic = hit.ic.expect("fixture hit is a public message");
    let resolved = repository.resolve_ic_message(ic).unwrap().unwrap();
    assert_eq!(resolved.id, hit.message_id);
    let jump = repository.resolve_ic_jump(ic).unwrap().unwrap();
    assert_eq!(jump.message_id, hit.message_id);
    assert_eq!(jump.conversation_id, hit.conversation_id);
    let index = repository
        .conversation_list_index(&hit.conversation_id)
        .unwrap()
        .expect("conversation index");
    assert_eq!(
        repository.list_conversations("", 100).unwrap()[index].id,
        hit.conversation_id
    );
    assert!(repository.ic_context(ic).unwrap().is_some());
}

#[test]
fn all_messages_retains_technical_rows_without_public_ic() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    import_fixture(&db);

    let repository = ReadRepository::open_read_only(&db).unwrap();
    let conversation = repository.list_conversations("", 100).unwrap().remove(0);
    let messages = repository.all_messages(&conversation.id, 500).unwrap();
    assert!(!messages.is_empty());
    assert!(messages
        .iter()
        .all(|message| !message.is_technical() || message.ic.is_none()));
}

#[test]
fn current_thread_stays_on_current_branch_and_finds_siblings() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    import_fixture(&db);

    let repository = ReadRepository::open_read_only(&db).unwrap();
    let thread = repository.current_thread("conv-branch-001").unwrap();
    let ids = thread
        .iter()
        .map(|message| message.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["msg-branch-root", "msg-branch-c"]);

    let branches = repository.branches_on_path("node-a3").unwrap();
    assert_eq!(branches.len(), 3);
}

#[test]
fn conversation_pages_are_stable() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    import_fixture(&db);

    let repository = ReadRepository::open_read_only(&db).unwrap();
    let first = repository.list_conversations_page("", 1, 0).unwrap();
    let second = repository.list_conversations_page("", 1, 1).unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_ne!(first[0].id, second[0].id);
}

#[test]
fn composite_reference_resolves_by_ic_and_message_id() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    import_fixture(&db);

    let repository = ReadRepository::open_read_only(&db).unwrap();
    let hit = repository.search("Hello", 1).unwrap().remove(0);
    let ic = hit.ic.expect("fixture hit is public");
    let reference = MessageReference::new(ic, hit.message_id.clone()).unwrap();

    let by_id = repository
        .resolve_message_id(&hit.message_id)
        .unwrap()
        .unwrap();
    let by_reference = repository.resolve_reference(&reference).unwrap().unwrap();
    assert_eq!(by_id, by_reference);
    assert_eq!(by_reference.reference, reference.human());

    let mismatch = MessageReference::new(ic + 1, hit.message_id).unwrap();
    assert!(repository.resolve_reference(&mismatch).is_err());
}

#[test]
fn opening_read_model_does_not_modify_database_or_create_sidecars() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    import_fixture(&db);
    let before = std::fs::read(&db).unwrap();
    let wal = PathBuf::from(format!("{}-wal", db.display()));
    let shm = PathBuf::from(format!("{}-shm", db.display()));
    assert!(!wal.exists());
    assert!(!shm.exists());

    {
        let repository = ReadRepository::open_read_only(&db).unwrap();
        assert!(!repository.list_conversations("", 10).unwrap().is_empty());
        assert!(!repository.search("Hello", 10).unwrap().is_empty());
    }

    assert_eq!(std::fs::read(&db).unwrap(), before);
    assert!(!wal.exists());
    assert!(!shm.exists());
}
