//! ms-todo's daemon (docs/blueprint/01-architecture.md#daemon-lifecycle).
//! It owns the sign-in and the cache: it's the only process that refreshes
//! the token, talks to Graph for data and touches the store (D-031). It
//! keeps the cache in step with Graph by delta sync (`sync`). Clients reach
//! it over a Unix socket in the instance's 0700 run directory, speaking
//! `ms-todo-protocol`.
//!
//! Started by `ms-todo daemon start` or automatically by any client, as a
//! detached `ms-todo daemon run --instance <name>`.

// Handlers fail with the wire type, `ErrorPayload`, which is over clippy's
// 128 bytes. It's the cold path, once per request, so boxing it everywhere
// would only add noise.
#![allow(clippy::result_large_err)]

mod completed;
mod doctor;
mod entities;
mod events;
mod folders;
mod freshness;
mod handlers;
mod idempotency;
mod list_resolution;
mod list_scope;
mod list_writes;
mod outbox;
mod reads;
mod seed;
mod server;
mod sync;
mod task_fields;
mod task_resolution;
mod task_writes;
mod undo;

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
        Err(server::Fatal::DatabaseTooNew(message)) => {
            eprintln!("ms-todo daemon: {message}");
            ExitCode::from(ms_todo_protocol::EXIT_DATABASE_TOO_NEW)
        }
        Err(server::Fatal::Other(message)) => {
            eprintln!("ms-todo daemon: {message}");
            ExitCode::FAILURE
        }
    }
}
