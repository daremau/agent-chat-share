//! `acs tui` — an interactive terminal UI that drives the existing
//! `share` and `export` pipelines. Implementation split across:
//!
//! - [`mod@mod`]: entry point, terminal guard, and event loop.
//! - `app`: pure reducer + state.
//! - `events`: `crossterm` event → `AppEvent` mapping.
//! - `ui`: `ratatui` rendering.

pub mod app;
pub mod events;
pub mod ui;

use std::io::{self, IsTerminal};

use anyhow::Result;
use crossterm::cursor::Show;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::tui::app::{App, ShouldExit};

/// Restores the terminal to its prior state. Runs on `Drop`, so it covers
/// normal quit, early `?` returns, and panicking unwinds.
struct TuiGuard {
    armed: bool,
}

impl TuiGuard {
    fn new() -> Result<Self> {
        enable_raw_mode()?;
        let mut out = io::stdout();
        execute!(out, EnterAlternateScreen, EnableMouseCapture,)?;
        Ok(Self { armed: true })
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for TuiGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let mut out = io::stdout();
        let _ = execute!(out, DisableMouseCapture, LeaveAlternateScreen, Show,);
        let _ = disable_raw_mode();
    }
}

/// Entry point. Errors out cleanly when stdout is not a TTY.
pub fn run() -> Result<()> {
    if !io::stdout().is_terminal() {
        anyhow::bail!("acs tui requires an interactive terminal (stdout is not a TTY)");
    }

    let guard = TuiGuard::new()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let result = event_loop(&mut terminal, &mut app);

    guard.disarm();
    result
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
