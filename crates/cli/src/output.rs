// Adapted from spotuify crates/spotuify-protocol/src/output.rs @ d807e5e4f9d2f09878cdc22309af3589623f7785
// (the `OutputFormat` enum).

//! What commands print (docs/blueprint/07-cli.md#output-contract). Results
//! go to stdout and errors to stderr, never both. JSON carries
//! `schema_version`; `raw` is exempt and prints Graph's body as it came.

use std::io::{IsTerminal, Write};

use ms_todo_core::{ErrorKind, display_safe};
use ms_todo_protocol::{Entity, SyncInfo, SyncState};
use serde::Serialize;
use serde_json::Value;

use crate::error::CliError;

/// Bumped when an output shape changes incompatibly. 2 since rung 3a, where
/// `id` became the local ID, with `graph_id` beside it (D-034).
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub enum OutputFormat {
    Table,
    Json,
    // One compact JSON object per line.
    Jsonl,
    // One ID per line, for piping into another command.
    Ids,
    // RFC 4180, with a header row; see `csv_columns`.
    Csv,
}

impl OutputFormat {
    /// Table for people, JSON for pipes and agents (docs/blueprint/07-cli.md).
    pub fn resolve(flag: Option<Self>) -> Self {
        flag.unwrap_or_else(|| {
            if std::io::stdout().is_terminal() {
                Self::Table
            } else {
                Self::Json
            }
        })
    }
}

/// Something a command prints on success that isn't a collection.
pub trait Render: Serialize {
    fn table_rows(&self) -> Vec<(&'static str, String)>;
}

/// A result with `schema_version` beside its own fields.
#[derive(Serialize)]
pub struct Versioned<'a, T: Serialize> {
    pub schema_version: u32,
    #[serde(flatten)]
    pub inner: &'a T,
}

pub fn print_success(format: OutputFormat, value: &impl Render) -> Result<(), CliError> {
    let versioned = Versioned {
        schema_version: SCHEMA_VERSION,
        inner: value,
    };
    match format {
        OutputFormat::Json | OutputFormat::Jsonl => print_json(format, &versioned),
        OutputFormat::Table => {
            let rows = value.table_rows();
            let width = rows.iter().map(|(label, _)| label.len()).max().unwrap_or(0);
            let mut stdout = std::io::stdout().lock();
            for (label, text) in rows {
                writeln!(stdout, "{label:<width$}  {}", display_safe(&text))?;
            }
            Ok(())
        }
        OutputFormat::Csv => {
            let json = serde_json::to_value(value).map_err(std::io::Error::from)?;
            let Value::Object(fields) = json else {
                return Err(CliError::message(
                    ErrorKind::Internal,
                    "this result isn't a record, so it has no CSV form".into(),
                ));
            };
            let headers: Vec<&str> = fields.keys().map(String::as_str).collect();
            let row: Vec<String> = fields.values().map(csv_cell).collect();
            write_csv(&headers, &[row])
        }
        OutputFormat::Ids => Err(ids_not_supported()),
    }
}

// One flat record's value as a CSV cell: arrays joined with `;`, null empty.
pub fn csv_cell(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(csv_cell).collect::<Vec<_>>().join(";"),
        other => other.to_string(),
    }
}

/// Write RFC 4180 CSV to stdout: the header row, then `rows`.
pub fn write_csv<H: AsRef<[u8]>>(headers: &[H], rows: &[Vec<String>]) -> Result<(), CliError> {
    let mut writer = csv::Writer::from_writer(std::io::stdout().lock());
    let written = writer
        .write_record(headers)
        .and_then(|()| rows.iter().try_for_each(|row| writer.write_record(row)));
    written.map_err(csv_io_error)?;
    writer.flush()?;
    Ok(())
}

// `csv` wraps I/O errors; unwrap them so a closed pipe stays one.
fn csv_io_error(error: csv::Error) -> std::io::Error {
    match error.into_kind() {
        csv::ErrorKind::Io(error) => error,
        other => std::io::Error::other(format!("{other:?}")),
    }
}

/// A collection's table: column headings, and a row of cells per item;
/// and its CSV columns, which are fuller and fixed (`csv_columns`).
pub struct Table {
    pub headings: &'static [&'static str],
    pub row: fn(&Entity) -> Vec<String>,
    pub csv_headings: &'static [&'static str],
    pub csv_row: fn(&Entity) -> Vec<String>,
}

#[derive(Serialize)]
struct CollectionEnvelope<'a> {
    schema_version: u32,
    sync: SyncInfo,
    items: &'a [Entity],
}

/// A collection from the cache. JSON carries the scope's sync state in its
/// envelope; the other formats have none, so while the cache is still
/// `initial` they say so on stderr.
pub fn print_collection(
    format: OutputFormat,
    items: &[Entity],
    sync: SyncInfo,
    table: &Table,
) -> Result<(), CliError> {
    if sync.state == SyncState::Initial && format != OutputFormat::Json {
        eprintln!(
            "ms-todo is still syncing for the first time, so this may be incomplete; \
             `ms-todo sync --wait` waits for it"
        );
    }
    let mut stdout = std::io::stdout().lock();
    match format {
        OutputFormat::Json => print_json(
            format,
            &CollectionEnvelope {
                schema_version: SCHEMA_VERSION,
                sync,
                items,
            },
        ),
        OutputFormat::Jsonl => {
            // No envelope in JSONL, so each record carries the version.
            for item in items {
                let record = Versioned {
                    schema_version: SCHEMA_VERSION,
                    inner: item,
                };
                serde_json::to_writer(&mut stdout, &record).map_err(std::io::Error::from)?;
                writeln!(stdout)?;
            }
            Ok(())
        }
        OutputFormat::Ids => {
            drop(stdout);
            print_ids(items.iter().filter_map(|item| item.get("id")?.as_str()))
        }
        OutputFormat::Csv => {
            drop(stdout);
            let rows: Vec<Vec<String>> = items.iter().map(table.csv_row).collect();
            write_csv(table.csv_headings, &rows)
        }
        OutputFormat::Table => {
            let rows: Vec<Vec<String>> = items.iter().map(table.row).collect();
            let headings: Vec<String> = table.headings.iter().map(|&h| h.to_owned()).collect();
            write_table(&mut stdout, &headings, &rows)?;
            Ok(())
        }
    }
}

/// One ID per line.
pub fn print_ids<'a>(ids: impl IntoIterator<Item = &'a str>) -> Result<(), CliError> {
    let mut stdout = std::io::stdout().lock();
    for id in ids {
        writeln!(stdout, "{id}")?;
    }
    Ok(())
}

/// `raw` prints Graph's body unchanged; a table is pretty JSON too.
pub fn print_raw(format: OutputFormat, body: &Value) -> Result<(), CliError> {
    match format {
        OutputFormat::Ids => Err(ids_not_supported()),
        OutputFormat::Csv => Err(CliError::message(
            ErrorKind::InvalidInput,
            "`raw` prints Graph's JSON as it came, so it has no CSV form; use --format json".into(),
        )),
        OutputFormat::Jsonl => print_json(format, body),
        OutputFormat::Json | OutputFormat::Table => print_json(OutputFormat::Json, body),
    }
}

pub fn print_json(format: OutputFormat, value: &impl Serialize) -> Result<(), CliError> {
    let mut stdout = std::io::stdout().lock();
    let written = if format == OutputFormat::Jsonl {
        serde_json::to_writer(&mut stdout, value)
    } else {
        serde_json::to_writer_pretty(&mut stdout, value)
    };
    // `from` keeps the I/O error kind, so a closed pipe stays one.
    written.map_err(std::io::Error::from)?;
    writeln!(stdout)?;
    Ok(())
}

fn ids_not_supported() -> CliError {
    CliError::message(
        ErrorKind::InvalidInput,
        "--format ids only works with commands that list things".into(),
    )
}

// Columns padded to the widest cell; the last column isn't padded, so long
// titles don't leave trailing spaces. Cells hold Graph's text, which goes
// straight to a terminal, so control characters are replaced first.
fn write_table(
    out: &mut impl Write,
    headings: &[String],
    rows: &[Vec<String>],
) -> std::io::Result<()> {
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| display_safe(cell).into_owned())
                .collect()
        })
        .collect();
    let widths: Vec<usize> = (0..headings.len())
        .map(|column| {
            std::iter::once(&headings[column])
                .chain(rows.iter().filter_map(|row| row.get(column)))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    for line in std::iter::once(headings).chain(rows.iter().map(Vec::as_slice)) {
        let mut text = String::new();
        for (column, cell) in line.iter().enumerate() {
            if column + 1 == line.len() {
                text.push_str(cell);
            } else {
                let pad = widths[column].saturating_sub(cell.chars().count());
                text.push_str(cell);
                text.push_str(&" ".repeat(pad + 2));
            }
        }
        writeln!(out, "{}", text.trim_end())?;
    }
    Ok(())
}

#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    error: ErrorBody<'a>,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    kind: &'a str,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    graph_code: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<&'a str>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    candidates: &'a [ms_todo_protocol::Candidate],
    #[serde(skip_serializing_if = "Option::is_none")]
    op_id: Option<&'a str>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    applied: &'a [String],
}

/// Errors always go to stderr, so stdout only ever holds a result.
pub fn print_error(format: OutputFormat, error: &CliError) {
    let mut stderr = std::io::stderr().lock();
    let written = match format {
        OutputFormat::Json | OutputFormat::Jsonl => {
            let envelope = ErrorEnvelope {
                error: ErrorBody {
                    kind: error.kind.as_str(),
                    message: &error.message,
                    graph_code: error.graph_code.as_deref(),
                    request_id: error.request_id.as_deref(),
                    candidates: &error.candidates,
                    op_id: error.op_id.as_deref(),
                    applied: &error.applied,
                },
            };
            serde_json::to_writer(&mut stderr, &envelope)
                .map_err(std::io::Error::other)
                .and_then(|()| writeln!(stderr))
        }
        OutputFormat::Table | OutputFormat::Ids | OutputFormat::Csv => {
            let mut text = format!("error: {}", error.message);
            for candidate in &error.candidates {
                text.push_str(&format!("\n  {}  {}", candidate.name, candidate.id));
            }
            if !error.applied.is_empty() {
                text.push_str(&format!("\nalready changed: {}", error.applied.join(" ")));
            }
            if let Some(op_id) = &error.op_id {
                text.push_str(&format!("\nop_id: {op_id}"));
            }
            // Candidates' names and messages can quote Graph's text.
            writeln!(stderr, "{}", display_safe(&text))
        }
    };
    // Nowhere left to report a failure to write to stderr.
    let _ = written;
}

#[cfg(test)]
mod tests {
    use super::write_table;

    #[test]
    fn tables_align_columns_and_leave_no_trailing_space() {
        let mut out = Vec::new();
        let headings = vec!["NAME".to_owned(), "ID".to_owned()];
        let rows = vec![
            vec!["Tasks".to_owned(), "a".to_owned()],
            vec!["Groceries".to_owned(), "bb".to_owned()],
        ];
        write_table(&mut out, &headings, &rows).expect("write");
        assert_eq!(
            String::from_utf8(out).expect("utf8"),
            "NAME       ID\nTasks      a\nGroceries  bb\n"
        );
    }
}
