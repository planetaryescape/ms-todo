//! Setting a task to nag, or stopping it: `nag` in our extension, the
//! interval in minutes, written through the outbox as My Day and
//! assignment are (D-054), so it syncs and `undo` puts it back. Only a
//! daemon notifies, so only machines running ms-todo nag.
//!
//! A nag starts at the task's reminder, so a task with no reminder is
//! refused rather than given a start time ms-todo would have to invent.

use ms_todo_protocol::{
    Clearable, ErrorPayload, NAG_MAX_MINUTES, NAG_MIN_MINUTES, TaskAction, TaskEdit,
};
use ms_todo_store::{NewOp, TaskRow};
use serde_json::{Map, Value, json};

use crate::task_children::invalid;
use crate::task_fields::reminder_at;
use crate::task_writes::task_extension_op;

/// The interval in minutes, in our extension.
pub(crate) const NAG: &str = "nag";

/// A task's nag interval in minutes, if it nags.
pub(crate) fn nag_of(row: &TaskRow) -> Option<u32> {
    row.extension
        .as_ref()?
        .get(NAG)?
        .as_u64()
        .and_then(|minutes| u32::try_from(minutes).ok())
}

/// `minutes`, or why it can't be an interval.
pub(crate) fn check(minutes: u32) -> Result<u32, ErrorPayload> {
    if (NAG_MIN_MINUTES..=NAG_MAX_MINUTES).contains(&minutes) {
        Ok(minutes)
    } else {
        Err(invalid(format!(
            "a nag repeats every {NAG_MIN_MINUTES} minutes to {} hours, not every {minutes} \
             minutes",
            NAG_MAX_MINUTES / 60
        )))
    }
}

/// A new task's `nag`, which needs its reminder.
pub(crate) fn new_task_fields(
    minutes: u32,
    reminder: Option<&str>,
) -> Result<Map<String, Value>, ErrorPayload> {
    let minutes = check(minutes)?;
    if reminder.is_none() {
        return Err(invalid(
            "a nag starts at the task's reminder, so --nag needs --reminder (or a !time in \
             the text)"
                .into(),
        ));
    }
    Ok(Map::from_iter([(NAG.to_owned(), json!(minutes))]))
}

/// An edit's nag change, checked against the edit's own reminder change,
/// and whether each task still needs a reminder of its own.
pub(crate) struct Change {
    value: Clearable<u32>,
    needs_reminder: bool,
}

impl Change {
    pub fn of(edit: &TaskEdit) -> Result<Option<Self>, ErrorPayload> {
        let value = match &edit.nag {
            None => return Ok(None),
            Some(Clearable::Set(minutes)) => Clearable::Set(check(*minutes)?),
            Some(Clearable::Clear) => Clearable::Clear,
        };
        let needs_reminder = match (&value, &edit.reminder) {
            (Clearable::Clear, _) => false,
            (Clearable::Set(_), Some(Clearable::Set(_))) => false,
            (Clearable::Set(_), Some(Clearable::Clear)) => {
                return Err(invalid(
                    "a nag starts at the task's reminder, so it can't be set while clearing \
                     the reminder"
                        .into(),
                ));
            }
            (Clearable::Set(_), None) => true,
        };
        Ok(Some(Self {
            value,
            needs_reminder,
        }))
    }

    /// Refuse the whole change when any task it would set to nag has no
    /// reminder, naming them all.
    pub fn check_reminders<'a>(
        &self,
        rows: impl IntoIterator<Item = &'a TaskRow>,
    ) -> Result<(), ErrorPayload> {
        if !self.needs_reminder {
            return Ok(());
        }
        let missing: Vec<String> = rows
            .into_iter()
            .filter(|row| reminder_at(&row.raw).is_none())
            .map(|row| format!("{:?}", row.title))
            .collect();
        if missing.is_empty() {
            return Ok(());
        }
        let (one, verb) = if missing.len() == 1 {
            ("it", "has")
        } else {
            ("each", "have")
        };
        Err(invalid(format!(
            "{} {verb} no reminder, and a nag starts at the reminder: give {one} one with \
             `tasks edit --reminder WHEN --nag EVERY`",
            missing.join(", ")
        )))
    }

    /// What it writes to `row`'s extension, or `None` when it's so
    /// already.
    pub fn fields(&self, row: &TaskRow) -> Option<Map<String, Value>> {
        let unchanged = match self.value {
            Clearable::Set(minutes) => nag_of(row) == Some(minutes),
            Clearable::Clear => nag_of(row).is_none(),
        };
        (!unchanged).then(|| Map::from_iter([(NAG.to_owned(), self.value())]))
    }

    /// What it writes: the minutes, or null to stop.
    pub fn value(&self) -> Value {
        match self.value {
            Clearable::Set(minutes) => json!(minutes),
            Clearable::Clear => Value::Null,
        }
    }

    /// The extension write for `row`, if it changes anything.
    pub fn op(&self, op_id: String, row: &TaskRow, action: TaskAction) -> Option<NewOp> {
        self.fields(row)
            .map(|fields| task_extension_op(op_id, row, fields, action))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(title: &str, reminder: bool, nag: Option<u32>) -> TaskRow {
        let mut raw = json!({ "id": "T1", "status": "notStarted" });
        if reminder {
            raw["reminderDateTime"] =
                json!({ "dateTime": "2026-09-27T09:00:00.0000000", "timeZone": "UTC" });
            raw["isReminderOn"] = json!(true);
        }
        TaskRow {
            local_id: "t1".into(),
            graph_id: Some("T1".into()),
            list_local_id: "l1".into(),
            title: title.into(),
            raw: raw.as_object().cloned().expect("object"),
            extension: nag.map(|minutes| json!({ "nag": minutes, "myDay": "x" })),
            sync_state: "synced".into(),
        }
    }

    fn edit(nag: Option<Clearable<u32>>, reminder: Option<Clearable<String>>) -> TaskEdit {
        TaskEdit {
            nag,
            reminder,
            ..TaskEdit::default()
        }
    }

    #[test]
    fn an_interval_is_five_minutes_to_a_day() {
        assert_eq!(check(5).expect("5"), 5);
        assert_eq!(check(1440).expect("1440"), 1440);
        assert!(check(4).is_err());
        assert!(check(1441).is_err());
    }

    #[test]
    fn a_task_without_a_reminder_is_refused_unless_the_edit_gives_it_one() {
        let change = Change::of(&edit(Some(Clearable::Set(15)), None))
            .expect("valid")
            .expect("a change");
        let bare = row("Call mum", false, None);
        let error = change.check_reminders([&bare]).expect_err("no reminder");
        assert!(
            error.message.contains("\"Call mum\" has no reminder"),
            "{}",
            error.message
        );
        assert!(change.check_reminders([&row("x", true, None)]).is_ok());
        let with_reminder = edit(Some(Clearable::Set(15)), Some(Clearable::Set("x".into())));
        let change = Change::of(&with_reminder)
            .expect("valid")
            .expect("a change");
        assert!(change.check_reminders([&bare]).is_ok());
        let clearing = edit(Some(Clearable::Set(15)), Some(Clearable::Clear));
        assert!(Change::of(&clearing).is_err());
        // Stopping needs nothing.
        let off = Change::of(&edit(Some(Clearable::Clear), None))
            .expect("valid")
            .expect("a change");
        assert!(off.check_reminders([&bare]).is_ok());
    }

    #[test]
    fn only_a_change_is_written() {
        let set = Change::of(&edit(Some(Clearable::Set(15)), None))
            .expect("valid")
            .expect("a change");
        assert_eq!(set.fields(&row("x", true, Some(15))), None);
        assert_eq!(
            set.fields(&row("x", true, Some(30))),
            Some(Map::from_iter([("nag".to_owned(), json!(15))]))
        );
        let off = Change::of(&edit(Some(Clearable::Clear), None))
            .expect("valid")
            .expect("a change");
        assert_eq!(off.fields(&row("x", true, None)), None);
        assert_eq!(
            off.fields(&row("x", true, Some(15))),
            Some(Map::from_iter([("nag".to_owned(), Value::Null)]))
        );
    }
}
