//! The Start and Repeat fields (D-066): read as the CLI's `--start` and
//! `--recur` are, and refused where the CLI's daemon would refuse them,
//! before anything is sent.

use chrono::NaiveDate;
use ms_todo_core::DATE_FORMAT;
use ms_todo_nlp::{ParseContext, read_due, read_recurrence};
use ms_todo_protocol::{Clearable, TaskEdit};

use super::{capitalised, clearable, set_value};
use crate::app::Task;
use crate::app::edit::Field;

/// The Start field's edit; `None` when it's what the task has. A start
/// date on a repeating task is refused: Microsoft To Do keeps none of its
/// own there (D-058), which the daemon refuses too.
pub(super) fn start_edit(
    typed: &str,
    task: &Task,
    now: &ParseContext,
) -> Result<Option<TaskEdit>, String> {
    let start = set_value(read_due(typed, now))?;
    if start == task.start {
        return Ok(None);
    }
    if start.is_some() && task.recurrence.is_some() {
        return Err(
            "It repeats, and Microsoft To Do keeps no start date on a repeating task; \
             empty Repeat first"
                .into(),
        );
    }
    Ok(Some(TaskEdit {
        start: Some(clearable(start.map(day))),
        ..TaskEdit::default()
    }))
}

/// The Repeat field's edit: a recurrence as `--recur` reads it, first due
/// on the next day it falls on from today, as `tasks edit --recur` without
/// `--due`; empty or `-` stops it repeating. `None` when it's unchanged,
/// including the phrase it was opened with.
pub(super) fn repeat_edit(
    typed: &str,
    task: &Task,
    now: &ParseContext,
) -> Result<Option<TaskEdit>, String> {
    let recurrence = if typed.is_empty() || typed == "-" {
        if task.recurrence.is_none() {
            return Ok(None);
        }
        Clearable::Clear
    } else if Field::Repeat.current(task) == typed {
        return Ok(None);
    } else {
        let read = read_recurrence(typed, now, None).map_err(|why| capitalised(&why.0))?;
        Clearable::Set(read.to_graph())
    };
    Ok(Some(TaskEdit {
        recurrence: Some(recurrence),
        ..TaskEdit::default()
    }))
}

/// What the Repeat field reads as it's typed: `→ every Monday, first due
/// Mon 28 Sep`, or what emptying it does.
pub(super) fn repeat_preview(typed: &str, now: &ParseContext) -> Result<String, String> {
    let typed = typed.trim();
    if typed.is_empty() || typed == "-" {
        return Ok("\u{2192} none: stops it repeating; its due date stays".into());
    }
    let read = read_recurrence(typed, now, None).map_err(|why| capitalised(&why.0))?;
    Ok(format!(
        "\u{2192} {}, first due {}",
        read.describe(),
        read.start.format("%a %-d %b")
    ))
}

fn day(date: NaiveDate) -> String {
    date.format(DATE_FORMAT).to_string()
}

#[cfg(test)]
mod tests {
    use ms_todo_protocol::{Request, TaskChange};
    use serde_json::json;

    use super::*;
    use crate::action::Action;
    use crate::app::edit::Field;
    use crate::app::tests::{act, seeded};
    use crate::app::{Effect, Mode, Msg};

    fn task(extra: serde_json::Value) -> Task {
        let mut entity = json!({ "id": "t1", "title": "Water plants" });
        entity
            .as_object_mut()
            .expect("object")
            .extend(extra.as_object().cloned().expect("object"));
        Task::from_entity(entity.as_object().expect("object")).expect("task")
    }

    fn repeating() -> Task {
        task(json!({
            "dueDateTime": { "dateTime": "2026-10-01T00:00:00.0000000", "timeZone": "Europe/London" },
            "recurrence": { "pattern": { "type": "weekly", "interval": 1, "daysOfWeek": ["monday"] } }
        }))
    }

    fn now() -> ParseContext {
        seeded().parse_context()
    }

    #[test]
    fn a_start_date_is_read_as_due_dates_are_and_cleared_by_nothing() {
        let plain = task(json!({}));
        let edit = start_edit("next mon", &plain, &now()).expect("read");
        assert_eq!(
            edit.and_then(|edit| edit.start),
            Some(Clearable::Set("2026-09-28".into()))
        );
        assert_eq!(start_edit("", &plain, &now()), Ok(None), "none to clear");
        let started = task(json!({
            "startDateTime": { "dateTime": "2026-09-28T00:00:00.0000000", "timeZone": "Europe/London" }
        }));
        assert_eq!(start_edit("2026-09-28", &started, &now()), Ok(None));
        let cleared = start_edit("-", &started, &now()).expect("read");
        assert_eq!(cleared.and_then(|edit| edit.start), Some(Clearable::Clear));
        assert!(start_edit("someday soon", &plain, &now()).is_err());
    }

    #[test]
    fn a_repeating_task_refuses_a_start_date_but_can_lose_one() {
        let why = start_edit("fri", &repeating(), &now()).expect_err("refused");
        assert!(why.contains("repeats"), "{why}");
        assert_eq!(start_edit("", &repeating(), &now()), Ok(None));
        // Its start date left as it was is no change, not a refusal.
        let mut started = repeating();
        started.start = chrono::NaiveDate::from_ymd_opt(2026, 9, 28);
        assert_eq!(start_edit("2026-09-28", &started, &now()), Ok(None));
    }

    /// Every recurrence the phrases make opens in the Repeat field as a
    /// phrase that reads back as the same recurrence.
    #[test]
    fn every_recurrence_opens_as_a_phrase_that_reads_back_the_same() {
        for typed in [
            "daily",
            "every 3 days",
            "every weekday",
            "every weekend",
            "every mon, wed",
            "every week",
            "every other week",
            "every 2 weeks on fri",
            "every 3 weeks on tue, thu",
            "every month",
            "every 1st",
            "every month on the 15th",
            "every 2 months on the 31st",
            "every last fri",
            "every second tue",
            "every 3 months on the first mon",
            "every year",
            "every 12 oct",
            "every 29 feb",
            "every mon until 31 dec",
            "every day for 10 times",
            "every month on the 1st for 1 times",
        ] {
            let graph = read_recurrence(typed, &now(), None)
                .unwrap_or_else(|why| unreachable!("{typed}: {why:?}"))
                .to_graph();
            let phrase = crate::app::task::recurrence_phrase(&graph)
                .unwrap_or_else(|| unreachable!("{typed}: no phrase for {graph}"));
            let again = read_recurrence(&phrase, &now(), None)
                .unwrap_or_else(|why| unreachable!("{typed} -> {phrase}: {why:?}"))
                .to_graph();
            assert_eq!(again, graph, "{typed} -> {phrase}");
        }
        // No words for it: the editor opens on its description, and Enter
        // on that changes nothing.
        let every_other_year = task(json!({
            "recurrence": read_recurrence("every 2 years", &now(), None).expect("read").to_graph()
        }));
        assert_eq!(every_other_year.repeat_phrase, None);
        let shown = Field::Repeat.current(&every_other_year);
        assert_eq!(Some(&shown), every_other_year.recurrence.as_ref());
        assert_eq!(repeat_edit(&shown, &every_other_year, &now()), Ok(None));
    }

    #[test]
    fn repeat_reads_the_clis_phrases_and_nothing_stops_it() {
        let plain = task(json!({}));
        let edit = repeat_edit("every mon", &plain, &now())
            .expect("read")
            .expect("an edit");
        let Some(Clearable::Set(recurrence)) = edit.recurrence else {
            unreachable!("set: {edit:?}");
        };
        assert_eq!(recurrence["pattern"]["type"], "weekly");
        assert_eq!(recurrence["pattern"]["daysOfWeek"], json!(["monday"]));
        // From today (Thu 24 Sep), as `tasks edit --recur` without --due.
        assert_eq!(recurrence["range"]["startDate"], "2026-09-28");
        assert_eq!(repeat_edit("", &plain, &now()), Ok(None));
        let stopped = repeat_edit("-", &repeating(), &now()).expect("read");
        assert_eq!(
            stopped.and_then(|edit| edit.recurrence),
            Some(Clearable::Clear)
        );
        // Opened on its phrase and left alone: nothing to send.
        let shown = repeating().repeat_phrase.expect("a phrase");
        assert_eq!(shown, "every mon");
        assert_eq!(repeat_edit(&shown, &repeating(), &now()), Ok(None));
        assert!(repeat_edit("every mon at 9am", &plain, &now()).is_err());
        assert_eq!(
            repeat_preview("weekday", &now()),
            Ok("\u{2192} every weekday, first due Thu 24 Sep".into())
        );
    }

    #[test]
    fn repeat_from_the_field_picker_sends_the_recurrence() {
        let mut app = seeded();
        act(&mut app, Action::MoveDown);
        act(&mut app, Action::Edit);
        act(&mut app, Action::EditField(Field::Repeat));
        assert!(matches!(
            &app.mode,
            Mode::Editing {
                field: Field::Repeat,
                ..
            }
        ));
        for ch in "daily".chars() {
            app.update(Msg::Char(ch));
        }
        let effects = act(&mut app, Action::Submit);
        let [
            Effect {
                request:
                    Request::ChangeTasks {
                        tasks,
                        change: TaskChange::Edit(edit),
                        ..
                    },
                ..
            },
        ] = effects.as_slice()
        else {
            unreachable!("one edit: {effects:?}");
        };
        assert_eq!(tasks, &["t2"]);
        assert!(matches!(edit.recurrence, Some(Clearable::Set(_))));
        assert_eq!(app.mode, Mode::Normal);
    }
}
