//! The pieces every schema is built from: objects, versioned results,
//! nullable and described fields, and the collections items come in.

use serde_json::{Value, json};

use crate::output::SCHEMA_VERSION;

pub(super) fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

/// A result with `schema_version` beside its fields.
pub(super) fn versioned(mut properties: Value, required: &[&str]) -> Value {
    properties["schema_version"] = json!({ "const": SCHEMA_VERSION });
    let mut required: Vec<&str> = required.to_vec();
    required.insert(0, "schema_version");
    object(properties, &required)
}

pub(super) fn nullable(kind: &str, description: &str) -> Value {
    described(json!({ "type": [kind, "null"] }), description)
}

pub(super) fn timestamp(description: &str) -> Value {
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

pub(super) fn collection(item: Value) -> Value {
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

/// A collection read from Graph as it is now: no `sync`.
pub(super) fn live_collection(item: Value) -> Value {
    versioned(
        json!({ "items": { "type": "array", "items": item } }),
        &["items"],
    )
}
