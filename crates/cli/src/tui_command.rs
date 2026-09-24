//! `ms-todo tui`: make sure the daemon runs and speaks this version, then
//! hand the terminal to the TUI, which talks to it over the same socket.

use std::time::Instant;

use ms_todo_core::{ErrorKind, Paths};
use ms_todo_tui::{Options, TuiError};

use crate::args::TuiArgs;
use crate::daemon_client;
use crate::error::CliError;

/// Where `MS_TODO_TUI_TRACE` names a file, the TUI writes its tracing
/// spans there: the latency measurements, among others.
const TRACE_ENV: &str = "MS_TODO_TUI_TRACE";

pub async fn tui(paths: &Paths, args: TuiArgs, started: Instant) -> Result<(), CliError> {
    // Started now, or restarted if it's another version; the TUI makes
    // its own connection.
    drop(daemon_client::connect(paths).await?);
    let options = Options {
        socket: paths.socket_path(),
        ascii: args.ascii,
        bench_startup: args.bench_startup,
        started,
        trace: std::env::var_os(TRACE_ENV).map(Into::into),
    };
    match ms_todo_tui::run(options).await {
        Ok(Some(report)) => {
            print!("{report}");
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(error @ TuiError::NotATerminal) => Err(CliError::new(ErrorKind::InvalidInput, &error)),
        Err(error) => Err(CliError::new(ErrorKind::Internal, &error)),
    }
}
