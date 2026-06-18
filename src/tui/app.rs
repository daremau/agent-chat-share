//! `App` — pure state and reducer for the TUI. No terminal or async
//! dependencies; everything is `#[cfg(test)]`-friendly.

use std::path::PathBuf;

use crate::adapters::{self, Scope, SessionRef};
use crate::model::Conversation;
use crate::render::{self, RenderOptions};
use crate::share::{self, ExportFormat, ShareResult};
use crate::tui::clipboard;
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

/// Which pane the vertical-nav keys (↑/↓, j/k) act on. `Tab` toggles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// Session list: vertical nav moves the selection cursor.
    Sessions,
    /// Transcript preview: vertical nav scrolls the text.
    Transcript,
}

/// Lines moved per `Ctrl-U`/`Ctrl-D` fast scroll.
const FAST_SCROLL_LINES: i64 = 15;

/// Strip ANSI escape sequences and control characters from text before it is
/// handed to the renderer. Session transcripts can embed raw escape sequences
/// (e.g. ANSI-colored tool output). If ratatui emits those verbatim the
/// terminal executes them, moving the cursor and leaving "ghost" glyphs that
/// never get repainted. Newlines are preserved; tabs become single spaces;
/// every other C0/C1 control byte and DEL is dropped.
pub(crate) fn sanitize_for_display(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.peek() {
                // CSI: ESC [ … final byte in 0x40–0x7e (e.g. color codes).
                Some('[') => {
                    chars.next();
                    while let Some(&n) = chars.peek() {
                        chars.next();
                        if ('\u{40}'..='\u{7e}').contains(&n) {
                            break;
                        }
                    }
                }
                // OSC: ESC ] … terminated by BEL or ST (ESC \).
                Some(']') => {
                    chars.next();
                    while let Some(&n) = chars.peek() {
                        if n == '\u{7}' {
                            chars.next();
                            break;
                        }
                        if n == '\u{1b}' {
                            chars.next();
                            if matches!(chars.peek(), Some('\\')) {
                                chars.next();
                            }
                            break;
                        }
                        chars.next();
                    }
                }
                // Lone ESC or other escape: drop just the ESC.
                _ => {}
            },
            '\n' => out.push('\n'),
            '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
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
    /// Number of lines in the loaded transcript; used to clamp `scroll`.
    pub transcript_lines: u16,
    /// Vertical scroll offset (in lines) of the transcript preview.
    pub scroll: u16,
    pub focus: Focus,
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
            transcript_lines: 0,
            scroll: 0,
            focus: Focus::Sessions,
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
        // Auto-preview the first session so the transcript is visible without
        // an explicit Enter.
        if !self.sessions.is_empty() {
            self.load_preview();
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
                    let text =
                        sanitize_for_display(&render::render(&conv, &RenderOptions::default()));
                    self.transcript_chars = text.chars().count();
                    self.transcript_lines =
                        text.lines().count().min(u16::MAX as usize) as u16;
                    self.transcript = Some(text);
                    self.scroll = 0;
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
            AppEvent::CycleTarget => {
                let idx = adapters::KNOWN_AGENTS
                    .iter()
                    .position(|a| *a == self.target)
                    .unwrap_or(0);
                let n = adapters::KNOWN_AGENTS.len();
                self.target = adapters::KNOWN_AGENTS[(idx + 1) % n].to_string();
            }
            AppEvent::ToggleFocus => {
                self.focus = match self.focus {
                    Focus::Sessions => Focus::Transcript,
                    Focus::Transcript => Focus::Sessions,
                };
            }
            // Vertical nav acts on whichever pane has focus.
            AppEvent::NavVertical(delta) => match self.focus {
                Focus::Sessions => self.move_cursor(delta),
                Focus::Transcript => self.scroll_by(delta),
            },
            // Fast scroll always targets the transcript, regardless of focus.
            AppEvent::ScrollFast(direction) => {
                self.scroll_by(direction * FAST_SCROLL_LINES);
            }
            AppEvent::SelectSession => {
                // Enter is context-sensitive: in the export modal it confirms
                // the write; otherwise it selects the highlighted session.
                if matches!(self.screen, Screen::ExportModal { .. }) {
                    self.confirm_export();
                } else {
                    self.load_preview();
                    // Selecting a session is a "now let me read it" gesture, so
                    // hand focus to the transcript for immediate scrolling.
                    self.focus = Focus::Transcript;
                }
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
            AppEvent::CopySeed => self.copy_active_command(),
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

    /// Copy the active modal's payload to the clipboard: the seed command in
    /// a `ShareModal`, or the output path in an `ExportModal`. Does nothing on
    /// other screens. Success and failure are both surfaced in the status line.
    fn copy_active_command(&mut self) {
        let text = match &self.screen {
            Screen::ShareModal(result) => result.seed_shell.clone(),
            Screen::ExportModal { out, .. } => out.display().to_string(),
            _ => return,
        };
        match clipboard::copy(&text) {
            Ok(tool) => self.last_error = Some(format!("copied to clipboard ({tool})")),
            Err(e) => self.last_error = Some(format!("copy failed: {e}")),
        }
    }

    /// Move the session selection cursor and auto-preview the new session.
    fn move_cursor(&mut self, delta: i64) {
        if self.sessions.is_empty() {
            return;
        }
        let len = self.sessions.len();
        self.cursor = (self.cursor as i64 + delta).rem_euclid(len as i64) as usize;
        // Show the newly highlighted session's transcript right away.
        self.load_preview();
    }

    /// Scroll the transcript preview, clamped to `[0, transcript_lines]`.
    fn scroll_by(&mut self, delta: i64) {
        let max = self.transcript_lines as i64;
        self.scroll = (self.scroll as i64 + delta).clamp(0, max) as u16;
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
        self.transcript_lines = 0;
        self.scroll = 0;
        self.selected_session = None;
        self.focus = Focus::Sessions;
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
    fn nav_with_empty_sessions_is_noop() {
        let mut app = new_app();
        app.sessions.clear();
        app.focus = Focus::Sessions;
        let cur = app.cursor;
        app.update(AppEvent::NavVertical(1));
        assert_eq!(app.cursor, cur);
    }

    #[test]
    fn nav_wraps_cursor_when_sessions_focused() {
        let mut app = new_app();
        app.focus = Focus::Sessions;
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
        app.update(AppEvent::NavVertical(1));
        assert_eq!(app.cursor, 0);
    }

    #[test]
    fn nav_scrolls_transcript_when_focused() {
        let mut app = new_app();
        app.focus = Focus::Transcript;
        app.transcript_lines = 100;
        app.cursor = 0;
        app.update(AppEvent::NavVertical(1));
        assert_eq!(app.scroll, 1);
        // Cursor must not move while the transcript is focused.
        assert_eq!(app.cursor, 0);
    }

    #[test]
    fn scroll_clamps_to_bounds() {
        let mut app = new_app();
        app.focus = Focus::Transcript;
        app.transcript_lines = 5;
        app.update(AppEvent::ScrollFast(1)); // would overshoot
        assert_eq!(app.scroll, 5);
        app.update(AppEvent::ScrollFast(-1)); // would underflow past 0
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn toggle_focus_flips_pane() {
        let mut app = new_app();
        app.focus = Focus::Sessions;
        app.update(AppEvent::ToggleFocus);
        assert_eq!(app.focus, Focus::Transcript);
        app.update(AppEvent::ToggleFocus);
        assert_eq!(app.focus, Focus::Sessions);
    }

    #[test]
    fn cycle_target_advances() {
        let mut app = new_app();
        let start = app.target.clone();
        app.update(AppEvent::CycleTarget);
        if adapters::KNOWN_AGENTS.len() > 1 {
            assert_ne!(app.target, start);
        }
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
    fn copy_seed_outside_modal_is_noop() {
        // No modal open: copy must not spawn anything or set a status.
        let mut app = new_app();
        app.screen = Screen::Preview;
        app.last_error = None;
        app.update(AppEvent::CopySeed);
        assert!(app.last_error.is_none());
    }

    #[test]
    fn sanitize_strips_ansi_and_controls() {
        // Color codes, a cursor move, a NUL, and a tab.
        let raw = "a\u{1b}[31mred\u{1b}[0m\tb\u{1b}[2Jc\0d";
        assert_eq!(sanitize_for_display(raw), "ared bcd");
    }

    #[test]
    fn sanitize_keeps_newlines_and_plain_text() {
        let raw = "line one\nline two";
        assert_eq!(sanitize_for_display(raw), "line one\nline two");
    }

    #[test]
    fn sanitize_drops_osc_sequence() {
        // OSC set-title terminated by BEL must vanish, surrounding text stays.
        let raw = "x\u{1b}]0;title\u{7}y";
        assert_eq!(sanitize_for_display(raw), "xy");
    }

    #[test]
    fn clear_error_clears_message() {
        let mut app = new_app();
        app.last_error = Some("boom".into());
        app.update(AppEvent::ClearError);
        assert!(app.last_error.is_none());
    }
}
