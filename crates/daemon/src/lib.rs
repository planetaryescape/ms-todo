//! ms-todo's daemon (docs/blueprint/01-architecture.md#daemon-lifecycle).
//! It owns the sign-in: it's the only process that refreshes the token and
//! talks to Graph for data (D-031). Clients reach it over a Unix socket in
//! the instance's 0700 run directory, speaking `ms-todo-protocol`.
//!
//! Started by `ms-todo daemon start` or automatically by any client, as a
//! detached `ms-todo daemon run --instance <name>`.

mod handlers;
mod list_resolution;
mod server;

use std::process::ExitCode;

use ms_todo_core::Paths;

/// Run the daemon in the foreground until `Shutdown`, SIGTERM or SIGINT.
/// Diagnostics go to stderr, which an auto-started daemon writes to
/// `<data_dir>/logs/daemon.log`.
pub fn run(paths: Paths) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("ms-todo daemon: cannot start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(server::serve(paths)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("ms-todo daemon: {message}");
            ExitCode::FAILURE
        }
    }
}
