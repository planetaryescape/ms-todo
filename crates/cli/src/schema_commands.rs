//! `ms-todo schema [CMD]`: each command's input and output as JSON Schema
//! (docs/blueprint/07-cli.md#output-contract). The input side is read from
//! the clap definitions, so it can't drift from the real flags; the output
//! side is `output_schemas`.

use clap::{ArgAction, CommandFactory};
use ms_todo_core::ErrorKind;
use serde_json::{Map, Value, json};

use crate::args::Cli;
use crate::error::CliError;
use crate::output::SCHEMA_VERSION;
use crate::output_schemas::{error_schema, output_schema};

/// Every command with a schema, as typed after `ms-todo`.
const COMMANDS: &[&str] = &[
    "auth login",
    "auth status",
    "auth logout",
    "auth bearer",
    "lists list",
    "lists move",
    "lists order",
    "folders list",
    "folders rename",
    "folders delete",
    "folders order",
    "tasks list",
    "tasks add",
    "tasks complete",
    "tasks reopen",
    "tasks edit",
    "tasks delete",
    "search",
    "done",
    "reschedule",
    "outbox list",
    "outbox retry",
    "outbox discard",
    "undo",
    "sync",
    "doctor",
    "schema",
    "raw",
    "daemon start",
    "daemon stop",
    "daemon status",
];

pub fn schema(words: &[String]) -> Result<Value, CliError> {
    let mut root = Cli::command();
    root.build();
    if words.is_empty() {
        let commands: Map<String, Value> = COMMANDS
            .iter()
            .map(|&name| Ok((name.to_owned(), describe(&root, name)?)))
            .collect::<Result<_, CliError>>()?;
        return Ok(json!({
            "schema_version": SCHEMA_VERSION,
            "commands": commands,
            "error": error_schema(),
        }));
    }
    let name = words.join(" ");
    if !COMMANDS.contains(&name.as_str()) {
        return Err(CliError::message(
            ErrorKind::InvalidInput,
            format!(
                "no schema for {name:?}; commands with one: {}",
                COMMANDS.join(", ")
            ),
        ));
    }
    let mut described = describe(&root, &name)?;
    described["schema_version"] = json!(SCHEMA_VERSION);
    described["command"] = json!(name);
    described["error"] = error_schema();
    Ok(described)
}

fn describe(root: &clap::Command, name: &str) -> Result<Value, CliError> {
    let mut command = root;
    for word in name.split(' ') {
        command = command.find_subcommand(word).ok_or_else(|| {
            CliError::message(ErrorKind::Internal, format!("no command {name:?}"))
        })?;
    }
    Ok(json!({
        "input": input_schema(command),
        "output": output_schema(name).unwrap_or(Value::Null),
    }))
}

/// A command's arguments as a JSON object schema: one property per flag or
/// positional argument, keyed by its name, with `x-cli` saying how to pass
/// it. The global `--format` and `--instance` apply to every command.
fn input_schema(command: &clap::Command) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for arg in command.get_arguments() {
        let id = arg.get_id().as_str();
        if arg.is_global_set() || id == "help" || id == "version" {
            continue;
        }
        let mut schema = match arg.get_action() {
            ArgAction::SetTrue | ArgAction::SetFalse => json!({ "type": "boolean" }),
            _ => {
                let values: Vec<String> = arg
                    .get_possible_values()
                    .iter()
                    .map(|value| value.get_name().to_owned())
                    .collect();
                let item = if values.is_empty() {
                    json!({ "type": "string" })
                } else {
                    json!({ "enum": values })
                };
                let many = arg
                    .get_num_args()
                    .is_some_and(|range| range.max_values() > 1);
                if many {
                    json!({ "type": "array", "items": item })
                } else {
                    item
                }
            }
        };
        if let Some(help) = arg.get_help() {
            schema["description"] = json!(help.to_string());
        }
        schema["x-cli"] = json!(match arg.get_long() {
            Some(long) => format!("--{long}"),
            None => format!(
                "<{}>",
                arg.get_value_names()
                    .and_then(|names| names.first())
                    .map_or(id, |name| name.as_str())
            ),
        });
        if arg.is_required_set() {
            required.push(id.to_owned());
        }
        properties.insert(id.to_owned(), schema);
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}
