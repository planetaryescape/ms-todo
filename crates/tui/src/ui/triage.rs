use ratatui::Frame;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use unicode_width::UnicodeWidthChar;

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
    let width = usize::from(block.inner(area).width);
    let mut lines = Vec::new();
    if let Some(task) = triage.current() {
        let title = ms_todo_core::one_line_safe(&task.title);
        lines.extend(
            title_lines(&title, width)
                .into_iter()
                .map(|line| Line::styled(line, theme.strong)),
        );
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

/// `title` in at most two lines of `width` columns, cut with an ellipsis
/// when it's longer, so the suggestion under it always fits the modal.
fn title_lines(title: &str, width: usize) -> Vec<String> {
    let width = width.max(2);
    let mut lines = vec![String::new()];
    let mut used = 0;
    let mut chars = title.chars().peekable();
    while let Some(ch) = chars.next() {
        let wide = ch.width().unwrap_or(0);
        if used + wide > width {
            if lines.len() == 2 {
                break;
            }
            lines.push(String::new());
            used = 0;
        }
        // The last column of the second line is the ellipsis's when
        // more follows.
        if lines.len() == 2 && chars.peek().is_some() && used + wide + 1 > width {
            break;
        }
        if let Some(line) = lines.last_mut() {
            line.push(ch);
        }
        used += wide;
    }
    let shown: usize = lines.iter().map(|line| line.chars().count()).sum();
    if shown < title.chars().count()
        && let Some(last) = lines.last_mut()
    {
        last.push('\u{2026}');
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::title_lines;

    #[test]
    fn a_long_title_takes_two_lines_and_ends_in_an_ellipsis() {
        assert_eq!(title_lines("Buy paint", 10), ["Buy paint"]);
        assert_eq!(title_lines("abcdefghij", 5), ["abcde", "fghij"]);
        assert_eq!(title_lines("abcdefghijk", 5), ["abcde", "fghi\u{2026}"]);
    }
}
