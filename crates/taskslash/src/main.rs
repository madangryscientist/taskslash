//! The `taskslash` command line: argv only, over `taskslash-core`.
//!
//! Exit codes: 0 ok, 1 a failure that is not usage, 2 a usage error. `taskslash run` exits with
//! its command's code instead. `taskslash history`, `taskslash status` and `taskslash events` exit 1
//! when no daemon is running, as `run` does; `events` exits 0 when the daemon hangs up. `taskslash
//! top` exits 1 when no daemon is running and 2 when stdout is not a terminal.
//! `taskslash moon` exits 1 when any target did not pass. `taskslash logs` exits 1 when the log it
//! names is not there. `taskslash <name>`, for a name that is not a
//! built-in, runs the plugin `taskslash-<name>` from `PATH` in place of taskslash, so the exit code
//! is the plugin's; exit 2 when there is none.

use clap::error::ErrorKind;
use clap::{ArgGroup, CommandFactory, Parser, Subcommand};
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitCode;
use taskslash_core::Error;
use taskslash_core::admission::waiting_line;
use taskslash_core::plugin;
use taskslash_core::protocol::{Class, Kind, LogSelect};
use text::{estimate_text, status_text, table};

mod text;
mod top;

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
    /// never-seen job runs, and its estimate fits beside what is committed. Waiting jobs are
    /// considered by class (raised one class per 15 minutes waited), then the submitting
    /// worktree's share of the machine, then time waited. A later job that fits may start
    /// before an earlier one, unless the earlier one has waited over 5 minutes. A waiting run prints
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
        /// The job's priority class, which orders the queue and sets its CPU weight: urgent,
        /// flake, interactive, normal or background. Default: `TASKSLASH_CLASS`, else normal.
        #[arg(long)]
        class: Option<Class>,
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
    /// Run moon targets, each as its own job, in dependency order.
    ///
    /// Replaces `moon run --concurrency N <targets>`: the daemon decides how many run at once.
    /// Each target and every target it depends on (requested or not) runs exactly once, after the
    /// ones it depends on have passed, as `moon run <target> --upstream none`, named
    /// `<repo>/<target>`. A target whose dependency failed is skipped. Ends with one line per
    /// target: pass, fail or skipped, its wall time, and `cached` when moon replayed it from cache.
    /// Exits 0 when every target passed, else 1; 2 when there is no moon workspace at or above the
    /// current directory or a target is wrong.
    Moon {
        /// `:<task>` (that task in every project that has it) or `<project>:<task>`.
        #[arg(required = true, value_name = "TARGET")]
        targets: Vec<String>,
        /// Pass `--force` to every `moon run`, so none is a cache hit.
        #[arg(long)]
        force: bool,
        /// The jobs' priority class, as for `run`. Default: `TASKSLASH_CLASS`, else normal.
        #[arg(long)]
        class: Option<Class>,
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
    /// Print a job's or a run's log: everything its command wrote, stdout and stderr together.
    ///
    /// `taskslash run` and `taskslash moon` copy each job's output into a log of its own beside the
    /// database (`TASKSLASH_LOGS` overrides the directory). The log goes to stdout unchanged.
    /// Exits 1 when there is no such job or run, when the run was recorded before logs were kept,
    /// or when its log has expired (the daemon deletes logs of jobs that ended over
    /// `TASKSLASH_LOG_DAYS` ago, 7 by default, and the oldest ones past `TASKSLASH_LOG_MIB`, 1024).
    #[command(group(ArgGroup::new("pick").required(true)))]
    Logs {
        /// A running job's number, as `status` shows it.
        #[arg(group = "pick")]
        job: Option<u64>,
        /// A finished run, by the id `history` shows.
        #[arg(long, value_name = "ID", group = "pick")]
        run: Option<i64>,
        /// The newest run of a key, running or not.
        #[arg(long, value_name = "KEY", group = "pick")]
        last: Option<String>,
        /// The newest run (of a key, when given) that exited with a code other than 0.
        #[arg(long, value_name = "KEY", num_args = 0..=1, group = "pick")]
        failed: Option<Option<String>>,
        /// Follow a running job's log until the job ends.
        #[arg(short, long)]
        follow: bool,
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
    /// Follow the daemon's job events: one line of JSON on stdout per event, as it happens.
    ///
    /// Each line is `{"type":"event","protocol":1,"seq":<n>,"event":{...}}`, `seq` counting up per
    /// daemon from 1, and `event.type` one of these (with its fields):
    ///
    /// - queued: job, key, kind, class, submitter, cwd, argv, at
    ///
    /// - started: job, unit, launch, log (the job's log file), at
    ///
    /// - finished: job, key, exit_code, wall_ms, cached, moon_hash, run (the history id), log, at
    ///
    /// - dropped: count
    ///
    /// Every queued job gets one finished: exit_code is null when its client went away, run when
    /// its row was not written, and a job that never started has exit_code, run and log null and
    /// wall_ms 0. Times are RFC 3339, UTC. Events carry no output: read the log. Only events from
    /// after the subscription are sent. A slow reader never slows a job: the daemon queues at most
    /// 1024 events for it, drops the rest, and sends one dropped (with how many, and the seq of the
    /// last one missed) before the next event it does take.
    ///
    /// A listener plugin runs this, or opens `TASKSLASH_SOCKET` and sends `{"type":"subscribe"}`
    /// itself (the reply is `{"type":"subscribed","protocol":1}`, then the event lines). Exits 0
    /// when the daemon hangs up, 1 when no daemon is running.
    Events,
    /// List the plugins on PATH: every executable named `taskslash-<name>`.
    ///
    /// `taskslash <name> [args...]` runs the first `taskslash-<name>` on PATH in place of taskslash,
    /// when `<name>` is not a built-in command. One line per name with the path that runs; later
    /// ones of the same name are listed as shadowed, and a name a built-in has is marked as never
    /// run.
    Plugins {
        /// Print one array of `{"name", "path", "shadowed", "builtin"}`.
        #[arg(long)]
        json: bool,
    },
    /// Show the machine, the jobs and the selected job's output on one screen, live.
    ///
    /// A strip of the machine's CPU, memory, pressure and load on top; the running jobs, the
    /// waiting ones in queue order and the jobs that finished since top opened (newest first) on
    /// the left; the selected job's log, its last lines, on the right, followed as it grows (a
    /// waiting job shows why it waits). Up/down or j/k select; q, Esc or Ctrl-C quit.
    ///
    /// Exits 1 when no daemon is running, 2 when stdout is not a terminal.
    Top,
    #[command(external_subcommand)]
    External(Vec<OsString>),
}

/// A finite number above 0.
fn positive(text: &str) -> Result<f64, String> {
    text.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
        .ok_or_else(|| format!("{text} is not a number above 0"))
}

/// `--class`, else `TASKSLASH_CLASS` when set and not empty, else normal. A `TASKSLASH_CLASS` that
/// is not a class is a usage error, as a bad `--class` is.
fn class(flag: Option<Class>) -> Class {
    if let Some(class) = flag {
        return class;
    }
    let Some(text) = std::env::var_os("TASKSLASH_CLASS").filter(|v| !v.is_empty()) else {
        return Class::Normal;
    };
    text.to_string_lossy().parse().unwrap_or_else(|e: String| {
        Cli::command()
            .error(ErrorKind::InvalidValue, format!("TASKSLASH_CLASS: {e}"))
            .exit()
    })
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match dispatch(cli.command) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(e) => {
            eprintln!("taskslash: {e}");
            ExitCode::from(if e.is_usage() { 2 } else { 1 })
        }
    }
}

/// The names of the built-in commands, `help` included.
fn builtins() -> Vec<String> {
    let mut command = Cli::command();
    command.build();
    command
        .get_subcommands()
        .map(|c| c.get_name().to_owned())
        .collect()
}

/// Runs the plugin for `taskslash <name> [args...]`; returns only when it could not be started.
fn plugin(argv: Vec<OsString>) -> Error {
    let mut argv = argv.into_iter();
    let name = argv.next().unwrap_or_default();
    let args: Vec<OsString> = argv.collect();
    let path = std::env::var_os("PATH").unwrap_or_default();
    if let Some(program) = plugin::find(&name, &path) {
        return plugin::exec(&program, &args);
    }
    let name = name.to_string_lossy();
    let builtins = builtins();
    let tip = plugin::closest(&name, &builtins)
        .map(|c| format!("\n\n  tip: a similar subcommand exists: '{c}'"))
        .unwrap_or_default();
    let message = format!(
        "unrecognized subcommand '{name}'{tip}\n\nNo program named {}{name} is on PATH.",
        plugin::PREFIX
    );
    Cli::command()
        .error(ErrorKind::InvalidSubcommand, message)
        .exit()
}

fn dispatch(command: Commands) -> Result<i32, Error> {
    let socket = taskslash_core::socket_path;
    match command {
        Commands::External(argv) => Err(plugin(argv)),
        Commands::Plugins { json } => {
            let path = std::env::var_os("PATH").unwrap_or_default();
            let plugins = plugin::list(&path, &builtins());
            let text = if json {
                plugin::list_json(&plugins)
            } else {
                plugin::list_text(&plugins)
            };
            write!(io::stdout(), "{text}")
                .map_err(|e| Error::new(format!("cannot write the plugins: {e}")))?;
            Ok(0)
        }
        Commands::Top => top::run(&socket()?),
        Commands::Events => {
            taskslash_core::events(&socket()?, &mut io::stdout().lock()).map(|()| 0)
        }
        Commands::Daemon => {
            taskslash_core::serve(&socket()?, &taskslash_core::db_path()?).map(|()| 0)
        }
        Commands::Run {
            name,
            kind,
            class: flag,
            cpu,
            mem,
            command,
        } => {
            let demand = taskslash_core::Demand { cpu, mem_mib: mem };
            taskslash_core::run(&socket()?, name, kind, class(flag), demand, &command)
        }
        Commands::Moon {
            targets,
            force,
            class: flag,
        } => {
            let cwd = std::env::current_dir()
                .map_err(|e| Error::new(format!("cannot read the current directory: {e}")))?;
            taskslash_core::moon(&socket()?, &cwd, &targets, force, class(flag))
        }
        Commands::Logs {
            job,
            run,
            last,
            failed,
            follow,
        } => {
            let select = match (job, run, last, failed) {
                (Some(job), ..) => LogSelect::Job { job },
                (_, Some(run), ..) => LogSelect::Run { run },
                (_, _, Some(key), _) => LogSelect::Last { key },
                (_, _, _, Some(key)) => LogSelect::Failed { key },
                (None, None, None, None) => return Err(Error::usage("name a job or a run")),
            };
            taskslash_core::logs(&socket()?, &select, follow, &mut io::stdout().lock()).map(|()| 0)
        }
        Commands::Why { job, json } => {
            let text = match (taskslash_core::why(&socket()?, job)?, json) {
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
            let (estimate, runs) = taskslash_core::history(&socket()?, &key)?;
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
            let status = taskslash_core::status(&socket()?)?;
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
