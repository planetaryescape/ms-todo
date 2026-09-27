//! Defer and Someday (docs/blueprint/05-custom-features.md#defer-and-someday,
//! rung 9a): `deferUntil` and `someday` in our extension, written through
//! the `task_extension` GET, merge and whole-document write, as My Day and
//! the assignee are. Nothing else changes on the task: its due date stays
//! the deadline the phone shows, and the phone doesn't hide it.
//!
//! Also what the everyday reads leave out: an open task that's Someday, or
//! deferred to a day after today (`ms_todo_core::deferral::hidden`). A
//! later rung's contexts are one more predicate beside [`hidden`].

use chrono::NaiveDate;
use ms_todo_core::DATE_FORMAT;
use ms_todo_core::deferral::{self, DEFER_UNTIL, SOMEDAY};
use ms_todo_protocol::{Clearable, DeferredFilter, ErrorPayload, NewTask, TaskEdit};
use ms_todo_store::TaskRow;
use serde_json::{Map, Value, json};

use crate::my_day::completed;
use crate::task_fields::parse_day;

/// The local day the defer rule reads against: a task deferred to a day
/// shows again from that day's midnight, as `--due today` counts days.
pub(crate) fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

/// A task's defer day, from our extension.
pub(crate) fn defer_until(row: &TaskRow) -> Option<NaiveDate> {
    deferral::defer_until(row.extension.as_ref())
}

/// Whether `row` is out of the everyday views on `today`.
pub(crate) fn hidden(row: &TaskRow, today: NaiveDate) -> bool {
    deferral::hidden(
        defer_until(row),
        deferral::someday(row.extension.as_ref()),
        completed(row),
        today,
    )
}

/// `rows` as `filter` wants them on `today`, keeping their order, and how
/// many it left out for being deferred or Someday.
pub(crate) fn keep(
    rows: Vec<TaskRow>,
    filter: DeferredFilter,
    today: NaiveDate,
) -> (Vec<TaskRow>, u64) {
    match filter {
        DeferredFilter::Include => (rows, 0),
        DeferredFilter::Hide => {
            let before = rows.len();
            let kept: Vec<TaskRow> = rows.into_iter().filter(|row| !hidden(row, today)).collect();
            let left_out = u64::try_from(before - kept.len()).unwrap_or(u64::MAX);
            (kept, left_out)
        }
        DeferredFilter::Only => (
            rows.into_iter().filter(|row| hidden(row, today)).collect(),
            0,
        ),
    }
}

/// The filter a read applies: a search finds deferred tasks too, since
/// looking for one is asking for it.
pub(crate) fn searched(filter: DeferredFilter, search: Option<&str>) -> DeferredFilter {
    match filter {
        DeferredFilter::Hide if search.is_some() => DeferredFilter::Include,
        filter => filter,
    }
}

/// `--deferred only`'s order when nothing else asks for one: deferred
/// tasks by the day they come back, then Someday. Stable, so ties keep
/// their order.
pub(crate) fn sort_by_return(rows: &mut [TaskRow]) {
    rows.sort_by_cached_key(|row| {
        let someday = deferral::someday(row.extension.as_ref());
        (someday, defer_until(row))
    });
}

/// A new task's defer fields for its create's extension, if any.
pub(crate) fn new_task_fields(task: &NewTask) -> Result<Map<String, Value>, ErrorPayload> {
    let mut fields = Map::new();
    if let Some(day) = &task.defer_until {
        fields.insert(DEFER_UNTIL.into(), json!(day_text(day)?));
    }
    if task.someday {
        fields.insert(SOMEDAY.into(), json!(true));
    }
    Ok(fields)
}

/// The extension fields an edit sets, a null removing one; empty when it
/// doesn't touch them. Taking a task out of Someday removes the field
/// rather than writing `false`, so the extension holds only what's set.
pub(crate) fn edit_fields(edit: &TaskEdit) -> Result<Map<String, Value>, ErrorPayload> {
    let mut fields = Map::new();
    if let Some(change) = &edit.defer_until {
        let value = match change {
            Clearable::Set(day) => json!(day_text(day)?),
            Clearable::Clear => Value::Null,
        };
        fields.insert(DEFER_UNTIL.into(), value);
    }
    if let Some(someday) = edit.someday {
        let value = if someday { json!(true) } else { Value::Null };
        fields.insert(SOMEDAY.into(), value);
    }
    Ok(fields)
}

/// Of `fields`, those that change `row`'s extension; `None` when none
/// does, so an edit to what's already there queues nothing.
pub(crate) fn plan(row: &TaskRow, fields: &Map<String, Value>) -> Option<Map<String, Value>> {
    let now = |key: &str| {
        row.extension
            .as_ref()
            .and_then(|extension| extension.get(key))
            .filter(|value| !value.is_null())
    };
    let changes: Map<String, Value> = fields
        .iter()
        .filter(|(key, value)| now(key) != Some(*value).filter(|value| !value.is_null()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    (!changes.is_empty()).then_some(changes)
}

fn day_text(day: &str) -> Result<String, ErrorPayload> {
    Ok(parse_day(day)?.format(DATE_FORMAT).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: &str, extension: Option<Value>) -> TaskRow {
        TaskRow {
            local_id: "t1".into(),
            graph_id: Some("T1".into()),
            list_local_id: "l1".into(),
            title: "Plan the trip".into(),
            raw: json!({ "id": "T1", "status": status })
                .as_object()
                .cloned()
                .expect("object"),
            extension,
            sync_state: "synced".into(),
        }
    }

    fn day(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, DATE_FORMAT).expect("day")
    }

    #[test]
    fn hide_counts_what_it_leaves_out_and_only_keeps_just_that() {
        let rows = || {
            vec![
                row("notStarted", None),
                row("notStarted", Some(json!({ "deferUntil": "2026-10-02" }))),
                row("notStarted", Some(json!({ "someday": true }))),
                row("completed", Some(json!({ "someday": true }))),
            ]
        };
        let today = day("2026-10-01");
        let (kept, hidden) = keep(rows(), DeferredFilter::Hide, today);
        assert_eq!((kept.len(), hidden), (2, 2));
        let (only, _) = keep(rows(), DeferredFilter::Only, today);
        assert_eq!(only.len(), 2);
        assert_eq!(keep(rows(), DeferredFilter::Include, today).0.len(), 4);
        // On its day it's back.
        assert_eq!(keep(rows(), DeferredFilter::Hide, day("2026-10-02")).1, 1);
    }

    #[test]
    fn edits_set_and_remove_fields_and_skip_what_is_already_there() {
        let edit = TaskEdit {
            defer_until: Some(Clearable::Set("2026-10-02".into())),
            someday: Some(false),
            ..TaskEdit::default()
        };
        let fields = edit_fields(&edit).expect("fields");
        assert_eq!(
            Value::Object(fields.clone()),
            json!({ "deferUntil": "2026-10-02", "someday": null })
        );
        let plain = row("notStarted", None);
        assert_eq!(
            plan(&plain, &fields).map(Value::Object),
            Some(json!({ "deferUntil": "2026-10-02" })),
            "removing what isn't there is nothing"
        );
        let already = row("notStarted", Some(json!({ "deferUntil": "2026-10-02" })));
        assert_eq!(plan(&already, &fields), None);
        let bad = TaskEdit {
            defer_until: Some(Clearable::Set("fri".into())),
            ..TaskEdit::default()
        };
        assert!(edit_fields(&bad).is_err());
    }

    #[test]
    fn return_order_is_by_day_then_someday() {
        let mut rows = vec![
            row("notStarted", Some(json!({ "someday": true }))),
            row("notStarted", Some(json!({ "deferUntil": "2026-10-09" }))),
            row("notStarted", Some(json!({ "deferUntil": "2026-10-02" }))),
        ];
        sort_by_return(&mut rows);
        let order: Vec<Option<NaiveDate>> = rows.iter().map(defer_until).collect();
        assert_eq!(
            order,
            [Some(day("2026-10-02")), Some(day("2026-10-09")), None]
        );
    }
}
