//! Map raw `crossterm` events into the TUI's pure `AppEvent` enum.

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

/// Pure events the reducer consumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEvent {
    Quit,
    CycleSourceNext,
    CycleSourcePrev,
    OpenTargetPicker,
    MoveCursor(i64),
    SelectSession,
    Reload,
    Share,
    Export,
    ConfirmExport,
    DismissModal,
    ToggleHelp,
    ClearError,
}

/// Translate a single `crossterm` event into an `AppEvent`, or `None` to
/// ignore it (resize, mouse motion, etc.).
pub fn map(event: Event) -> Option<AppEvent> {
    match event {
        Event::Key(k) => key(k),
        Event::Resize(_, _) => None,
        _ => None,
    }
}

fn key(k: KeyEvent) -> Option<AppEvent> {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && k.code == KeyCode::Char('c') {
        return Some(AppEvent::Quit);
    }
    match k.code {
        KeyCode::Char('q') => Some(AppEvent::Quit),
        KeyCode::Char('?') => Some(AppEvent::ToggleHelp),
        KeyCode::Char('r') => Some(AppEvent::Reload),
        KeyCode::Char('s') => Some(AppEvent::Share),
        KeyCode::Char('e') => Some(AppEvent::Export),
        KeyCode::Char('j') | KeyCode::Down => Some(AppEvent::MoveCursor(1)),
        KeyCode::Char('k') | KeyCode::Up => Some(AppEvent::MoveCursor(-1)),
        KeyCode::Left => Some(AppEvent::CycleSourcePrev),
        KeyCode::Right => Some(AppEvent::CycleSourceNext),
        KeyCode::Tab => Some(AppEvent::OpenTargetPicker),
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
    fn tab_picks_target() {
        assert_eq!(map(k(KeyCode::Tab)), Some(AppEvent::OpenTargetPicker));
    }

    #[test]
    fn arrows_and_jk_move_cursor() {
        assert_eq!(map(k(KeyCode::Down)), Some(AppEvent::MoveCursor(1)));
        assert_eq!(map(k(KeyCode::Up)), Some(AppEvent::MoveCursor(-1)));
        assert_eq!(map(k(KeyCode::Char('j'))), Some(AppEvent::MoveCursor(1)));
        assert_eq!(map(k(KeyCode::Char('k'))), Some(AppEvent::MoveCursor(-1)));
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
    fn resize_is_ignored() {
        assert_eq!(map(Event::Resize(80, 24)), None);
    }
}
