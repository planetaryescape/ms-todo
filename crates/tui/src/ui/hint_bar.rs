// Adapted from mxr crates/tui/src/ui/hint_bar.rs @ dfb23d10138b1cfc24f8ea7450d3426e5e4da37a:
// contextual hints from the keybinding registry on the left, the sync
// state on the right. Changes: a prompt or the delete confirmation takes
// the bar over, since that's where the typing is.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{ACCENT, DIM, ERROR};
use crate::app::{App, Mode};
use crate::keybindings::hints;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let glyphs = &app.glyphs;
    let hint_spans = |context| {
        let mut spans = Vec::new();
        for (keys, label) in hints(context) {
            spans.push(Span::styled(
                format!(" {keys} "),
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(format!("{label} "), Style::default().fg(DIM)));
        }
        spans
    };
    let left = match &app.mode {
        Mode::Adding { text } => {
            let list = match &app.shown {
                Some(ms_todo_protocol::Scope::List { id }) => {
                    app.list_name(id).unwrap_or("Tasks").to_owned()
                }
                shown => format!("Tasks, from {}", app.scope_name(shown.as_ref())),
            };
            Line::from(vec![
                Span::styled(format!(" Add to {list}: "), Style::default().fg(ACCENT)),
                Span::raw(text.clone()),
                Span::raw(glyphs.cursor),
            ])
        }
        Mode::Filtering { text } => {
            let mut spans = vec![
                Span::styled(" / ", Style::default().fg(ACCENT)),
                Span::raw(text.clone()),
                Span::raw(glyphs.cursor),
            ];
            if let Some(error) = &app.filter_error {
                spans.push(Span::styled(
                    format!("  {error}"),
                    Style::default().fg(ERROR),
                ));
            }
            Line::from(spans)
        }
        Mode::ConfirmDelete { title, .. } => {
            let mut spans = vec![Span::styled(
                format!(" Delete \"{title}\"? "),
                Style::default().fg(ERROR).add_modifier(Modifier::BOLD),
            )];
            spans.extend(hint_spans(app.context()));
            Line::from(spans)
        }
        _ => Line::from(hint_spans(app.context())),
    };
    let (dot, state, style) = if app.activity.in_progress {
        (glyphs.pending, "syncing", Style::default().fg(ACCENT))
    } else if app.activity.last_error.is_some() {
        (glyphs.failed, "sync failed", Style::default().fg(ERROR))
    } else {
        (glyphs.connected, "synced", Style::default().fg(DIM))
    };
    let right = format!("{dot} {state} ");
    let width = u16::try_from(right.chars().count()).unwrap_or(u16::MAX);
    let [hints_area, state_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(width)]).areas(area);
    frame.render_widget(Paragraph::new(left), hints_area);
    frame.render_widget(Paragraph::new(right).style(style), state_area);
}
