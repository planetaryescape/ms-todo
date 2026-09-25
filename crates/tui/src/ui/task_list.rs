use chrono::{Datelike, NaiveDate};
use ms_todo_protocol::Scope;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};
use unicode_width::UnicodeWidthStr;

use super::{focused, pane, selection};
use crate::app::scope::{completed_groups, planned_groups};
use crate::app::{App, Connection, Pane, SyncMarker, Task};
use crate::glyphs::Glyphs;
use crate::theme::Theme;

pub fn draw<'a>(frame: &mut Frame, area: Rect, app: &'a App) {
    let has_focus = focused(app, Pane::Tasks);
    let name = app.view_name();
    let title = match &app.filter {
        Some(filter) => Line::from(vec![
            Span::raw(format!(" {name} / ")),
            Span::styled(filter.clone(), app.theme.search_match),
            Span::raw(" "),
        ]),
        None => Line::from(format!(" {name} ")),
    };
    let theme = &app.theme;
    let block = pane(theme, title, has_focus);
    if app.tasks.is_empty() || app.loading() {
        let text = empty_text(app);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let middle = Rect {
            y: inner.y + inner.height / 2,
            height: 1.min(inner.height),
            ..inner
        };
        frame.render_widget(
            Paragraph::new(text)
                .alignment(Alignment::Center)
                .style(theme.text_dim),
            middle,
        );
        return;
    }
    let glyphs = &app.glyphs;
    let today = app.clock.today();
    let row =
        |task: &'a Task| task_row(task, app.selection.contains(&task.id), glyphs, theme, today);
    // Planned is grouped by how soon, Completed by day; a header row goes
    // before each group.
    let groups: Option<Vec<(String, Vec<usize>)>> = match &app.shown {
        _ if app.filter.is_some() => None,
        Some(Scope::Planned) => Some(
            planned_groups(&app.tasks, today)
                .into_iter()
                .map(|(group, members)| (group.name().to_owned(), members))
                .collect(),
        ),
        Some(Scope::Completed) => Some(completed_groups(&app.tasks, today)),
        _ => None,
    };
    let (rows, selected) = match groups {
        Some(groups) => {
            let mut rows = Vec::new();
            let mut selected = 0;
            for (heading, members) in groups {
                rows.push(Row::new(vec![Cell::from(""), Cell::from(heading)]).style(theme.title));
                for index in members {
                    if index == app.task_index {
                        selected = rows.len();
                    }
                    rows.push(row(&app.tasks[index]));
                }
            }
            (rows, selected)
        }
        None => (app.tasks.iter().map(row).collect(), app.task_index),
    };
    let status_width = u16::try_from(glyphs.open.width()).unwrap_or(1);
    let flags_width = u16::try_from(3 * flag_slot(glyphs)).unwrap_or(u16::MAX);
    let table = Table::new(
        rows,
        [
            Constraint::Length(status_width),
            Constraint::Fill(1),
            Constraint::Length(11),
            Constraint::Length(flags_width),
            Constraint::Length(2),
        ],
    )
    .block(block)
    .row_highlight_style(selection(theme, has_focus));
    let mut state = TableState::default().with_selected(Some(selected));
    frame.render_stateful_widget(table, area, &mut state);
}

/// Why the list is empty: never a bare empty list while it may just not
/// have synced yet (docs/blueprint/08-tui.md, "syncing").
fn empty_text(app: &App) -> String {
    match (&app.connection, app.seeded) {
        (Connection::Lost(why), false) => format!("Can't reach the daemon: {why}"),
        (_, false) => "Connecting to the daemon…".into(),
        _ if !app.lists_ready => "Syncing…".into(),
        // The rows on hand are another scope's; never show them as this one.
        _ if app.loading() => "Loading…".into(),
        _ if !app.tasks_ready => "Syncing…".into(),
        _ => match &app.filter {
            Some(filter) => format!("No task matches \"{filter}\""),
            None => "Nothing here".into(),
        },
    }
}

fn task_row<'a>(
    task: &'a Task,
    selected: bool,
    glyphs: &Glyphs,
    theme: &Theme,
    today: NaiveDate,
) -> Row<'a> {
    let status = if task.completed {
        glyphs.done
    } else {
        glyphs.open
    };
    let title_style = if task.completed {
        theme.completed
    } else {
        theme.text
    };
    let due = task.due.map_or_else(
        || Span::raw(""),
        |due| {
            let style = if due < today && !task.completed {
                theme.overdue
            } else if due == today {
                theme.due_today
            } else {
                theme.text_dim
            };
            Span::styled(due_label(due, today), style)
        },
    );
    let slot = flag_slot(glyphs);
    // Each marker keeps its own place, blank when it doesn't apply, so a
    // marker lines up down the list whichever others a task has.
    let flag = |shown: bool, glyph: &'static str, style| {
        let glyph = if shown { glyph } else { "" };
        let padding = " ".repeat(slot.saturating_sub(glyph.width()));
        Span::styled(format!("{glyph}{padding}"), style)
    };
    let flags = vec![
        flag(task.important(), glyphs.important, theme.important),
        flag(
            task.recurrence.is_some(),
            glyphs.recurring,
            theme.text_muted,
        ),
        flag(task.reminder.is_some(), glyphs.reminder, theme.text_muted),
    ];
    Row::new(vec![
        Cell::from(status),
        Cell::from(if selected {
            Line::from(vec![
                Span::styled(format!("{} ", glyphs.selected), theme.key),
                Span::styled(task.title.as_str(), title_style.patch(theme.accent)),
            ])
        } else {
            Line::from(Span::styled(task.title.as_str(), title_style))
        }),
        Cell::from(Line::from(due).alignment(Alignment::Right)),
        Cell::from(Line::from(flags)),
        Cell::from(sync_marker(task.sync, glyphs, theme)),
    ])
}

/// Cells each marker (important, recurring, reminder) gets: the widest of
/// them by display width, plus one. Some fonts draw a width-1 symbol such
/// as the star a little wider than its cell, so the spare cell keeps it
/// from running into the next marker.
fn flag_slot(glyphs: &Glyphs) -> usize {
    [glyphs.important, glyphs.recurring, glyphs.reminder]
        .iter()
        .map(|glyph| glyph.width())
        .max()
        .unwrap_or(1)
        + 1
}

/// A task's sync marker: pending dim, unknown amber, failed red, and
/// nothing once synced (docs/blueprint/08-tui.md).
pub fn sync_marker(sync: SyncMarker, glyphs: &Glyphs, theme: &Theme) -> Span<'static> {
    match sync {
        SyncMarker::Synced => Span::raw(""),
        SyncMarker::Pending => Span::styled(glyphs.pending, theme.sync_pending),
        SyncMarker::Unknown => Span::styled(glyphs.unknown, theme.sync_unknown),
        SyncMarker::Failed => Span::styled(glyphs.failed, theme.sync_failed),
    }
}

/// "Today", "Tomorrow", "Yesterday", "Mon 28" within a week either side,
/// else "Oct 1", with the year when it isn't this one.
pub fn due_label(due: NaiveDate, today: NaiveDate) -> String {
    match due.signed_duration_since(today).num_days() {
        0 => "Today".into(),
        1 => "Tomorrow".into(),
        -1 => "Yesterday".into(),
        days if (-6..=6).contains(&days) => due.format("%a %-d").to_string(),
        _ if due.year() == today.year() => due.format("%b %-d").to_string(),
        _ => due.format("%b %-d %Y").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;

    #[test]
    fn due_dates_read_relative_to_today() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 24).expect("date");
        let label = |days: i64| due_label(today + Duration::days(days), today);
        assert_eq!(label(0), "Today");
        assert_eq!(label(1), "Tomorrow");
        assert_eq!(label(-1), "Yesterday");
        assert_eq!(label(4), "Mon 28");
        assert_eq!(label(7), "Oct 1");
        assert_eq!(label(120), "Jan 22 2027");
    }
}
