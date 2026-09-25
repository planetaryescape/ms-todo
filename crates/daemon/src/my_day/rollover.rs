//! The daily My Day rollover (docs/blueprint/05-custom-features.md#my-day,
//! D-024): every task in an earlier day's My Day leaves it, and an open one
//! loses the due date ms-todo set for it, if that's still its due date. A
//! completed task keeps its due date.
//!
//! It runs at the first tick after `my_day.rollover_time` each day, once,
//! recorded as `my_day.last_rollover`, and once when the daemon starts
//! after days off. It's one command of per-task operations, so `outbox
//! list` shows it and one `undo` reverses it. Each extension write expects
//! the task still to be in the My Day it was planned from, so a task put
//! back in My Day on another machine meanwhile is left there. Running it
//! again finds nothing to do.

use std::sync::Arc;
use std::time::Duration;

use chrono::NaiveDate;
use ms_todo_core::DATE_FORMAT;
use ms_todo_protocol::{ErrorPayload, ResponseData, TaskAction};
use ms_todo_store::{LISTS_SCOPE, TaskRow};
use serde_json::{Value, json};

use super::{
    MY_DAY, TaskPlan, command_ops, completed, config::rollover_due, dry_run_plan, due_changed,
    my_day_of, remove_plan,
};
use crate::handlers::{State, store_error};
use crate::task_writes::queue;

/// The day the last rollover ran for.
pub(super) const LAST_ROLLOVER: &str = "my_day.last_rollover";
/// The open tasks the last rollover took out: `{ "from": day, "tasks":
/// [local IDs] }`, which My Day suggests.
pub(super) const LEFT_OVER: &str = "my_day.left_over";

/// How often the daemon looks at the clock.
const TICK: Duration = Duration::from_secs(60);

/// Taking `row` out of an earlier day's My Day, if it's in one before
/// `today`.
pub(crate) fn rollover_plan(row: &TaskRow, today: NaiveDate) -> Option<TaskPlan> {
    let day = my_day_of(row).filter(|day| *day < today)?;
    let mut plan = remove_plan(row)?;
    plan.expect = Some(json!({ MY_DAY: day.format(DATE_FORMAT).to_string() }));
    Some(plan)
}

/// Who started a rollover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Origin {
    /// `myday rollover`.
    User,
    /// The daemon's daily run. Its operations carry `"origin": "auto"`, so
    /// a plain `undo` passes over it for the user's own last change.
    Auto,
}

/// `myday rollover [--dry-run]`, and the daily run.
pub(crate) async fn rollover(
    state: &State,
    dry_run: bool,
    op_id: String,
    origin: Origin,
) -> Result<ResponseData, ErrorPayload> {
    let today = state.my_day.today();
    let rows = state
        .store
        .my_day_before(today)
        .await
        .map_err(store_error)?;
    let planned: Vec<(&TaskRow, TaskPlan)> = rows
        .iter()
        .filter_map(|row| rollover_plan(row, today).map(|plan| (row, plan)))
        .collect();
    if dry_run {
        let changes = json!({
            "date": today.format(DATE_FORMAT).to_string(),
            MY_DAY: null,
            "due_cleared": due_changed(&planned),
        });
        let plan = dry_run_plan(TaskAction::MyDayRollover, &planned, changes);
        return Ok(ResponseData::Plan(plan));
    }
    let mut ops = command_ops(&op_id, &planned, TaskAction::MyDayRollover);
    if origin == Origin::Auto {
        for op in &mut ops {
            op.payload["origin"] = json!("auto");
        }
    }
    let answer = queue(state, &op_id, None, ops, TaskAction::MyDayRollover).await?;
    let today_text = today.format(DATE_FORMAT).to_string();
    let left_over = left_over(&planned).to_string();
    let mut settings = vec![(LAST_ROLLOVER, today_text.as_str())];
    // A rerun with nothing to take out keeps what the last one left.
    if !planned.is_empty() {
        settings.push((LEFT_OVER, left_over.as_str()));
    }
    state
        .store
        .set_settings(&settings)
        .await
        .map_err(store_error)?;
    Ok(answer)
}

/// What the rollover leaves for My Day to suggest: its open tasks, and the
/// latest day they were in My Day for.
fn left_over(planned: &[(&TaskRow, TaskPlan)]) -> Value {
    let open: Vec<&TaskRow> = planned
        .iter()
        .map(|(row, _)| *row)
        .filter(|row| !completed(row))
        .collect();
    let from = open
        .iter()
        .filter_map(|row| my_day_of(row))
        .max()
        .map(|day| day.format(DATE_FORMAT).to_string());
    json!({
        "from": from,
        "tasks": open.iter().map(|row| row.local_id.as_str()).collect::<Vec<_>>(),
    })
}

/// The day the last rollover ran for.
pub(crate) async fn last_rollover(state: &State) -> Result<Option<NaiveDate>, ErrorPayload> {
    Ok(state
        .store
        .setting(LAST_ROLLOVER)
        .await
        .map_err(store_error)?
        .and_then(|day| NaiveDate::parse_from_str(&day, DATE_FORMAT).ok()))
}

/// Roll My Day over when it's due, for as long as the daemon runs. It
/// waits for the first sync pass, so a catch-up after days off plans from
/// what Graph holds now.
pub(crate) async fn run(state: Arc<State>) {
    let mut syncs = state.syncer.subscribe();
    if syncs.wait_for(|status| status.finished > 0).await.is_err() {
        return;
    }
    loop {
        tick(&state).await;
        tokio::time::sleep(TICK).await;
    }
}

async fn tick(state: &State) {
    // Nothing is known until the lists have synced once; a rollover then
    // would record the day as done with nothing taken out. Only looked
    // at: asking `freshness` would start a sync each minute while offline.
    match state.store.scope(LISTS_SCOPE).await {
        Ok(Some(row)) if row.is_ready() => {}
        _ => return,
    }
    let today = state.my_day.today();
    let last = match last_rollover(state).await {
        Ok(last) => last,
        Err(error) => {
            eprintln!(
                "ms-todo daemon: cannot read the last My Day rollover: {}",
                error.message
            );
            return;
        }
    };
    if !rollover_due(last, today) {
        return;
    }
    let op_id = uuid::Uuid::new_v4().to_string();
    match rollover(state, false, op_id.clone(), Origin::Auto).await {
        Ok(ResponseData::Applied(applied)) => eprintln!(
            "ms-todo daemon: My Day rolled over to {today}: {} task(s) taken out (op {op_id})",
            applied.items.len()
        ),
        Ok(_) => {}
        Err(error) => eprintln!(
            "ms-todo daemon: the My Day rollover failed; it's tried again in a minute: {}",
            error.message
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: &str, due: Option<&str>, extension: Value) -> TaskRow {
        let mut raw = json!({ "status": status });
        if let Some(due) = due {
            raw["dueDateTime"] =
                json!({ "dateTime": format!("{due}T00:00:00.0000000"), "timeZone": "UTC" });
        }
        TaskRow {
            local_id: format!("t-{status}-{}", due.unwrap_or("none")),
            graph_id: Some("T".into()),
            list_local_id: "l1".into(),
            title: "t".into(),
            raw: raw.as_object().cloned().expect("object"),
            extension: Some(extension),
            sync_state: "synced".into(),
        }
    }

    fn day(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, DATE_FORMAT).expect("day")
    }

    #[test]
    fn an_open_task_loses_the_due_date_ms_todo_set_and_expects_yesterday() {
        let task = row(
            "notStarted",
            Some("2026-09-24"),
            json!({ "myDay": "2026-09-24", "myDayDueSet": true }),
        );
        let plan = rollover_plan(&task, day("2026-09-25")).expect("plan");
        assert_eq!(plan.due, Some(None));
        assert_eq!(plan.expect, Some(json!({ "myDay": "2026-09-24" })));
        assert_eq!(
            Value::Object(plan.fields),
            json!({ "myDay": null, "myDayDueSet": null })
        );
    }

    #[test]
    fn a_users_due_date_and_a_completed_tasks_are_kept() {
        let users = row(
            "notStarted",
            Some("2026-09-30"),
            json!({ "myDay": "2026-09-24", "myDayDueSet": true }),
        );
        assert_eq!(
            rollover_plan(&users, day("2026-09-25")).expect("plan").due,
            None
        );
        let done = row(
            "completed",
            Some("2026-09-24"),
            json!({ "myDay": "2026-09-24", "myDayDueSet": true }),
        );
        let plan = rollover_plan(&done, day("2026-09-25")).expect("plan");
        assert_eq!(plan.due, None);
        assert_eq!(plan.fields["myDay"], Value::Null);
    }

    #[test]
    fn todays_my_day_stays() {
        let today = row("notStarted", None, json!({ "myDay": "2026-09-25" }));
        assert_eq!(rollover_plan(&today, day("2026-09-25")), None);
    }

    #[test]
    fn what_is_left_over_is_the_open_tasks() {
        let open = row(
            "notStarted",
            Some("2026-09-23"),
            json!({ "myDay": "2026-09-23" }),
        );
        let done = row("completed", None, json!({ "myDay": "2026-09-24" }));
        let planned: Vec<(&TaskRow, TaskPlan)> = [&open, &done]
            .into_iter()
            .map(|row| (row, rollover_plan(row, day("2026-09-25")).expect("plan")))
            .collect();
        assert_eq!(
            left_over(&planned),
            json!({ "from": "2026-09-23", "tasks": [open.local_id] })
        );
    }
}
