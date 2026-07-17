use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use crate::tui::state::{Overlay, PAGE_SIZE};
use crate::tui::text::first_line;

pub fn render_overlay(frame: &mut Frame, overlay: &Overlay, area: Rect) {
    frame.render_widget(Clear, area);
    match overlay {
        Overlay::Branches { choices, selected } => {
            let items = choices
                .iter()
                .map(|choice| {
                    let ic = choice.ic.map(|value| format!("[IC:{value}] ")).unwrap_or_default();
                    ListItem::new(format!("{ic}{} · {}", choice.role.as_deref().unwrap_or("node"), first_line(&choice.preview, 48)))
                })
                .collect::<Vec<_>>();
            let mut state = ListState::default();
            state.select(Some(*selected));
            frame.render_stateful_widget(
                List::new(items)
                    .block(Block::default().title("Branch alternatives").borders(Borders::ALL))
                    .highlight_style(Style::default().bg(Color::Blue)),
                area,
                &mut state,
            );
        }
        Overlay::Context { lines } => frame.render_widget(
            Paragraph::new(lines.join("\n"))
                .block(Block::default().title("IC context · Esc to close").borders(Borders::ALL))
                .wrap(Wrap { trim: true }),
            area,
        ),
        Overlay::SearchResults {
            query,
            hits,
            selected,
        } => {
            if hits.is_empty() {
                frame.render_widget(
                    Paragraph::new(format!("No results for “{query}”\n\nEsc to close"))
                        .block(Block::default().title("Search results").borders(Borders::ALL)),
                    area,
                );
                return;
            }
            let items = hits
                .iter()
                .map(|hit| {
                    let ic = hit
                        .ic
                        .map(|value| format!("[IC:{value}] "))
                        .unwrap_or_default();
                    ListItem::new(vec![
                        Line::from(format!(
                            "{ic}{} · {}",
                            hit.role.to_uppercase(),
                            hit.conversation_title
                        )),
                        Line::from(first_line(&hit.excerpt, 56))
                            .style(Style::default().fg(Color::DarkGray)),
                    ])
                })
                .collect::<Vec<_>>();
            let capped = hits.len() >= PAGE_SIZE;
            let overlay_title = if capped {
                format!("Search · “{query}” · {} shown · more may exist", hits.len())
            } else {
                format!("Search · “{query}” · Enter open")
            };
            let mut state = ListState::default();
            state.select(Some(*selected));
            frame.render_stateful_widget(
                List::new(items)
                    .block(Block::default().title(overlay_title).borders(Borders::ALL))
                    .highlight_style(Style::default().bg(Color::Blue)),
                area,
                &mut state,
            );
        }
        Overlay::Help => frame.render_widget(
            Paragraph::new(
                "RecallEngine browse — local mail-like reader\n\n\
                 Left: conversation titles only — j/k schedules loading after 180 ms\n\
                 Thread mode: root → export current_node branch; search/IC/branch switches active branch\n\
                 All messages: ascending IC order, capped at 500 when larger\n\n\
                 /  search overlay (never replaces conversation list)\n\
                 i  jump by IC, message ID, or composite reference\n\
                 Enter  load immediately when pending, then focus reader\n\
                 v  Thread / All messages\n\
                 b  branch alternatives (when shown on selected message)\n\
                 t  show/hide technical messages (grouped when hidden)\n\
                 y  copy [IC:n | msg:id] — status confirms or preserves reference on failure\n\
                 c  IC neighborhood context\n\
                 Tab  Conversations → Reader → Inspector\n\
                 Esc  resync sidebar after IC/search/branch jump\n\
                 ●  conversation has branches\n\
                 q  quit\n\n\
                 Layout medium (80–119 cols): Conversations + Reader, or Conversations + Inspector\n\
                 Layout compact (<80 cols): one pane at a time via Tab\n\n\
                 Esc or ? closes overlays",
            )
            .block(Block::default().title("RecallEngine browse help").borders(Borders::ALL))
            .wrap(Wrap { trim: true }),
            area,
        ),
    }
}

pub fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}
