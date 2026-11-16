//! The wire protocol: newline delimited JSON on a unix socket, one request per connection.
//!
//! A job: the client sends `submit`; the daemon answers `start` (or `error`, then closes). The
//! client runs the command the way `start.launch` says, sends `exited`, and the daemon answers
//! `recorded` once the run's row is written. An EOF before `exited` ends the job as dropped, and its
//! row has no exit code.
//!
//! A query: the client sends `history`; the daemon answers `runs` and closes.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};

/// A message from the command line to the daemon.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Ask to start a job. `argv` has at least one element.
    Submit {
        name: Option<String>,
        argv: Vec<String>,
        cwd: String,
    },
    /// The job's command has exited with `code`.
    Exited { job: u64, code: i32 },
    /// Ask for the recorded runs of `key`, newest first.
    History { key: String },
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
    /// The job's end is recorded.
    Recorded { job: u64 },
    /// The answer to `history`: at most 20 runs, newest first.
    Runs { runs: Vec<Run> },
    /// The request was refused; the daemon closes the connection after this.
    Error { message: String },
}

/// One recorded run. A cost field is `None` when it was not measured, never 0 for unknown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub id: i64,
    pub key: String,
    pub argv: Vec<String>,
    pub cwd: String,
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
}

/// `runs` as a pretty JSON array and a newline, as `taskslash history --json` prints it.
pub fn runs_json(runs: &[Run]) -> String {
    let mut text = serde_json::to_string_pretty(runs).unwrap_or_else(|_| "[]".to_owned());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip_as_tagged_json_lines() {
        let submit = ClientMessage::Submit {
            name: Some("probe".into()),
            argv: vec!["sh".into(), "-c".into(), "exit 3".into()],
            cwd: "/work".into(),
        };
        let start = DaemonMessage::Start {
            job: 3,
            unit: "taskslash-1-3".into(),
            launch: Launch::Scope,
        };
        let history = ClientMessage::History {
            key: "probe".into(),
        };
        let mut wire = Vec::new();
        send(&mut wire, &submit).unwrap();
        send(&mut wire, &start).unwrap();
        send(&mut wire, &history).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&wire),
            "{\"type\":\"submit\",\"name\":\"probe\",\"argv\":[\"sh\",\"-c\",\"exit 3\"],\"cwd\":\"/work\"}\n\
             {\"type\":\"start\",\"job\":3,\"unit\":\"taskslash-1-3\",\"launch\":\"scope\"}\n\
             {\"type\":\"history\",\"key\":\"probe\"}\n"
        );
        let mut reader = wire.as_slice();
        assert_eq!(receive(&mut reader).unwrap(), Some(submit));
        assert_eq!(receive(&mut reader).unwrap(), Some(start));
        assert_eq!(receive(&mut reader).unwrap(), Some(history));
        assert_eq!(receive::<DaemonMessage>(&mut reader).unwrap(), None);
    }
}
