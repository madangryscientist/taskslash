//! The client side: `taskslash run` asks the daemon for a job, runs the command as this process's
//! child the way the daemon says, reports how it ended, and returns its exit code;
//! `taskslash history` asks for a key's recorded runs.

use crate::Error;
use crate::launch;
use crate::protocol::{ClientMessage, DaemonMessage, Launch, Run, receive, send};
use std::env;
use std::ffi::OsString;
use std::io::{self, BufReader};
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::ExitStatus;

/// The exit code when the command cannot be started, as a shell uses.
const CANNOT_RUN: i32 = 127;

/// A connection to the daemon: its two halves, and how to word an I/O failure on it.
struct Connection<'a> {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    socket: &'a Path,
}

impl<'a> Connection<'a> {
    fn open(socket: &'a Path) -> Result<Self, Error> {
        let stream = UnixStream::connect(socket).map_err(|e| {
            Error::new(format!(
                "no daemon at {} ({e}). Start it with 'systemctl --user start taskslash' or 'taskslash daemon'.",
                socket.display()
            ))
        })?;
        let writer = stream
            .try_clone()
            .map_err(|e| Error::new(format!("cannot use the daemon's socket: {e}")))?;
        Ok(Self {
            reader: BufReader::new(stream),
            writer,
            socket,
        })
    }

    /// Sends `message` and reads the reply.
    fn ask(&mut self, message: &ClientMessage) -> Result<DaemonMessage, Error> {
        let socket = self.socket;
        let lost =
            |e: io::Error| Error::new(format!("lost the daemon at {}: {e}", socket.display()));
        send(&mut self.writer, message).map_err(lost)?;
        match receive(&mut self.reader).map_err(lost)? {
            Some(DaemonMessage::Error { message }) => {
                Err(Error::new(format!("the daemon refused: {message}")))
            }
            Some(reply) => Ok(reply),
            None => Err(Error::new("the daemon closed the connection")),
        }
    }
}

/// Runs `argv` as a job of the daemon at `socket`. Returns the command's exit code (128+N when a
/// signal N killed it, 127 when it cannot be started). An `Err` means the command was never run.
///
/// `argv` is passed to the child as it is; the daemon is sent a lossy UTF-8 copy.
pub fn run(socket: &Path, name: Option<String>, argv: &[OsString]) -> Result<i32, Error> {
    let mut daemon = Connection::open(socket)?;
    let cwd = env::current_dir()
        .map_err(|e| Error::new(format!("cannot read the current directory: {e}")))?;
    let submit = ClientMessage::Submit {
        name,
        argv: argv
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect(),
        cwd: cwd.to_string_lossy().into_owned(),
    };
    let (job, unit, launch) = match daemon.ask(&submit)? {
        DaemonMessage::Start { job, unit, launch } => (job, unit, launch),
        other => return Err(Error::new(format!("unexpected reply {other:?}"))),
    };

    let program = Path::new(&argv[0]);
    let code = if launch == Launch::Scope && !launch::can_start(program) {
        eprintln!(
            "taskslash: cannot run {}: no such executable",
            program.display()
        );
        CANNOT_RUN
    } else {
        match launch::command(launch, &unit, argv).status() {
            Ok(status) => exit_code(status),
            Err(e) => {
                eprintln!("taskslash: cannot run {}: {e}", program.display());
                CANNOT_RUN
            }
        }
    };

    // The child has run; a daemon that went away meanwhile costs a warning, not its exit code.
    match daemon.ask(&ClientMessage::Exited { job, code }) {
        Ok(DaemonMessage::Recorded { .. }) => {}
        Ok(_) => eprintln!("taskslash: warning: the daemon did not record job {job}"),
        Err(e) => eprintln!("taskslash: warning: could not report job {job} to the daemon: {e}"),
    }
    Ok(code)
}

/// The recorded runs of `key`, newest first, at most 20.
pub fn history(socket: &Path, key: &str) -> Result<Vec<Run>, Error> {
    let mut daemon = Connection::open(socket)?;
    match daemon.ask(&ClientMessage::History {
        key: key.to_owned(),
    })? {
        DaemonMessage::Runs { runs } => Ok(runs),
        other => Err(Error::new(format!("unexpected reply {other:?}"))),
    }
}

/// A shell's exit code for `status`: the code, or 128+N when signal N killed it.
fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1)
}
