//! `attachments list|add|download|delete` (docs/blueprint/07-cli.md,
//! D-056): a task's files. Reads come from the task the daemon has
//! cached; adds and deletes are `ChangeTasks` requests on the one task, so
//! they go through the outbox and `undo` reverses them. No file's bytes
//! cross the socket (D-033): this resolves paths and checks them, and the
//! daemon reads and writes the files.

use std::path::{Path, PathBuf};

use ms_todo_core::{ErrorKind, MAX_ATTACHMENT_BYTES, Paths};
use ms_todo_protocol::{DownloadedFile, Entity, Request, ResponseData, TaskChange};
use serde::Serialize;
use serde_json::Value;

use crate::args::{AttachmentsCommand, LinkArgs};
use crate::child_commands::{delete, number, numbered, write_child};

use crate::csv_columns::text;
use crate::daemon_client;
use crate::error::CliError;
use crate::link_commands;
use crate::output::{
    OutputFormat, SCHEMA_VERSION, Table, Versioned, print_collection, print_ids, print_json,
    write_csv,
};

pub const ATTACHMENT_COLUMNS: &[&str] =
    &["index", "id", "name", "size", "content_type", "modified"];

pub const ATTACHMENTS_TABLE: Table = Table {
    headings: &["#", "NAME", "SIZE", "TYPE", "MODIFIED", "ID"],
    row: |attachment| {
        vec![
            number(attachment),
            text(attachment, "name").to_owned(),
            size(attachment),
            text(attachment, "contentType").to_owned(),
            text(attachment, "lastModifiedDateTime").to_owned(),
            text(attachment, "id").to_owned(),
        ]
    },
    csv_headings: ATTACHMENT_COLUMNS,
    csv_row: |attachment| {
        vec![
            number(attachment),
            text(attachment, "id").to_owned(),
            text(attachment, "name").to_owned(),
            size(attachment),
            text(attachment, "contentType").to_owned(),
            text(attachment, "lastModifiedDateTime").to_owned(),
        ]
    },
    bold_matches: None,
};

pub const DOWNLOAD_COLUMNS: &[&str] = &["id", "name", "path", "bytes", "sha256"];

pub async fn run(
    paths: &Paths,
    command: AttachmentsCommand,
    format: OutputFormat,
) -> Result<(), CliError> {
    match command {
        AttachmentsCommand::List(task) => list(paths, task, format).await,
        AttachmentsCommand::Add { task, files, write } => {
            let files = files
                .iter()
                .map(|file| checked(file))
                .collect::<Result<Vec<_>, _>>()?;
            write_child(
                paths,
                task,
                TaskChange::AddAttachments { files },
                write,
                format,
            )
            .await
        }
        AttachmentsCommand::Download {
            task,
            attachments,
            out,
            force,
        } => download(paths, task, attachments, out, force, format).await,
        AttachmentsCommand::Delete {
            task,
            attachments,
            yes,
            write,
        } => {
            let change = TaskChange::DeleteAttachments { attachments };
            delete(paths, task, change, write, yes, format).await
        }
    }
}

/// The task's attachments, each with `index` (its number from 1) and
/// Graph's metadata.
async fn list(paths: &Paths, task: LinkArgs, format: OutputFormat) -> Result<(), CliError> {
    let (task, sync) = link_commands::task(paths, task).await?;
    if task.get("hasAttachments") == Some(&Value::Bool(true))
        && !task.contains_key("attachments")
        && format != OutputFormat::Json
    {
        eprintln!(
            "note: Microsoft To Do says this task has attachments that haven't synced yet; \
             `ms-todo sync --wait` fetches them"
        );
    }
    print_collection(
        format,
        &numbered(&task, "attachments"),
        sync,
        &ATTACHMENTS_TABLE,
    )
}

/// `file` as an absolute path, checked to be a regular file of 25 MB or
/// less, so a mistake is caught before anything is queued. The daemon
/// checks it again, and again when it reads it.
fn checked(file: &Path) -> Result<String, CliError> {
    let absolute = std::path::absolute(file).map_err(|error| {
        CliError::message(
            ErrorKind::InvalidInput,
            format!("{}: {error}", file.display()),
        )
    })?;
    let invalid = |why: String| CliError::message(ErrorKind::InvalidInput, why);
    let metadata = std::fs::metadata(&absolute).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => invalid(format!("there's no file at {}", file.display())),
        _ => invalid(format!("cannot read {}: {error}", file.display())),
    })?;
    if !metadata.is_file() {
        return Err(invalid(format!("{} isn't a file", file.display())));
    }
    if metadata.len() > MAX_ATTACHMENT_BYTES as u64 {
        return Err(invalid(format!(
            "{} is {} bytes; Microsoft To Do takes attachments up to 25 MB ({MAX_ATTACHMENT_BYTES} \
             bytes)",
            file.display(),
            metadata.len()
        )));
    }
    absolute.into_os_string().into_string().map_err(|path| {
        invalid(format!(
            "{} isn't valid UTF-8, which ms-todo needs to name the file",
            PathBuf::from(path).display()
        ))
    })
}

#[derive(Serialize)]
struct Downloaded<'a> {
    task_id: &'a str,
    files: &'a [DownloadedFile],
}

async fn download(
    paths: &Paths,
    task: LinkArgs,
    attachments: Vec<String>,
    out: Option<PathBuf>,
    force: bool,
    format: OutputFormat,
) -> Result<(), CliError> {
    let out = match out {
        Some(out) => std::path::absolute(&out),
        None => std::env::current_dir(),
    }
    .map_err(|error| {
        CliError::message(ErrorKind::InvalidInput, format!("where to save: {error}"))
    })?;
    let request = Request::DownloadAttachments {
        task: task.task,
        list: task.list,
        attachments,
        out_dir: out.to_string_lossy().into_owned(),
        force,
    };
    let (task_id, files) = match daemon_client::ask(paths, request).await? {
        ResponseData::Downloaded { task_id, files } => (task_id, files),
        _ => return Err(crate::unexpected_response()),
    };
    match format {
        OutputFormat::Json => print_json(
            format,
            &Versioned {
                schema_version: SCHEMA_VERSION,
                inner: &Downloaded {
                    task_id: &task_id,
                    files: &files,
                },
            },
        ),
        OutputFormat::Jsonl => {
            for file in &files {
                print_json(
                    format,
                    &Versioned {
                        schema_version: SCHEMA_VERSION,
                        inner: file,
                    },
                )?;
            }
            Ok(())
        }
        OutputFormat::Ids => print_ids(files.iter().map(|file| file.path.as_str())),
        OutputFormat::Csv => {
            let rows: Vec<Vec<String>> = files
                .iter()
                .map(|file| {
                    vec![
                        file.id.clone(),
                        file.name.clone(),
                        file.path.clone(),
                        file.bytes.to_string(),
                        file.sha256.clone(),
                    ]
                })
                .collect();
            write_csv(DOWNLOAD_COLUMNS, &rows)
        }
        OutputFormat::Table => {
            use std::io::Write;
            let mut stdout = std::io::stdout().lock();
            for file in &files {
                writeln!(
                    stdout,
                    "Saved {:?} to {} ({} bytes)",
                    ms_todo_core::display_safe(&file.name),
                    ms_todo_core::display_safe(&file.path),
                    file.bytes
                )?;
            }
            Ok(())
        }
    }
}

fn size(attachment: &Entity) -> String {
    attachment
        .get("size")
        .and_then(Value::as_u64)
        .map(|size| size.to_string())
        .unwrap_or_default()
}
