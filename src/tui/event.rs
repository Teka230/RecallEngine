use super::state::{App, Focus, Overlay, ReaderMode};
use crate::read_model::MessageView;

use crate::Result;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

pub fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if handle_overlay_key(app, key)? {
        return Ok(());
    }
    if app.focus == Focus::Input {
        match key.code {
            KeyCode::Esc => {
                app.focus = Focus::Reader;
                app.refresh_status();
                return Ok(());
            }
            KeyCode::Tab => {
                app.focus = Focus::Conversations;
                app.refresh_status();
                return Ok(());
            }
            KeyCode::Enter => {
                let value = app
                    .search_input
                    .lines()
                    .first()
                    .cloned()
                    .unwrap_or_default();
                match app.input_mode {
                    crate::tui::state::InputMode::Search => app.submit_search(value)?,
                    crate::tui::state::InputMode::Jump => app.open_reference_input(&value, true)?,
                }
                app.focus = Focus::Reader;
                app.search_input = ratatui_textarea::TextArea::default();
                app.refresh_status();
                return Ok(());
            }
            _ => {
                app.search_input.input(key);
                return Ok(());
            }
        }
    }
    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true
        }
        KeyCode::Char('?') => app.overlay = Some(Overlay::Help),
        KeyCode::Tab => {
            app.focus = match app.focus {
                Focus::Conversations => Focus::Reader,
                Focus::Reader => Focus::Inspector,
                Focus::Inspector => Focus::Input,
                Focus::Input => Focus::Conversations,
            };
            if app.focus == Focus::Inspector {
                app.refresh_inspector()?;
            }
            app.refresh_status();
        }
        KeyCode::Esc => {
            app.sync_sidebar_to_active()?;
            app.refresh_status();
        }
        KeyCode::Char('/') => app.start_search(),
        KeyCode::Char('i') => app.start_ic_jump(),
        KeyCode::Char('v') => app.toggle_mode()?,
        KeyCode::Char('t') => {
            app.technical_visible = !app.technical_visible;
            app.invalidate_reader_cache();
            if !app.technical_visible
                && app
                    .selected_message()
                    .is_some_and(MessageView::is_technical)
            {
                app.move_message(-1)?;
            } else {
                app.update_reader_scroll();
            }
            app.refresh_status();
        }
        KeyCode::Char('b') => app.open_branches()?,
        KeyCode::Char('y') => app.copy_citation(),
        KeyCode::Char('d') => {
            app.focus = Focus::Inspector;
            app.refresh_inspector()?;
        }
        KeyCode::Char('c') => app.open_context()?,
        KeyCode::Char('j') | KeyCode::Down => match app.focus {
            Focus::Conversations => app.move_conversation(1)?,
            Focus::Reader | Focus::Inspector => app.move_message(1)?,
            Focus::Input => {}
        },
        KeyCode::Char('k') | KeyCode::Up => match app.focus {
            Focus::Conversations => app.move_conversation(-1)?,
            Focus::Reader | Focus::Inspector => app.move_message(-1)?,
            Focus::Input => {}
        },
        KeyCode::Enter if app.focus == Focus::Conversations => {
            if let Some(id) = app
                .pending_conversation_id
                .take()
                .or_else(|| app.selected_conversation_id().map(str::to_owned))
            {
                app.loading_conversation = false;
                if app.conversation_id.as_deref() != Some(id.as_str()) {
                    app.load_conversation(&id, true)?;
                } else {
                    app.focus = Focus::Reader;
                    app.refresh_status();
                }
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_overlay_key(app: &mut App, key: KeyEvent) -> Result<bool> {
    let Some(overlay) = app.overlay.take() else {
        return Ok(false);
    };
    match overlay {
        Overlay::Branches {
            choices,
            mut selected,
        } => match key.code {
            KeyCode::Esc => {}
            KeyCode::Char('j') | KeyCode::Down => {
                selected = (selected + 1).min(choices.len().saturating_sub(1));
                app.overlay = Some(Overlay::Branches { choices, selected });
            }
            KeyCode::Char('k') | KeyCode::Up => {
                selected = selected.saturating_sub(1);
                app.overlay = Some(Overlay::Branches { choices, selected });
            }
            KeyCode::Enter => {
                if let Some(choice) = choices.get(selected) {
                    let node_id = choice.node_id.clone();
                    app.messages = app.repository.thread_for_node(&node_id)?;
                    app.bump_messages_generation();
                    app.reader_mode = ReaderMode::Thread;
                    app.focus = Focus::Reader;
                    app.sync_sidebar_to_active()?;
                    app.show_first_message()?;
                    app.refresh_status();
                }
            }
            _ => app.overlay = Some(Overlay::Branches { choices, selected }),
        },
        Overlay::Context { lines } => match key.code {
            KeyCode::Esc | KeyCode::Char('c') | KeyCode::Enter => {}
            _ => app.overlay = Some(Overlay::Context { lines }),
        },
        Overlay::SearchResults {
            query,
            hits,
            mut selected,
        } => match key.code {
            KeyCode::Esc => {}
            KeyCode::Char('j') | KeyCode::Down => {
                selected = (selected + 1).min(hits.len().saturating_sub(1));
                app.overlay = Some(Overlay::SearchResults {
                    query,
                    hits,
                    selected,
                });
            }
            KeyCode::Char('k') | KeyCode::Up => {
                selected = selected.saturating_sub(1);
                app.overlay = Some(Overlay::SearchResults {
                    query,
                    hits,
                    selected,
                });
            }
            KeyCode::Enter => {
                if let Some(hit) = hits.get(selected) {
                    let message_id = hit.message_id.clone();
                    app.open_search_hit(&message_id)?;
                }
            }
            _ => {
                app.overlay = Some(Overlay::SearchResults {
                    query,
                    hits,
                    selected,
                });
            }
        },
        Overlay::Help => match key.code {
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Enter => {}
            _ => app.overlay = Some(Overlay::Help),
        },
    }
    Ok(true)
}

pub fn handle_mouse(app: &mut App, mouse: MouseEvent) -> Result<()> {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let x = mouse.column;
            let y = mouse.row;
            if x >= app.conversations_rect.x
                && x < app.conversations_rect.x + app.conversations_rect.width
                && y >= app.conversations_rect.y
                && y < app.conversations_rect.y + app.conversations_rect.height
            {
                app.focus = Focus::Conversations;
            } else if x >= app.reader_rect.x
                && x < app.reader_rect.x + app.reader_rect.width
                && y >= app.reader_rect.y
                && y < app.reader_rect.y + app.reader_rect.height
            {
                if app.focus != Focus::Reader {
                    app.focus = Focus::Reader;
                }
            } else if x >= app.inspector_rect.x
                && x < app.inspector_rect.x + app.inspector_rect.width
                && y >= app.inspector_rect.y
                && y < app.inspector_rect.y + app.inspector_rect.height
            {
                app.focus = Focus::Inspector;
                app.refresh_inspector()?;
            } else if x >= app.input_rect.x
                && x < app.input_rect.x + app.input_rect.width
                && y >= app.input_rect.y
                && y < app.input_rect.y + app.input_rect.height
            {
                app.focus = Focus::Input;
            }
            app.refresh_status();
        }
        MouseEventKind::ScrollDown => match app.focus {
            Focus::Conversations => app.move_conversation(1)?,
            Focus::Reader | Focus::Inspector => app.move_message(1)?,
            _ => {}
        },
        MouseEventKind::ScrollUp => match app.focus {
            Focus::Conversations => app.move_conversation(-1)?,
            Focus::Reader | Focus::Inspector => app.move_message(-1)?,
            _ => {}
        },
        _ => {}
    }
    Ok(())
}
