use chrono::{Datelike, NaiveDate};
use ms_todo_protocol::Scope;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};

use super::{ACCENT, AMBER, DIM, ERROR, focused, pane, selection};
use crate::app::scope::planned_groups;
use crate::app::{App, Connection, Pane, SyncMarker, Task};
use crate::glyphs::Glyphs;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    let has_focus = focused(app, Pane::Tasks);
    let name = app.scope_name(app.shown.as_ref());
    let title = match &app.filter {
        Some(filter) => format!(" {name} / {filter} "),
        None => format!(" {name} "),
    };
    let block = pane(title, has_focus);
    if app.tasks.is_empty() {
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
                .style(Style::default().fg(DIM)),
            middle,
        );
        return;
    }
    let glyphs = &app.glyphs;
    let today = app.clock.today;
    let row = |task| task_row(task, glyphs, today);
    // Planned is grouped; a header row goes before each group.
    let (rows, selected) = if app.shown == Some(Scope::Planned) && app.filter.is_none() {
        let mut rows = Vec::new();
        let mut selected = 0;
        for (group, members) in planned_groups(&app.tasks, today) {
            rows.push(
                Row::new(vec![Cell::from(""), Cell::from(group.name())])
                    .style(Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            );
            for index in members {
                if index == app.task_index {
                    selected = rows.len();
                }
                rows.push(row(&app.tasks[index]));
            }
        }
        (rows, selected)
    } else {
        (app.tasks.iter().map(row).collect(), app.task_index)
    };
    let status_width = u16::try_from(glyphs.open.chars().count()).unwrap_or(1);
    let table = Table::new(
        rows,
        [
            Constraint::Length(status_width),
            Constraint::Fill(1),
            Constraint::Length(11),
            Constraint::Length(5),
            Constraint::Length(2),
        ],
    )
    .block(block)
    .row_highlight_style(selection(has_focus));
    let mut state = TableState::default().with_selected(Some(selected));
    frame.render_stateful_widget(table, area, &mut state);
}

/// Why the list is empty: never a bare empty list while it may just not
/// have synced yet (docs/blueprint/08-tui.md, "syncing").
fn empty_text(app: &App) -> String {
    match (&app.connection, app.seeded) {
        (Connection::Lost(why), false) => format!("Can't reach the daemon: {why}"),
        (_, false) => "Connecting to the daemon…".into(),
        _ if !app.tasks_ready || !app.lists_ready => "Syncing…".into(),
        _ => match &app.filter {
            Some(filter) => format!("No task matches \"{filter}\""),
            None => "Nothing here".into(),
        },
    }
}

fn task_row<'a>(task: &'a Task, glyphs: &Glyphs, today: NaiveDate) -> Row<'a> {
    let status = if task.completed {
        glyphs.done
    } else {
        glyphs.open
    };
    let title_style = if task.completed {
        Style::default().fg(DIM).add_modifier(Modifier::CROSSED_OUT)
    } else {
        Style::default()
    };
    let due = task.due.map_or_else(
        || Span::raw(""),
        |due| {
            let style = if due < today && !task.completed {
                Style::default().fg(ERROR)
            } else if due == today {
                Style::default().fg(ACCENT)
            } else {
                Style::default().fg(DIM)
            };
            Span::styled(due_label(due, today), style)
        },
    );
    let mut flags = Vec::new();
    if task.important {
        flags.push(Span::styled(glyphs.important, Style::default().fg(AMBER)));
    }
    if task.recurrence.is_some() {
        flags.push(Span::styled(glyphs.recurring, Style::default().fg(DIM)));
    }
    if task.reminder.is_some() {
        flags.push(Span::styled(glyphs.reminder, Style::default().fg(DIM)));
    }
    Row::new(vec![
        Cell::from(status),
        Cell::from(Span::styled(task.title.as_str(), title_style)),
        Cell::from(Line::from(due).alignment(Alignment::Right)),
        Cell::from(Line::from(flags)),
        Cell::from(sync_marker(task.sync, glyphs)),
    ])
}

/// A task's sync marker: pending dim, unknown amber, failed red, and
/// nothing once synced (docs/blueprint/08-tui.md).
pub fn sync_marker(sync: SyncMarker, glyphs: &Glyphs) -> Span<'static> {
    match sync {
        SyncMarker::Synced => Span::raw(""),
        SyncMarker::Pending => Span::styled(glyphs.pending, Style::default().fg(DIM)),
        SyncMarker::Unknown => Span::styled(glyphs.unknown, Style::default().fg(AMBER)),
        SyncMarker::Failed => Span::styled(glyphs.failed, Style::default().fg(ERROR)),
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
