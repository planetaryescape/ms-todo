//! `tasks links` and `tasks open` (D-050): a task's links, as the TUI's
//! `o` finds them, from the task the daemon has cached.

use ms_todo_core::links::{Link, openable, task_links};
use ms_todo_core::{ErrorKind, Paths};
use ms_todo_protocol::{Entity, Request, ResponseData, SyncInfo};
use ms_todo_tui::open::Opener;
use serde::Serialize;
use serde_json::{Value, json};

use crate::args::LinkArgs;
use crate::csv_columns::text;
use crate::daemon_client;
use crate::error::CliError;
use crate::output::{OutputFormat, Render, Table, print_collection, print_ids, print_success};

pub const LINK_COLUMNS: &[&str] = &["url", "text", "source"];

pub const LINKS_TABLE: Table = Table {
    headings: &["#", "TEXT", "URL", "SOURCE"],
    row: |link| {
        vec![
            link.get("index").map(Value::to_string).unwrap_or_default(),
            text(link, "text").to_owned(),
            text(link, "url").to_owned(),
            text(link, "source").to_owned(),
        ]
    },
    csv_headings: LINK_COLUMNS,
    csv_row: |link| {
        LINK_COLUMNS
            .iter()
            .map(|column| text(link, column).to_owned())
            .collect()
    },
    bold_matches: None,
};

pub async fn links(paths: &Paths, args: LinkArgs, format: OutputFormat) -> Result<(), CliError> {
    let (task, sync) = task(paths, args).await?;
    let links = task_links(&task);
    if format == OutputFormat::Ids {
        return print_ids(links.iter().map(|link| link.url.as_str()));
    }
    let items: Vec<Entity> = links
        .iter()
        .enumerate()
        .map(|(at, link)| entity(at + 1, link))
        .collect();
    print_collection(format, &items, sync, &LINKS_TABLE)
}

/// A link as `tasks links` prints it: `index` is what `--index` takes.
fn entity(index: usize, link: &Link) -> Entity {
    let value = json!({
        "index": index,
        "url": link.url,
        "text": link.text,
        "source": link.source.as_str(),
        "openable": openable(&link.url).is_ok(),
    });
    value.as_object().cloned().unwrap_or_default()
}

#[derive(Debug, Serialize)]
pub struct Opened {
    pub url: String,
    pub text: String,
}

impl Render for Opened {
    fn table_rows(&self) -> Vec<(&'static str, String)> {
        vec![("Opened", self.url.clone()), ("Text", self.text.clone())]
    }
}

pub async fn open(
    paths: &Paths,
    args: LinkArgs,
    index: Option<usize>,
    format: OutputFormat,
    opener: &dyn Opener,
) -> Result<(), CliError> {
    let (task, _) = task(paths, args).await?;
    let opened = open_link(&task_links(&task), index, opener)?;
    print_success(format, &opened)
}

/// Open the link `index` picks (from 1), or the only one. Several and no
/// `index` is an error listing them, so a script never waits on a choice.
fn open_link(
    links: &[Link],
    index: Option<usize>,
    opener: &dyn Opener,
) -> Result<Opened, CliError> {
    let link = match (links, index) {
        ([], _) => {
            return Err(CliError::message(
                ErrorKind::NotFound,
                "the task has no links".into(),
            ));
        }
        (_, Some(index)) => index
            .checked_sub(1)
            .and_then(|at| links.get(at))
            .ok_or_else(|| {
                CliError::message(
                    ErrorKind::InvalidInput,
                    format!(
                        "--index {index}: the task has {} links, from 1",
                        links.len()
                    ),
                )
            })?,
        ([only], None) => only,
        (_, None) => {
            let listed: Vec<String> = links
                .iter()
                .enumerate()
                .map(|(at, link)| format!("  {}  {}  {}", at + 1, link.text, link.url))
                .collect();
            return Err(CliError::message(
                ErrorKind::InvalidInput,
                format!(
                    "the task has {} links; pick one with --index:\n{}",
                    links.len(),
                    listed.join("\n")
                ),
            ));
        }
    };
    let url = openable(&link.url).map_err(|why| {
        CliError::message(
            ErrorKind::InvalidInput,
            format!("not opening {}: {why}", link.url),
        )
    })?;
    opener.open(&url)?;
    Ok(Opened {
        url: url.to_string(),
        text: link.text.clone(),
    })
}

async fn task(paths: &Paths, args: LinkArgs) -> Result<(Entity, SyncInfo), CliError> {
    let request = Request::GetTasks {
        tasks: vec![args.task],
        list: args.list,
    };
    match daemon_client::ask(paths, request).await? {
        ResponseData::Tasks { mut items, sync } if items.len() == 1 => Ok((items.remove(0), sync)),
        _ => Err(crate::unexpected_response()),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use ms_todo_core::links::links;
    use url::Url;

    use super::*;

    #[derive(Default)]
    struct Recorder(RefCell<Vec<String>>);

    impl Opener for Recorder {
        fn open(&self, url: &Url) -> std::io::Result<()> {
            self.0.borrow_mut().push(url.to_string());
            Ok(())
        }
    }

    fn two() -> Vec<Link> {
        links(
            [("https://mail.example.com/1", Some("The email"))],
            Some(("see https://example.com/a", false)),
        )
    }

    #[test]
    fn the_only_link_opens_without_an_index() {
        let opener = Recorder::default();
        let one = links([], Some(("https://example.com/a.", false)));
        let opened = open_link(&one, None, &opener).expect("opened");
        assert_eq!(opened.url, "https://example.com/a");
        assert_eq!(*opener.0.borrow(), ["https://example.com/a"]);
    }

    #[test]
    fn several_links_need_an_index_and_are_listed() {
        let opener = Recorder::default();
        let error = open_link(&two(), None, &opener).expect_err("several");
        assert_eq!(error.kind, ErrorKind::InvalidInput);
        assert!(error.message.contains("--index"), "{}", error.message);
        assert!(
            error
                .message
                .contains("1  The email  https://mail.example.com/1")
                && error
                    .message
                    .contains("2  example.com  https://example.com/a"),
            "{}",
            error.message
        );
        assert!(opener.0.borrow().is_empty(), "nothing opened");
        open_link(&two(), Some(2), &opener).expect("second");
        assert_eq!(*opener.0.borrow(), ["https://example.com/a"]);
        for bad in [0, 3] {
            let error = open_link(&two(), Some(bad), &opener).expect_err("out of range");
            assert_eq!(error.kind, ErrorKind::InvalidInput);
        }
    }

    #[test]
    fn other_schemes_are_refused_and_no_links_is_not_found() {
        let opener = Recorder::default();
        let evil = links([("javascript:alert(1)", None)], None);
        let error = open_link(&evil, None, &opener).expect_err("refused");
        assert!(
            error.message.contains("only http, https and mailto"),
            "{}",
            error.message
        );
        assert!(opener.0.borrow().is_empty());
        let error = open_link(&[], None, &opener).expect_err("none");
        assert_eq!(error.kind, ErrorKind::NotFound);
    }
}
