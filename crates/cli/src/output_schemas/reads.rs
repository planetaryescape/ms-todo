//! What read commands print beyond a bare entity: `tasks parse`, `lists
//! show`, and tasks found by `tasks list`, `search`, `waiting`, `myday
//! suggest` and `done`.

use serde_json::{Value, json};

use super::builders::{nullable, object, versioned};
use super::entities::{list_entity, task_entity};
use crate::output::SCHEMA_VERSION;

/// `tasks parse`: the task the text reads as, written nowhere.
pub(super) fn parsed_task() -> Value {
    let span = object(
        json!({
            "start": { "type": "integer", "description": "Byte offset into the input" },
            "end": { "type": "integer" },
            "kind": { "enum": ["list", "label", "priority", "recurrence", "reminder", "start", "date", "my_day", "syntax"] },
            "text": { "type": "string" }
        }),
        &["start", "end", "kind", "text"],
    );
    versioned(
        json!({
            "input": { "type": "string" },
            "title": { "type": "string", "description": "What's left once the recognised parts are taken out" },
            "list": {
                "type": ["object", "null"],
                "description": "The list #List names; null is the default, Tasks",
                "properties": { "id": { "type": "string" }, "name": { "type": "string" } }
            },
            "due": nullable("string", "YYYY-MM-DD"),
            "start": nullable("string", "YYYY-MM-DD"),
            "reminder": nullable("string", "YYYY-MM-DDTHH:MM, local time"),
            "recurrence": {
                "type": ["object", "null"],
                "description": "Graph's patternedRecurrence, less range.recurrenceTimeZone (the daemon adds it), with a description"
            },
            "importance": { "enum": ["low", "normal", "high", null] },
            "priority": { "type": ["integer", "null"], "description": "The p1–p4 typed" },
            "categories": { "type": "array", "items": { "type": "string" } },
            "my_day": { "type": "boolean", "description": "+myday or * was typed: the task goes in today's My Day" },
            "spans": { "type": "array", "items": span },
            "warnings": { "type": "array", "items": { "type": "string" } }
        }),
        &[
            "input",
            "title",
            "list",
            "due",
            "start",
            "reminder",
            "recurrence",
            "importance",
            "priority",
            "categories",
            "my_day",
            "spans",
            "warnings",
        ],
    )
}

/// A task from `tasks list`: with a filter and no `--list`, from every
/// list, each with `list`.
pub(super) fn every_list_task() -> Value {
    let mut schema = task_entity();
    schema["properties"]["list"] = json!({
        "type": "string",
        "description": "The name of the task's list: only with --assignee, or a filter (--status, --due, --importance, --category) and no --list"
    });
    schema
}

/// `lists show`: the list, with how many tasks it holds.
pub(super) fn shown_list() -> Value {
    let mut schema = list_entity();
    let properties = &mut schema["properties"];
    properties["schema_version"] = json!({ "const": SCHEMA_VERSION });
    properties["open_count"] = json!({ "type": "integer" });
    properties["completed_count"] = json!({ "type": "integer" });
    schema
}

pub(super) fn search_result() -> Value {
    let mut schema = task_entity();
    schema["properties"]["list"] =
        json!({ "type": "string", "description": "The name of the task's list" });
    schema["properties"]["snippet"] = json!({
        "type": "string",
        "description": "The part of the title or notes that matched, on one line: each match between ** and **, … where it was cut"
    });
    schema["required"]
        .as_array_mut()
        .expect("task_entity lists required keys")
        .extend([json!("list"), json!("snippet")]);
    schema["description"] = json!(
        "A task that matched, best match first: the task as `tasks list` gives it, with `list` and `snippet`"
    );
    schema
}

pub(super) fn waiting_result() -> Value {
    let mut schema = task_entity();
    schema["properties"]["list"] =
        json!({ "type": "string", "description": "The name of the task's list" });
    schema["required"]
        .as_array_mut()
        .expect("task_entity lists required keys")
        .push(json!("list"));
    schema["description"] = json!(
        "An open task someone is assigned, grouped by person: the task as `tasks list` gives it, with `list`; `tasks list --assignee` gives the same shape"
    );
    schema
}

pub(super) fn suggestion() -> Value {
    let mut schema = task_entity();
    schema["properties"]["list"] =
        json!({ "type": "string", "description": "The name of the task's list" });
    schema["properties"]["suggestion"] = json!({
        "enum": ["due_today", "overdue", "left_over"],
        "description": "Why: due today, overdue, or left open in an earlier My Day the rollover emptied"
    });
    schema["properties"]["left_from"] = json!({
        "type": ["string", "null"],
        "format": "date",
        "description": "For left_over: the day of the My Day it was in"
    });
    schema["required"]
        .as_array_mut()
        .expect("task_entity lists required keys")
        .extend([json!("list"), json!("suggestion")]);
    schema["description"] = json!(
        "An open task My Day suggests: the task as `tasks list` gives it, with `list` and `suggestion`; due today first, then overdue, then left over"
    );
    schema
}

pub(super) fn done_result() -> Value {
    let mut schema = task_entity();
    schema["properties"]["list"] =
        json!({ "type": "string", "description": "The name of the task's list" });
    schema["properties"]["completed_on"] = json!({
        "type": ["string", "null"],
        "format": "date",
        "description": "The local day it was completed. Microsoft To Do keeps the day, not the time. Null while the completion hasn't reached Microsoft To Do"
    });
    schema["required"]
        .as_array_mut()
        .expect("task_entity lists required keys")
        .extend([json!("list"), json!("completed_on")]);
    schema["description"] = json!(
        "A completed task, newest completion first: the task as `tasks list` gives it, with `list` and `completed_on`"
    );
    schema
}
