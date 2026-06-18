//! `App` — pure state and reducer for the TUI. No terminal or async
//! dependencies; everything is `#[cfg(test)]`-friendly.

use std::path::PathBuf;

use crate::adapters::{self, Scope, SessionRef};
use crate::model::Conversation;
use crate::render::{self, RenderOptions};
use crate::share::{self, ExportFormat, ShareResult};
use crate::tui::events::AppEvent;

/// TUI screen state. The reducer transitions between these.
#[derive(Debug, Clone)]
pub enum Screen {
    /// No source agent chosen yet; sidebar lists known agents.
    PickSource,
    /// Source chosen, sidebar shows sessions.
    PickSession,
    /// A session is loaded; preview pane shows the transcript.
    Preview,
    /// `share` ran; modal shows the result.
    ShareModal(ShareResult),
    /// `export` form is open.
    ExportModal {
        agent: String,
        session: Option<String>,
        format: ExportFormat,
        out: PathBuf,
    },
    /// Help overlay.
    Help,
}

/// Reducer exit signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShouldExit {
    No,
    Yes,
}

#[derive(Debug, Clone)]
pub struct App {
    pub source: String,
    pub target: String,
    pub sessions: Vec<SessionRef>,
    pub cursor: usize,
    pub selected_session: Option<String>,
    pub transcript: Option<String>,
    pub transcript_chars: usize,
    pub screen: Screen,
    pub last_error: Option<String>,
}

impl App {
    pub fn new() -> Self {
        let source = adapters::KNOWN_AGENTS
            .first()
            .copied()
            .unwrap_or("claude")
            .to_string();
        let target = adapters::KNOWN_AGENTS
            .get(1)
            .copied()
            .unwrap_or("codex")
            .to_string();
        let mut app = Self {
            source,
            target,
            sessions: Vec::new(),
            cursor: 0,
            selected_session: None,
            transcript: None,
            transcript_chars: 0,
            screen: Screen::PickSource,
            last_error: None,
        };
        app.load_sessions();
        app
    }

    pub fn status_message(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn selected_session_summary(&self) -> Option<&SessionRef> {
        self.sessions.get(self.cursor)
    }

    /// Discover sessions for the current source agent. Errors are stored in
    /// `last_error` and the session list is cleared; the TUI keeps running.
    pub fn load_sessions(&mut self) {
        self.sessions.clear();
        self.cursor = 0;
        match adapters::get(&self.source) {
            Ok(adapter) => match adapter.discover(&Scope::default()) {
                Ok(s) => self.sessions = s,
                Err(e) => self.last_error = Some(format!("discover failed: {e}")),
            },
            Err(e) => self.last_error = Some(format!("adapter failed: {e}")),
        }
    }

    /// Load the currently highlighted session into the preview pane.
    pub fn load_preview(&mut self) {
        let Some(session) = self.selected_session_summary().cloned() else {
            return;
        };
        self.selected_session = Some(session.id.clone());
        match adapters::get(&self.source) {
            Ok(adapter) => match adapter.read(Some(&session.id), &Scope::default()) {
                Ok(conv) => {
                    let text = render::render(&conv, &RenderOptions::default());
                    self.transcript_chars = text.chars().count();
                    self.transcript = Some(text);
                    self.screen = Screen::Preview;
                    self.last_error = None;
                }
                Err(e) => self.last_error = Some(format!("read failed: {e}")),
            },
            Err(e) => self.last_error = Some(format!("adapter failed: {e}")),
        }
    }

    /// Run the shared `share::run` against the current selection and source/target.
    pub fn run_share(&mut self) {
        let Some(session_id) = self.selected_session.clone() else {
            self.last_error = Some("no session selected".to_string());
            return;
        };
        match share::run(&self.source, &self.target, Some(&session_id), None) {
            Ok(result) => {
                self.screen = Screen::ShareModal(result);
                self.last_error = None;
            }
            Err(e) => self.last_error = Some(format!("share failed: {e:#}")),
        }
    }

    /// Build the export modal with sensible defaults.
    pub fn open_export_modal(&mut self) {
        let Some(session_id) = self.selected_session.clone() else {
            self.last_error = Some("no session selected".to_string());
            return;
        };
        let out = PathBuf::from(format!("{}-transcript.md", session_id));
        self.screen = Screen::ExportModal {
            agent: self.source.clone(),
            session: Some(session_id),
            format: ExportFormat::Transcript,
            out,
        };
    }

    pub fn confirm_export(&mut self) {
        let Screen::ExportModal {
            agent,
            session,
            format,
            out,
        } = self.screen.clone()
        else {
            return;
        };
        let result = share::export(&agent, session.as_deref(), format, Some(out.clone()));
        match result {
            Ok(Some(path)) => {
                self.last_error = Some(format!("wrote {}", path.display()));
                self.screen = Screen::Preview;
            }
            Ok(None) => {
                self.screen = Screen::Preview;
            }
            Err(e) => self.last_error = Some(format!("export failed: {e:#}")),
        }
    }

    /// Pure reducer. Returns whether the event loop should exit.
    pub fn update(&mut self, ev: AppEvent) -> ShouldExit {
        match ev {
            AppEvent::Quit => return ShouldExit::Yes,
            AppEvent::CycleSourceNext => {
                self.cycle_source(true);
                self.load_sessions();
            }
            AppEvent::CycleSourcePrev => {
                self.cycle_source(false);
                self.load_sessions();
            }
            AppEvent::OpenTargetPicker => {
                // Surface the popover by stepping into Help-style transient
                // screen. Concrete popover is rendered in the UI layer; for
                // now we just toggle target through cycle logic.
                let idx = adapters::KNOWN_AGENTS
                    .iter()
                    .position(|a| *a == self.target)
                    .unwrap_or(0);
                let n = adapters::KNOWN_AGENTS.len();
                self.target = adapters::KNOWN_AGENTS[(idx + 1) % n].to_string();
            }
            AppEvent::MoveCursor(delta) => {
                if !self.sessions.is_empty() {
                    let len = self.sessions.len();
                    let cur = (self.cursor as i64 + delta).rem_euclid(len as i64) as usize;
                    self.cursor = cur;
                }
            }
            AppEvent::SelectSession => {
                self.load_preview();
            }
            AppEvent::Reload => {
                self.load_sessions();
            }
            AppEvent::Share => {
                self.run_share();
            }
            AppEvent::Export => {
                self.open_export_modal();
            }
            AppEvent::ConfirmExport => self.confirm_export(),
            AppEvent::DismissModal => {
                if matches!(
                    self.screen,
                    Screen::ShareModal(_) | Screen::ExportModal { .. }
                ) {
                    self.screen = Screen::Preview;
                }
            }
            AppEvent::ToggleHelp => {
                self.screen = match self.screen {
                    Screen::Help => Screen::Preview,
                    _ => Screen::Help,
                };
            }
            AppEvent::ClearError => {
                self.last_error = None;
            }
        }
        ShouldExit::No
    }

    fn cycle_source(&mut self, forward: bool) {
        let idx = adapters::KNOWN_AGENTS
            .iter()
            .position(|a| *a == self.source)
            .unwrap_or(0);
        let n = adapters::KNOWN_AGENTS.len();
        let next = if forward {
            (idx + 1) % n
        } else {
            (idx + n - 1) % n
        };
        self.source = adapters::KNOWN_AGENTS[next].to_string();
        self.transcript = None;
        self.transcript_chars = 0;
        self.selected_session = None;
        self.screen = Screen::PickSession;
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)]
fn _conv_marker() -> Option<Conversation> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_app() -> App {
        App::new()
    }

    #[test]
    fn quit_exits() {
        let mut app = new_app();
        assert_eq!(app.update(AppEvent::Quit), ShouldExit::Yes);
    }

    #[test]
    fn cycle_source_advances_and_wraps() {
        let mut app = new_app();
        let start = app.source.clone();
        app.update(AppEvent::CycleSourceNext);
        assert_ne!(app.source, start);
        // Walk the rest of the ring (we already took one step).
        for _ in 1..adapters::KNOWN_AGENTS.len() {
            app.update(AppEvent::CycleSourceNext);
        }
        assert_eq!(app.source, start);
    }

    #[test]
    fn cycle_source_prev_wraps_backwards() {
        let mut app = new_app();
        let start = app.source.clone();
        app.update(AppEvent::CycleSourcePrev);
        assert_ne!(app.source, start);
        for _ in 1..adapters::KNOWN_AGENTS.len() {
            app.update(AppEvent::CycleSourcePrev);
        }
        assert_eq!(app.source, start);
    }

    #[test]
    fn move_cursor_with_empty_sessions_is_noop() {
        let mut app = new_app();
        app.sessions.clear();
        let cur = app.cursor;
        app.update(AppEvent::MoveCursor(1));
        assert_eq!(app.cursor, cur);
    }

    #[test]
    fn move_cursor_wraps_in_synthetic_list() {
        let mut app = new_app();
        app.sessions = vec![
            SessionRef {
                id: "a".into(),
                summary: "a".into(),
                updated_at: chrono::Utc::now(),
                message_count: 1,
            },
            SessionRef {
                id: "b".into(),
                summary: "b".into(),
                updated_at: chrono::Utc::now(),
                message_count: 1,
            },
        ];
        app.cursor = 1;
        app.update(AppEvent::MoveCursor(1));
        assert_eq!(app.cursor, 0);
    }

    #[test]
    fn toggle_help_flips_screen() {
        let mut app = new_app();
        app.screen = Screen::Preview;
        app.update(AppEvent::ToggleHelp);
        assert!(matches!(app.screen, Screen::Help));
        app.update(AppEvent::ToggleHelp);
        assert!(matches!(app.screen, Screen::Preview));
    }

    #[test]
    fn share_without_session_sets_error() {
        let mut app = new_app();
        app.selected_session = None;
        app.update(AppEvent::Share);
        assert!(app.last_error.is_some());
    }

    #[test]
    fn export_without_session_sets_error() {
        let mut app = new_app();
        app.selected_session = None;
        app.update(AppEvent::Export);
        assert!(app.last_error.is_some());
    }

    #[test]
    fn dismiss_modal_returns_to_preview() {
        let mut app = new_app();
        app.screen = Screen::ShareModal(ShareResult {
            transcript_path: PathBuf::from("x"),
            seed_shell: "echo".into(),
            message_count: 1,
        });
        app.update(AppEvent::DismissModal);
        assert!(matches!(app.screen, Screen::Preview));
    }

    #[test]
    fn clear_error_clears_message() {
        let mut app = new_app();
        app.last_error = Some("boom".into());
        app.update(AppEvent::ClearError);
        assert!(app.last_error.is_none());
    }
}
