//! The wire protocol: newline delimited JSON on a unix socket, one request per connection.
//!
//! A job: the client sends `submit`; the daemon answers `start` (or `error`, then closes). A job
//! that has to queue is sent `waiting` first, when it is queued and every 15 s after. The client
//! runs the command the way `start.launch` says, sends `exited`, and the daemon answers
//! `recorded` once the run's row is written. An EOF before `exited` ends the job as dropped, and its
//! row has no exit code.
//!
//! A query: the client sends `history`; the daemon answers `runs` (the key's estimate and its runs)
//! and closes. Or it sends `status`; the daemon answers `status` (the machine's latest snapshot, the
//! running jobs and the waiting ones). Or `why`; the daemon answers `why` (whether the job waits,
//! runs or is not held, and why it waits).

use crate::admission::Reason;
use crate::estimate::Estimate;
pub use crate::order::Class;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::{self, BufRead, Write};
use std::str::FromStr;

/// The cost class of a job (spike 4.4): which defaults it gets before it has history.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Test,
    Typecheck,
    Build,
    Browser,
    #[default]
    Command,
}

impl Kind {
    pub const ALL: [Self; 5] = [
        Self::Test,
        Self::Typecheck,
        Self::Build,
        Self::Browser,
        Self::Command,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Test => "test",
            Self::Typecheck => "typecheck",
            Self::Build => "build",
            Self::Browser => "browser",
            Self::Command => "command",
        }
    }

    /// The CPU a job of this kind is assumed to take before it has history, in threads.
    pub fn default_cpu_threads(self) -> f64 {
        match self {
            Self::Build | Self::Browser => 4.0,
            Self::Test | Self::Typecheck | Self::Command => 2.0,
        }
    }

    /// The memory a job of this kind is assumed to take before it has history, in MiB.
    pub fn default_mem_mib(self) -> u64 {
        match self {
            Self::Test | Self::Command => 2048,
            Self::Typecheck => 3072,
            Self::Build | Self::Browser => 4096,
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Kind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == s)
            .ok_or_else(|| {
                format!("unknown kind {s:?}: one of test, typecheck, build, browser, command")
            })
    }
}

/// A message from the command line to the daemon.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Ask to start a job. `argv` has at least one element.
    /// `cpu` (threads) and `mem_mib`, when given, replace the learned estimate's figures.
    Submit {
        name: Option<String>,
        kind: Kind,
        argv: Vec<String>,
        cwd: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cpu: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mem_mib: Option<u64>,
        class: Class,
        /// The nearest ancestor of `cwd` (or `cwd`) holding a `.git`, else `cwd`.
        submitter: String,
    },
    /// The job's command has exited with `code`.
    Exited { job: u64, code: i32 },
    /// Ask for the recorded runs of `key`, newest first.
    History { key: String },
    /// Ask what the machine and the running jobs are doing.
    Status,
    /// Ask why job `job` waits.
    Why { job: u64 },
}

/// How the client runs a job's command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Launch {
    /// In a transient systemd user scope named by `start.unit`, so the daemon can measure it.
    Scope,
    /// As a plain child: no systemd user session, so only wall time and exit code are recorded.
    Plain,
}

impl Launch {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scope => "scope",
            Self::Plain => "plain",
        }
    }
}

/// A message from the daemon to the command line.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonMessage {
    /// The job is admitted: run it now, under the systemd unit `unit` when `launch` is `scope`.
    Start {
        job: u64,
        unit: String,
        launch: Launch,
    },
    /// The job is queued, for `reason`, and has waited `waited_s`, `position` (1-based) in the
    /// queue's order; `start` follows when it is admitted.
    Waiting {
        reason: Reason,
        waited_s: u64,
        position: usize,
    },
    /// The job's end is recorded.
    Recorded { job: u64 },
    /// The answer to `history`: the key's estimate and at most 20 runs, newest first.
    Runs { estimate: Estimate, runs: Vec<Run> },
    /// The answer to `status`.
    Status(Box<Status>),
    /// The answer to `why`: `reason`, `waited_s` and `position` are set when `state` is `waiting`.
    Why {
        job: u64,
        state: JobState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<Reason>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        waited_s: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<usize>,
    },
    /// The request was refused; the daemon closes the connection after this.
    Error { message: String },
}

/// Where a job is, as `why` answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Waiting,
    Running,
    /// The daemon does not hold it: never submitted, or ended (its record is in `history`).
    NotFound,
}

/// One recorded run. A cost field is `None` when it was not measured, never 0 for unknown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub id: i64,
    pub key: String,
    pub kind: Kind,
    pub argv: Vec<String>,
    pub cwd: String,
    pub class: Class,
    pub submitter: String,
    pub launch: Launch,
    /// RFC 3339, UTC.
    pub started_at: String,
    pub wall_ms: i64,
    pub cpu_usec: Option<i64>,
    /// The most CPU the job used over any 5 s, in threads.
    pub peak_cpu_5s: Option<f64>,
    pub peak_mem_bytes: Option<i64>,
    pub written_bytes: Option<i64>,
    /// `None` when the client went away before reporting how the command ended.
    pub exit_code: Option<i32>,
    /// The mean of the machine's `some avg10` pressures over the run; `None` when no sample of the
    /// machine fell inside it.
    pub machine_cpu_some: Option<f64>,
    pub machine_mem_some: Option<f64>,
    pub machine_io_some: Option<f64>,
}

/// `{"estimate":...,"runs":[...]}`, pretty, and a newline, as `taskslash history --json` prints it.
pub fn history_json(estimate: &Estimate, runs: &[Run]) -> String {
    #[derive(Serialize)]
    struct History<'a> {
        estimate: &'a Estimate,
        runs: &'a [Run],
    }
    let mut text = serde_json::to_string_pretty(&History { estimate, runs }).unwrap_or_default();
    text.push('\n');
    text
}

/// A waiting job's reason as one line of JSON, as `taskslash why --json` prints it.
pub fn reason_json(reason: &Reason) -> String {
    let mut text = serde_json::to_string(reason).unwrap_or_default();
    text.push('\n');
    text
}

/// Writes one message as a line of JSON.
pub fn send<T: Serialize>(writer: &mut impl Write, message: &T) -> io::Result<()> {
    let mut line = serde_json::to_string(message)?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush()
}

/// Reads one line of JSON. `Ok(None)` is a clean end of stream.
pub fn receive<T: DeserializeOwned>(reader: &mut impl BufRead) -> io::Result<Option<T>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(serde_json::from_str(&line)?))
}

mod status;
pub use status::{JobStatus, Status, WaitingStatus, status_json};

#[cfg(test)]
mod tests;
