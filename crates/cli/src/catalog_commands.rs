//! `categories` and `extensions` (rung 8e, D-058): read from and written
//! straight to Graph through the daemon, with `--dry-run`,
//! `--idempotency-key` and `undo` as every mutation has them. A delete
//! asks in a terminal and exits 2 anywhere else without `--yes`.

use std::io::Write;

use ms_todo_core::{ErrorKind, Paths};
use ms_todo_protocol::{
    Applied, CategoryChange, Entity, ExtensionChange, ExtensionOwner, OwnerKind, Plan, Request,
    ResponseData, TaskAction,
};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::catalog_args::{
    CatalogWriteArgs, CategoriesCommand, ExtensionsCommand, OwnerArgs, OwnerKindArg,
};
use crate::confirm::{can_prompt, confirm};
use crate::csv_columns::text;
use crate::daemon_client;
use crate::error::CliError;
use crate::output::{
    OutputFormat, Render, SCHEMA_VERSION, Table, Versioned, print_ids, print_json,
    print_live_collection, print_success, write_csv,
};
use crate::task_commands::op_id_unless;
use crate::task_output::{past_tense, verb};

pub const CATEGORY_COLUMNS: &[&str] = &["id", "name", "color"];
pub const EXTENSION_COLUMNS: &[&str] = &["name", "data"];

pub const CATEGORIES_TABLE: Table = Table {
    headings: &["NAME", "COLOR", "ID"],
    row: |category| {
        vec![
            text(category, "displayName").to_owned(),
            text(category, "color").to_owned(),
            text(category, "id").to_owned(),
        ]
    },
    csv_headings: CATEGORY_COLUMNS,
    csv_row: category_row,
    bold_matches: None,
};

pub const EXTENSIONS_TABLE: Table = Table {
    headings: &["NAME", "DATA"],
    row: extension_row,
    csv_headings: EXTENSION_COLUMNS,
    csv_row: extension_row,
    bold_matches: None,
};

fn category_row(category: &Entity) -> Vec<String> {
    vec![
        text(category, "id").to_owned(),
        text(category, "displayName").to_owned(),
        text(category, "color").to_owned(),
    ]
}

fn extension_row(extension: &Entity) -> Vec<String> {
    vec![
        text(extension, "extensionName").to_owned(),
        Value::Object(data(extension)).to_string(),
    ]
}

/// An extension's own fields: not Graph's `id`, `extensionName` or
/// annotations.
fn data(extension: &Entity) -> Map<String, Value> {
    extension
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "id" | "extensionName") && !key.contains('@'))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

pub async fn categories(
    paths: &Paths,
    command: CategoriesCommand,
    format: OutputFormat,
) -> Result<(), CliError> {
    match command {
        CategoriesCommand::List => {
            let items = match daemon_client::ask(paths, Request::ListCategories).await? {
                ResponseData::Categories { items } => items,
                _ => return Err(crate::unexpected_response()),
            };
            print_live_collection(format, &items, &CATEGORIES_TABLE)
        }
        CategoriesCommand::Create { name, color, write } => {
            let change = CategoryChange::Create { name, color };
            change_category(paths, change, write, format).await
        }
        CategoriesCommand::Recolor {
            category,
            color,
            write,
        } => {
            let change = CategoryChange::Recolor { category, color };
            change_category(paths, change, write, format).await
        }
        CategoriesCommand::Delete {
            category,
            yes,
            write,
        } => {
            let change = CategoryChange::Delete { category };
            let request = |dry_run, key| category_request(change.clone(), dry_run, key);
            destructive(
                paths,
                yes,
                write,
                format,
                request,
                "Delete the category? Tasks keep its name as a label; `ms-todo undo` makes it again.",
            )
            .await
        }
    }
}

pub async fn extensions(
    paths: &Paths,
    command: ExtensionsCommand,
    format: OutputFormat,
) -> Result<(), CliError> {
    match command {
        ExtensionsCommand::List(owner) => {
            let request = Request::ListExtensions {
                owner: owner_of(owner),
            };
            let items = extension_items(paths, request).await?;
            print_live_collection(format, &items, &EXTENSIONS_TABLE)
        }
        ExtensionsCommand::Get { owner, name } => {
            let request = Request::GetExtension {
                owner: owner_of(owner),
                name,
            };
            let mut items = extension_items(paths, request).await?;
            let Some(extension) = items.pop() else {
                return Err(crate::unexpected_response());
            };
            match format {
                OutputFormat::Ids => print_ids([text(&extension, "extensionName")]),
                OutputFormat::Csv => write_csv(EXTENSION_COLUMNS, &[extension_row(&extension)]),
                _ => print_success(format, &Document(extension)),
            }
        }
        ExtensionsCommand::Set {
            owner,
            name,
            json,
            write,
        } => {
            let data = match serde_json::from_str::<Value>(&json) {
                Ok(Value::Object(data)) => data,
                Ok(_) => {
                    return Err(CliError::message(
                        ErrorKind::InvalidInput,
                        "--json must be a JSON object, such as '{\"key\": \"value\"}'".into(),
                    ));
                }
                Err(error) => {
                    return Err(CliError::message(
                        ErrorKind::InvalidInput,
                        format!("--json isn't valid JSON: {error}"),
                    ));
                }
            };
            let request = Request::ChangeExtension {
                owner: owner_of(owner),
                change: ExtensionChange::Set { name, data },
                dry_run: write.dry_run,
                op_id: op_id_unless(write.dry_run),
                idempotency_key: write.idempotency.idempotency_key,
            };
            send(paths, request, format).await
        }
        ExtensionsCommand::Delete {
            owner,
            name,
            yes,
            write,
        } => {
            let owner = owner_of(owner);
            let request = |dry_run, key| Request::ChangeExtension {
                owner: owner.clone(),
                change: ExtensionChange::Delete { name: name.clone() },
                dry_run,
                op_id: op_id_unless(dry_run),
                idempotency_key: key,
            };
            destructive(
                paths,
                yes,
                write,
                format,
                request,
                "Delete the extension? `ms-todo undo` writes it back.",
            )
            .await
        }
    }
}

/// An open extension as `extensions get` prints it.
#[derive(Serialize)]
#[serde(transparent)]
struct Document(Entity);

impl Render for Document {
    fn table_rows(&self) -> Vec<(&'static str, String)> {
        let mut rows = vec![("Name", text(&self.0, "extensionName").to_owned())];
        rows.push(("Data", Value::Object(data(&self.0)).to_string()));
        rows
    }
}

async fn extension_items(paths: &Paths, request: Request) -> Result<Vec<Entity>, CliError> {
    match daemon_client::ask(paths, request).await? {
        ResponseData::Extensions { items } => Ok(items),
        _ => Err(crate::unexpected_response()),
    }
}

fn owner_of(owner: OwnerArgs) -> ExtensionOwner {
    ExtensionOwner {
        kind: match owner.kind {
            OwnerKindArg::List => OwnerKind::List,
            OwnerKindArg::Task => OwnerKind::Task,
        },
        id: owner.id,
        list: owner.list,
    }
}

async fn change_category(
    paths: &Paths,
    change: CategoryChange,
    write: CatalogWriteArgs,
    format: OutputFormat,
) -> Result<(), CliError> {
    let request = category_request(change, write.dry_run, write.idempotency.idempotency_key);
    send(paths, request, format).await
}

fn category_request(change: CategoryChange, dry_run: bool, key: Option<String>) -> Request {
    Request::ChangeCategory {
        change,
        dry_run,
        op_id: op_id_unless(dry_run),
        idempotency_key: key,
    }
}

/// A delete: with `--yes` or `--dry-run` at once; in a terminal after
/// showing the plan and asking; anywhere else, exit 2.
async fn destructive(
    paths: &Paths,
    yes: bool,
    write: CatalogWriteArgs,
    format: OutputFormat,
    request: impl Fn(bool, Option<String>) -> Request,
    question: &str,
) -> Result<(), CliError> {
    let key = write.idempotency.idempotency_key;
    if yes || write.dry_run {
        return send(paths, request(write.dry_run, key), format).await;
    }
    if !can_prompt() {
        return Err(CliError::message(
            ErrorKind::InvalidInput,
            "there's no terminal to ask in: pass --yes to delete (`ms-todo undo` brings it \
             back), or --dry-run to see what would be deleted"
                .into(),
        ));
    }
    let plan = match daemon_client::ask(paths, request(true, None)).await? {
        ResponseData::Plan(plan) => plan,
        _ => return Err(crate::unexpected_response()),
    };
    eprintln!("{}", describe(&plan).join("\n"));
    if !confirm(question)? {
        eprintln!("Nothing was deleted.");
        return Ok(());
    }
    send(paths, request(false, key), format).await
}

async fn send(paths: &Paths, request: Request, format: OutputFormat) -> Result<(), CliError> {
    let op_id = match &request {
        Request::ChangeCategory { op_id, .. } | Request::ChangeExtension { op_id, .. } => {
            op_id.clone()
        }
        _ => None,
    };
    let answer = match op_id {
        Some(op_id) => {
            daemon_client::ask_mutation(paths, request, &op_id, "with the matching `list` command")
                .await?
        }
        None => daemon_client::ask(paths, request).await?,
    };
    match answer {
        ResponseData::Plan(plan) => print_plan(format, &plan),
        ResponseData::Applied(applied) => print_applied(format, &applied),
        _ => Err(crate::unexpected_response()),
    }
}

/// What a category or extension plan says: the thing, and what's sent.
fn describe(plan: &Plan) -> Vec<String> {
    let target = &plan.changes["target"];
    let name = target["displayName"]
        .as_str()
        .or_else(|| target["extensionName"].as_str())
        .unwrap_or_default();
    let mut lines = vec![format!("Would {} {name:?}", verb(plan.action))];
    let body = &plan.changes["body"];
    if !body.is_null() {
        lines.push(format!("Sending: {body}"));
    }
    lines
}

#[derive(Serialize)]
struct DryRun<'a> {
    dry_run: bool,
    #[serde(flatten)]
    plan: &'a Plan,
}

fn print_plan(format: OutputFormat, plan: &Plan) -> Result<(), CliError> {
    match format {
        OutputFormat::Json | OutputFormat::Jsonl => print_json(
            format,
            &Versioned {
                schema_version: SCHEMA_VERSION,
                inner: &DryRun {
                    dry_run: true,
                    plan,
                },
            },
        ),
        OutputFormat::Ids => print_ids(
            plan.changes["target"]["id"]
                .as_str()
                .or_else(|| plan.changes["target"]["extensionName"].as_str()),
        ),
        OutputFormat::Csv => write_csv(
            &["action", "name", "body"],
            &[vec![
                verb(plan.action).to_owned(),
                describe(plan)[0].clone(),
                plan.changes["body"].to_string(),
            ]],
        ),
        OutputFormat::Table => {
            let mut stdout = std::io::stdout().lock();
            for line in describe(plan) {
                writeln!(stdout, "{line}")?;
            }
            Ok(())
        }
    }
}

pub(crate) fn print_applied(format: OutputFormat, applied: &Applied) -> Result<(), CliError> {
    let categories = matches!(
        applied.action,
        TaskAction::CategoryCreate | TaskAction::CategoryRecolor | TaskAction::CategoryDelete
    );
    match format {
        OutputFormat::Json | OutputFormat::Jsonl => print_json(
            format,
            &Versioned {
                schema_version: SCHEMA_VERSION,
                inner: applied,
            },
        ),
        OutputFormat::Ids if categories => {
            print_ids(applied.items.iter().map(|item| text(item, "id")))
        }
        OutputFormat::Ids => {
            print_ids(applied.items.iter().map(|item| text(item, "extensionName")))
        }
        OutputFormat::Csv if categories => {
            let rows: Vec<Vec<String>> = applied.items.iter().map(category_row).collect();
            write_csv(CATEGORY_COLUMNS, &rows)
        }
        OutputFormat::Csv => {
            let rows: Vec<Vec<String>> = applied.items.iter().map(extension_row).collect();
            write_csv(EXTENSION_COLUMNS, &rows)
        }
        OutputFormat::Table => {
            let mut stdout = std::io::stdout().lock();
            if let Some(undone) = &applied.undoes {
                writeln!(stdout, "Undid {undone}:")?;
            }
            for item in &applied.items {
                if categories {
                    let color = match text(item, "color") {
                        "" => String::new(),
                        color => format!(" ({color})"),
                    };
                    writeln!(
                        stdout,
                        "{} {:?}{color}  {}",
                        past_tense(applied.action),
                        text(item, "displayName"),
                        text(item, "id")
                    )?;
                } else {
                    writeln!(
                        stdout,
                        "{} {:?}  {}",
                        past_tense(applied.action),
                        text(item, "extensionName"),
                        Value::Object(data(item))
                    )?;
                }
            }
            Ok(())
        }
    }
}
