use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::domain::reference::MessageReference;
use crate::tui::overlays::{centered_rect, render_overlay};
use crate::tui::state::{App, Focus};
use crate::tui::text::{asset_status_label, truncate, wrap_content};

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < 60 || area.height < 15 {
        frame.render_widget(
            Paragraph::new(
                "Terminal too small
Resize to at least 60 × 15, then press q to quit.",
            )
            .block(
                Block::default()
                    .title("RecallEngine browse")
                    .borders(Borders::ALL),
            )
            .alignment(ratatui::layout::Alignment::Center),
            area,
        );
        return;
    }

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    let main_area = layout[0];
    let input_area = layout[1];
    let status_area = layout[2];

    app.conversations_rect = Rect::default();
    app.reader_rect = Rect::default();
    app.inspector_rect = Rect::default();
    app.input_rect = input_area;

    if area.width < 80 {
        match app.focus {
            Focus::Conversations => {
                app.conversations_rect = main_area;
                render_conversations(frame, app, main_area);
            }
            Focus::Reader => {
                app.reader_rect = main_area;
                render_reader(frame, app, main_area);
            }
            Focus::Inspector => {
                app.inspector_rect = main_area;
                render_inspector(frame, app, main_area);
            }
            Focus::Input => {}
        }
    } else {
        let main_columns = if area.width >= 120 {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(28),
                    Constraint::Percentage(52),
                    Constraint::Percentage(20),
                ])
                .split(main_area)
        } else {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(36), Constraint::Percentage(64)])
                .split(main_area)
        };

        app.conversations_rect = main_columns[0];
        render_conversations(frame, app, main_columns[0]);

        if main_columns.len() == 2 && app.focus == Focus::Inspector {
            app.inspector_rect = main_columns[1];
            render_inspector(frame, app, main_columns[1]);
        } else {
            app.reader_rect = main_columns[1];
            render_reader(frame, app, main_columns[1]);
        }
        if main_columns.len() == 3 {
            app.inspector_rect = main_columns[2];
            render_inspector(frame, app, main_columns[2]);
        }
    }

    let input_title = match app.input_mode {
        crate::tui::state::InputMode::Search => "Search (Enter to submit, Esc to cancel)",
        crate::tui::state::InputMode::Jump => "Jump (Enter to submit, Esc to cancel)",
    };

    let mut block = Block::default().borders(Borders::ALL).title(input_title);
    if app.focus == Focus::Input {
        block = block.border_style(Style::default().fg(Color::Cyan));
    }
    app.search_input.set_block(block);
    frame.render_widget(&app.search_input, input_area);

    frame.render_widget(
        Paragraph::new(status_line(app)).style(Style::default().fg(Color::DarkGray)),
        status_area,
    );
    if let Some(overlay) = &app.overlay {
        render_overlay(frame, overlay, centered_rect(72, 60, area));
    }
}

pub fn status_line(app: &App) -> String {
    format!(
        "[{}] {}  ·  / search  i IC  v mode  b branches  t technical  y copy  ? help  q quit",
        app.focus.label(),
        app.status
    )
}

pub fn pane_block(title: String, focused: bool) -> Block<'static> {
    let mut block = Block::default().title(title).borders(Borders::ALL);
    if focused {
        block = block.border_style(Style::default().fg(Color::Cyan));
    }
    block
}

pub fn render_conversations(frame: &mut Frame, app: &App, area: Rect) {
    let title = app.conversations_panel_title();
    let title_width = usize::from(area.width.saturating_sub(4).max(12));
    let items: Vec<ListItem> = app
        .conversations
        .iter()
        .map(|conversation| {
            let label = if conversation.title.trim().is_empty() {
                "Untitled".to_string()
            } else {
                truncate(&conversation.title, title_width)
            };
            let line = if conversation.has_branches {
                Line::from(vec![
                    Span::styled("● ", Style::default().fg(Color::Yellow)),
                    Span::raw(label),
                ])
            } else {
                Line::from(vec![Span::raw(format!("  {label}"))])
            };
            ListItem::new(line).style(Style::default().fg(Color::DarkGray))
        })
        .collect();
    let list = List::new(items)
        .block(pane_block(title, app.focus == Focus::Conversations))
        .highlight_style(
            Style::default()
                .fg(Color::White)
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut state = app.conversations_state;
    frame.render_stateful_widget(list, area, &mut state);
}

pub fn render_reader(frame: &mut Frame, app: &mut App, area: Rect) {
    app.reader_wrap_width = area.width;
    let wrap_width = usize::from(area.width.saturating_sub(4).max(20));
    let title = app.reader_panel_title();
    let lines = if app.loading_conversation && app.messages.is_empty() {
        vec![Line::from("Loading conversation…")]
    } else {
        app.reader_lines(wrap_width)
    };
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(pane_block(title, app.focus == Focus::Reader))
            .scroll((app.reader_scroll, 0))
            .wrap(Wrap { trim: false }),
        area,
    );
}

pub fn build_reader_lines(app: &App, wrap_width: usize) -> Vec<Line<'static>> {
    if app.messages.is_empty() {
        return vec![Line::from("No messages to display")];
    }
    let mut lines = Vec::new();
    let mut index = 0;
    while index < app.messages.len() {
        if !app.technical_visible && app.messages[index].is_technical() {
            let start = index;
            while index < app.messages.len() && app.messages[index].is_technical() {
                index += 1;
            }
            let count = index - start;
            let selected = (start..index).contains(&app.message_selected);
            let prefix = if selected { ">" } else { " " };
            let label = if count == 1 {
                "1 technical message hidden · press t".to_string()
            } else {
                format!("{count} technical messages hidden · press t")
            };
            lines.push(Line::from(format!("{prefix} ▸ {label}")));
            continue;
        }
        lines.extend(message_lines(app, index, wrap_width));
        index += 1;
    }
    lines
}

pub fn message_lines(app: &App, index: usize, wrap_width: usize) -> Vec<Line<'static>> {
    let message = &app.messages[index];
    let selected = index == app.message_selected;
    let prefix = if selected { ">" } else { " " };
    let ic = message
        .ic
        .map(|value| format!("[IC:{value}] "))
        .unwrap_or_default();
    let branch_hint = if selected {
        app.branch_alternatives
            .map(|count| format!(" · {count} alternatives · press b"))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let heading = Line::from(vec![
        Span::styled(prefix, Style::default().fg(Color::Yellow)),
        Span::styled(
            format!(" {ic}{}", message.role.to_uppercase()),
            if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Green)
            },
        ),
        Span::raw(format!(
            " · {}{}",
            message.timestamp.as_deref().unwrap_or("unknown date"),
            branch_hint
        )),
    ]);
    let mut block = vec![heading];
    block.extend(
        wrap_content(&message.content, wrap_width)
            .into_iter()
            .map(Line::from),
    );
    block.push(Line::from(""));
    block
}

pub fn render_inspector(frame: &mut Frame, app: &App, area: Rect) {
    let lines = if let Some(message) = app.selected_message() {
        let stable_reference = message
            .ic
            .and_then(|ic| MessageReference::new(ic, message.id.clone()).ok())
            .map(|reference| reference.human())
            .unwrap_or_else(|| "none (technical role)".into());
        let mut lines = vec![
            Line::from(format!("UUID: {}", truncate(&message.id, 24))),
            Line::from(format!("Stable reference: {stable_reference}")),
            Line::from(format!("Role: {}", message.role)),
            Line::from(format!("Node: {}", truncate(&message.node_id, 24))),
            Line::from(format!(
                "Parent: {}",
                message
                    .parent_node_id
                    .as_deref()
                    .map(|id| truncate(id, 20))
                    .unwrap_or_else(|| "root".into())
            )),
        ];
        if !app.assets.is_empty() {
            lines.push(Line::from("Assets:"));
            lines.extend(app.assets.iter().map(|asset| {
                Line::from(format!(
                    "{} {}",
                    asset_status_label(asset),
                    truncate(&asset.name, 22)
                ))
            }));
        }
        lines
    } else {
        vec![Line::from("Select a message to inspect")]
    };
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(pane_block(
                "Inspector".into(),
                app.focus == Focus::Inspector,
            ))
            .wrap(Wrap { trim: true }),
        area,
    );
}
