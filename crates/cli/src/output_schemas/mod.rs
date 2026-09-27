//! The JSON Schemas of what each command prints on stdout with `--format
//! json`, at [`SCHEMA_VERSION`], and of the error JSON on stderr
//! (docs/blueprint/07-cli.md#output-contract). Written by hand next to the
//! types they describe; `tests/schema_cli.rs` snapshots them and checks
//! real output against them, so a change to either shows up in review.

mod builders;
mod doctor;
mod entities;
mod reads;
mod writes;

use ms_todo_core::ErrorKind;
use serde_json::{Value, json};

use crate::output::SCHEMA_VERSION;
use builders::{collection, live_collection, nullable, object, timestamp, versioned};
use doctor::{daemon_state, doctor, service};
use entities::{
    attachment, candidate, category, extension, folder, link, linked_resource, list_entity,
    outbox_op, step, task_entity,
};
use reads::{
    active_context, context_item, done_result, every_list_task, next_result, parsed_task, scoped,
    search_result, shown_list, suggestion, task_collection, waiting_result,
};
use writes::{applied, catalog_applied, catalog_plan, list_applied, list_plan, plan};

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
        "lists show" => shown_list(),
        "tasks show" => {
            let mut schema = task_entity();
            schema["properties"]["schema_version"] = json!({ "const": SCHEMA_VERSION });
            schema["properties"]["related"] = json!({
                "type": "array",
                "description": "Only when it's linked to other tasks (tasks relate): each, as this cache has it; id, title, list_id and status are null for one it doesn't have",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": ["string", "null"] },
                        "graph_id": { "type": "string" },
                        "title": { "type": ["string", "null"] },
                        "list_id": { "type": ["string", "null"] },
                        "status": { "type": ["string", "null"] }
                    },
                    "required": ["id", "graph_id", "title", "list_id", "status"]
                }
            });
            schema
        }
        "tasks list" => task_collection(every_list_task()),
        "myday list" => collection(task_entity()),
        "lists create" | "lists rename" | "lists delete" => {
            json!({ "oneOf": [list_applied(), list_plan()] })
        }
        "categories list" => live_collection(category()),
        "extensions list" | "extensions get" => live_collection(extension()),
        "categories create" | "categories recolor" | "categories delete" | "extensions set"
        | "extensions delete" => json!({ "oneOf": [catalog_applied(), catalog_plan()] }),
        "daemon restart" => {
            let (properties, required) = daemon_state();
            versioned(properties, required)
        }
        "daemon logs" => versioned(
            json!({
                "path": { "type": "string", "description": "The daemon's log file" },
                "lines": { "type": "array", "items": { "type": "string" } }
            }),
            &["path", "lines"],
        ),
        "waiting" => task_collection(waiting_result()),
        "next" => scoped(collection(next_result())),
        "myday suggest" => scoped(collection(suggestion())),
        "ctx" | "ctx show" => active_context(),
        "ctx list" => live_collection(context_item()),
        "myday add" | "myday remove" | "myday rollover" => json!({ "oneOf": [applied(), plan()] }),
        "search" => {
            let mut schema = scoped(collection(search_result()));
            schema["properties"]["semantic"] = json!({
                "type": "object",
                "description": "With --semantic only: the embedding model, and how many tasks in the search's scope it hasn't embedded for their current text yet (missing from items, or ranked by their old text)",
                "properties": {
                    "model": { "type": "string" },
                    "pending": { "type": "integer" }
                },
                "required": ["model", "pending"]
            });
            schema
        }
        "done" => collection(done_result()),
        "tasks add" | "tasks complete" | "tasks reopen" | "tasks edit" | "tasks move"
        | "tasks nag" | "tasks relate" | "tasks unrelate" | "tasks delete" | "reschedule"
        | "lists merge" => json!({ "oneOf": [applied(), plan()] }),
        "undo" => json!({ "oneOf": [applied(), list_applied(), catalog_applied()] }),
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
        "daemon install" | "daemon uninstall" => {
            let service = service();
            versioned(
                service["properties"].clone(),
                &["installed", "manager", "path", "changed", "now"],
            )
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
