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
use taskslash_core::estimate::{Estimate, Source};
use taskslash_core::protocol::{Kind, Run, Status};

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
    /// Waits while the daemon will not start it yet: at most one job whose key has never finished
    /// a run runs at a time, and a second one says `taskslash: waiting: <reason>` on stderr.
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
            command,
        } => taskslash_core::run(&socket, name, kind, &command),
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

/// A few lines: CPU, the three `some avg10` pressures, memory, each disk, each mount, each job.
fn status_text(status: &Status) -> String {
    let m = &status.machine;
    let pct = |share: f64| format!("{:.0}%", share * 100.0);
    let mut out = format!(
        "cpu       {} busy on {} threads ({} online), load {:.2} {:.2} {:.2}\n",
        pct(m.cpu.busy),
        m.cpu.threads,
        m.cpu.threads_online,
        m.load[0],
        m.load[1],
        m.load[2]
    );
    out.push_str(&format!(
        "pressure  cpu {:.2}  memory {:.2}  io {:.2}  (some avg10, %)\n",
        m.cpu.pressure.some_avg10, m.memory.pressure.some_avg10, m.io.pressure.some_avg10
    ));
    out.push_str(&format!(
        "memory    {} MiB available of {} MiB, swap {} of {} MiB free\n",
        m.memory.available_mib, m.memory.total_mib, m.memory.swap_free_mib, m.memory.swap_total_mib
    ));
    for disk in &m.io.disks {
        out.push_str(&format!(
            "disk      {} {} busy, {:.1} MiB/s written\n",
            disk.name,
            pct(disk.busy),
            disk.write_mib_s
        ));
    }
    for mount in &m.mounts {
        out.push_str(&format!(
            "mount     {} {} MiB free of {} MiB ({} used)\n",
            mount.path,
            mount.free_mib,
            mount.total_mib,
            pct(mount.used)
        ));
    }
    for job in &status.jobs {
        let cpu = job
            .cpu_threads
            .map_or("-".to_owned(), |t| format!("{t:.2}"));
        let mem = job.mem_mib.map_or("-".to_owned(), |m| m.to_string());
        out.push_str(&format!(
            "job {}     {} since {}, {cpu} threads, {mem} MiB\n",
            job.job, job.key, job.started_at
        ));
    }
    for w in &status.waiting {
        out.push_str(&format!(
            "waiting {} {} ({}) for {:.0}s: {}\n",
            w.job, w.key, w.kind, w.waited_s, w.reason
        ));
    }
    out
}

/// Two lines: each figure with where it came from, then how many runs fed it.
fn estimate_text(e: &Estimate) -> String {
    let from = |source: Source| match source {
        Source::History => "history",
        Source::Default => "default",
    };
    let peak = e
        .cpu_peak_5s
        .map_or(String::new(), |p| format!(", peak {p:.2} over 5s"));
    let duration = e.duration_s.map_or("-".to_owned(), |d| format!("{d:.1}s"));
    format!(
        "estimate  cpu {:.2} threads{peak} ({}), memory {} MiB ({}), write {:.2} MiB/s ({}), \
         duration {duration} ({})\nsamples   {} measured, {} calm\n",
        e.cpu_threads,
        from(e.source.cpu),
        e.mem_mib,
        from(e.source.mem),
        e.write_mib_s,
        from(e.source.io),
        from(e.source.duration),
        e.samples.measured,
        e.samples.calm,
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use taskslash_core::estimate::estimate;
    use taskslash_core::protocol::Launch;

    #[test]
    fn a_plain_run_shows_a_dash_for_each_cost() {
        let run = Run {
            id: 1,
            key: "t1".into(),
            kind: Kind::Command,
            argv: vec!["sh".into(), "-c".into(), "exit 4".into()],
            cwd: "/work".into(),
            launch: Launch::Plain,
            started_at: "2026-11-16T02:17:44.464Z".into(),
            wall_ms: 1234,
            cpu_usec: None,
            peak_cpu_5s: None,
            peak_mem_bytes: None,
            written_bytes: None,
            exit_code: Some(4),
            machine_cpu_some: None,
            machine_mem_some: None,
            machine_io_some: None,
        };
        let text = table(&[run]);
        let line: Vec<&str> = text.lines().nth(1).unwrap().split_whitespace().collect();
        assert_eq!(
            line,
            ["2026-11-16T02:17:44.464Z", "1.2s", "-", "-", "-", "-", "4"]
        );
    }

    #[test]
    fn an_estimate_with_no_history_says_default_for_each_figure() {
        let text = estimate_text(&estimate(&[], Kind::Typecheck));
        assert_eq!(
            text,
            "estimate  cpu 2.00 threads (default), memory 3072 MiB (default), \
             write 0.00 MiB/s (default), duration - (default)\n\
             samples   0 measured, 0 calm\n"
        );
    }
}
