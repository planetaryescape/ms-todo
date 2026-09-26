//! What a write did, and what a dry run says it would do: for tasks,
//! lists and folders, and categories and extensions.

use serde_json::{Value, json};

use super::builders::{object, versioned};
use super::entities::{candidate, category, extension, list_entity, task_entity};

pub(super) fn applied() -> Value {
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

pub(super) fn plan() -> Value {
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

/// What a folder change did: the lists it changed, as they are now.
pub(super) fn list_applied() -> Value {
    let mut schema = applied();
    let properties = &mut schema["properties"];
    properties["action"] = json!({
        "enum": ["move_list", "order_list", "rename_folder", "delete_folder", "order_folder", "create_list", "rename_list", "delete_list", "undo"]
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
pub(super) fn list_plan() -> Value {
    versioned(
        json!({
            "dry_run": { "const": true },
            "action": { "enum": ["move_list", "order_list", "rename_folder", "delete_folder", "order_folder", "create_list", "rename_list", "delete_list"] },
            "lists": {
                "type": "array",
                "description": "Each list that would change; absent when none would",
                "items": object(
                    json!({
                        "id": { "type": "string" },
                        "name": { "type": "string" },
                        "changes": {
                            "type": "object",
                            "description": "A folder change: the fields of ms-todo's extension it gets (folder, order, folderOrder; null removes one). A create: displayName and folder. A rename: displayName. A delete: deleted, tasks (how many go with it) and undoable"
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

/// What a category or extension write did.
pub(super) fn catalog_applied() -> Value {
    versioned(
        json!({
            "op_id": { "type": "string", "description": "What `undo` knows the change by" },
            "action": { "enum": ["category_create", "category_recolor", "category_delete", "extension_set", "extension_delete"] },
            "items": {
                "type": "array",
                "description": "The category or extension as it is now; for a delete, as it was",
                "items": { "oneOf": [category(), extension()] }
            },
            "undoes": { "type": "string", "description": "For an undo: the op_id it undoes" },
            "list_ids": { "type": "array", "maxItems": 0 }
        }),
        &["op_id", "action", "items"],
    )
}

/// What a category or extension write would do.
pub(super) fn catalog_plan() -> Value {
    versioned(
        json!({
            "dry_run": { "const": true },
            "action": { "enum": ["category_create", "category_recolor", "category_delete", "extension_set", "extension_delete"] },
            "changes": object(
                json!({
                    "target": { "type": "object", "description": "The category or extension as it is now (a create's: its name)" },
                    "body": { "description": "What's sent; null for a delete" }
                }),
                &["target", "body"],
            )
        }),
        &["dry_run", "action", "changes"],
    )
}
