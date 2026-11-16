//! The wire protocol: newline delimited JSON on a unix socket, one job per connection.
//!
//! The client sends `submit`; the daemon answers `start` (or `error`, then closes). The client runs
//! the command, sends `exited`, and the daemon answers `recorded`. An EOF before `exited` ends the
//! job as dropped.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};

/// A message from `taskslash run` to the daemon.
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
}

/// A message from the daemon to `taskslash run`.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonMessage {
    /// The job is admitted: run it now. `unit` is the systemd unit name it will run under.
    Start { job: u64, unit: String },
    /// The job's end is recorded.
    Recorded { job: u64 },
    /// The request was refused; the daemon closes the connection after this.
    Error { message: String },
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
            unit: "taskslash-job-3".into(),
        };
        let mut wire = Vec::new();
        send(&mut wire, &submit).unwrap();
        send(&mut wire, &start).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&wire),
            "{\"type\":\"submit\",\"name\":\"probe\",\"argv\":[\"sh\",\"-c\",\"exit 3\"],\"cwd\":\"/work\"}\n\
             {\"type\":\"start\",\"job\":3,\"unit\":\"taskslash-job-3\"}\n"
        );
        let mut reader = wire.as_slice();
        assert_eq!(receive(&mut reader).unwrap(), Some(submit));
        assert_eq!(receive(&mut reader).unwrap(), Some(start));
        assert_eq!(receive::<DaemonMessage>(&mut reader).unwrap(), None);
    }
}
