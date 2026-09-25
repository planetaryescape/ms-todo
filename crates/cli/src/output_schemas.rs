//! The JSON Schemas of what each command prints on stdout with `--format
//! json`, at [`SCHEMA_VERSION`], and of the error JSON on stderr
//! (docs/blueprint/07-cli.md#output-contract). Written by hand next to the
//! types they describe; `tests/schema_cli.rs` snapshots them and checks
//! real output against them, so a change to either shows up in review.

use ms_todo_core::ErrorKind;
use serde_json::{Value, json};

use crate::output::SCHEMA_VERSION;

/// The stdout schema of `command` (e.g. `tasks list`), or `None` if it has
/// no JSON output of its own.
pub fn output_schema(command: &str) -> Option<Value> {
    Some(match command {
        "auth login" | "auth status" => versioned(
            json!({
                "signed_in": { "type": "boolean" },
                "account": nullable("string", "The sign-in name"),
                "display_name": nullable("string", ""),
                "expires_at": timestamp("When the access token expires"),
                "expires_in_seconds": { "type": "integer" },
                "client_id": nullable("string", "The configured client ID"),
                "client_id_source": nullable("string", "env, config or bundled"),
                "token_client_id": { "type": "string", "description": "Only when the sign-in belongs to another client ID" },
                "scopes": { "type": "array", "items": { "type": "string" } },
                "token_path": { "type": "string" },
                "config_file": { "type": "string" },
                "instance": { "type": "string" }
            }),
            &[
                "signed_in",
                "expires_at",
                "scopes",
                "token_path",
                "instance",
            ],
        ),
        "auth logout" => versioned(
            json!({
                "signed_in": { "const": false },
                "removed": { "type": "boolean", "description": "Whether a stored sign-in was deleted" },
                "token_path": { "type": "string" }
            }),
            &["signed_in", "removed", "token_path"],
        ),
        "auth bearer" => versioned(
            json!({
                "access_token": { "type": "string" },
                "expires_at": timestamp("")
            }),
            &["access_token", "expires_at"],
        ),
        "lists list" => collection(list_entity()),
        "tasks list" | "myday list" => collection(task_entity()),
        "waiting" => collection(waiting_result()),
        "myday suggest" => collection(suggestion()),
        "myday add" | "myday remove" | "myday rollover" => json!({ "oneOf": [applied(), plan()] }),
        "search" => collection(search_result()),
        "done" => collection(done_result()),
        "tasks add" | "tasks complete" | "tasks reopen" | "tasks edit" | "tasks move"
        | "tasks delete" | "reschedule" => json!({ "oneOf": [applied(), plan()] }),
        "undo" => json!({ "oneOf": [applied(), list_applied()] }),
        "lists move" | "lists order" | "folders rename" | "folders delete" | "folders order" => {
            json!({ "oneOf": [list_applied(), list_plan()] })
        }
        "folders list" => collection(folder()),
        "tasks links" => collection(link()),
        "steps list" => collection(step()),
        "links list" => collection(linked_resource()),
        "steps add" | "steps edit" | "steps check" | "steps uncheck" | "steps delete"
        | "links add" | "links edit" | "links delete" | "attachments add"
        | "attachments delete" => {
            json!({ "oneOf": [applied(), plan()] })
        }
        "attachments list" => collection(attachment()),
        "attachments download" => versioned(
            json!({
                "task_id": { "type": "string", "description": "The task's local ID" },
                "files": {
                    "type": "array",
                    "description": "Each file written, in order",
                    "items": object(
                        json!({
                            "id": { "type": "string", "description": "The attachment's ID" },
                            "name": { "type": "string", "description": "Its name in Microsoft To Do" },
                            "path": { "type": "string", "description": "Where it was written: a safe name in the directory, numbered if the name was taken" },
                            "bytes": { "type": "integer" },
                            "sha256": { "type": "string", "description": "The bytes' sha256, as hex" }
                        }),
                        &["id", "name", "path", "bytes", "sha256"],
                    )
                }
            }),
            &["task_id", "files"],
        ),
        "tasks parse" => parsed_task(),
        "tasks suggest-list" => versioned(
            json!({
                "title": { "type": "string" },
                "list_id": nullable("string", "The suggested list's ID; null for no suggestion"),
                "list_name": nullable("string", ""),
                "confidence": {
                    "type": ["number", "null"],
                    "description": "From 0 to 1, at least suggest.min_confidence"
                }
            }),
            &["title", "list_id", "list_name", "confidence"],
        ),
        "tasks open" => versioned(
            json!({
                "url": { "type": "string", "description": "The URL handed to the system's opener" },
                "text": { "type": "string" }
            }),
            &["url", "text"],
        ),
        "outbox list" | "outbox retry" | "outbox discard" => versioned(
            json!({
                "items": {
                    "type": "array",
                    "description": "Newest first; retry and discard give the one operation",
                    "items": outbox_op()
                }
            }),
            &["items"],
        ),
        "sync" => versioned(
            json!({
                "waited": { "type": "boolean", "description": "False when the sync was only asked for" },
                "scopes": { "type": "integer", "description": "Scopes synced: the lists, and each list's tasks" },
                "changed": { "type": "integer", "description": "Lists and tasks added, changed or removed" },
                "generation": { "type": "integer", "description": "The lists scope's sync generation afterwards" }
            }),
            &["waited", "scopes", "changed", "generation"],
        ),
        "doctor" => doctor(),
        "daemon start" | "daemon status" => {
            let (properties, required) = daemon_state();
            versioned(properties, required)
        }
        "daemon stop" => versioned(
            json!({
                "running": { "const": false },
                "stopped_pid": nullable("integer", "The PID stopped, or null if none was running")
            }),
            &["running", "stopped_pid"],
        ),
        "raw" => json!({
            "description": "Microsoft Graph's response body, exactly as it came: no schema_version, no envelope"
        }),
        "schema" => versioned(
            json!({
                "command": { "type": "string" },
                "input": { "type": "object" },
                "output": { "type": "object" },
                "error": { "type": "object" },
                "commands": { "type": "object" }
            }),
            &[],
        ),
        _ => return None,
    })
}

/// `tasks parse`: the task the text reads as, written nowhere.
fn parsed_task() -> Value {
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

/// What every command prints on stderr when it fails, in `json` or `jsonl`.
pub fn error_schema() -> Value {
    let kinds: Vec<&str> = ErrorKind::ALL.iter().map(|kind| kind.as_str()).collect();
    object(
        json!({
            "error": object(
                json!({
                    "kind": { "enum": kinds },
                    "message": { "type": "string" },
                    "graph_code": { "type": "string", "description": "Graph's error.code" },
                    "request_id": { "type": "string", "description": "Graph's request-id header" },
                    "candidates": {
                        "type": "array",
                        "description": "What an ambiguous name matched; pick one ID",
                        "items": candidate()
                    },
                    "op_id": { "type": "string", "description": "The failed mutation's op_id" },
                    "applied": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Local IDs a multi-task change changed before it failed"
                    }
                }),
                &["kind", "message"],
            )
        }),
        &["error"],
    )
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

/// A result with `schema_version` beside its fields.
fn versioned(mut properties: Value, required: &[&str]) -> Value {
    properties["schema_version"] = json!({ "const": SCHEMA_VERSION });
    let mut required: Vec<&str> = required.to_vec();
    required.insert(0, "schema_version");
    object(properties, &required)
}

fn nullable(kind: &str, description: &str) -> Value {
    described(json!({ "type": [kind, "null"] }), description)
}

fn timestamp(description: &str) -> Value {
    described(
        json!({ "type": "string", "format": "date-time" }),
        description,
    )
}

fn described(mut schema: Value, description: &str) -> Value {
    if !description.is_empty() {
        schema["description"] = json!(description);
    }
    schema
}

fn candidate() -> Value {
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

fn outbox_op() -> Value {
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
            "description": "ms-todo's own open extension (com.planetaryescape.mstodo), when it has one and it has been fetched",
            "items": { "type": "object" }
        }
    })
}

fn list_entity() -> Value {
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

fn task_entity() -> Value {
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
    let mut schema = object(
        properties,
        &["id", "graph_id", "sync_state", "list_id", "title"],
    );
    schema["description"] =
        json!("A task: every field Microsoft Graph returns, with `id` replaced");
    schema
}

fn search_result() -> Value {
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

fn waiting_result() -> Value {
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

fn suggestion() -> Value {
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

fn done_result() -> Value {
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

fn collection(item: Value) -> Value {
    versioned(
        json!({
            "sync": object(
                json!({
                    "state": {
                        "enum": ["initial", "ready"],
                        "description": "initial until this scope's first sync finishes: an empty `items` then doesn't mean empty"
                    },
                    "generation": { "type": "integer", "description": "Goes up by one per finished sync of the scope" }
                }),
                &["state", "generation"],
            ),
            "items": { "type": "array", "items": item }
        }),
        &["sync", "items"],
    )
}

fn applied() -> Value {
    versioned(
        json!({
            "op_id": { "type": "string", "description": "What `undo` and `outbox list` know the change by" },
            "action": { "enum": ["add", "complete", "reopen", "edit", "delete", "undo", "move", "my_day_add", "my_day_remove", "my_day_rollover", "step_add", "step_edit", "step_check", "step_uncheck", "step_delete", "link_add", "link_edit", "link_delete", "attachment_add", "attachment_delete"] },
            "items": {
                "type": "array",
                "items": task_entity(),
                "description": "Each task as ms-todo has it now, sync_state pending until the write reaches Microsoft To Do; for delete, as it was"
            },
            "undoes": { "type": "string", "description": "For an undo: the op_id it undoes" },
            "list_ids": {
                "type": "array",
                "items": { "type": "string" },
                "description": "The local ID of each item's list, in order"
            },
            "rolled": {
                "type": "array",
                "description": "Recurring tasks completed: still open, due on the next date",
                "items": object(
                    json!({ "id": { "type": "string" }, "next_due": { "type": "string", "format": "date" } }),
                    &["id", "next_due"],
                )
            },
            "refused": {
                "type": "array",
                "description": "For an undo of a change to several tasks: those left alone because a field it set has changed since",
                "items": object(
                    json!({
                        "id": { "type": "string" },
                        "title": { "type": "string" },
                        "reason": { "type": "string" }
                    }),
                    &["id", "title", "reason"],
                )
            }
        }),
        &["op_id", "action", "items", "list_ids"],
    )
}

/// What a folder change did: the lists it changed, as they are now.
fn list_applied() -> Value {
    let mut schema = applied();
    let properties = &mut schema["properties"];
    properties["action"] = json!({
        "enum": ["move_list", "order_list", "rename_folder", "delete_folder", "order_folder", "undo"]
    });
    properties["items"] = json!({
        "type": "array",
        "items": list_entity(),
        "description": "Each list changed, as ms-todo has it now: sync_state pending until the write reaches Microsoft To Do. Empty when nothing needed to change"
    });
    properties["list_ids"]["description"] = json!("Each item's own ID, in order");
    if let Some(properties) = properties.as_object_mut() {
        properties.remove("rolled");
        properties.remove("refused");
    }
    schema
}

/// What a folder change would do.
fn list_plan() -> Value {
    versioned(
        json!({
            "dry_run": { "const": true },
            "action": { "enum": ["move_list", "order_list", "rename_folder", "delete_folder", "order_folder"] },
            "lists": {
                "type": "array",
                "description": "Each list that would change; absent when none would",
                "items": object(
                    json!({
                        "id": { "type": "string" },
                        "name": { "type": "string" },
                        "changes": {
                            "type": "object",
                            "description": "The fields of ms-todo's extension it gets: folder, order, folderOrder; null removes one"
                        }
                    }),
                    &["id", "name", "changes"],
                )
            },
            "changes": { "const": null }
        }),
        &["dry_run", "action", "changes"],
    )
}

fn folder() -> Value {
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

fn link() -> Value {
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
fn step() -> Value {
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
fn attachment() -> Value {
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
fn linked_resource() -> Value {
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

fn plan() -> Value {
    versioned(
        json!({
            "dry_run": { "const": true },
            "action": { "enum": ["add", "complete", "reopen", "edit", "delete", "move", "my_day_add", "my_day_remove", "my_day_rollover", "step_add", "step_edit", "step_check", "step_uncheck", "step_delete", "link_add", "link_edit", "link_delete", "attachment_add", "attachment_delete"] },
            "list": candidate(),
            "targets": {
                "type": "array",
                "items": object(
                    json!({
                        "id": { "type": "string" },
                        "title": { "type": "string" },
                        "list_id": { "type": "string" }
                    }),
                    &["id", "title", "list_id"],
                )
            },
            "changes": { "description": "The Graph fields each target gets; null for delete. For My Day: myDay (the day, or null), and the tasks whose due date is set (due_today) or cleared (due_cleared). For steps, links and attachments: each write, as { collection, verb (create, update or delete), id, body, carried }, and for an attachment's create the file it's read from, as file: { path, bytes, modified }" }
        }),
        &["dry_run", "action", "changes"],
    )
}

/// `daemon status`'s fields, which `doctor` nests without a version.
fn daemon_state() -> (Value, &'static [&'static str]) {
    (
        json!({
            "running": { "type": "boolean" },
            "ready": { "type": "boolean" },
            "pid": nullable("integer", ""),
            "version": nullable("string", ""),
            "protocol_version": nullable("integer", ""),
            "started_at": nullable("string", ""),
            "signed_in": nullable("boolean", ""),
            "problem": { "type": "string" },
            "instance": { "type": "string" },
            "socket": { "type": "string" }
        }),
        &["running", "ready", "instance", "socket"],
    )
}

fn doctor() -> Value {
    let failure = object(
        json!({
            "kind": { "type": "string" },
            "message": { "type": "string" },
            "at": nullable("string", "")
        }),
        &["kind", "message"],
    );
    let mut last_error = failure.clone();
    last_error["properties"]["scope"] = json!({ "type": "string" });
    let (properties, required) = daemon_state();
    let daemon = object(properties, required);
    versioned(
        json!({
            "sign_in": object(
                json!({
                    "signed_in": { "type": "boolean" },
                    "expires_at": nullable("string", ""),
                    "token_path": { "type": "string" }
                }),
                &["signed_in", "token_path"],
            ),
            "daemon": daemon,
            "database": {
                "type": ["object", "null"],
                "properties": { "path": { "type": "string" }, "bytes": { "type": "integer" } }
            },
            "syncing": nullable("boolean", ""),
            "scopes": {
                "type": "array",
                "items": object(
                    json!({
                        "scope": { "type": "string", "description": "lists, or tasks:<list graph_id>" },
                        "list_id": nullable("string", ""),
                        "list_name": nullable("string", ""),
                        "state": { "enum": ["initial", "ready"] },
                        "generation": { "type": "integer" },
                        "in_progress": { "type": "boolean" },
                        "last_success_at": nullable("string", ""),
                        "last_changed_count": { "type": "integer" },
                        "last_error": { "oneOf": [failure.clone(), { "type": "null" }] },
                        "mode": {
                            "enum": ["enumeration", "delta", "unknown"],
                            "description": "delta once a pass saved a delta link: the next pass asks Graph only for changes. enumeration until then, or after Graph rejected the link: the next pass reads the whole scope. unknown: a newer daemon's mode this ms-todo doesn't know"
                        },
                        "last_delta_at": nullable("string", "When a delta round of this scope last finished")
                    }),
                    &["scope", "state", "generation", "in_progress", "mode"],
                )
            },
            "last_error": {
                "description": "The most recent failure of any scope",
                "oneOf": [last_error, { "type": "null" }]
            },
            "outbox": {
                "type": ["object", "null"],
                "description": "How many writes are in each state; flagged are unknown for over 24 hours",
                "properties": {
                    "pending": { "type": "integer" },
                    "inflight": { "type": "integer" },
                    "unknown": { "type": "integer" },
                    "failed": { "type": "integer" },
                    "done": { "type": "integer" },
                    "flagged": { "type": "integer" }
                }
            },
            "suggest": {
                "type": ["object", "null"],
                "description": "List suggestions (rung 6b); null when the daemon didn't report",
                "properties": {
                    "enabled": { "type": "boolean" },
                    "provider": nullable("string", "typesafe"),
                    "sends": nullable("string", "What leaves this machine when enabled"),
                    "problem": nullable("string", "Why suggestions are off or failing")
                }
            },
            "my_day": {
                "type": ["object", "null"],
                "description": "My Day (rung 7); null when the daemon didn't report",
                "properties": {
                    "date": { "type": "string", "format": "date", "description": "My Day's day now" },
                    "count": { "type": "integer", "description": "Open tasks in it" },
                    "rollover_time": { "type": "string", "description": "HH:MM, local: when a day's My Day ends" },
                    "last_rollover": nullable("string", "The day the last rollover ran for"),
                    "phone": { "type": "string", "description": "What the phone's own My Day depends on" }
                }
            },
            "problems": { "type": "array", "items": { "type": "string" } }
        }),
        &["sign_in", "daemon", "database", "scopes", "problems"],
    )
}
