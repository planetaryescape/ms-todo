//! The `ms-todo` command line. Every feature has a CLI subcommand (D-009).

mod args;
mod auth_commands;
mod error;
mod output;

use std::process::ExitCode;

use clap::Parser;
use ms_todo_core::{Instance, Paths};
use ms_todo_graph::auth::{Authenticator, Endpoints};

pub use args::Cli;
use args::{AuthCommand, Command};
use error::CliError;
use output::{OutputFormat, print_error, print_success};

/// Parse arguments, run the command, and return the exit code from
/// docs/blueprint/07-cli.md. clap handles `--help`, `--version` and usage
/// errors itself (exit 2).
pub fn run() -> ExitCode {
    let cli = Cli::parse();
    let format = OutputFormat::resolve(cli.global.format);
    match execute(cli, format) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            print_error(format, &error);
            ExitCode::from(error.kind.exit_code())
        }
    }
}

fn execute(cli: Cli, format: OutputFormat) -> Result<(), CliError> {
    let instance = Instance::detect(cli.global.instance.as_deref())?;
    let paths = Paths::resolve(instance)?;
    let auth = Authenticator::new(paths.auth_dir(), Endpoints::default())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async {
        match cli.command {
            Command::Auth(AuthCommand::Login) => {
                let status = auth_commands::login(&paths, &auth).await?;
                print_success(format, &status)?;
            }
            Command::Auth(AuthCommand::Status) => {
                let status = auth_commands::status(&paths, &auth).await?;
                print_success(format, &status)?;
            }
            Command::Auth(AuthCommand::Logout) => {
                let logout = auth_commands::logout(&auth).await?;
                print_success(format, &logout)?;
            }
        }
        Ok(())
    })
}
