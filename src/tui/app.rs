use std::{
    io::{self, stdout},
    path::PathBuf,
    time::Duration,
};

use ratatui::crossterm::{
    event::{self, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::{read_model::ReadRepository, Result};

use super::event::handle_key;
use super::render::render;
use super::state::App;

pub fn run(db_path: PathBuf, ic: Option<i64>, conversation: Option<String>) -> Result<()> {
    let repository = ReadRepository::open_read_only(&db_path)?;
    let mut app = App::new(repository)?;
    if let Some(ic) = ic {
        app.open_ic(ic)?;
    } else if let Some(conversation) = conversation {
        app.load_conversation(&conversation, true)?;
    }
    run_terminal(&mut app)
}
pub fn run_terminal(app: &mut App) -> Result<()> {
    enable_raw_mode()?;
    let mut out = stdout();
    if let Err(error) = execute!(
        out,
        EnterAlternateScreen,
        ratatui::crossterm::event::EnableMouseCapture,
        ratatui::crossterm::cursor::Hide
    ) {
        let _ = disable_raw_mode();
        return Err(error.into());
    }
    let backend = CrosstermBackend::new(out);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = disable_raw_mode();
            let mut out = stdout();
            let _ = execute!(out, LeaveAlternateScreen, ratatui::crossterm::cursor::Show);
            return Err(error.into());
        }
    };
    let outcome = run_loop(&mut terminal, app);
    let restore = restore_terminal(&mut terminal);
    outcome.and(restore)
}

pub fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        ratatui::crossterm::event::DisableMouseCapture,
        ratatui::crossterm::cursor::Show
    )?;
    terminal.show_cursor()?;
    Ok(())
}

pub fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    while !app.should_quit {
        app.flush_pending_conversation_load()?;
        terminal.draw(|frame| render(frame, app))?;
        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.kind == event::KeyEventKind::Press {
                        handle_key(app, key)?;
                    }
                }
                Event::Mouse(mouse) => {
                    crate::tui::event::handle_mouse(app, mouse)?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}
