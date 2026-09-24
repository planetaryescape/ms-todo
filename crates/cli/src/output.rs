// Adapted from spotuify crates/spotuify-protocol/src/output.rs @ d807e5e4f9d2f09878cdc22309af3589623f7785
// (the `OutputFormat` enum). F1 ships `table` and `json`; `jsonl` and `ids`
// arrive with collections in rung 1.

use std::io::{IsTerminal, Write};

use serde::Serialize;

use crate::error::CliError;

#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub enum OutputFormat {
    Table,
    Json,
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

/// Something a command prints on success.
pub trait Render: Serialize {
    fn table_rows(&self) -> Vec<(&'static str, String)>;
}

pub fn print_success(format: OutputFormat, value: &impl Render) -> std::io::Result<()> {
    let mut stdout = std::io::stdout().lock();
    match format {
        OutputFormat::Json => {
            serde_json::to_writer_pretty(&mut stdout, value).map_err(std::io::Error::other)?;
            writeln!(stdout)
        }
        OutputFormat::Table => {
            let rows = value.table_rows();
            let width = rows.iter().map(|(label, _)| label.len()).max().unwrap_or(0);
            for (label, text) in rows {
                writeln!(stdout, "{label:<width$}  {text}")?;
            }
            Ok(())
        }
    }
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
}

/// Errors always go to stderr, so stdout only ever holds a result.
pub fn print_error(format: OutputFormat, error: &CliError) {
    let mut stderr = std::io::stderr().lock();
    let written = match format {
        OutputFormat::Json => {
            let envelope = ErrorEnvelope {
                error: ErrorBody {
                    kind: error.kind.as_str(),
                    message: &error.message,
                    graph_code: error.graph_code.as_deref(),
                    request_id: error.request_id.as_deref(),
                },
            };
            serde_json::to_writer(&mut stderr, &envelope)
                .map_err(std::io::Error::other)
                .and_then(|()| writeln!(stderr))
        }
        OutputFormat::Table => writeln!(stderr, "error: {}", error.message),
    };
    // Nowhere left to report a failure to write to stderr.
    let _ = written;
}
