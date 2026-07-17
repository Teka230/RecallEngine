use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::layout::Rect;

use recall_engine::read_model::ReadRepository;
use recall_engine::storage::Database;
use recall_engine::tui::event::{handle_key, handle_mouse};
use recall_engine::tui::state::{App, Focus};

fn setup_empty_app() -> App {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("history.sqlite");
    let _database = Database::open(&db).unwrap();
    let repo = ReadRepository::open_read_only(&db).unwrap();
    App::new(repo).unwrap()
}

#[test]
fn test_handle_key_focus_navigation() {
    let mut app = setup_empty_app();

    // Initial focus should be Conversations
    assert_eq!(app.focus, Focus::Conversations);

    let tab_key = KeyEvent {
        code: KeyCode::Tab,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    };

    // Tab -> Reader
    handle_key(&mut app, tab_key).unwrap();
    assert_eq!(app.focus, Focus::Reader);

    // Tab -> Inspector
    handle_key(&mut app, tab_key).unwrap();
    assert_eq!(app.focus, Focus::Inspector);

    // Tab -> Input
    handle_key(&mut app, tab_key).unwrap();
    assert_eq!(app.focus, Focus::Input);

    // Tab in Input -> Conversations
    handle_key(&mut app, tab_key).unwrap();
    assert_eq!(app.focus, Focus::Conversations);
}

#[test]
fn test_handle_mouse_hit_testing() {
    let mut app = setup_empty_app();

    app.conversations_rect = Rect::new(0, 0, 20, 100);
    app.reader_rect = Rect::new(20, 0, 60, 80);
    app.inspector_rect = Rect::new(80, 0, 20, 80);
    app.input_rect = Rect::new(20, 80, 80, 20);

    let mut click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    };

    // Click Reader
    click.column = 25;
    click.row = 10;
    handle_mouse(&mut app, click).unwrap();
    assert_eq!(app.focus, Focus::Reader);

    // Click Inspector
    click.column = 85;
    click.row = 10;
    handle_mouse(&mut app, click).unwrap();
    assert_eq!(app.focus, Focus::Inspector);

    // Click Input
    click.column = 25;
    click.row = 85;
    handle_mouse(&mut app, click).unwrap();
    assert_eq!(app.focus, Focus::Input);

    // Click Conversations
    click.column = 5;
    click.row = 10;
    handle_mouse(&mut app, click).unwrap();
    assert_eq!(app.focus, Focus::Conversations);
}
