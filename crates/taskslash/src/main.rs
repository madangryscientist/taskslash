//! The `taskslash` command line: argv only, over `taskslash-core`.
//!
//! Exit codes: 0 ok, 1 a failure that is not usage, 2 a usage error. `taskslash run` exits with
//! its command's code instead. `taskslash history` exits 1 when no daemon is running, as `run` does.

use clap::{Parser, Subcommand};
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;
use taskslash_core::Error;
use taskslash_core::protocol::Run;

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
    /// Show the recorded runs of a key, newest first (at most 20).
    ///
    /// A run's key is its `--name`, else the first two words of its command. A cost the daemon
    /// could not measure (no systemd user session) shows as `-`, and as null with `--json`.
    History {
        /// The key: a `--name`, or the first two words of a command (`"bunx moon"`).
        key: String,
        /// Print the runs as a JSON array.
        #[arg(long)]
        json: bool,
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
        Commands::Daemon => taskslash_core::serve(&socket, &taskslash_core::db_path()?).map(|()| 0),
        Commands::Run { name, command } => taskslash_core::run(&socket, name, &command),
        Commands::History { key, json } => {
            let runs = taskslash_core::history(&socket, &key)?;
            let text = if json {
                taskslash_core::protocol::runs_json(&runs)
            } else {
                table(&runs)
            };
            write!(io::stdout(), "{text}")
                .map_err(|e| Error::new(format!("cannot write the runs: {e}")))?;
            Ok(0)
        }
    }
}

/// One header line and one line per run, `-` for a cost that was not measured.
fn table(runs: &[Run]) -> String {
    const MIB: f64 = (1u64 << 20) as f64;
    let mut out = format!(
        "{:<24}  {:>8}  {:>8}  {:>11}  {:>12}  {:>11}  {:>4}\n",
        "started_at", "wall", "cpu_s", "peak_cpu_5s", "peak_mem_mib", "written_mib", "exit"
    );
    for run in runs {
        let or_dash = |v: Option<String>| v.unwrap_or_else(|| "-".to_owned());
        let mib = |v: Option<i64>| or_dash(v.map(|b| format!("{:.1}", b as f64 / MIB)));
        out.push_str(&format!(
            "{:<24}  {:>8}  {:>8}  {:>11}  {:>12}  {:>11}  {:>4}\n",
            run.started_at,
            format!("{:.1}s", run.wall_ms as f64 / 1000.0),
            or_dash(run.cpu_usec.map(|u| format!("{:.2}", u as f64 / 1e6))),
            or_dash(run.peak_cpu_5s.map(|p| format!("{p:.2}"))),
            mib(run.peak_mem_bytes),
            mib(run.written_bytes),
            or_dash(run.exit_code.map(|c| c.to_string())),
        ));
    }
    out
}
