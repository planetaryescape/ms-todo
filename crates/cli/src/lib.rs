//! The `ms-todo` command line. Every feature has a CLI subcommand (D-009).
//! Commands that need Graph ask the daemon (D-031); only `auth login`,
//! `status` and `logout` sign in or read the token themselves.

// Commands fail with `CliError`, which carries what the error JSON needs
// and is over clippy's 128 bytes. It's the cold path, once per command, so
// boxing it everywhere would only add noise.
#![allow(clippy::result_large_err)]

mod args;
mod auth_commands;
mod confirm;
mod csv_columns;
mod daemon_client;
mod daemon_commands;
mod data_commands;
mod doctor_commands;
mod error;
mod outbox_commands;
mod output;
mod output_schemas;
mod schema_commands;
mod sync_commands;
mod task_commands;
mod task_output;
mod time;

use std::process::ExitCode;

use clap::Parser;
use ms_todo_core::{Instance, Paths};
use ms_todo_graph::auth::{Authenticator, Endpoints};
use ms_todo_protocol::TaskChange;

pub use args::Cli;
use args::{AuthCommand, Command, DaemonCommand, ListsCommand, OutboxCommand, TasksCommand};
use error::CliError;
use output::{OutputFormat, print_collection, print_error, print_raw, print_success};

/// Runs the daemon in this process; the root binary passes
/// `ms_todo_daemon::run`, so this crate never depends on the daemon or its
/// Graph client.
pub type DaemonEntry = fn(Paths) -> ExitCode;

/// Parse arguments, run the command, and return the exit code from
/// docs/blueprint/07-cli.md. clap handles `--help`, `--version` and usage
/// errors itself (exit 2).
pub fn run(daemon: DaemonEntry) -> ExitCode {
    let cli = Cli::parse();
    let format = OutputFormat::resolve(cli.global.format);
    match execute(cli, format, daemon) {
        Ok(code) => code,
        Err(error) if error.stdout_closed => ExitCode::SUCCESS,
        Err(error) => {
            print_error(format, &error);
            ExitCode::from(error.kind.exit_code())
        }
    }
}

fn execute(cli: Cli, format: OutputFormat, daemon: DaemonEntry) -> Result<ExitCode, CliError> {
    let instance = Instance::detect(cli.global.instance.as_deref())?;
    let paths = Paths::resolve(instance)?;
    if let Command::Daemon(DaemonCommand::Run) = cli.command {
        return Ok(daemon(paths));
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(dispatch(cli.command, &paths, format))?;
    Ok(ExitCode::SUCCESS)
}

async fn dispatch(command: Command, paths: &Paths, format: OutputFormat) -> Result<(), CliError> {
    match command {
        Command::Auth(AuthCommand::Login) => print_success(
            format,
            &auth_commands::login(paths, &authenticator(paths)?).await?,
        ),
        Command::Auth(AuthCommand::Status) => print_success(
            format,
            &auth_commands::status(paths, &authenticator(paths)?).await?,
        ),
        Command::Auth(AuthCommand::Logout) => print_success(
            format,
            &auth_commands::logout(&authenticator(paths)?).await?,
        ),
        Command::Auth(AuthCommand::Bearer { reveal_secret }) => {
            let bearer = data_commands::bearer(paths, reveal_secret).await?;
            if format == OutputFormat::Table {
                // Just the token, for `$(ms-todo auth bearer --reveal-secret --format table)`.
                println!("{}", bearer.access_token);
                Ok(())
            } else {
                print_success(format, &bearer)
            }
        }
        Command::Lists(ListsCommand::List) => {
            let (items, sync) = data_commands::lists(paths).await?;
            print_collection(format, &items, sync, &data_commands::LISTS_TABLE)
        }
        Command::Tasks(TasksCommand::List { list }) => {
            let (items, sync) = data_commands::tasks(paths, list).await?;
            print_collection(format, &items, sync, &data_commands::TASKS_TABLE)
        }
        Command::Sync { wait } => print_success(format, &sync_commands::sync(paths, wait).await?),
        Command::Doctor => print_success(format, &doctor_commands::doctor(paths).await?),
        Command::Schema { command } => print_raw(format, &schema_commands::schema(&command)?),
        Command::Tasks(TasksCommand::Add(args)) => task_commands::add(paths, args, format).await,
        Command::Tasks(TasksCommand::Complete(args)) => {
            task_commands::change(paths, args, TaskChange::Complete, format).await
        }
        Command::Tasks(TasksCommand::Reopen(args)) => {
            task_commands::change(paths, args, TaskChange::Reopen, format).await
        }
        Command::Tasks(TasksCommand::Delete { targets, yes }) => {
            task_commands::delete(paths, targets, yes, format).await
        }
        Command::Tasks(TasksCommand::Edit(args)) => task_commands::edit(paths, args, format).await,
        Command::Raw(args) => print_raw(format, &task_commands::raw(paths, args).await?),
        Command::Outbox(OutboxCommand::List { state }) => {
            outbox_commands::list(paths, state, format).await
        }
        Command::Outbox(OutboxCommand::Retry { op }) => {
            outbox_commands::retry(paths, op, format).await
        }
        Command::Outbox(OutboxCommand::Discard { op, yes }) => {
            outbox_commands::discard(paths, op, yes, format).await
        }
        Command::Undo(args) => task_commands::undo(paths, args, format).await,
        Command::Daemon(DaemonCommand::Start) => {
            print_success(format, &daemon_commands::start(paths).await?)
        }
        Command::Daemon(DaemonCommand::Stop) => {
            print_success(format, &daemon_commands::stop(paths).await?)
        }
        Command::Daemon(DaemonCommand::Status) => {
            print_success(format, &daemon_commands::status(paths).await)
        }
        Command::Daemon(DaemonCommand::Run) => {
            unreachable!("`daemon run` returns from `execute` before the runtime starts")
        }
    }
}

fn authenticator(paths: &Paths) -> Result<Authenticator, CliError> {
    Ok(Authenticator::new(paths.auth_dir(), Endpoints::default())?)
}

/// The daemon answered a request with the wrong kind of data.
fn unexpected_response() -> CliError {
    daemon_client::mismatch("the daemon sent an unexpected answer")
}
