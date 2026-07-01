//! Map raw `crossterm` events into the TUI's pure `AppEvent` enum.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

/// Lines the transcript scrolls per mouse-wheel / trackpad notch.
const WHEEL_LINES: i64 = 3;

/// Pure events the reducer consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEvent {
    Quit,
    CycleSourceNext,
    CycleSourcePrev,
    /// Cycle the target agent (bound to Space).
    CycleTarget,
    /// Switch focus between the session list and the transcript (bound to Tab).
    ToggleFocus,
    /// Vertical nav: moves the session cursor or scrolls the transcript,
    /// depending on which pane has focus. Positive is down.
    NavVertical(i64),
    /// Fast scroll the transcript; `1` is down, `-1` is up (Ctrl-D/Ctrl-U).
    ScrollFast(i64),
    /// Mouse-wheel / trackpad scroll of the transcript, in lines. Always
    /// targets the transcript regardless of focus. Positive is down.
    ScrollWheel(i64),
    SelectSession,
    Reload,
    Share,
    Export,
    ConfirmExport,
    /// Copy the active modal's command/path to the system clipboard.
    CopySeed,
    DismissModal,
    ToggleHelp,
    ClearError,
}

/// Translate a single `crossterm` event into an `AppEvent`, or `None` to
/// ignore it (resize, mouse motion, etc.).
pub fn map(event: Event) -> Option<AppEvent> {
    match event {
        Event::Key(k) => key(k),
        Event::Mouse(m) => mouse(m),
        _ => None,
    }
}

/// Map wheel/trackpad scrolling to transcript scroll. Other mouse events
/// (motion, clicks) are ignored; capturing the mouse is only to stop the
/// terminal from injecting arrow keys for scroll.
fn mouse(m: MouseEvent) -> Option<AppEvent> {
    match m.kind {
        MouseEventKind::ScrollDown => Some(AppEvent::ScrollWheel(WHEEL_LINES)),
        MouseEventKind::ScrollUp => Some(AppEvent::ScrollWheel(-WHEEL_LINES)),
        _ => None,
    }
}

fn key(k: KeyEvent) -> Option<AppEvent> {
    // Ctrl chords take priority and never fall through to the plain-key table.
    if k.modifiers.contains(KeyModifiers::CONTROL) {
        return match k.code {
            KeyCode::Char('c') => Some(AppEvent::Quit),
            KeyCode::Char('d') => Some(AppEvent::ScrollFast(1)),
            KeyCode::Char('u') => Some(AppEvent::ScrollFast(-1)),
            _ => None,
        };
    }
    match k.code {
        KeyCode::Char('q') => Some(AppEvent::Quit),
        KeyCode::Char('?') => Some(AppEvent::ToggleHelp),
        KeyCode::Char('r') => Some(AppEvent::Reload),
        KeyCode::Char('s') => Some(AppEvent::Share),
        KeyCode::Char('e') => Some(AppEvent::Export),
        KeyCode::Char('c') => Some(AppEvent::CopySeed),
        KeyCode::Char('j') | KeyCode::Down => Some(AppEvent::NavVertical(1)),
        KeyCode::Char('k') | KeyCode::Up => Some(AppEvent::NavVertical(-1)),
        KeyCode::Left => Some(AppEvent::CycleSourcePrev),
        KeyCode::Right => Some(AppEvent::CycleSourceNext),
        KeyCode::Tab => Some(AppEvent::ToggleFocus),
        KeyCode::Char(' ') => Some(AppEvent::CycleTarget),
        KeyCode::Enter => Some(AppEvent::SelectSession),
        KeyCode::Esc => Some(AppEvent::DismissModal),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    #[test]
    fn q_is_quit() {
        assert_eq!(map(k(KeyCode::Char('q'))), Some(AppEvent::Quit));
    }

    #[test]
    fn ctrl_c_is_quit() {
        let ev = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(map(ev), Some(AppEvent::Quit));
    }

    #[test]
    fn left_right_cycle_source() {
        assert_eq!(map(k(KeyCode::Left)), Some(AppEvent::CycleSourcePrev));
        assert_eq!(map(k(KeyCode::Right)), Some(AppEvent::CycleSourceNext));
    }

    #[test]
    fn tab_toggles_focus() {
        assert_eq!(map(k(KeyCode::Tab)), Some(AppEvent::ToggleFocus));
    }

    #[test]
    fn space_cycles_target() {
        assert_eq!(map(k(KeyCode::Char(' '))), Some(AppEvent::CycleTarget));
    }

    #[test]
    fn arrows_and_jk_nav_vertical() {
        assert_eq!(map(k(KeyCode::Down)), Some(AppEvent::NavVertical(1)));
        assert_eq!(map(k(KeyCode::Up)), Some(AppEvent::NavVertical(-1)));
        assert_eq!(map(k(KeyCode::Char('j'))), Some(AppEvent::NavVertical(1)));
        assert_eq!(map(k(KeyCode::Char('k'))), Some(AppEvent::NavVertical(-1)));
    }

    #[test]
    fn ctrl_du_fast_scroll() {
        let ctrl_d = Event::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        let ctrl_u = Event::Key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert_eq!(map(ctrl_d), Some(AppEvent::ScrollFast(1)));
        assert_eq!(map(ctrl_u), Some(AppEvent::ScrollFast(-1)));
    }

    #[test]
    fn enter_selects_esc_dismisses() {
        assert_eq!(map(k(KeyCode::Enter)), Some(AppEvent::SelectSession));
        assert_eq!(map(k(KeyCode::Esc)), Some(AppEvent::DismissModal));
    }

    #[test]
    fn help_share_export_reload() {
        assert_eq!(map(k(KeyCode::Char('?'))), Some(AppEvent::ToggleHelp));
        assert_eq!(map(k(KeyCode::Char('s'))), Some(AppEvent::Share));
        assert_eq!(map(k(KeyCode::Char('e'))), Some(AppEvent::Export));
        assert_eq!(map(k(KeyCode::Char('r'))), Some(AppEvent::Reload));
    }

    #[test]
    fn c_copies_seed() {
        assert_eq!(map(k(KeyCode::Char('c'))), Some(AppEvent::CopySeed));
    }

    #[test]
    fn resize_is_ignored() {
        assert_eq!(map(Event::Resize(80, 24)), None);
    }

    #[test]
    fn wheel_scrolls_transcript() {
        use crossterm::event::MouseEvent;
        let wheel = |kind| {
            Event::Mouse(MouseEvent {
                kind,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            })
        };
        assert_eq!(
            map(wheel(MouseEventKind::ScrollDown)),
            Some(AppEvent::ScrollWheel(WHEEL_LINES))
        );
        assert_eq!(
            map(wheel(MouseEventKind::ScrollUp)),
            Some(AppEvent::ScrollWheel(-WHEEL_LINES))
        );
        // Motion and other mouse events are ignored.
        assert_eq!(map(wheel(MouseEventKind::Moved)), None);
    }
}
