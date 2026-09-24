//! The `ms-todo` command line. Every feature has a CLI subcommand (D-009).
//! Commands that need Graph ask the daemon (D-031); only `auth login`,
//! `status` and `logout` sign in or read the token themselves.

mod args;
mod auth_commands;
mod daemon_client;
mod daemon_commands;
mod data_commands;
mod error;
mod output;
mod time;

use std::process::ExitCode;

use clap::Parser;
use ms_todo_core::{Instance, Paths};
use ms_todo_graph::auth::{Authenticator, Endpoints};

pub use args::Cli;
use args::{AuthCommand, Command, DaemonCommand, ListsCommand, RawArgs, RawMethod, TasksCommand};
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
            let items = data_commands::lists(paths).await?;
            print_collection(format, &items, &data_commands::LISTS_TABLE)
        }
        Command::Tasks(TasksCommand::List { list }) => {
            let items = data_commands::tasks(paths, list).await?;
            print_collection(format, &items, &data_commands::TASKS_TABLE)
        }
        Command::Raw(RawArgs {
            method: RawMethod::Get,
            path,
        }) => print_raw(format, &data_commands::raw_get(paths, path).await?),
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
