use ms_todo_protocol::Candidate;
use ratatui::Frame;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph, Wrap};

use super::{centered, pane, selection};
use crate::app::App;
use crate::keybindings::{EDITOR_KEYS, help_rows};

/// Every key, from the registry, then the line editor's.
pub fn help(frame: &mut Frame, app: &App) {
    let theme = &app.theme;
    let rows = help_rows();
    let editor = EDITOR_KEYS
        .iter()
        .map(|(keys, label)| ((*keys).to_owned(), *label));
    let key_width = rows
        .iter()
        .map(|(keys, _)| keys.as_str())
        .chain(EDITOR_KEYS.iter().map(|(keys, _)| *keys))
        .map(|keys| keys.chars().count())
        .max()
        .unwrap_or(0);
    let row = |(keys, label): (String, &'static str)| {
        Line::from(vec![
            Span::styled(format!(" {keys:<key_width$}  "), theme.key),
            Span::raw(label),
        ])
    };
    let mut lines: Vec<Line> = rows.into_iter().map(row).collect();
    lines.push(Line::default());
    lines.push(Line::styled(" Typing, in any prompt:", theme.text_dim));
    lines.extend(editor.map(row));
    lines.push(Line::default());
    lines.push(Line::styled(
        " Scripts and agents: use the `ms-todo` commands.",
        theme.text_dim,
    ));
    let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
    let area = centered(frame.area(), 60, height);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .block(pane(
                theme,
                format!(" ms-todo {} \u{b7} Keys ", app.version),
                true,
            ))
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// Undoing a recurring completion: Microsoft To Do made a completed copy,
/// and which one to delete is the user's choice, never a guess
/// (docs/blueprint/08-tui.md).
pub fn picker(frame: &mut Frame, app: &App, candidates: &[Candidate], index: usize) {
    let theme = &app.theme;
    let items: Vec<ListItem> = candidates
        .iter()
        .map(|candidate| {
            let list = candidate
                .list_id
                .as_deref()
                .and_then(|id| app.list_name(id))
                .unwrap_or("");
            let created = candidate
                .created_at
                .as_deref()
                .map(|at| at.get(..16).unwrap_or(at).replace('T', " "))
                .unwrap_or_default();
            ListItem::new(Line::from(vec![
                Span::raw(candidate.name.clone()),
                Span::styled(format!("  {list}  created {created}"), theme.text_dim),
            ]))
        })
        .collect();
    let height = u16::try_from(candidates.len() + 4).unwrap_or(u16::MAX);
    let area = centered(frame.area(), 70, height);
    frame.render_widget(Clear, area);
    let block = pane(theme, " Which completed copy to delete? ", true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(Line::styled(
            "Undo deletes the completed copy you pick:",
            theme.text_dim,
        ))
        .wrap(Wrap { trim: true }),
        ratatui::layout::Rect { height: 1, ..inner },
    );
    let mut state = ListState::default().with_selected(Some(index));
    frame.render_stateful_widget(
        List::new(items).highlight_style(selection(theme, true)),
        ratatui::layout::Rect {
            y: inner.y + 2,
            height: inner.height.saturating_sub(2),
            ..inner
        },
        &mut state,
    );
}
