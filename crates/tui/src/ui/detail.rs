use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::task_list::{due_label, sync_marker};
use super::{focused, line_input, pane, selection};
use crate::app::edit::{Field, importance_name};
use crate::app::line_editor::LineEditor;
use crate::app::steps::{ChildTarget, DetailRow};
use crate::app::{App, Mode, Pane, SyncMarker, Task};
use crate::theme::Theme;

/// The detail pane: every field of the selected task. The fields `e`
/// edits are always shown, empty or not, so the cursor can reach them;
/// the one being edited shows what's typed with its cursor, and under it
/// what a date resolves to, the format, or why it can't be sent.
pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let has_focus = focused(app, Pane::Detail);
    let theme = &app.theme;
    let block = pane(theme, " Detail ", has_focus);
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
    let cursor = app.detail_row_now();
    let child_editing = match &app.mode {
        Mode::EditingChild {
            id,
            target,
            input,
            error,
        } if *id == task.id => Some((target, input, error.as_deref())),
        _ => None,
    };
    let label = |name: &'static str| Span::styled(format!("{name:<11}"), theme.text_dim);
    let field = |name: &'static str, value: String| Line::from(vec![label(name), Span::raw(value)]);
    let none = || Span::styled("none", theme.text_dim);
    // An editable field's lines: its value, reversed under the cursor, or
    // what's being typed.
    let editable = |which: Field, value: Vec<Span<'static>>| -> Vec<Line<'static>> {
        match editing {
            Some((edited, input, error)) if edited == which => {
                let (row, column) = input.cursor();
                let typing = theme.accent;
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
                        spans.extend(line_input::spans(line, cursor, typing, &app.glyphs, theme));
                        Line::from(spans)
                    })
                    .collect();
                let under = match (error, app.date_preview()) {
                    (Some(why), _) => Span::styled(why.to_owned(), theme.error),
                    (None, Some(Ok(resolved))) => Span::styled(resolved, typing),
                    (None, Some(Err(why))) => Span::styled(why, theme.text_dim),
                    (None, None) => Span::styled(which.format_hint(), theme.text_dim),
                };
                lines.push(Line::from(vec![label(""), under]));
                lines
            }
            _ => {
                let mut spans = vec![label(which.name())];
                spans.extend(value);
                let line = Line::from(spans);
                let picking = which == Field::Importance && choosing_importance;
                if (has_focus && cursor == Some(DetailRow::Field(which))) || picking {
                    vec![line.style(selection(theme, true))]
                } else {
                    vec![line]
                }
            }
        }
    };
    let mut lines = editable(
        Field::Title,
        vec![Span::styled(task.title.clone(), theme.strong)],
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
    let highlight = |line: Line<'static>, row: DetailRow| {
        if has_focus && cursor == Some(row) && child_editing.is_none() {
            line.style(selection(theme, true))
        } else {
            line
        }
    };
    // A step or link being typed: what's typed with its cursor, and under
    // it why it can't be sent.
    let typing = |before: Vec<Span<'static>>| -> Vec<Line<'static>> {
        let Some((_, input, error)) = child_editing else {
            return Vec::new();
        };
        let mut spans = before;
        spans.extend(line_input::single(input, theme.accent, &app.glyphs, theme));
        let mut lines = vec![Line::from(spans)];
        if let Some(why) = error {
            lines.push(Line::from(vec![
                label(""),
                Span::styled(why.to_owned(), theme.error),
            ]));
        }
        lines
    };
    let steps_heading = match task.steps_done() {
        Some((checked, total)) => Span::raw(format!("{checked} of {total} done")),
        None => none(),
    };
    lines.push(highlight(
        Line::from(vec![label("Steps"), steps_heading]),
        DetailRow::Steps,
    ));
    for (at, step) in task.steps.iter().enumerate() {
        let (mark, style) = if step.checked {
            (app.glyphs.done, theme.completed)
        } else {
            (app.glyphs.open, theme.text)
        };
        let before = vec![label(""), Span::styled(format!("{mark} "), theme.text_dim)];
        match child_editing {
            Some((ChildTarget::Step(id), ..)) if *id == step.id => lines.extend(typing(before)),
            _ => {
                let mut spans = before;
                spans.push(Span::styled(step.name.clone(), style));
                lines.push(highlight(Line::from(spans), DetailRow::Step(at)));
            }
        }
    }
    if let Some((ChildTarget::NewStep, ..)) = child_editing {
        lines.extend(typing(vec![
            label(""),
            Span::styled(format!("{} ", app.glyphs.open), theme.text_dim),
        ]));
    }
    match child_editing {
        Some((ChildTarget::Link(_), ..)) => lines.extend(typing(vec![label("Link")])),
        _ => {
            // A named link's URL goes on a line of its own, under the name.
            let (first, url) = match task.linked.first() {
                Some((url, Some(name))) if name != url => {
                    (Span::raw(name.clone()), Some(url.clone()))
                }
                Some((url, _)) => (Span::styled(url.clone(), theme.link), None),
                None => (none(), None),
            };
            lines.push(highlight(
                Line::from(vec![label("Link"), first]),
                DetailRow::Link,
            ));
            if let Some(url) = url {
                lines.push(highlight(
                    Line::from(vec![label(""), Span::styled(url, theme.link)]),
                    DetailRow::Link,
                ));
            }
        }
    }
    if !task.categories.is_empty() {
        lines.push(field("Categories", task.categories.join(", ")));
    }
    let (state, style) = match task.sync {
        SyncMarker::Synced => ("synced", theme.text),
        SyncMarker::Pending => (
            "pending: waiting to reach Microsoft To Do",
            theme.sync_pending,
        ),
        SyncMarker::Unknown => (
            "outcome unknown: resolve it with `ms-todo outbox list`",
            theme.sync_unknown,
        ),
        SyncMarker::Failed => (
            "failed: Microsoft To Do rejected a change",
            theme.sync_failed,
        ),
    };
    lines.push(Line::from(vec![
        label("Sync"),
        sync_marker(task.sync, &app.glyphs, theme),
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
            Span::styled(why.clone(), theme.error),
        ]));
    }
    lines.push(Line::default());
    // Rendering html notes is the costly part of a frame: skip it while
    // they're being edited, when what's typed is shown instead.
    let editing_notes = editing.is_some_and(|(field, ..)| field == Field::Notes);
    match (!editing_notes).then(|| task.notes()).flatten() {
        Some(notes) => {
            lines.extend(editable(Field::Notes, Vec::new()));
            lines.extend(notes.lines().map(|line| linked(line, theme)));
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

/// A line of notes with its URLs drawn as links (D-050). They're styled,
/// not OSC 8 hyperlinks: terminals such as Ghostty and iTerm already make
/// a URL on screen Cmd-clickable, and `o` and `y` follow one.
fn linked(line: &str, theme: &Theme) -> Line<'static> {
    let mut spans = Vec::new();
    let mut at = 0;
    for range in ms_todo_core::links::find_urls(line) {
        spans.push(Span::raw(line[at..range.start].to_owned()));
        spans.push(Span::styled(line[range.clone()].to_owned(), theme.link));
        at = range.end;
    }
    spans.push(Span::raw(line[at..].to_owned()));
    Line::from(spans)
}

/// The due date, with how soon when it's near.
fn due(task: &Task, app: &App) -> Vec<Span<'static>> {
    let Some(due) = task.due else {
        return vec![Span::styled("none", app.theme.text_dim)];
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
