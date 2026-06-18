//! `acs tui` — an interactive terminal UI that drives the existing
//! `share` and `export` pipelines. Implementation split across:
//!
//! - [`mod@mod`]: entry point, terminal guard, and event loop.
//! - `app`: pure reducer + state.
//! - `events`: `crossterm` event → `AppEvent` mapping.
//! - `ui`: `ratatui` rendering.

pub mod app;
pub mod clipboard;
pub mod events;
pub mod ui;

use std::io::{self, IsTerminal};

use anyhow::Result;
use crossterm::cursor::Show;
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::tui::app::{App, ShouldExit};

/// Restores the terminal to its prior state on `Drop`, so it covers every
/// exit path: normal quit, early `?` returns, and panicking unwinds.
struct TuiGuard;

impl TuiGuard {
    fn new() -> Result<Self> {
        enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, EnterAlternateScreen)?;
        Ok(Self)
    }
}

impl Drop for TuiGuard {
    fn drop(&mut self) {
        let mut out = io::stdout();
        let _ = execute!(out, LeaveAlternateScreen, Show);
        let _ = disable_raw_mode();
    }
}

/// Entry point. Errors out cleanly when stdout is not a TTY.
pub fn run() -> Result<()> {
    if !io::stdout().is_terminal() {
        anyhow::bail!("acs tui requires an interactive terminal (stdout is not a TTY)");
    }

    let _guard = TuiGuard::new()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    // `_guard` restores the terminal when it drops at the end of this scope,
    // regardless of whether the loop returns Ok or an error.
    event_loop(&mut terminal, &mut app)
}

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    use std::time::Duration;

    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        if crossterm::event::poll(Duration::from_millis(100))? {
            let event = crossterm::event::read()?;
            if let Some(ev) = events::map(event) {
                if matches!(app.update(ev), ShouldExit::Yes) {
                    return Ok(());
                }
            }
        }
    }
}
