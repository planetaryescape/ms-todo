//! `attachments add|delete|download` (docs/blueprint/07-cli.md, D-056): a
//! task's files. Their metadata lives in the task's JSON under
//! `attachments` (`ms_todo_store::children`), so adding or deleting one is
//! a `child` outbox operation like a step's, applied to the cache at once
//! and undone by `undo`. The bytes never cross the socket (D-033): a
//! client names files by absolute path, and the daemon reads and writes
//! them.
//!
//! - **Add** checks each file now (a regular file, 25 MB at most) and
//!   records its size and modified time; it's read when the operation is
//!   sent, and refused then if it changed meanwhile
//!   (`outbox::attachment_write`).
//! - **Delete** keeps the attachment's bytes before the DELETE is sent
//!   (`kept`), so `undo` can attach it again for a week.
//! - **Download** writes each file safely into a directory (`files`).
//!
//! An attachment is named by its number from 1 (as `attachments list`
//! shows them), its ID, or its exact name.

pub(crate) mod files;
pub(crate) mod kept;

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::outbox::move_job::{blocking, sha256_hex};
use ms_todo_core::ErrorKind;
use ms_todo_graph::MAX_ATTACHMENT_BYTES;
use ms_todo_protocol::{DownloadedFile, ErrorPayload, ResponseData, TaskAction, TaskChange};
use ms_todo_store::{
    ATTACHMENTS, ChildVerb, Entity, LOCAL_CHILD_PREFIX, OpState, OutboxRow, children, find_child,
};
use serde_json::{Value, json};

use crate::handlers::{State, error_payload, graph_error};
use crate::task_children::{ChildWrite, change_children, find_all, invalid};
use crate::task_resolution::{Target, resolve_tasks};
use crate::task_writes::Targets;

/// On an attachment's delete: keep no copy for undo, as the user asked.
pub(crate) const NO_UNDO: &str = "no_undo";

/// A file to attach, as checked when it was queued: what the send
/// compares it with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Source {
    pub path: PathBuf,
    pub bytes: u64,
    /// Its modified time, in nanoseconds since 1970.
    pub modified: u64,
}

impl Source {
    /// The file at `path` as it is now: a regular file of 25 MB or less.
    pub fn check(path: &Path) -> Result<Self, String> {
        if !path.is_absolute() {
            return Err(format!(
                "{} isn't an absolute path; the CLI resolves paths before sending them",
                path.display()
            ));
        }
        let metadata = std::fs::metadata(path).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => format!("there's no file at {}", path.display()),
            _ => format!("cannot read {}: {error}", path.display()),
        })?;
        if !metadata.is_file() {
            return Err(format!("{} isn't a file", path.display()));
        }
        let bytes = metadata.len();
        if bytes > MAX_ATTACHMENT_BYTES as u64 {
            return Err(format!(
                "{} is {bytes} bytes; Microsoft To Do takes attachments up to 25 MB ({} bytes)",
                path.display(),
                MAX_ATTACHMENT_BYTES
            ));
        }
        let modified = metadata
            .modified()
            .ok()
            .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
            .and_then(|since| u64::try_from(since.as_nanos()).ok())
            .unwrap_or_default();
        Ok(Self {
            path: path.to_owned(),
            bytes,
            modified,
        })
    }

    /// As an operation's payload keeps it.
    pub fn to_json(&self) -> Value {
        json!({
            "path": self.path.to_string_lossy(),
            "bytes": self.bytes,
            "modified": self.modified,
        })
    }

    /// From an operation's payload.
    pub fn of(payload: &Value) -> Option<Self> {
        let file = &payload["file"];
        Some(Self {
            path: PathBuf::from(file["path"].as_str()?),
            bytes: file["bytes"].as_u64()?,
            modified: file["modified"].as_u64()?,
        })
    }
}

pub(crate) async fn change(
    state: &State,
    targets: &Targets<'_>,
    change: TaskChange,
    dry_run: bool,
    op_id: String,
) -> Result<ResponseData, ErrorPayload> {
    let writes = |raw: &Entity| match change {
        TaskChange::AddAttachments { files } => Ok((TaskAction::AttachmentAdd, add(&files)?)),
        TaskChange::DeleteAttachments {
            attachments,
            no_undo,
        } => {
            let items = children(raw, ATTACHMENTS);
            let writes = find_all(items, &attachments, "attachment", "name")?
                .iter()
                .map(|item| {
                    let action = TaskAction::AttachmentDelete;
                    let mut write = ChildWrite::delete(ATTACHMENTS, id_of(item), action);
                    if no_undo {
                        write.extra.insert(NO_UNDO.into(), Value::Bool(true));
                    }
                    write
                })
                .collect();
            Ok((TaskAction::AttachmentDelete, writes))
        }
        _ => Err(error_payload(
            ErrorKind::Internal,
            "not a change to attachments".into(),
        )),
    };
    change_children(state, targets, "attachments", dry_run, op_id, writes).await
}

fn one(targets: Vec<Target>) -> Result<Target, ErrorPayload> {
    match <[Target; 1]>::try_from(targets) {
        Ok([one]) => Ok(one),
        Err(_) => Err(invalid(
            "attachments change one task at a time; name one task".into(),
        )),
    }
}

/// An attachment's create for each file, in order; every file must pass
/// [`Source::check`], or nothing is queued.
fn add(files: &[String]) -> Result<Vec<ChildWrite>, ErrorPayload> {
    if files.is_empty() {
        return Err(invalid("name at least one file to attach".into()));
    }
    files
        .iter()
        .map(|file| {
            let source = Source::check(Path::new(file)).map_err(invalid)?;
            Ok(create(&source, &file_name(&source.path)))
        })
        .collect()
}

/// The create that attaches `source` as `name`.
fn create(source: &Source, name: &str) -> ChildWrite {
    let content_type = mime_guess::from_path(name)
        .first_or_octet_stream()
        .essence_str()
        .to_owned();
    let body = json!({ "name": name, "contentType": content_type, "size": source.bytes });
    let mut write = ChildWrite::create(ATTACHMENTS, body, TaskAction::AttachmentAdd);
    write.extra.insert("file".into(), source.to_json());
    write
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || "attachment".to_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn id_of(item: &Value) -> &str {
    item["id"].as_str().unwrap_or_default()
}

/// `DownloadAttachments`: fetch each attachment named (every one when
/// none is) and write it into `out_dir`.
pub(crate) async fn download(
    state: &State,
    task: &str,
    list: Option<&str>,
    names: &[String],
    out_dir: &str,
    force: bool,
) -> Result<ResponseData, ErrorPayload> {
    let target = one(resolve_tasks(state, &[task.to_owned()], list).await?)?;
    let Some(task_graph_id) = target.row.graph_id.clone() else {
        return Err(invalid(format!(
            "{:?} hasn't reached Microsoft To Do yet, so it has no attachments there",
            target.title()
        )));
    };
    let listed = attachments_of(state, &target, &task_graph_id).await?;
    let chosen = if names.is_empty() {
        listed
    } else {
        find_all(&listed, names, "attachment", "name")?
    };
    if chosen.is_empty() {
        return Err(error_payload(
            ErrorKind::NotFound,
            format!("{:?} has no attachments", target.title()),
        ));
    }
    if let Some(pending) = chosen
        .iter()
        .find(|item| id_of(item).starts_with(LOCAL_CHILD_PREFIX))
    {
        return Err(invalid(format!(
            "{:?} hasn't reached Microsoft To Do yet; download it once it has (`ms-todo outbox list`)",
            pending["name"].as_str().unwrap_or_default()
        )));
    }
    let out = PathBuf::from(out_dir);
    // Opened once: every file is written relative to this handle, so the
    // path swapped for a link during a download can't redirect it.
    let dir = blocking(move || files::Dir::open(&out))
        .await
        .map_err(|error| invalid(format!("can't save attachments there: {error}")))?;
    let dir = std::sync::Arc::new(dir);
    let mut written = Vec::new();
    for item in chosen {
        let id = id_of(&item).to_owned();
        let name = item["name"].as_str().unwrap_or("attachment").to_owned();
        let bytes = state
            .graph
            .download_attachment(&target.list.graph_id, &task_graph_id, &id)
            .await
            .map_err(graph_error)?;
        let count = bytes.len() as u64;
        let (dir, file_name) = (std::sync::Arc::clone(&dir), name.clone());
        let (path, sha256) = blocking(move || {
            let sha256 = sha256_hex(&bytes);
            let path = dir.write_new(&file_name, &bytes, force)?;
            Ok((path, sha256))
        })
        .await
        .map_err(|error| {
            error_payload(
                ErrorKind::Internal,
                format!("cannot save {name:?}: {error}"),
            )
        })?;
        written.push(DownloadedFile {
            id,
            name,
            path: path.to_string_lossy().into_owned(),
            bytes: count,
            sha256,
        });
    }
    Ok(ResponseData::Downloaded {
        task_id: target.local_id().to_owned(),
        files: written,
    })
}

/// The task's attachments as cached, or from Graph when the cache has no
/// list for it yet.
async fn attachments_of(
    state: &State,
    target: &Target,
    task_graph_id: &str,
) -> Result<Vec<Value>, ErrorPayload> {
    if target.row.raw.contains_key(ATTACHMENTS) {
        return Ok(children(&target.row.raw, ATTACHMENTS).to_vec());
    }
    let listed = state
        .graph
        .list_attachments(&target.list.graph_id, task_graph_id)
        .await
        .map_err(graph_error)?;
    Ok(listed.into_iter().map(Value::Object).collect())
}

/// The writes that undo `op`, an attachment's add or delete, on the task
/// `raw` (D-056): an add by deleting the attachment, a delete by
/// attaching again the bytes kept before it was sent. Refused when the
/// attachment was deleted since, or its bytes weren't kept or are past
/// their week.
pub(crate) fn inverse(
    kept_root: &Path,
    op: &OutboxRow,
    raw: &Entity,
) -> Result<Vec<ChildWrite>, String> {
    let id = op.payload["id"].as_str().unwrap_or_default();
    let before = op
        .rollback
        .as_ref()
        .and_then(|before| find_child(before, ATTACHMENTS, id))
        .map(|(_, item)| item.clone());
    let now = find_child(raw, ATTACHMENTS, id).map(|(_, item)| item.clone());
    let named = now
        .as_ref()
        .or(before.as_ref())
        .or(Some(op.body()))
        .and_then(|item| item["name"].as_str())
        .map(|name| format!(" {name:?}"))
        .unwrap_or_default();
    match ChildVerb::of(&op.payload) {
        Some(ChildVerb::Create) => match now {
            Some(_) => Ok(vec![ChildWrite::delete(
                ATTACHMENTS,
                id,
                TaskAction::AttachmentDelete,
            )]),
            None => Err(format!("its attachment{named} was deleted since")),
        },
        Some(ChildVerb::Delete) => {
            let Some(before) = before else {
                return Err(format!(
                    "ms-todo has no record of the attachment{named} it deleted"
                ));
            };
            if op.payload[NO_UNDO] == true {
                return Err(format!(
                    "the attachment{named} was deleted with --no-undo, so ms-todo kept no copy"
                ));
            }
            if op.state != OpState::Done {
                return Err(format!(
                    "deleting the attachment{named} hasn't reached Microsoft To Do yet; undo it once it has"
                ));
            }
            let kept = kept::path(kept_root, &op.op_id);
            let source = Source::check(&kept).map_err(|_| {
                format!(
                    "ms-todo no longer has a copy of the attachment{named}: a deleted attachment is \
                     kept for {} days",
                    kept::KEPT_FOR.as_secs() / 86_400
                )
            })?;
            let name = before["name"].as_str().unwrap_or("attachment");
            let mut write = create(&source, name);
            if let Some(content_type) = before.get("contentType") {
                write.body["contentType"] = content_type.clone();
            }
            Ok(vec![write])
        }
        _ => Err(format!("ms-todo can't tell what {} did", op.op_id)),
    }
}

#[cfg(test)]
mod tests {
    use ms_todo_store::{OpKind, child_payload};

    use super::*;

    fn raw(value: Value) -> Entity {
        value.as_object().cloned().expect("object")
    }

    fn op(payload: Value, before: Value, state: OpState) -> OutboxRow {
        OutboxRow {
            op_id: "op-1".into(),
            seq: 1,
            command_id: "op-1".into(),
            created_at: 0,
            entity_local_id: "t1".into(),
            list_local_id: "l1".into(),
            op: OpKind::Child,
            action: "attachment_delete".into(),
            payload,
            depends_on: None,
            undoes: None,
            attempts: 1,
            next_attempt_at: 0,
            state,
            last_error: None,
            rollback: before.as_object().cloned(),
            sent_at: None,
            unknown_since: None,
            note: None,
            finished_at: None,
            progress: None,
            title: None,
        }
    }

    #[test]
    fn a_file_is_checked_when_it_is_queued() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("invoice.pdf");
        std::fs::write(&file, b"%PDF-1.7").expect("write");
        let writes = add(&[file.to_string_lossy().into_owned()]).expect("add");
        let payload = writes[0].payload();
        assert_eq!(payload["body"]["name"], "invoice.pdf");
        assert_eq!(payload["body"]["contentType"], "application/pdf");
        assert_eq!(payload["file"]["bytes"], 8);
        assert!(
            payload["id"]
                .as_str()
                .expect("id")
                .starts_with(LOCAL_CHILD_PREFIX)
        );
        assert!(add(&["relative.pdf".into()]).is_err());
        assert!(
            add(&[dir.path().to_string_lossy().into_owned()]).is_err(),
            "a directory"
        );
        let missing = dir.path().join("missing").to_string_lossy().into_owned();
        let error = add(&[missing]).expect_err("missing");
        assert!(error.message.contains("no file"), "{}", error.message);
    }

    #[test]
    fn over_25_mb_is_refused_before_anything_is_queued() {
        let dir = tempfile::tempdir().expect("tempdir");
        let big = dir.path().join("big.bin");
        let file = std::fs::File::create(&big).expect("create");
        file.set_len(MAX_ATTACHMENT_BYTES as u64 + 1).expect("size");
        let error = add(&[big.to_string_lossy().into_owned()]).expect_err("too big");
        assert_eq!(error.kind, "invalid_input");
        assert!(error.message.contains("25 MB"), "{}", error.message);
    }

    #[test]
    fn an_attachment_is_named_by_number_id_or_exact_name() {
        let task = raw(json!({ ATTACHMENTS: [
            { "id": "A1", "name": "a.pdf" },
            { "id": "A2", "name": "b.pdf" },
            { "id": "A3", "name": "b.pdf" }
        ] }));
        let names = |names: &[&str]| {
            names
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>()
        };
        let find =
            |names: &[String]| find_all(children(&task, ATTACHMENTS), names, "attachment", "name");
        let found = find(&names(&["A3", "1", "a.pdf"])).expect("found");
        let ids: Vec<&str> = found.iter().map(id_of).collect();
        assert_eq!(ids, ["A3", "A1"]);
        let shared = find(&names(&["b.pdf"])).expect_err("ambiguous");
        assert!(shared.message.contains("(2, 3)"), "{}", shared.message);
        assert_eq!(find(&names(&["4"])).expect_err("none").kind, "not_found");
    }

    #[test]
    fn a_delete_is_undone_from_the_kept_copy_until_it_is_gone() {
        let kept_root = tempfile::tempdir().expect("tempdir");
        let before = json!({ ATTACHMENTS: [
            { "id": "A1", "name": "a.pdf", "contentType": "application/pdf", "size": 300 }
        ] });
        let delete = op(
            child_payload(ATTACHMENTS, ChildVerb::Delete, "A1", json!({}), &[]),
            before,
            OpState::Done,
        );
        let refused = inverse(kept_root.path(), &delete, &raw(json!({}))).expect_err("not kept");
        assert!(refused.contains("7 days"), "{refused}");
        let kept = kept::path(kept_root.path(), "op-1");
        std::fs::write(&kept, b"pdf!").expect("write");
        let writes = inverse(kept_root.path(), &delete, &raw(json!({}))).expect("undoable");
        let payload = writes[0].payload();
        assert_eq!(payload["verb"], "create");
        assert_eq!(payload["body"]["name"], "a.pdf");
        assert_eq!(payload["file"]["path"], kept.to_string_lossy().as_ref());
        assert_eq!(payload["file"]["bytes"], 4);
    }

    #[test]
    fn an_add_is_undone_by_deleting_it_unless_it_is_gone() {
        let add = op(
            child_payload(
                ATTACHMENTS,
                ChildVerb::Create,
                "A9",
                json!({ "name": "a.pdf" }),
                &[],
            ),
            json!({}),
            OpState::Done,
        );
        let now = raw(json!({ ATTACHMENTS: [{ "id": "A9", "name": "a.pdf", "size": 999 }] }));
        let writes = inverse(Path::new("/kept"), &add, &now).expect("undoable");
        assert_eq!(writes[0].verb, ChildVerb::Delete);
        assert!(inverse(Path::new("/kept"), &add, &raw(json!({ ATTACHMENTS: [] }))).is_err());
    }
}
