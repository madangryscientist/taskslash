//! `taskslash run`: ask the daemon for a job, run the command as this process's child, report how
//! it ended, and return its exit code.

use crate::Error;
use crate::protocol::{ClientMessage, DaemonMessage, receive, send};
use std::env;
use std::ffi::OsString;
use std::io::{self, BufReader};
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Command, ExitStatus};

/// The exit code when the command cannot be started, as a shell uses.
const CANNOT_RUN: i32 = 127;

/// Runs `argv` as a job of the daemon at `socket`. Returns the command's exit code (128+N when a
/// signal N killed it). An `Err` means the command was never run.
///
/// `argv` is passed to the child as it is; the daemon is sent a lossy UTF-8 copy.
pub fn run(socket: &Path, name: Option<String>, argv: &[OsString]) -> Result<i32, Error> {
    let stream = UnixStream::connect(socket).map_err(|e| {
        Error::new(format!(
            "no daemon at {} ({e}). Start it with 'systemctl --user start taskslash' or 'taskslash daemon'.",
            socket.display()
        ))
    })?;
    let cwd = env::current_dir()
        .map_err(|e| Error::new(format!("cannot read the current directory: {e}")))?;
    let mut writer = stream
        .try_clone()
        .map_err(|e| Error::new(format!("cannot use the daemon's socket: {e}")))?;
    let mut reader = BufReader::new(stream);
    let lost = |e: io::Error| Error::new(format!("lost the daemon at {}: {e}", socket.display()));

    let submit = ClientMessage::Submit {
        name,
        argv: argv
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect(),
        cwd: cwd.to_string_lossy().into_owned(),
    };
    send(&mut writer, &submit).map_err(lost)?;
    let job = match receive(&mut reader).map_err(lost)? {
        Some(DaemonMessage::Start { job, .. }) => job,
        Some(DaemonMessage::Error { message }) => {
            return Err(Error::new(format!("the daemon refused the job: {message}")));
        }
        Some(other) => return Err(Error::new(format!("unexpected reply {other:?}"))),
        None => return Err(Error::new("the daemon closed the connection")),
    };

    let code = match Command::new(&argv[0]).args(&argv[1..]).status() {
        Ok(status) => exit_code(status),
        Err(e) => {
            eprintln!("taskslash: cannot run {}: {e}", argv[0].to_string_lossy());
            CANNOT_RUN
        }
    };

    // The child has run; a daemon that went away meanwhile costs a warning, not its exit code.
    let report = send(&mut writer, &ClientMessage::Exited { job, code })
        .and_then(|()| receive::<DaemonMessage>(&mut reader));
    match report {
        Ok(Some(DaemonMessage::Recorded { .. })) => {}
        Ok(_) => eprintln!("taskslash: warning: the daemon did not record job {job}"),
        Err(e) => eprintln!("taskslash: warning: could not report job {job} to the daemon: {e}"),
    }
    Ok(code)
}

/// A shell's exit code for `status`: the code, or 128+N when signal N killed it.
fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1)
}
