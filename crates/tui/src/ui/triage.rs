use ratatui::Frame;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use super::{centered, pane};
use crate::app::App;
use crate::app::triage::{Suggestion, Triage};

/// "Suggest lists for inbox": the task on screen, how far through the
/// inbox it is, and the list the daemon suggests for it, or why none.
/// The hint bar has the keys.
pub fn draw(frame: &mut Frame, app: &App, triage: &Triage) {
    let theme = &app.theme;
    let area = centered(frame.area(), 64, 8);
    frame.render_widget(Clear, area);
    let title = match triage.tasks.len() {
        0 => " Suggest lists for inbox ".to_owned(),
        count => format!(" Inbox {} of {count} ", triage.at + 1),
    };
    let block = pane(theme, title, true);
    let mut lines = Vec::new();
    if let Some(task) = triage.current() {
        lines.push(Line::styled(
            ms_todo_core::one_line_safe(&task.title),
            theme.strong,
        ));
        lines.push(Line::default());
    }
    let label = |text: &'static str| Span::styled(text, theme.text_dim);
    lines.push(match &triage.suggestion {
        Suggestion::Loading => Line::from(label("Reading the inbox\u{2026}")),
        Suggestion::Asking => Line::from(vec![
            label("Suggested  "),
            Span::styled("asking\u{2026}", theme.text_dim),
        ]),
        Suggestion::Found(list) => Line::from(vec![
            label("Suggested  "),
            Span::styled(ms_todo_core::one_line_safe(&list.list_name), theme.accent),
            Span::styled(
                format!("  {:.0}% sure", list.confidence * 100.0),
                theme.text_dim,
            ),
        ]),
        Suggestion::Nothing => Line::from(vec![
            label("Suggested  "),
            Span::styled("no list is likely enough", theme.text_muted),
        ]),
        Suggestion::Failed(why) => Line::styled(why.clone(), theme.error),
    });
    frame.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}
