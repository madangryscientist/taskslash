//! The wire protocol: newline delimited JSON on a unix socket, one request per connection.
//!
//! A job: the client sends `submit`; the daemon answers `start` (or `error`, then closes). A job
//! that has to queue is sent `waiting` first, when it is queued and every 15 s after. The client
//! runs the command the way `start.launch` says, sends `exited`, and the daemon answers
//! `recorded` once the run's row is written. An EOF before `exited` ends the job as dropped, and its
//! row has no exit code.
//!
//! A query: the client sends `history`; the daemon answers `runs` (the key's estimate and its runs)
//! and closes. Or it sends `keys`; the daemon answers `keys` (every key with runs, newest first). Or it sends `status`; the daemon answers `status` (the machine's latest snapshot, the
//! running jobs and the waiting ones). Or `why`; the daemon answers `why` (whether the job waits,
//! runs or is not held, and why it waits). Or `log`; the daemon answers `log` (the job or run it
//! picked and where its log is).
//!
//! A subscriber: the client sends `subscribe`; the daemon answers `subscribed`, then sends one
//! `event` per job event (queued, started, finished) until the client hangs up. It never replays
//! an event from before `subscribed`, and never waits for a slow subscriber: what does not fit in
//! its queue of 1024 is dropped and counted in a `dropped` event.

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

/// The version of the contract a plugin gets: `TASKSLASH_PROTOCOL` in its environment. Bumped on
/// any breaking change to [`ClientMessage`] or [`DaemonMessage`].
pub const PROTOCOL_VERSION: u32 = 1;

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
    /// The job's command has exited with `code`. A moon target's job also says whether moon
    /// replayed it from cache and the hash moon ran it under.
    Exited {
        job: u64,
        code: i32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cached: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        moon_hash: Option<String>,
    },
    /// Ask for the recorded runs of `key`, newest first: at most `limit` (20 when not given), and
    /// only those whose submitter is `within` or whose submitter's directory name is, when given.
    History {
        key: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
        #[serde(default, rename = "where", skip_serializing_if = "Option::is_none")]
        within: Option<String>,
    },
    /// Ask for every key with recorded runs, the most recently run first.
    Keys,
    /// Ask what the machine and the running jobs are doing.
    Status,
    /// Ask why job `job` waits.
    Why { job: u64 },
    /// Ask which job or run `select` picks and where its log is.
    Log { select: LogSelect },
    /// Follow the daemon's job events: `subscribed`, then one `event` per job event from then on,
    /// until the client hangs up.
    Subscribe,
}

/// Which log `taskslash logs` asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum LogSelect {
    /// A job the daemon holds, by its number.
    Job { job: u64 },
    /// A recorded run, by its id.
    Run { run: i64 },
    /// The newest run of `key`, running or not.
    Last { key: String },
    /// The newest recorded run, of `key` when given, whose exit code is set and not 0.
    Failed { key: Option<String> },
}

/// What a `log` query found: a running or waiting job (`job`), or a recorded run (`run`), and its
/// log file. `log_path` is `None` for a waiting job and for a run recorded before capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogFound {
    pub job: Option<u64>,
    pub run: Option<i64>,
    pub log_path: Option<String>,
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
    /// The job is admitted: run it now, under the systemd unit `unit` when `launch` is `scope`,
    /// copying its output into the file `log`.
    Start {
        job: u64,
        unit: String,
        launch: Launch,
        log: String,
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
    /// The answer to `history`: the key's estimate and its runs, newest first.
    Runs { estimate: Estimate, runs: Vec<Run> },
    /// The answer to `keys`: every key with recorded runs, the most recently run first.
    Keys { keys: Vec<KeySummary> },
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
    /// The answer to `log`: `None` when nothing matches.
    Log { found: Option<LogFound> },
    /// The answer to `subscribe`; `event` lines follow.
    Subscribed { protocol: u32 },
    /// One job event. `seq` counts up per daemon from 1; a `dropped` event stands for the events
    /// this subscriber missed and carries the seq of the last of them.
    Event {
        protocol: u32,
        seq: u64,
        event: JobEvent,
    },
    /// The request was refused; the daemon closes the connection after this.
    Error { message: String },
}

/// What happened to a job, as a subscriber sees it. Every `queued` is followed by exactly one
/// `finished`; `started` comes between them when the job ran. Times are RFC 3339, UTC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEvent {
    /// The job was submitted and takes its place in the queue.
    Queued {
        job: u64,
        key: String,
        kind: Kind,
        class: Class,
        submitter: String,
        cwd: String,
        argv: Vec<String>,
        at: String,
    },
    /// The job was admitted and its client told to run it, its output going into `log`.
    Started {
        job: u64,
        unit: String,
        launch: Launch,
        log: String,
        at: String,
    },
    /// The job ended. `exit_code` is null when the client went away first, `run` when its row was
    /// not written. A job that never started has `exit_code`, `run` and `log` null and `wall_ms` 0.
    Finished {
        job: u64,
        key: String,
        exit_code: Option<i32>,
        wall_ms: u64,
        cached: Option<bool>,
        moon_hash: Option<String>,
        run: Option<i64>,
        log: Option<String>,
        at: String,
    },
    /// This subscriber was too slow to take `count` events, which it will not get.
    Dropped { count: u64 },
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
    /// Whether moon replayed the run from its cache, so it is not a cost sample; `None` for a
    /// run that is not a moon target's.
    pub cached: Option<bool>,
    /// The hash moon ran the target under; `None` when it gave none.
    pub moon_hash: Option<String>,
    /// The file its output was copied into; `None` for a run recorded before capture. The file
    /// may be gone: retention deletes old logs and keeps the path.
    pub log_path: Option<String>,
}

/// A key with recorded runs, as `keys` answers: how many, and the newest one's start, exit code
/// and submitter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeySummary {
    pub key: String,
    pub runs: u64,
    /// RFC 3339, UTC.
    pub last_started_at: String,
    /// `None` when the newest run's client went away before reporting how the command ended.
    pub last_exit_code: Option<i32>,
    pub last_submitter: String,
}

/// The last part of a submitter's path: its worktree, as `top` and `history` show it.
pub fn worktree(submitter: &str) -> &str {
    std::path::Path::new(submitter)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(submitter)
}

/// Whether a run from `submitter` is one `--where <within>` keeps: the submitter is `within`, or its
/// directory name is.
pub fn submitted_from(submitter: &str, within: &str) -> bool {
    submitter == within
        || submitter
            .strip_suffix(within)
            .is_some_and(|head| head.ends_with('/'))
}

/// The keys as a pretty JSON array and a newline, as `taskslash history --json` with no key prints
/// it: each summary with its `worktree` besides.
pub fn keys_json(keys: &[KeySummary]) -> String {
    #[derive(Serialize)]
    struct Entry<'a> {
        #[serde(flatten)]
        summary: &'a KeySummary,
        worktree: &'a str,
    }
    let entries: Vec<Entry> = keys
        .iter()
        .map(|summary| Entry {
            summary,
            worktree: worktree(&summary.last_submitter),
        })
        .collect();
    let mut text = serde_json::to_string_pretty(&entries).unwrap_or_default();
    text.push('\n');
    text
}

/// Up to five of `keys` that `text` may have meant: those containing it (ignoring case) in their
/// order, else the closest by Jaro similarity above 0.7 (ignoring case), best first.
pub fn suggest<'a>(text: &str, keys: &'a [String]) -> Vec<&'a str> {
    const MAX: usize = 5;
    let lower = text.to_lowercase();
    let containing: Vec<&str> = keys
        .iter()
        .filter(|k| k.to_lowercase().contains(&lower))
        .map(String::as_str)
        .take(MAX)
        .collect();
    if !containing.is_empty() {
        return containing;
    }
    let mut near: Vec<(f64, &str)> = keys
        .iter()
        .map(|k| (strsim::jaro(&lower, &k.to_lowercase()), k.as_str()))
        .filter(|(score, _)| *score > 0.7)
        .collect();
    near.sort_by(|a, b| b.0.total_cmp(&a.0));
    near.into_iter().map(|(_, k)| k).take(MAX).collect()
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
