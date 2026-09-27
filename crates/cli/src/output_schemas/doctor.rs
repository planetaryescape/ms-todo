//! `daemon status` and `doctor`.

use serde_json::{Value, json};

use super::builders::{nullable, object, versioned};

/// `daemon status`'s fields, which `doctor` nests without a version.
pub(super) fn daemon_state() -> (Value, &'static [&'static str]) {
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

pub(super) fn doctor() -> Value {
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
            "semantic": {
                "type": ["object", "null"],
                "description": "Semantic search (rung 9c); null when the daemon didn't report",
                "properties": {
                    "enabled": { "type": "boolean", "description": "[search] semantic in config.toml" },
                    "model": { "type": "string", "description": "The embedding model, name@revision" },
                    "state": {
                        "enum": ["off", "loading", "ready", "failed", "unknown"],
                        "description": "off: nothing downloaded. loading: downloading or reading the model. ready: tasks are embedded as they change. failed: see problem; the next search tries again"
                    },
                    "model_dir": { "type": "string", "description": "Where the model's files are kept" },
                    "download_bytes": { "type": "integer", "description": "The size of the one-time download" },
                    "indexed": { "type": "integer", "description": "Live tasks embedded for their current text" },
                    "pending": { "type": "integer", "description": "Live tasks not embedded yet" },
                    "problem": nullable("string", "Why it's off or failing")
                }
            },
            "nag": {
                "type": ["object", "null"],
                "description": "Nag reminders (rung 9b); null when the daemon didn't report",
                "properties": {
                    "active": { "type": "boolean", "description": "[nag] enabled, and this system can notify" },
                    "notifier": nullable("string", "How notifications are shown (osascript), or null where they can't be"),
                    "quiet_hours": nullable("string", "HH:MM-HH:MM, local, when nothing is shown; null for none"),
                    "count": { "type": "integer", "description": "Open tasks set to nag" },
                    "problem": nullable("string", "Why nagging is off or failing")
                }
            },
            "contexts": {
                "type": ["object", "null"],
                "description": "Contexts (rung 9d); null when the daemon didn't report",
                "properties": {
                    "active": { "type": ["string", "null"], "description": "The active context's name" },
                    "defined": { "type": "integer", "description": "How many config.toml defines" },
                    "problems": { "type": "array", "items": { "type": "string" }, "description": "Each also in `problems`, prefixed `contexts:`" }
                }
            },
            "problems": { "type": "array", "items": { "type": "string" } }
        }),
        &["sign_in", "daemon", "database", "scopes", "problems"],
    )
}
