//! The `taskslash` command line: argv only, over `taskslash-core`.
//!
//! Exit codes: 0 ok, 1 a failure that is not usage, 2 a usage error. `taskslash run` exits with
//! its command's code instead.

use clap::{Parser, Subcommand};
use std::ffi::OsString;
use std::process::ExitCode;
use taskslash_core::Error;

/// Admission control for builds and tests on this machine.
#[derive(Parser)]
#[command(name = "taskslash", version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the daemon that admits and records jobs. Runs until killed.
    Daemon,
    /// Run a command as a job of the daemon.
    ///
    /// Exits with the command's exit code (128+N when signal N killed it, 127 when it cannot be
    /// started). Exits 1 when no daemon is running, which a command's own exit 1 can also mean.
    Run {
        /// A name for the job.
        #[arg(long)]
        name: Option<String>,
        /// The command and its arguments.
        #[arg(required = true, trailing_var_arg = true, value_name = "COMMAND")]
        command: Vec<OsString>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match dispatch(cli.command) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(e) => {
            eprintln!("taskslash: {e}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch(command: Commands) -> Result<i32, Error> {
    let socket = taskslash_core::socket_path()?;
    match command {
        Commands::Daemon => taskslash_core::serve(&socket).map(|()| 0),
        Commands::Run { name, command } => taskslash_core::run(&socket, name, &command),
    }
}
