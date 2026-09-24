// Adapted from spotuify crates/spotuify-protocol/src/output.rs @ d807e5e4f9d2f09878cdc22309af3589623f7785
// (the `OutputFormat` enum).

//! What commands print (docs/blueprint/07-cli.md#output-contract). Results
//! go to stdout and errors to stderr, never both. JSON carries
//! `schema_version`; `raw` is exempt and prints Graph's body as it came.

use std::io::{IsTerminal, Write};

use ms_todo_core::ErrorKind;
use ms_todo_protocol::Entity;
use serde::Serialize;
use serde_json::Value;

use crate::error::CliError;

/// Bumped when an output shape changes incompatibly. It goes to 2 in rung
/// 3a, when `id` becomes the local ID (D-034).
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub enum OutputFormat {
    Table,
    Json,
    // One compact JSON object per line.
    Jsonl,
    // One ID per line, for piping into another command.
    Ids,
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

#[derive(Serialize)]
struct Versioned<'a, T: Serialize> {
    schema_version: u32,
    #[serde(flatten)]
    inner: &'a T,
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
                writeln!(stdout, "{label:<width$}  {text}")?;
            }
            Ok(())
        }
        OutputFormat::Ids => Err(ids_not_supported()),
    }
}

/// A collection's table: column headings, and a row of cells per item.
pub struct Table {
    pub headings: &'static [&'static str],
    pub row: fn(&Entity) -> Vec<String>,
}

#[derive(Serialize)]
struct CollectionEnvelope<'a> {
    schema_version: u32,
    items: &'a [Entity],
}

pub fn print_collection(
    format: OutputFormat,
    items: &[Entity],
    table: &Table,
) -> Result<(), CliError> {
    let mut stdout = std::io::stdout().lock();
    match format {
        OutputFormat::Json => print_json(
            format,
            &CollectionEnvelope {
                schema_version: SCHEMA_VERSION,
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
            for item in items {
                if let Some(id) = item.get("id").and_then(Value::as_str) {
                    writeln!(stdout, "{id}")?;
                }
            }
            Ok(())
        }
        OutputFormat::Table => {
            let rows: Vec<Vec<String>> = items.iter().map(table.row).collect();
            let headings: Vec<String> = table.headings.iter().map(|&h| h.to_owned()).collect();
            write_table(&mut stdout, &headings, &rows)?;
            Ok(())
        }
    }
}

/// `raw` prints Graph's body unchanged; a table is pretty JSON too.
pub fn print_raw(format: OutputFormat, body: &Value) -> Result<(), CliError> {
    match format {
        OutputFormat::Ids => Err(ids_not_supported()),
        OutputFormat::Jsonl => print_json(format, body),
        OutputFormat::Json | OutputFormat::Table => print_json(OutputFormat::Json, body),
    }
}

fn print_json(format: OutputFormat, value: &impl Serialize) -> Result<(), CliError> {
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
// titles don't leave trailing spaces.
fn write_table(
    out: &mut impl Write,
    headings: &[String],
    rows: &[Vec<String>],
) -> std::io::Result<()> {
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
    candidates: &'a [ms_todo_protocol::ListRef],
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
                },
            };
            serde_json::to_writer(&mut stderr, &envelope)
                .map_err(std::io::Error::other)
                .and_then(|()| writeln!(stderr))
        }
        OutputFormat::Table | OutputFormat::Ids => {
            let mut text = format!("error: {}", error.message);
            for candidate in &error.candidates {
                text.push_str(&format!("\n  {}  {}", candidate.name, candidate.id));
            }
            writeln!(stderr, "{text}")
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
