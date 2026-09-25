use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::task_list::{due_label, sync_marker};
use super::{ACCENT, AMBER, DIM, ERROR, focused, line_input, pane, selection};
use crate::app::edit::{Field, importance_name};
use crate::app::line_editor::LineEditor;
use crate::app::{App, Mode, Pane, SyncMarker, Task};

/// The detail pane: every field of the selected task. The fields `e`
/// edits are always shown, empty or not, so the cursor can reach them;
/// the one being edited shows what's typed with its cursor, and under it
/// what a date resolves to, the format, or why it can't be sent.
pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let has_focus = focused(app, Pane::Detail);
    let block = pane(" Detail ".into(), has_focus);
    let Some(task) = app.selected() else {
        frame.render_widget(block, area);
        return;
    };
    let editing: Option<(Field, &LineEditor, Option<&str>)> = match &app.mode {
        Mode::Editing {
            id,
            field,
            input,
            error,
        } if *id == task.id => Some((*field, input, error.as_deref())),
        _ => None,
    };
    let choosing_importance =
        matches!(&app.mode, Mode::ChoosingImportance { id } if *id == task.id);
    let label = |name: &'static str| Span::styled(format!("{name:<11}"), Style::default().fg(DIM));
    let field = |name: &'static str, value: String| Line::from(vec![label(name), Span::raw(value)]);
    let none = || Span::styled("none", Style::default().fg(DIM));
    // An editable field's lines: its value, reversed under the cursor, or
    // what's being typed.
    let editable = |which: Field, value: Vec<Span<'static>>| -> Vec<Line<'static>> {
        match editing {
            Some((edited, input, error)) if edited == which => {
                let (row, column) = input.cursor();
                let typing = Style::default().fg(ACCENT);
                let mut lines: Vec<Line> = input
                    .lines()
                    .iter()
                    .enumerate()
                    .map(|(at, line)| {
                        let mut spans = vec![if at == 0 {
                            label(which.name())
                        } else {
                            label("")
                        }];
                        let cursor = (at == row).then_some(column);
                        spans.extend(line_input::spans(line, cursor, typing, &app.glyphs));
                        Line::from(spans)
                    })
                    .collect();
                let under = match (error, app.date_preview()) {
                    (Some(why), _) => Span::styled(why.to_owned(), Style::default().fg(ERROR)),
                    (None, Some(Ok(resolved))) => Span::styled(resolved, typing),
                    (None, Some(Err(why))) => Span::styled(why, Style::default().fg(DIM)),
                    (None, None) => Span::styled(which.format_hint(), Style::default().fg(DIM)),
                };
                lines.push(Line::from(vec![label(""), under]));
                lines
            }
            _ => {
                let mut spans = vec![label(which.name())];
                spans.extend(value);
                let line = Line::from(spans);
                let picking = which == Field::Importance && choosing_importance;
                if (has_focus && app.detail_field == which) || picking {
                    vec![line.style(selection(true))]
                } else {
                    vec![line]
                }
            }
        }
    };
    let mut lines = editable(
        Field::Title,
        vec![Span::styled(
            task.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )],
    );
    lines.push(Line::default());
    lines.push(field(
        "Status",
        if task.completed { "completed" } else { "open" }.into(),
    ));
    if let Some(name) = app.list_name(&task.list_id) {
        lines.push(field("List", name.to_owned()));
    }
    lines.extend(editable(Field::Due, due(task, app)));
    lines.extend(editable(
        Field::Reminder,
        vec![task.reminder.map_or_else(none, |reminder| {
            Span::raw(reminder.format("%a %-d %b %Y %H:%M").to_string())
        })],
    ));
    lines.extend(editable(
        Field::Importance,
        vec![Span::raw(importance_name(task.importance))],
    ));
    if let Some(recurrence) = &task.recurrence {
        lines.push(field("Repeats", recurrence.clone()));
    }
    if let Some((checked, total)) = task.steps {
        lines.push(field("Steps", format!("{checked} of {total} done")));
    }
    if !task.categories.is_empty() {
        lines.push(field("Categories", task.categories.join(", ")));
    }
    let (state, style) = match task.sync {
        SyncMarker::Synced => ("synced", Style::default()),
        SyncMarker::Pending => (
            "pending: waiting to reach Microsoft To Do",
            Style::default().fg(DIM),
        ),
        SyncMarker::Unknown => (
            "outcome unknown: resolve it with `ms-todo outbox list`",
            Style::default().fg(AMBER),
        ),
        SyncMarker::Failed => (
            "failed: Microsoft To Do rejected a change",
            Style::default().fg(ERROR),
        ),
    };
    lines.push(Line::from(vec![
        label("Sync"),
        sync_marker(task.sync, &app.glyphs),
        Span::raw(if task.sync == SyncMarker::Synced {
            ""
        } else {
            " "
        }),
        Span::styled(state, style),
    ]));
    if let Some(why) = app.rejections.get(&task.id) {
        lines.push(Line::from(vec![
            label(""),
            Span::styled(why.clone(), Style::default().fg(ERROR)),
        ]));
    }
    lines.push(Line::default());
    // Rendering html notes is the costly part of a frame: skip it while
    // they're being edited, when what's typed is shown instead.
    let editing_notes = editing.is_some_and(|(field, ..)| field == Field::Notes);
    match (!editing_notes).then(|| task.notes()).flatten() {
        Some(notes) => {
            lines.extend(editable(Field::Notes, Vec::new()));
            lines.extend(notes.lines().map(|line| Line::raw(line.to_owned())));
        }
        None => lines.extend(editable(Field::Notes, vec![none()])),
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// The due date, with how soon when it's near.
fn due(task: &Task, app: &App) -> Vec<Span<'static>> {
    let Some(due) = task.due else {
        return vec![Span::styled("none", Style::default().fg(DIM))];
    };
    let mut shown = due.format("%a %-d %b %Y").to_string();
    let today = app.clock.today();
    if due < today && !task.completed {
        shown.push_str(" (overdue)");
    } else if due.signed_duration_since(today).num_days().abs() <= 1 {
        shown = format!("{shown} ({})", due_label(due, today));
    }
    vec![Span::raw(shown)]
}
