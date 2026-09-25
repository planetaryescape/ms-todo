use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;

/// The top row: which app and version, and the view, on the left; the
/// sync state on the right.
pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    frame.buffer_mut().set_style(area, theme.header_bar);
    let left = Line::from(vec![
        Span::styled(" ms-todo", theme.title),
        Span::styled(
            format!(" {} \u{b7} {}", app.version, app.view_name()),
            theme.text_dim,
        ),
    ]);
    let glyphs = &app.glyphs;
    let (dot, state, style) = if app.sign_in_required {
        (glyphs.disconnected, "sign in", theme.warning)
    } else if app.activity.in_progress {
        (glyphs.pending, "syncing", theme.accent)
    } else if app.activity.last_error.is_some() {
        (glyphs.failed, "sync failed", theme.sync_failed)
    } else {
        (glyphs.connected, "synced", theme.text_dim)
    };
    let right = format!("{dot} {state} ");
    let width = u16::try_from(right.chars().count()).unwrap_or(u16::MAX);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(width)]).areas(area);
    frame.render_widget(Paragraph::new(left), left_area);
    frame.render_widget(Paragraph::new(right).style(style), right_area);
}
