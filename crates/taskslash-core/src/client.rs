//! The client side: `taskslash run` asks the daemon for a job, runs the command as this process's
//! child the way the daemon says, reports how it ended, and returns its exit code;
//! `taskslash history` asks for a key's recorded runs, `taskslash status` for what is running, and
//! `taskslash why` why a job waits.

use crate::Error;
use crate::admission::{Reason, waiting_line};
use crate::estimate::Estimate;
use crate::launch;
use crate::protocol::{
    Class, ClientMessage, DaemonMessage, JobState, Kind, Launch, Run, Status, receive, send,
};
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
        send(&mut self.writer, message).map_err(|e| self.lost(e))?;
        self.next()
    }

    /// Reads the next message from the daemon.
    fn next(&mut self) -> Result<DaemonMessage, Error> {
        match receive(&mut self.reader).map_err(|e| self.lost(e))? {
            Some(DaemonMessage::Error { message }) => {
                Err(Error::new(format!("the daemon refused: {message}")))
            }
            Some(reply) => Ok(reply),
            None => Err(Error::new("the daemon closed the connection")),
        }
    }

    fn lost(&self, e: io::Error) -> Error {
        Error::new(format!("lost the daemon at {}: {e}", self.socket.display()))
    }
}

/// What the caller says a job costs, replacing the learned estimate: CPU threads and MiB.
#[derive(Debug, Clone, Copy, Default)]
pub struct Demand {
    pub cpu: Option<f64>,
    pub mem_mib: Option<u64>,
}

/// Runs `argv` as a job of kind `kind` and class `class` of the daemon at `socket`, once the daemon admits it (a job
/// that has to wait says why on stderr, when queued and every 15 s). Returns the command's exit code (128+N when a
/// signal N killed it, 127 when it cannot be started). An `Err` means the command was never run.
///
/// `argv` is passed to the child as it is; the daemon is sent a lossy UTF-8 copy.
pub fn run(
    socket: &Path,
    name: Option<String>,
    kind: Kind,
    class: Class,
    demand: Demand,
    argv: &[OsString],
) -> Result<i32, Error> {
    let mut daemon = Connection::open(socket)?;
    let cwd = env::current_dir()
        .map_err(|e| Error::new(format!("cannot read the current directory: {e}")))?;
    let submit = ClientMessage::Submit {
        name,
        kind,
        argv: argv
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect(),
        cwd: cwd.to_string_lossy().into_owned(),
        cpu: demand.cpu,
        mem_mib: demand.mem_mib,
        class,
        submitter: submitter(&cwd).to_string_lossy().into_owned(),
    };
    let mut reply = daemon.ask(&submit)?;
    let (job, unit, launch) = loop {
        match reply {
            DaemonMessage::Start { job, unit, launch } => break (job, unit, launch),
            DaemonMessage::Waiting {
                reason,
                waited_s,
                position,
            } => eprintln!("{}", waiting_line(&reason, waited_s, position)),
            other => return Err(Error::new(format!("unexpected reply {other:?}"))),
        }
        reply = daemon.next()?;
    };

    let program = Path::new(&argv[0]);
    let code = if launch == Launch::Scope && !launch::can_start(program) {
        eprintln!(
            "taskslash: cannot run {}: no such executable",
            program.display()
        );
        CANNOT_RUN
    } else {
        match launch::command(launch, &unit, class, argv).status() {
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

/// The estimate of `key` and its recorded runs, newest first, at most 20.
pub fn history(socket: &Path, key: &str) -> Result<(Estimate, Vec<Run>), Error> {
    let mut daemon = Connection::open(socket)?;
    match daemon.ask(&ClientMessage::History {
        key: key.to_owned(),
    })? {
        DaemonMessage::Runs { estimate, runs } => Ok((estimate, runs)),
        other => Err(Error::new(format!("unexpected reply {other:?}"))),
    }
}

/// What the machine and the running jobs are doing, from the daemon's latest readings.
pub fn status(socket: &Path) -> Result<Status, Error> {
    let mut daemon = Connection::open(socket)?;
    match daemon.ask(&ClientMessage::Status)? {
        DaemonMessage::Status(status) => Ok(*status),
        other => Err(Error::new(format!("unexpected reply {other:?}"))),
    }
}

/// Where job `job` is: waiting (with its reason, seconds waited and place in line), or running.
/// An `Err` when the daemon does not hold it.
pub fn why(socket: &Path, job: u64) -> Result<Option<(Reason, u64, usize)>, Error> {
    let mut daemon = Connection::open(socket)?;
    match daemon.ask(&ClientMessage::Why { job })? {
        DaemonMessage::Why {
            state: JobState::Waiting,
            reason: Some(reason),
            waited_s,
            position,
            ..
        } => Ok(Some((reason, waited_s.unwrap_or(0), position.unwrap_or(1)))),
        DaemonMessage::Why {
            state: JobState::Running,
            ..
        } => Ok(None),
        DaemonMessage::Why { .. } => Err(Error::new(format!(
            "job {job} not found (an ended job's record is in history)"
        ))),
        other => Err(Error::new(format!("unexpected reply {other:?}"))),
    }
}

/// Who submits from `cwd`: the nearest ancestor of it, itself first, with a `.git` entry (a
/// directory in a clone, a file in a worktree), else `cwd`.
fn submitter(cwd: &Path) -> &Path {
    cwd.ancestors()
        .find(|dir| dir.join(".git").symlink_metadata().is_ok())
        .unwrap_or(cwd)
}

/// A shell's exit code for `status`: the code, or 128+N when signal N killed it.
fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1)
}

#[cfg(test)]
mod tests {
    use super::submitter;
    use std::fs;
    use std::path::Path;

    #[test]
    fn the_submitter_is_the_nearest_directory_with_a_git_entry() {
        let root = std::env::temp_dir().join(format!("tks-{}", std::process::id()));
        let deep = root.join("wt/a/b");
        fs::create_dir_all(&deep).unwrap();
        // No .git under `root`: the cwd itself, unless a directory above `root` has one.
        let outside = submitter(&deep);
        assert!(
            outside == deep || !outside.starts_with(&root),
            "{outside:?}"
        );
        fs::create_dir(root.join(".git")).unwrap();
        assert_eq!(submitter(&deep), root, "a clone's .git directory");
        fs::write(root.join("wt/.git"), "gitdir: /elsewhere\n").unwrap();
        assert_eq!(submitter(&deep), root.join("wt"), "a worktree's .git file");
        assert_eq!(
            submitter(&root.join("wt")),
            root.join("wt"),
            "the cwd itself counts"
        );
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(submitter(Path::new("/")), Path::new("/"));
    }
}
