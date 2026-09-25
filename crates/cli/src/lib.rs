//! The `ms-todo` command line. Every feature has a CLI subcommand (D-009).
//! Commands that need Graph ask the daemon (D-031); only `auth login`,
//! `status` and `logout` sign in or read the token themselves.

// Commands fail with `CliError`, which carries what the error JSON needs
// and is over clippy's 128 bytes. It's the cold path, once per command, so
// boxing it everywhere would only add noise.
#![allow(clippy::result_large_err)]

mod args;
mod auth_commands;
mod bulk_commands;
mod confirm;
mod csv_columns;
mod daemon_client;
mod daemon_commands;
mod data_commands;
mod doctor_commands;
mod done_command;
mod error;
mod folder_commands;
mod link_commands;
mod outbox_commands;
mod output;
mod output_schemas;
mod phrases;
mod schema_commands;
mod sync_commands;
mod task_commands;
mod task_output;
mod time;
mod tui_command;

use std::io::IsTerminal;
use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches};
use ms_todo_core::{Instance, Paths};
use ms_todo_graph::auth::{Authenticator, Endpoints};
use ms_todo_protocol::TaskChange;

pub use args::Cli;
use args::{
    AuthCommand, Command, DaemonCommand, FoldersCommand, GlobalArgs, ListsCommand, OutboxCommand,
    TasksCommand, TuiArgs,
};
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
    // The TUI's cold start is measured from here.
    let started = std::time::Instant::now();
    // Parsed by hand rather than `Cli::parse()` to keep clap's `Command`:
    // it has the name as typed (`mst`, D-039) for the help below.
    let mut definition = Cli::command();
    let cli = match Cli::from_arg_matches(&definition.get_matches_mut()) {
        Ok(cli) => cli,
        Err(error) => error.exit(),
    };
    let format = OutputFormat::resolve(cli.global.format);
    let command = match cli.command {
        Some(command) => command,
        None if bare_opens_tui(
            std::io::stdin().is_terminal(),
            std::io::stdout().is_terminal(),
        ) =>
        {
            Command::Tui(TuiArgs::default())
        }
        None => {
            // As clap does for a missing subcommand.
            eprint!("{}", definition.render_help());
            return ExitCode::from(2);
        }
    };
    match execute(&cli.global, command, format, daemon, started) {
        Ok(code) => code,
        Err(error) if error.stdout_closed => ExitCode::SUCCESS,
        Err(error) => {
            print_error(format, &error);
            ExitCode::from(error.kind.exit_code())
        }
    }
}

/// Whether `ms-todo` with no command opens the TUI: only when a person is
/// at the terminal on both ends. A script, a pipe or an agent gets help and
/// exit 2 instead, as before the TUI existed.
fn bare_opens_tui(stdin_is_terminal: bool, stdout_is_terminal: bool) -> bool {
    stdin_is_terminal && stdout_is_terminal
}

fn execute(
    global: &GlobalArgs,
    command: Command,
    format: OutputFormat,
    daemon: DaemonEntry,
    started: std::time::Instant,
) -> Result<ExitCode, CliError> {
    let instance = Instance::detect(global.instance.as_deref())?;
    let paths = Paths::resolve(instance)?;
    if let Command::Daemon(DaemonCommand::Run) = command {
        return Ok(daemon(paths));
    }
    if let Command::Daemon(DaemonCommand::Launch) = command {
        return Ok(daemon_client::launch(&paths));
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    if let Command::Tui(args) = command {
        runtime.block_on(tui_command::tui(&paths, args, started))?;
        return Ok(ExitCode::SUCCESS);
    }
    runtime.block_on(dispatch(command, &paths, format))?;
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
        Command::Lists(ListsCommand::Move(args)) => {
            folder_commands::move_lists(paths, args, format).await
        }
        Command::Lists(ListsCommand::Order(args)) => {
            folder_commands::order_list(paths, args, format).await
        }
        Command::Folders(FoldersCommand::List) => {
            let (items, sync) = folder_commands::list(paths).await?;
            print_collection(format, &items, sync, &folder_commands::FOLDERS_TABLE)
        }
        Command::Folders(FoldersCommand::Rename(args)) => {
            folder_commands::rename(paths, args, format).await
        }
        Command::Folders(FoldersCommand::Delete(args)) => {
            folder_commands::delete(paths, args, format).await
        }
        Command::Folders(FoldersCommand::Order(args)) => {
            folder_commands::order_folder(paths, args, format).await
        }
        Command::Tasks(TasksCommand::List { list, search }) => {
            let (items, sync) = data_commands::tasks(paths, list, search).await?;
            print_collection(format, &items, sync, &data_commands::TASKS_TABLE)
        }
        Command::Search(args) => {
            let (items, sync) = data_commands::search(paths, args).await?;
            print_collection(format, &items, sync, &data_commands::SEARCH_TABLE)
        }
        Command::Done(args) => done_command::done(paths, args, format).await,
        Command::Reschedule(args) => task_commands::reschedule(paths, args, format).await,
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
        Command::Tasks(TasksCommand::Links(args)) => {
            link_commands::links(paths, args, format).await
        }
        Command::Tasks(TasksCommand::Open { task, index }) => {
            let opener = ms_todo_tui::open::SystemOpener;
            link_commands::open(paths, task, index, format, &opener).await
        }
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
        Command::Daemon(DaemonCommand::Run | DaemonCommand::Launch) => {
            unreachable!("`daemon run|launch` return from `execute` before the runtime starts")
        }
        Command::Tui(_) => unreachable!("`tui` returns from `execute` before `dispatch`"),
    }
}

fn authenticator(paths: &Paths) -> Result<Authenticator, CliError> {
    Ok(Authenticator::new(paths.auth_dir(), Endpoints::default())?)
}

/// The daemon answered a request with the wrong kind of data.
fn unexpected_response() -> CliError {
    daemon_client::mismatch("the daemon sent an unexpected answer")
}

#[cfg(test)]
mod tests {
    use super::bare_opens_tui;

    #[test]
    fn a_bare_command_opens_the_tui_only_with_a_terminal_on_both_ends() {
        assert!(bare_opens_tui(true, true));
        assert!(!bare_opens_tui(true, false), "piped: `mst | less`");
        assert!(!bare_opens_tui(false, true), "fed: `mst < file`");
        assert!(!bare_opens_tui(false, false), "an agent or a script");
    }
}
