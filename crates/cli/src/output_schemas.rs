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
        "tasks list" => collection(task_entity()),
        "search" => collection(search_result()),
        "tasks add" | "tasks complete" | "tasks reopen" | "tasks edit" | "tasks delete" => {
            json!({ "oneOf": [applied(), plan()] })
        }
        "undo" => applied(),
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
            "action": { "enum": ["add", "edit", "complete", "reopen", "delete"] },
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
    properties["status"] =
        json!({ "type": "string", "description": "notStarted or completed, among Graph's values" });
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
            "action": { "enum": ["add", "complete", "reopen", "edit", "delete", "undo"] },
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
            }
        }),
        &["op_id", "action", "items", "list_ids"],
    )
}

fn plan() -> Value {
    versioned(
        json!({
            "dry_run": { "const": true },
            "action": { "enum": ["add", "complete", "reopen", "edit", "delete"] },
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
            "changes": { "description": "The Graph fields each target gets; null for delete" }
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
            "problems": { "type": "array", "items": { "type": "string" } }
        }),
        &["sign_in", "daemon", "database", "scopes", "problems"],
    )
}
