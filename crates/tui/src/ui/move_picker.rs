use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph};

use super::{centered, line_input, pane, selection};
use crate::app::App;
use crate::app::line_editor::LineEditor;

/// The most lists shown at once; the list scrolls past them.
const ROWS: u16 = 12;

/// "Move to list…": the query, then the lists it matches, best first, each
/// with its folder.
pub fn draw(
    frame: &mut Frame,
    app: &App,
    ids: &[String],
    what: &str,
    query: &LineEditor,
    index: usize,
) {
    let targets = app.move_targets(ids, &query.text());
    let rows = u16::try_from(targets.len()).unwrap_or(ROWS).clamp(1, ROWS);
    let area = centered(frame.area(), 60, rows + 4);
    frame.render_widget(Clear, area);
    let theme = &app.theme;
    let title = ms_todo_core::one_line_safe(&format!(" Move {what} to "));
    let block = pane(theme, title, true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [input, _, list] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(Line::from(
            [Span::styled("> ", theme.accent)]
                .into_iter()
                .chain(line_input::single(
                    query,
                    Style::default(),
                    &app.glyphs,
                    theme,
                ))
                .collect::<Vec<_>>(),
        )),
        input,
    );
    if targets.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("No list matches", theme.text_dim)),
            list,
        );
        return;
    }
    let width = usize::from(list.width);
    let rows: Vec<ListItem> = targets
        .iter()
        .map(|target| {
            let name = ms_todo_core::one_line_safe(&target.name);
            let folder = target
                .folder
                .as_deref()
                .map(ms_todo_core::one_line_safe)
                .unwrap_or_default();
            let gap = width
                .saturating_sub(name.chars().count() + folder.chars().count())
                .max(1);
            ListItem::new(Line::from(vec![
                Span::raw(name),
                Span::raw(" ".repeat(gap)),
                Span::styled(folder, theme.text_dim),
            ]))
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(index));
    frame.render_stateful_widget(
        List::new(rows).highlight_style(selection(theme, true)),
        list,
        &mut state,
    );
}
