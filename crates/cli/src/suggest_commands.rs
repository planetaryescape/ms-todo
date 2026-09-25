//! List suggestions in the CLI (rung 6b, D-053): `tasks suggest-list`,
//! and the note `tasks add` prints when a task went to the inbox and
//! another list looks likely. A suggestion never changes where a task
//! goes; the daemon asks the provider, when `[suggest]` turns it on.

use ms_todo_core::Paths;
use ms_todo_protocol::{ListSuggestion, Request, ResponseData};
use serde::Serialize;

use crate::daemon_client;
use crate::error::CliError;
use crate::output::{OutputFormat, Render, print_ids, print_success};

/// What `tasks suggest-list` prints: the list, or nulls for none.
#[derive(Serialize)]
pub struct Suggested {
    pub title: String,
    pub list_id: Option<String>,
    pub list_name: Option<String>,
    pub confidence: Option<f64>,
}

impl Suggested {
    fn new(title: String, suggestion: Option<ListSuggestion>) -> Self {
        match suggestion {
            Some(list) => Self {
                title,
                list_id: Some(list.list_id),
                list_name: Some(list.list_name),
                confidence: Some(list.confidence),
            },
            None => Self {
                title,
                list_id: None,
                list_name: None,
                confidence: None,
            },
        }
    }
}

impl Render for Suggested {
    fn table_rows(&self) -> Vec<(&'static str, String)> {
        let list = match (&self.list_name, self.confidence) {
            (Some(name), Some(confidence)) => format!("{name} ({confidence:.2})"),
            _ => "no suggestion".into(),
        };
        vec![("Title", self.title.clone()), ("List", list)]
    }
}

/// `tasks suggest-list TITLE`, in every format; `ids` prints the list's
/// ID, or nothing.
pub async fn suggest_list(
    paths: &Paths,
    title: String,
    format: OutputFormat,
) -> Result<(), CliError> {
    let suggestion = ask(paths, &title).await?;
    let suggested = Suggested::new(title, suggestion);
    if format == OutputFormat::Ids {
        return print_ids(suggested.list_id.as_deref());
    }
    print_success(format, &suggested)
}

pub async fn ask(paths: &Paths, title: &str) -> Result<Option<ListSuggestion>, CliError> {
    let request = Request::SuggestList {
        title: title.to_owned(),
    };
    match daemon_client::ask(paths, request).await? {
        ResponseData::ListSuggestion { suggestion } => Ok(suggestion),
        _ => Err(crate::unexpected_response()),
    }
}

/// After `tasks add` put a task in the inbox: the note naming a likely
/// list, what to do about it (move the task added, or for a dry run, add
/// it with the list).
pub fn add_note(list: &ListSuggestion, added: Option<&str>) -> String {
    let name = ms_todo_core::one_line_safe(&list.list_name);
    // For the shell: plain when nothing in it needs quoting.
    let argument = if name
        .chars()
        .all(|ch| ch.is_alphanumeric() || "-_.".contains(ch))
    {
        name.clone()
    } else {
        format!("{name:?}")
    };
    let what = match added {
        Some(id) => format!("move it with `ms-todo tasks move {id} --to {argument}`"),
        None => format!(
            "re-run with --list {argument} or {}",
            ms_todo_nlp::list_token(&name)
        ),
    };
    format!(
        "suggested list: {name} ({:.2}) \u{2014} {what}",
        list.confidence
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(name: &str) -> ListSuggestion {
        ListSuggestion {
            list_id: "l1".into(),
            list_name: name.into(),
            confidence: 0.857,
        }
    }

    #[test]
    fn a_dry_run_is_told_how_to_add_it_there() {
        assert_eq!(
            add_note(&list("Finances"), None),
            "suggested list: Finances (0.86) \u{2014} re-run with --list Finances or #Finances"
        );
    }

    #[test]
    fn an_added_task_is_told_how_to_move() {
        assert_eq!(
            add_note(&list("Home admin"), Some("t1")),
            "suggested list: Home admin (0.86) \u{2014} move it with \
             `ms-todo tasks move t1 --to \"Home admin\"`"
        );
        assert_eq!(
            add_note(&list("Home admin"), None),
            "suggested list: Home admin (0.86) \u{2014} re-run with --list \"Home admin\" or \
             #\"Home admin\""
        );
    }
}
