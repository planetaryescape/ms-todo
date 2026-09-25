//! The task field flags rung 8e added to `tasks add` and `tasks edit`:
//! `--start`, `--recur`, `--category` and `--body-file`, read into what
//! the daemon takes.

use std::io::Read;
use std::path::Path;

use chrono::NaiveDate;
use ms_todo_core::{DATE_FORMAT, ErrorKind};
use ms_todo_nlp::{Recurrence, read_recurrence};
use ms_todo_protocol::Clearable;

use crate::error::CliError;
use crate::phrases;

/// The notes: `--body`, or `--body-file`'s content (`-` is stdin).
pub fn body(body: Option<String>, file: Option<&Path>) -> Result<Option<String>, CliError> {
    let Some(file) = file else {
        return Ok(body);
    };
    let read = if file == Path::new("-") {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text).map(|_| text)
    } else {
        std::fs::read_to_string(file)
    };
    read.map(Some).map_err(|error| {
        CliError::message(
            ErrorKind::InvalidInput,
            format!("can't read the notes from {}: {error}", file.display()),
        )
    })
}

/// `--recur`, its first time `anchor` when a due date is also given.
pub fn recurrence(text: &str, anchor: Option<NaiveDate>) -> Result<Recurrence, CliError> {
    read_recurrence(text, &phrases::now(), anchor).map_err(|error| {
        CliError::message(ErrorKind::InvalidInput, format!("--recur: {}", error.0))
    })
}

/// A `YYYY-MM-DD` the flags produced, as a date.
pub fn day(value: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(value, DATE_FORMAT).ok()
}

/// `tasks edit`'s recurrence change: `--recur` read against `--due` when
/// that sets a day, or `--clear-recur`.
pub fn edit_recurrence(
    recur: Option<&str>,
    clear: bool,
    due: Option<&Clearable<String>>,
) -> Result<Option<Clearable<serde_json::Value>>, CliError> {
    if clear {
        return Ok(Some(Clearable::Clear));
    }
    let Some(recur) = recur else {
        return Ok(None);
    };
    let anchor = match due {
        Some(Clearable::Set(due)) => day(due),
        Some(Clearable::Clear) => {
            return Err(CliError::message(
                ErrorKind::InvalidInput,
                "--recur needs a due date to start on, so it can't go with clearing the due date"
                    .into(),
            ));
        }
        None => None,
    };
    Ok(Some(Clearable::Set(recurrence(recur, anchor)?.to_graph())))
}

/// `tasks edit`'s categories: `--category`s replacing them, or
/// `--clear-categories`.
pub fn edit_categories(categories: Vec<String>, clear: bool) -> Option<Vec<String>> {
    if clear {
        Some(Vec::new())
    } else if categories.is_empty() {
        None
    } else {
        Some(categories)
    }
}
