//! `ratatui` rendering. Pure function from `&App` to a frame; no events,
//! no IO, no terminal side effects.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::tui::app::{App, Focus, Screen};

const KEYMAP: &str = " ←/→ source · Space target · Tab focus · ↑/↓ move/scroll · ^U/^D fast scroll · s share · e export · r reload · ? help · q quit ";

/// Border style for a pane, brighter when it currently has focus.
fn border_style(focused: bool) -> Style {
    if focused {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(f, outer[0], app);
    draw_body(f, outer[1], app);
    draw_status(f, outer[2], app);
    draw_footer(f, outer[3]);

    if matches!(app.screen, Screen::Help) {
        draw_help_modal(f, area);
    }
    if let Screen::ShareModal(result) = &app.screen {
        draw_share_modal(f, area, result);
    }
    if let Screen::ExportModal { format, out, .. } = &app.screen {
        draw_export_modal(f, area, *format, out);
    }
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let session = app
        .selected_session
        .clone()
        .unwrap_or_else(|| "—".to_string());
    let msgs = app.transcript.as_ref().map(|_| "loaded").unwrap_or("none");
    let line = Line::from(vec![
        Span::styled("acs · tui", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("   "),
        Span::styled(&app.source, Style::default().fg(Color::Cyan)),
        Span::raw(" → "),
        Span::styled(&app.target, Style::default().fg(Color::Cyan)),
        Span::raw(format!(
            "   session: {session}   {msgs}   {} chars",
            app.transcript_chars
        )),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_body(f: &mut Frame, area: Rect, app: &App) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(28), Constraint::Min(20)])
        .split(area);

    draw_sidebar(f, cols[0], app);
    draw_preview(f, cols[1], app);
}

fn draw_sidebar(f: &mut Frame, area: Rect, app: &App) {
    let title = match app.screen {
        Screen::PickSource => "Agents",
        _ => "Sessions",
    };
    let items: Vec<ListItem> = match app.screen {
        Screen::PickSource => adapters_candidates()
            .iter()
            .map(|a| ListItem::new(a.to_string()))
            .collect(),
        _ => app
            .sessions
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let style = if i == app.cursor {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let summary = if s.summary.is_empty() {
                    "(no summary)".to_string()
                } else {
                    crate::tui::app::sanitize_for_display(&s.summary)
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{}  ", truncate(&s.id, 10)), style),
                    Span::styled(truncate(&summary, 14), style),
                ]))
            })
            .collect(),
    };
    let focused = matches!(app.focus, Focus::Sessions);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(border_style(focused));
    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn draw_preview(f: &mut Frame, area: Rect, app: &App) {
    let focused = matches!(app.focus, Focus::Transcript);
    let title = if app.transcript.is_some() && app.transcript_lines > 0 {
        format!("Transcript · line {}/{}", app.scroll, app.transcript_lines)
    } else {
        "Transcript".to_string()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(border_style(focused));

    // Body text. For a loaded transcript we render only the visible window of
    // lines (`visible_slice`) rather than the whole thing: the previous code
    // cloned and re-wrapped the entire transcript every frame, which made a
    // large session sluggish to scroll. `visible_slice` borrows a subslice, so
    // there is no per-frame allocation and ratatui only lays out one screenful.
    let body: &str = match &app.transcript {
        Some(t) => {
            let inner_height = area.height.saturating_sub(2) as usize; // minus borders
            visible_slice(t, app.scroll as usize, inner_height)
        }
        None => match app.screen {
            Screen::PickSource => "Use ←/→ to pick a source agent, then press → to continue.",
            _ => "Pick a session and press Enter to preview.",
        },
    };

    // The slice already starts at the scroll offset, so no `.scroll()` here.
    let p = Paragraph::new(body).block(block).wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

/// Borrow the window of `text` spanning `count` newline-separated lines starting
/// at line `start` (0-indexed). Returns a subslice with no allocation. Used to
/// keep transcript rendering O(viewport) instead of O(transcript).
fn visible_slice(text: &str, start: usize, count: usize) -> &str {
    if count == 0 {
        return "";
    }
    let bytes = text.as_bytes();

    // Byte offset where line `start` begins.
    let mut begin = 0;
    if start > 0 {
        let mut seen = 0;
        let mut found = false;
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'\n' {
                seen += 1;
                if seen == start {
                    begin = i + 1;
                    found = true;
                    break;
                }
            }
        }
        if !found {
            return ""; // scrolled past the end
        }
    }

    // Byte offset where the window ends: start of line `start + count`, minus
    // the trailing newline (or end of text if there are fewer lines).
    let mut end = text.len();
    let mut seen = 0;
    for (i, &b) in bytes[begin..].iter().enumerate() {
        if b == b'\n' {
            seen += 1;
            if seen == count {
                end = begin + i;
                break;
            }
        }
    }
    &text[begin..end]
}

fn draw_status(f: &mut Frame, area: Rect, app: &App) {
    let msg = app.status_message().unwrap_or("");
    let style = if app.status_message().is_some() {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    f.render_widget(Paragraph::new(Span::styled(msg, style)), area);
}

fn draw_footer(f: &mut Frame, area: Rect) {
    let p = Paragraph::new(KEYMAP).style(Style::default().fg(Color::DarkGray));
    f.render_widget(p, area);
}

fn draw_help_modal(f: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Help · press ? or Esc to close");
    let text = "\
←/→        cycle source agent
Space      cycle target agent
Tab        switch focus: session list ⇄ transcript
↑/↓ or j/k focused pane: move cursor / scroll transcript
Ctrl-D/U   fast scroll transcript (any focus)
Enter      open session and focus the transcript
s          share (writes transcript, shows seed command)
e          export (writes transcript or JSON to a path)
c          copy the open modal's command/path to clipboard
r          reload session list
?          toggle this help
q / Ctrl-C quit

The TUI never spawns the target agent. Copy the printed
seed command and run it yourself in a real terminal.";
    let p = Paragraph::new(text).block(block).wrap(Wrap { trim: false });
    let rect = centered_rect(60, 60, area);
    f.render_widget(Clear, rect);
    f.render_widget(p, rect);
}

fn draw_share_modal(f: &mut Frame, area: Rect, result: &crate::share::ShareResult) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Share · c copy command · Enter/Esc close");
    let body = format!(
        "Wrote {} ({} turns)\n\nRun this to continue (press c to copy):\n\n{}\n",
        result.transcript_path.display(),
        result.message_count,
        result.seed_shell,
    );
    let p = Paragraph::new(body).block(block).wrap(Wrap { trim: false });
    let rect = centered_rect(70, 40, area);
    f.render_widget(Clear, rect);
    f.render_widget(p, rect);
}

fn draw_export_modal(
    f: &mut Frame,
    area: Rect,
    format: crate::share::ExportFormat,
    out: &std::path::Path,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Export · Enter write · c copy path · Esc cancel");
    let fmt = match format {
        crate::share::ExportFormat::Transcript => "transcript",
        crate::share::ExportFormat::Json => "json",
    };
    let body = format!(
        "Format: {fmt}\nPath:   {}\n\nPress Enter to write, Esc to cancel.",
        out.display(),
    );
    let p = Paragraph::new(body).block(block).wrap(Wrap { trim: false });
    let rect = centered_rect(70, 30, area);
    f.render_widget(Clear, rect);
    f.render_widget(p, rect);
}

fn adapters_candidates() -> &'static [&'static str] {
    crate::adapters::KNOWN_AGENTS
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let pop_y = Layout::default()
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
        .split(pop_y[1])[1]
}

#[cfg(test)]
mod tests {
    use super::visible_slice;

    #[test]
    fn window_from_top() {
        let text = "l0\nl1\nl2\nl3\nl4";
        assert_eq!(visible_slice(text, 0, 3), "l0\nl1\nl2");
    }

    #[test]
    fn window_offset_into_middle() {
        let text = "l0\nl1\nl2\nl3\nl4";
        assert_eq!(visible_slice(text, 2, 2), "l2\nl3");
    }

    #[test]
    fn window_past_end_is_clamped_to_remaining() {
        let text = "l0\nl1\nl2";
        // Asking for more lines than remain returns what's left, no panic.
        assert_eq!(visible_slice(text, 1, 10), "l1\nl2");
    }

    #[test]
    fn start_beyond_end_is_empty() {
        let text = "l0\nl1";
        assert_eq!(visible_slice(text, 9, 5), "");
    }

    #[test]
    fn zero_count_is_empty() {
        assert_eq!(visible_slice("a\nb", 0, 0), "");
    }
}
