//! The things commands print: lists, tasks and their parts, categories,
//! extensions, folders, links and outbox writes.

use serde_json::{Value, json};

use super::builders::{nullable, object, timestamp};

/// What every cached entity carries besides Graph's own fields.
fn identity() -> Value {
    json!({
        "id": { "type": "string", "description": "ms-todo's local ID; stable, and what commands take" },
        "graph_id": nullable("string", "Microsoft Graph's ID, for `raw` and debugging; commands take it too"),
        "sync_state": {
            "enum": ["synced", "pending", "unknown", "failed"],
            "description": "From its outbox writes: synced; pending (queued or being sent); unknown (a write may or may not have reached Microsoft To Do; never retry it, see `outbox list`); failed (a write was rejected and rolled back; see `outbox list`)"
        },
        "extensions": {
            "type": "array",
            "description": "ms-todo's own open extension (com.planetaryescape.mstodo), when it has one and it has been fetched. A task's may hold myDay, myDayDueSet, assignee, assigneeStatusSet and nag (minutes between nag reminders)",
            "items": { "type": "object" }
        }
    })
}

pub(super) fn list_entity() -> Value {
    let mut properties = identity();
    properties["displayName"] = json!({ "type": "string" });
    properties["wellknownListName"] =
        json!({ "type": "string", "description": "defaultList for \"Tasks\"" });
    properties["isOwner"] = json!({ "type": "boolean" });
    properties["isShared"] = json!({ "type": "boolean" });
    properties["folder"] = nullable(
        "string",
        "The folder the list is in (ms-todo's own; the To Do apps don't show it), or null",
    );
    let mut schema = object(properties, &["id", "graph_id", "sync_state", "displayName"]);
    schema["description"] =
        json!("A task list: every field Microsoft Graph returns, with `id` replaced");
    schema
}

pub(super) fn task_entity() -> Value {
    let mut properties = identity();
    properties["list_id"] =
        json!({ "type": "string", "description": "The local ID of the task's list" });
    properties["title"] = json!({ "type": "string" });
    properties["status"] = json!({ "type": "string", "description": "notStarted, waitingOnOthers or completed, among Graph's values" });
    properties["importance"] = json!({ "enum": ["low", "normal", "high"] });
    properties["dueDateTime"] =
        json!({ "type": "object", "description": "Graph's dateTimeTimeZone; a date only" });
    properties["reminderDateTime"] = json!({ "type": "object" });
    properties["isReminderOn"] = json!({ "type": "boolean" });
    properties["categories"] = json!({ "type": "array", "items": { "type": "string" } });
    properties["body"] = json!({ "type": "object" });
    properties["recurrence"] = json!({ "type": "object" });
    properties["createdDateTime"] = json!({ "type": "string" });
    properties["lastModifiedDateTime"] = json!({ "type": "string" });
    properties["attachments"] = json!({
        "type": "array",
        "description": "Its attachments' metadata, as `attachments list` gives it without `index`; absent until a sync has fetched it",
        "items": { "type": "object" }
    });
    properties["defer_until"] = nullable(
        "string",
        "YYYY-MM-DD, a local day: the task is hidden from everyday views until then (ms-todo's own; the To Do apps don't hide it). On its day it shows again",
    );
    properties["someday"] = json!({ "type": "boolean", "description": "Parked as Someday: hidden from everyday views until taken out (ms-todo's own)" });
    let mut schema = object(
        properties,
        &[
            "id",
            "graph_id",
            "sync_state",
            "list_id",
            "title",
            "defer_until",
            "someday",
        ],
    );
    schema["description"] =
        json!("A task: every field Microsoft Graph returns, with `id` replaced");
    schema
}

pub(super) fn candidate() -> Value {
    object(
        json!({
            "id": { "type": "string" },
            "name": { "type": "string" },
            "created_at": { "type": "string", "description": "For a task: when Graph created it" },
            "list_id": { "type": "string", "description": "For a task: its list's local ID" }
        }),
        &["id", "name"],
    )
}

pub(super) fn category() -> Value {
    object(
        json!({
            "id": { "type": "string" },
            "displayName": { "type": "string", "description": "Unique ignoring case; Graph can't rename one" },
            "color": { "type": "string", "description": "preset0 to preset24, or none" }
        }),
        &["id", "displayName", "color"],
    )
}

pub(super) fn extension() -> Value {
    let mut schema = object(
        json!({
            "extensionName": { "type": "string" },
            "id": { "type": "string" }
        }),
        &["extensionName"],
    );
    schema["description"] = json!(
        "An open extension: its name, Graph's id and annotations, and its own fields beside them"
    );
    schema
}

pub(super) fn folder() -> Value {
    object(
        json!({
            "name": { "type": "string" },
            "lists": {
                "type": "array",
                "items": { "type": "string" },
                "description": "The local IDs of its lists, in order"
            },
            "list_count": { "type": "integer" },
            "open_count": { "type": "integer", "description": "Open tasks in all its lists" }
        }),
        &["name", "lists", "list_count", "open_count"],
    )
}

pub(super) fn link() -> Value {
    object(
        json!({
            "index": { "type": "integer", "description": "From 1: what `tasks open --index` takes" },
            "url": { "type": "string" },
            "text": { "type": "string", "description": "A linked resource's name, a markdown link's text, or the host" },
            "source": { "enum": ["linked_resource", "notes"] },
            "openable": { "type": "boolean", "description": "Whether `tasks open` opens it: http, https or mailto" }
        }),
        &["index", "url", "text", "source", "openable"],
    )
}

/// A step: Graph's checklist item, numbered.
pub(super) fn step() -> Value {
    object(
        json!({
            "index": { "type": "integer", "description": "From 1, in Graph's order (the order added): what a STEP argument takes" },
            "id": { "type": "string", "description": "Graph's ID; `local-…` until Microsoft To Do has the step" },
            "displayName": { "type": "string" },
            "isChecked": { "type": "boolean" },
            "checkedDateTime": timestamp("When it was checked, while it is"),
            "createdDateTime": timestamp("")
        }),
        &["index", "id", "displayName", "isChecked"],
    )
}

/// A task's attachment: Graph's metadata, numbered.
pub(super) fn attachment() -> Value {
    object(
        json!({
            "index": { "type": "integer", "description": "From 1, in Graph's order: what an ATTACHMENT argument takes" },
            "id": { "type": "string", "description": "Graph's ID; `local-…` until Microsoft To Do has the file" },
            "name": { "type": "string" },
            "contentType": { "type": "string" },
            "size": { "type": "integer", "description": "Microsoft To Do's size, a few hundred bytes more than the file's; the file's own byte count until it's uploaded" },
            "lastModifiedDateTime": timestamp("")
        }),
        &["index", "id", "name"],
    )
}

/// A task's link: Graph's linked resource, numbered.
pub(super) fn linked_resource() -> Value {
    object(
        json!({
            "index": { "type": "integer", "description": "From 1; a task has at most one" },
            "id": { "type": "string", "description": "Graph's ID; `local-…` until Microsoft To Do has the link" },
            "webUrl": { "type": "string" },
            "displayName": { "type": "string" },
            "applicationName": { "type": "string" },
            "externalId": { "type": "string" }
        }),
        &["index", "id", "webUrl", "applicationName"],
    )
}

pub(super) fn outbox_op() -> Value {
    object(
        json!({
            "op_id": { "type": "string" },
            "command_id": { "type": "string", "description": "The op_id the change printed; a change to several tasks has one operation per task" },
            "action": { "enum": ["add", "edit", "complete", "reopen", "delete", "move", "my_day_add", "my_day_remove", "my_day_rollover"] },
            "task_id": { "type": "string" },
            "list_id": { "type": "string" },
            "title": nullable("string", ""),
            "state": {
                "enum": ["pending", "inflight", "unknown", "failed", "done"],
                "description": "unknown: it may or may not have reached Microsoft To Do; never resend it yourself. failed: rejected, and its local change rolled back"
            },
            "attempts": { "type": "integer" },
            "created_at": { "type": "integer", "description": "Unix seconds" },
            "next_attempt_at": nullable("integer", "Unix seconds: when a pending write is tried again"),
            "sent_at": nullable("integer", ""),
            "unknown_since": nullable("integer", ""),
            "depends_on": nullable("string", "The operation this one waits for"),
            "undoes": nullable("string", "For an undo: the op_id it undoes"),
            "last_error": {
                "oneOf": [
                    object(json!({ "kind": { "type": "string" }, "message": { "type": "string" } }), &["kind", "message"]),
                    { "type": "null" }
                ]
            },
            "note": nullable("string", "What was seen while it was unknown"),
            "flagged": { "type": "boolean", "description": "unknown for over 24 hours: resolve it with `outbox retry` or `outbox discard`" },
            "changes": { "description": "The Graph fields it sends: a failed add keeps the task's content here" }
        }),
        &[
            "op_id",
            "command_id",
            "action",
            "task_id",
            "list_id",
            "state",
            "attempts",
            "created_at",
            "flagged",
            "changes",
        ],
    )
}
