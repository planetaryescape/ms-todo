use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{ACCENT, DIM, ERROR};
use crate::app::App;

/// The top row: which app and version, and the view, on the left; the
/// sync state on the right.
pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let left = Line::from(vec![
        Span::styled(
            " ms-todo",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {} \u{b7} {}", app.version, app.view_name()),
            Style::default().fg(DIM),
        ),
    ]);
    let glyphs = &app.glyphs;
    let (dot, state, style) = if app.activity.in_progress {
        (glyphs.pending, "syncing", Style::default().fg(ACCENT))
    } else if app.activity.last_error.is_some() {
        (glyphs.failed, "sync failed", Style::default().fg(ERROR))
    } else {
        (glyphs.connected, "synced", Style::default().fg(DIM))
    };
    let right = format!("{dot} {state} ");
    let width = u16::try_from(right.chars().count()).unwrap_or(u16::MAX);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(width)]).areas(area);
    frame.render_widget(Paragraph::new(left), left_area);
    frame.render_widget(Paragraph::new(right).style(style), right_area);
}
