//! The `taskslash` command line: argv only, over `taskslash-core`.
//!
//! Exit codes: 0 ok, 1 a failure that is not usage, 2 a usage error. `taskslash run` exits with
//! its command's code instead. `taskslash history` and `taskslash status` exit 1 when no daemon is
//! running, as `run` does.

use clap::{Parser, Subcommand};
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;
use taskslash_core::Error;
use taskslash_core::admission::waiting_line;
use taskslash_core::protocol::Kind;
use text::{estimate_text, status_text, table};

mod text;

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
    /// Waits while the daemon will not start it yet: until no pressure gate holds, no other
    /// never-seen job runs, and its estimate fits beside what is committed. A waiting run prints
    /// `taskslash: waiting <n>s (<place> in line): <reason>` on stderr when queued and every 15 s.
    ///
    /// Exits with the command's exit code (128+N when signal N killed it, 127 when it cannot be
    /// started). Exits 1 when no daemon is running, which a command's own exit 1 can also mean.
    Run {
        /// A name for the job.
        #[arg(long)]
        name: Option<String>,
        /// The job's cost class, whose defaults it gets until it has history: test, typecheck,
        /// build, browser or command.
        #[arg(long, default_value_t = Kind::Command)]
        kind: Kind,
        /// The CPU it takes, in threads, in place of the learned estimate.
        #[arg(long, value_parser = positive)]
        cpu: Option<f64>,
        /// The memory it takes, in MiB, in place of the learned estimate. With `--cpu`, the job is
        /// never held as never seen.
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        mem: Option<u64>,
        /// The command and its arguments.
        #[arg(required = true, trailing_var_arg = true, value_name = "COMMAND")]
        command: Vec<OsString>,
    },
    /// Show the cost estimate of a key and its recorded runs, newest first (at most 20).
    ///
    /// Each figure of the estimate comes from the key's history, or from its kind's default when
    /// no run feeds it (the kind is the newest run's). A run's key is its `--name`, else the first two words of its command. A cost the daemon
    /// could not measure (no systemd user session) shows as `-`, and as null with `--json`.
    History {
        /// The key: a `--name`, or the first two words of a command (`"bunx moon"`).
        key: String,
        /// Print `{"estimate": ..., "runs": [...]}`.
        #[arg(long)]
        json: bool,
    },
    /// Show what the machine and the running jobs are doing, from the daemon's latest readings.
    ///
    /// The machine is sampled once a second (free space on the watched mounts every 30 s). A job
    /// with no live reading yet, or one run unmeasured, shows `-`, and null with `--json`.
    Status {
        /// Print the answer as one line of JSON.
        #[arg(long)]
        json: bool,
    },
    /// Say why a job waits, or `running`. Exits 1 for a job the daemon does not hold (an ended
    /// job's record is in `history`).
    Why {
        /// The job's number, as `status` shows it.
        job: u64,
        /// Print the reason object (`{"kind":"running"}` for a running job).
        #[arg(long)]
        json: bool,
    },
}

/// A finite number above 0.
fn positive(text: &str) -> Result<f64, String> {
    text.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or_else(|| format!("{text} is not a number above 0"))
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
        Commands::Run {
            name,
            kind,
            cpu,
            mem,
            command,
        } => {
            let demand = taskslash_core::Demand { cpu, mem_mib: mem };
            taskslash_core::run(&socket, name, kind, demand, &command)
        }
        Commands::Why { job, json } => {
            let text = match (taskslash_core::why(&socket, job)?, json) {
                (Some((reason, _, _)), true) => taskslash_core::protocol::reason_json(&reason),
                (Some((reason, waited_s, position)), false) => {
                    let line = waiting_line(&reason, waited_s, position);
                    format!("{}\n", line.trim_start_matches("taskslash: "))
                }
                (None, true) => "{\"kind\":\"running\"}\n".to_owned(),
                (None, false) => "running\n".to_owned(),
            };
            write!(io::stdout(), "{text}")
                .map_err(|e| Error::new(format!("cannot write the reason: {e}")))?;
            Ok(0)
        }
        Commands::History { key, json } => {
            let (estimate, runs) = taskslash_core::history(&socket, &key)?;
            let text = if json {
                taskslash_core::protocol::history_json(&estimate, &runs)
            } else {
                format!("{}{}", estimate_text(&estimate), table(&runs))
            };
            write!(io::stdout(), "{text}")
                .map_err(|e| Error::new(format!("cannot write the runs: {e}")))?;
            Ok(0)
        }
        Commands::Status { json } => {
            let status = taskslash_core::status(&socket)?;
            let text = if json {
                taskslash_core::protocol::status_json(status)
            } else {
                status_text(&status)
            };
            write!(io::stdout(), "{text}")
                .map_err(|e| Error::new(format!("cannot write the status: {e}")))?;
            Ok(0)
        }
    }
}
