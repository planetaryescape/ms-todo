//! A task's attachments in the TUI (rung 8b, D-056): listed in the detail
//! pane under the link. `A` (or `a` on the attachment rows) asks for a
//! file's path and attaches it; Enter or `o` on an attachment downloads it
//! into `[attachments] download_dir` and opens it with the system's
//! opener; `d` deletes it after a `y`. The daemon reads and writes the
//! files: the TUI sends paths, never bytes, and opens only a file the
//! daemon says it wrote in that directory.

use std::path::{Path, PathBuf};

use ms_todo_protocol::{ErrorPayload, Request, ResponseData, TaskChange};

use super::{App, Effect, Level, LineEditor, LocalEffect, Mode, Tag, Task, Write};

/// Where files go and how typed paths are read, fixed when the TUI
/// starts, so `update` does no I/O for them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Places {
    /// `[attachments] download_dir`, as its real path.
    pub download_dir: PathBuf,
    /// Where the TUI was started: a relative path is read from here.
    pub cwd: PathBuf,
    /// `~` in a typed path.
    pub home: Option<PathBuf>,
}

impl App {
    /// `A`: ask which file to attach to `task`.
    pub(super) fn start_attach(&mut self, task: &Task) {
        self.mode = Mode::Attaching {
            id: task.id.clone(),
            input: LineEditor::single(""),
            error: None,
        };
    }

    /// Enter in the path prompt: attach the file, or say why the path
    /// can't be read. The daemon checks the file itself.
    pub(super) fn submit_attach(&mut self) -> Vec<Effect> {
        let Mode::Attaching { id, input, .. } = &self.mode else {
            return Vec::new();
        };
        let (id, typed) = (id.clone(), input.text().trim().to_owned());
        if typed.is_empty() {
            self.mode = Mode::Normal;
            return Vec::new();
        }
        let path = match resolve_path(&typed, &self.places) {
            Ok(path) => path,
            Err(why) => {
                if let Mode::Attaching { error, .. } = &mut self.mode {
                    *error = Some(why);
                }
                return Vec::new();
            }
        };
        self.mode = Mode::Normal;
        let change = TaskChange::AddAttachments {
            files: vec![path.to_string_lossy().into_owned()],
        };
        vec![super::change(Write::Edit, vec![id], change)]
    }

    /// Enter or `o` on an attachment: save it into the download directory;
    /// its answer opens it.
    pub(super) fn download_attachment(&mut self, task: &Task, at: usize) -> Vec<Effect> {
        let Some(attachment) = task.attachments.get(at) else {
            return Vec::new();
        };
        if attachment.uploading() {
            self.show(
                Level::Info,
                "Still uploading; open it once it's in Microsoft To Do",
            );
            return Vec::new();
        }
        self.show(
            Level::Info,
            &format!("Downloading {}\u{2026}", attachment.name),
        );
        vec![Effect {
            tag: Tag::Download,
            request: Request::DownloadAttachments {
                task: task.id.clone(),
                list: None,
                attachments: vec![attachment.id.clone()],
                out_dir: self.places.download_dir.to_string_lossy().into_owned(),
                force: false,
            },
        }]
    }

    /// The daemon saved (or couldn't save) an attachment: open it, if it's
    /// where it was asked to go.
    pub(super) fn downloaded(&mut self, result: Result<ResponseData, ErrorPayload>) {
        let file = match result {
            Ok(ResponseData::Downloaded { files, .. }) => files.into_iter().next(),
            Ok(_) => None,
            Err(error) => {
                self.show(
                    Level::Error,
                    &format!("Couldn't download it: {}", error.message),
                );
                return;
            }
        };
        let Some(file) = file else {
            self.show(Level::Error, "The daemon sent an unexpected answer");
            return;
        };
        let path = PathBuf::from(&file.path);
        let inside = path.parent() == Some(self.places.download_dir.as_path());
        match url::Url::from_file_path(&path) {
            Ok(url) if inside => {
                self.show(Level::Info, &format!("Saved {}", file.path));
                self.local = Some(LocalEffect::Open(url));
            }
            _ => self.show(
                Level::Error,
                &format!(
                    "Saved {}, but not opening it: it isn't in the download directory",
                    file.path
                ),
            ),
        }
    }

    /// `d` on an attachment: ask first.
    pub(super) fn confirm_attachment_delete(&mut self, task: &Task, at: usize) {
        let Some(attachment) = task.attachments.get(at) else {
            return;
        };
        self.mode = Mode::ConfirmDeleteChild {
            id: task.id.clone(),
            change: TaskChange::DeleteAttachments {
                attachments: vec![attachment.id.clone()],
            },
            what: format!("attachment \"{}\"", attachment.name),
        };
    }
}

/// A typed path as an absolute one: `~` is the home directory, and a
/// relative path is read from where the TUI started.
pub fn resolve_path(typed: &str, places: &Places) -> Result<PathBuf, String> {
    let expanded =
        expand_home(typed, places.home.as_deref()).ok_or("~ can't be read: HOME isn't set")?;
    if expanded.is_absolute() {
        return Ok(expanded);
    }
    if !places.cwd.is_absolute() {
        return Err("give the full path: the TUI's directory isn't known".into());
    }
    Ok(join_clean(&places.cwd, &expanded))
}

/// `path` with a leading `~` (alone, or `~/…`) as `home`; `None` when it
/// has one and there's no home. `~user` is left as it is.
pub fn expand_home(path: &str, home: Option<&Path>) -> Option<PathBuf> {
    match path.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            Some(home?.join(rest.trim_start_matches('/')))
        }
        _ => Some(PathBuf::from(path)),
    }
}

/// `base` then `relative`, with `.` dropped, so a path reads plainly.
fn join_clean(base: &Path, relative: &Path) -> PathBuf {
    let mut joined = base.to_owned();
    for part in relative.components() {
        match part {
            std::path::Component::CurDir => {}
            other => joined.push(other),
        }
    }
    joined
}

#[cfg(test)]
mod tests {
    use ms_todo_protocol::{DownloadedFile, Scope};
    use serde_json::json;

    use super::*;
    use crate::action::Action;
    use crate::app::steps::DetailRow;
    use crate::app::tests::{act, answer_seed, seed, seeded, task};
    use crate::app::{Msg, Pane};
    use crate::keybindings::Context;

    /// "Taxes" first, with a file and one still uploading.
    fn filed() -> App {
        let mut app = seeded().with_places(places());
        let effects = app.update(Msg::Event(ms_todo_protocol::Event::ResyncNeeded));
        let taxes = task(
            "t9",
            "Taxes",
            json!({
                "hasAttachments": true,
                "attachments": [
                    { "id": "A1", "name": "return.pdf", "size": 48_000 },
                    { "id": "local-1", "name": "receipt.jpg", "size": 900 }
                ]
            }),
        );
        answer_seed(
            &mut app,
            &effects[0],
            seed(Scope::List { id: "home".into() }, vec![taxes]),
        );
        app.task_index = 0;
        app.focus = Pane::Detail;
        app
    }

    fn to_row(app: &mut App, row: DetailRow) {
        act(app, Action::JumpTop);
        for _ in 0..20 {
            if app.detail_row_now() == Some(row) {
                return;
            }
            act(app, Action::MoveDown);
        }
        unreachable!("never reached {row:?}");
    }

    #[test]
    fn a_attaches_the_file_at_the_path_typed() {
        let mut app = filed();
        app.focus = Pane::Tasks;
        act(&mut app, Action::Attach);
        assert_eq!(app.context(), Context::Prompt);
        for ch in "~/scan.pdf".chars() {
            app.update(Msg::Char(ch));
        }
        let effects = act(&mut app, Action::Submit);
        match &effects[0].request {
            Request::ChangeTasks { tasks, change, .. } => {
                assert_eq!(tasks, &["t9".to_owned()]);
                assert_eq!(
                    change,
                    &TaskChange::AddAttachments {
                        files: vec!["/home/bk/scan.pdf".into()]
                    }
                );
            }
            other => unreachable!("not a change: {other:?}"),
        }
        assert_eq!(app.mode, Mode::Normal);
        // On the Files rows, `a` attaches too.
        app.focus = Pane::Detail;
        to_row(&mut app, DetailRow::Attachment(0));
        act(&mut app, Action::Add);
        assert!(matches!(app.mode, Mode::Attaching { .. }));
        act(&mut app, Action::Cancel);
    }

    #[test]
    fn enter_on_a_file_saves_it_and_its_answer_opens_it() {
        let mut app = filed();
        to_row(&mut app, DetailRow::Attachment(0));
        assert_eq!(app.context(), Context::Steps);
        let effects = act(&mut app, Action::EditHere);
        assert_eq!(effects[0].tag, Tag::Download);
        assert_eq!(
            effects[0].request,
            Request::DownloadAttachments {
                task: "t9".into(),
                list: None,
                attachments: vec!["A1".into()],
                out_dir: "/home/bk/Downloads".into(),
                force: false,
            }
        );
        let saved = |path: &str| Msg::Response {
            tag: Tag::Download,
            result: Ok(ResponseData::Downloaded {
                task_id: "t9".into(),
                files: vec![DownloadedFile {
                    id: "A1".into(),
                    name: "return.pdf".into(),
                    path: path.into(),
                    bytes: 47_800,
                    sha256: "ab".into(),
                }],
            }),
        };
        app.update(saved("/home/bk/Downloads/return (1).pdf"));
        let url = url::Url::from_file_path("/home/bk/Downloads/return (1).pdf").expect("url");
        assert_eq!(app.local.take(), Some(LocalEffect::Open(url)));
        app.update(saved("/etc/return.pdf"));
        assert_eq!(app.local, None, "never opened outside the directory");
        // `o` and `e` do the same; one still uploading can't be yet.
        assert_eq!(act(&mut app, Action::OpenLink)[0].tag, Tag::Download);
        assert_eq!(act(&mut app, Action::Edit)[0].tag, Tag::Download);
        to_row(&mut app, DetailRow::Attachment(1));
        assert!(act(&mut app, Action::EditHere).is_empty());
    }

    #[test]
    fn d_deletes_a_file_after_asking() {
        let mut app = filed();
        to_row(&mut app, DetailRow::Attachment(0));
        assert!(act(&mut app, Action::Delete).is_empty());
        assert_eq!(app.context(), Context::Confirm);
        let effects = act(&mut app, Action::Confirm);
        match &effects[0].request {
            Request::ChangeTasks { change, .. } => assert_eq!(
                change,
                &TaskChange::DeleteAttachments {
                    attachments: vec!["A1".into()]
                }
            ),
            other => unreachable!("not a change: {other:?}"),
        }
    }

    fn places() -> Places {
        Places {
            download_dir: PathBuf::from("/home/bk/Downloads"),
            cwd: PathBuf::from("/home/bk/work"),
            home: Some(PathBuf::from("/home/bk")),
        }
    }

    #[test]
    fn a_typed_path_is_made_absolute() {
        let places = places();
        assert_eq!(
            resolve_path("~/invoice.pdf", &places),
            Ok(PathBuf::from("/home/bk/invoice.pdf"))
        );
        assert_eq!(
            resolve_path("./scan.pdf", &places),
            Ok(PathBuf::from("/home/bk/work/scan.pdf"))
        );
        assert_eq!(
            resolve_path("/tmp/a b.txt", &places),
            Ok(PathBuf::from("/tmp/a b.txt"))
        );
        assert_eq!(
            resolve_path("~bob/x", &places),
            Ok(PathBuf::from("/home/bk/work/~bob/x")),
            "only ~ alone is home"
        );
        let homeless = Places {
            home: None,
            ..places
        };
        assert!(resolve_path("~/x", &homeless).is_err());
    }
}
