use std::{path::PathBuf, time::Duration};

use crate::tui::state::{
    App, Focus, ReaderMode, ALL_MESSAGES_CAP, CONVERSATION_LOAD_DEBOUNCE, PAGE_SIZE,
    THREAD_CACHE_LIMIT,
};
use crate::{cli::AssetMode, commands::import::run_chatgpt_import, read_model::ReadRepository};

#[test]
fn labels_reader_modes() {
    assert_eq!(ReaderMode::Thread.label(), "Thread");
    assert_eq!(ReaderMode::AllMessages.label(), "All messages · IC order");
}

#[test]
fn public_limits_are_stable() {
    assert_eq!(PAGE_SIZE, 150);
    assert_eq!(ALL_MESSAGES_CAP, 500);
    assert_eq!(THREAD_CACHE_LIMIT, 12);
    assert_eq!(CONVERSATION_LOAD_DEBOUNCE, Duration::from_millis(180));
}

#[test]
fn focus_labels_are_stable() {
    assert_eq!(Focus::Reader.label(), "Reader");
}

#[test]
fn debounce_keeps_current_conversation_and_loads_only_latest_selection() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chatgpt-sanitized");
    run_chatgpt_import(fixture, db.clone(), AssetMode::External, None, false, None).unwrap();
    let mut app = App::new(ReadRepository::open_read_only(&db).unwrap()).unwrap();
    assert!(app.conversations.len() >= 2);

    app.conversation_nav_at -= CONVERSATION_LOAD_DEBOUNCE;
    app.flush_pending_conversation_load().unwrap();
    let first = app.conversation_id.clone().unwrap();
    let second = app.conversations[1].id.clone();

    app.schedule_conversation_load(first.clone());
    app.schedule_conversation_load(second.clone());
    app.flush_pending_conversation_load().unwrap();
    assert_eq!(app.conversation_id.as_deref(), Some(first.as_str()));
    assert_eq!(
        app.pending_conversation_id.as_deref(),
        Some(second.as_str())
    );

    app.conversation_nav_at -= CONVERSATION_LOAD_DEBOUNCE + Duration::from_millis(1);
    app.flush_pending_conversation_load().unwrap();
    assert_eq!(app.conversation_id.as_deref(), Some(second.as_str()));
    assert!(app.pending_conversation_id.is_none());
}

#[test]
fn thread_cache_is_bounded_and_refreshes_lru_order_on_read() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/chatgpt-sanitized");
    run_chatgpt_import(fixture, db.clone(), AssetMode::External, None, false, None).unwrap();
    let mut app = App::new(ReadRepository::open_read_only(&db).unwrap()).unwrap();
    app.thread_cache.clear();
    app.thread_cache_order.clear();

    for index in 0..12 {
        app.store_thread_cache_entry(format!("key-{index}"), &[]);
    }
    assert!(app.cached_thread_entry("key-0").is_some());
    app.store_thread_cache_entry("key-12".into(), &[]);

    assert_eq!(app.thread_cache.len(), 12);
    assert!(app.thread_cache.contains_key("key-0"));
    assert!(!app.thread_cache.contains_key("key-1"));
}
