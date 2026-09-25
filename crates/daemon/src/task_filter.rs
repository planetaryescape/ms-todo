//! `tasks list`'s filters and order (docs/blueprint/07-cli.md): applied to
//! the cached rows in memory, which at To Do's scale is quicker to get
//! right than a query per combination, and reads due dates by the same
//! rounding rule as everything else (S11).

use std::cmp::Ordering;

use chrono::NaiveDate;
use ms_todo_protocol::{DueFilter, ErrorPayload, Importance, TaskFilter, TaskSort};
use ms_todo_store::TaskRow;
use serde_json::Value;

use crate::task_fields::{graph_due_date, parse_day};

/// A [`DueFilter`] with its days read.
enum Due {
    Today,
    Overdue,
    None,
    Any,
    On(NaiveDate),
    Before(NaiveDate),
    After(NaiveDate),
}

/// `rows` narrowed and ordered by `filter`, `today` being the local day.
/// Without a sort they keep the order they came in.
pub(crate) fn apply(
    rows: Vec<TaskRow>,
    filter: &TaskFilter,
    today: NaiveDate,
) -> Result<Vec<TaskRow>, ErrorPayload> {
    let due = filter.due.as_ref().map(read_due).transpose()?;
    let category = filter.category.as_deref().map(str::trim);
    let mut kept: Vec<TaskRow> = rows
        .into_iter()
        .filter(|row| {
            let status = text(row, "status");
            filter.status.is_none_or(|wanted| wanted.matches(status))
                && filter
                    .importance
                    .is_none_or(|wanted| text(row, "importance") == importance_name(wanted))
                && category.is_none_or(|wanted| has_category(row, wanted))
                && due
                    .as_ref()
                    .is_none_or(|due| due_matches(due, row, status, today))
        })
        .collect();
    match filter.sort {
        // Keys read once per task: a due date is parsed and converted. No
        // due date sorts last; ties keep their order (the sort is stable).
        Some(TaskSort::Due) => kept.sort_by_cached_key(|row| {
            let due = graph_due_date(&row.raw);
            (due.is_none(), due)
        }),
        Some(TaskSort::Title) => kept.sort_by_cached_key(|row| row.title.to_lowercase()),
        Some(sort) => kept.sort_by(|one, other| compare(sort, one, other)),
        None => {}
    }
    if let Some(limit) = filter.limit {
        kept.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    }
    Ok(kept)
}

fn read_due(due: &DueFilter) -> Result<Due, ErrorPayload> {
    Ok(match due {
        DueFilter::Today => Due::Today,
        DueFilter::Overdue => Due::Overdue,
        DueFilter::None => Due::None,
        DueFilter::Any => Due::Any,
        DueFilter::On(day) => Due::On(parse_day(day)?),
        DueFilter::Before(day) => Due::Before(parse_day(day)?),
        DueFilter::After(day) => Due::After(parse_day(day)?),
    })
}

fn due_matches(due: &Due, row: &TaskRow, status: &str, today: NaiveDate) -> bool {
    let date = graph_due_date(&row.raw);
    match due {
        Due::Today => date == Some(today),
        // Overdue is open by definition: a completed task is done, late or not.
        Due::Overdue => status != "completed" && date.is_some_and(|date| date < today),
        Due::None => date.is_none(),
        Due::Any => date.is_some(),
        Due::On(day) => date == Some(*day),
        Due::Before(day) => date.is_some_and(|date| date < *day),
        Due::After(day) => date.is_some_and(|date| date > *day),
    }
}

fn has_category(row: &TaskRow, wanted: &str) -> bool {
    row.raw
        .get("categories")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|category| category.trim().eq_ignore_ascii_case(wanted))
}

fn compare(sort: TaskSort, one: &TaskRow, other: &TaskRow) -> Ordering {
    match sort {
        TaskSort::Importance => rank(one).cmp(&rank(other)),
        // RFC 3339 in UTC sorts as text; newest first.
        TaskSort::Created => text(other, "createdDateTime").cmp(text(one, "createdDateTime")),
        TaskSort::Modified => {
            text(other, "lastModifiedDateTime").cmp(text(one, "lastModifiedDateTime"))
        }
        // Sorted by cached keys in `apply`.
        TaskSort::Due | TaskSort::Title => Ordering::Equal,
    }
}

/// High first.
fn rank(row: &TaskRow) -> u8 {
    match text(row, "importance") {
        "high" => 0,
        "low" => 2,
        _ => 1,
    }
}

fn importance_name(importance: Importance) -> &'static str {
    match importance {
        Importance::Low => "low",
        Importance::Normal => "normal",
        Importance::High => "high",
    }
}

fn text<'a>(row: &'a TaskRow, key: &str) -> &'a str {
    row.raw.get(key).and_then(Value::as_str).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use ms_todo_protocol::StatusFilter;
    use serde_json::json;

    use super::*;

    fn row(title: &str, raw: Value) -> TaskRow {
        let mut raw = raw.as_object().cloned().expect("object");
        raw.insert("title".into(), json!(title));
        TaskRow {
            local_id: title.into(),
            graph_id: None,
            list_local_id: "l1".into(),
            title: title.into(),
            raw,
            extension: None,
            sync_state: "synced".into(),
        }
    }

    fn due(day: &str) -> Value {
        json!({ "dateTime": format!("{day}T00:00:00.0000000"), "timeZone": "UTC" })
    }

    fn titles(rows: &[TaskRow]) -> Vec<&str> {
        rows.iter().map(|row| row.title.as_str()).collect()
    }

    fn rows() -> Vec<TaskRow> {
        vec![
            row(
                "late",
                json!({ "status": "notStarted", "importance": "high", "dueDateTime": due("2026-09-20"),
                        "categories": ["Errands"], "createdDateTime": "2026-09-01T00:00:00Z" }),
            ),
            row(
                "done late",
                json!({ "status": "completed", "importance": "normal", "dueDateTime": due("2026-09-21"),
                        "createdDateTime": "2026-09-02T00:00:00Z" }),
            ),
            row(
                "today",
                json!({ "status": "inProgress", "importance": "low", "dueDateTime": due("2026-09-25"),
                        "createdDateTime": "2026-09-03T00:00:00Z" }),
            ),
            row(
                "someday",
                json!({ "status": "notStarted", "importance": "normal", "categories": ["errands "],
                        "createdDateTime": "2026-09-04T00:00:00Z" }),
            ),
        ]
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 9, 25).expect("day")
    }

    fn filtered(filter: TaskFilter) -> Vec<String> {
        titles(&apply(rows(), &filter, today()).expect("filter"))
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn due_filters_read_local_days_and_overdue_is_open_only() {
        let by = |due| {
            filtered(TaskFilter {
                due: Some(due),
                ..TaskFilter::default()
            })
        };
        assert_eq!(by(DueFilter::Overdue), ["late"]);
        assert_eq!(by(DueFilter::Today), ["today"]);
        assert_eq!(by(DueFilter::None), ["someday"]);
        assert_eq!(by(DueFilter::Any), ["late", "done late", "today"]);
        assert_eq!(
            by(DueFilter::Before("2026-09-25".into())),
            ["late", "done late"]
        );
        assert_eq!(by(DueFilter::After("2026-09-21".into())), ["today"]);
        assert_eq!(by(DueFilter::On("2026-09-21".into())), ["done late"]);
        let bad = apply(
            rows(),
            &TaskFilter {
                due: Some(DueFilter::On("soon".into())),
                ..TaskFilter::default()
            },
            today(),
        );
        assert!(bad.is_err());
    }

    #[test]
    fn status_importance_and_category_narrow_together() {
        assert_eq!(
            filtered(TaskFilter {
                status: Some(StatusFilter::Open),
                ..TaskFilter::default()
            }),
            ["late", "today", "someday"]
        );
        assert_eq!(
            filtered(TaskFilter {
                status: Some(StatusFilter::InProgress),
                ..TaskFilter::default()
            }),
            ["today"]
        );
        assert_eq!(
            filtered(TaskFilter {
                category: Some("ERRANDS".into()),
                importance: Some(Importance::Normal),
                ..TaskFilter::default()
            }),
            ["someday"]
        );
    }

    #[test]
    fn sorts_are_stable_and_the_limit_comes_after() {
        let sorted = |sort| {
            filtered(TaskFilter {
                sort: Some(sort),
                ..TaskFilter::default()
            })
        };
        assert_eq!(
            sorted(TaskSort::Due),
            ["late", "done late", "today", "someday"]
        );
        assert_eq!(
            sorted(TaskSort::Importance),
            ["late", "done late", "someday", "today"]
        );
        assert_eq!(
            sorted(TaskSort::Created),
            ["someday", "today", "done late", "late"]
        );
        assert_eq!(
            sorted(TaskSort::Title),
            ["done late", "late", "someday", "today"]
        );
        assert_eq!(
            filtered(TaskFilter {
                sort: Some(TaskSort::Due),
                limit: Some(1),
                ..TaskFilter::default()
            }),
            ["late"]
        );
    }
}
