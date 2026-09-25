use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::pane;
use crate::app::App;

pub fn draw(frame: &mut Frame, area: Rect, app: &App, command: &str) {
    let block = pane(&app.theme, " Sign in ", true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let lines = vec![
        Line::styled("Sign in to Microsoft To Do", app.theme.title),
        Line::raw(""),
        Line::raw("Open another terminal and run:"),
        Line::styled(command.to_owned(), app.theme.accent),
        Line::raw(""),
        Line::raw("Then reopen this TUI."),
    ];
    let command_lines = command.width().div_ceil(usize::from(inner.width.max(1)));
    let height = u16::try_from(5 + command_lines)
        .unwrap_or(inner.height)
        .min(inner.height);
    let text_area = Rect {
        y: inner.y + inner.height.saturating_sub(height) / 2,
        height,
        ..inner
    };
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: false }),
        text_area,
    );
}
