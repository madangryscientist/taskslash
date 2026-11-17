//! `taskslash logs`: prints a job's or a run's log, and follows a running job's to its end.

use super::Connection;
use crate::Error;
use crate::protocol::{ClientMessage, DaemonMessage, JobState, LogFound, LogSelect};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;

/// How often `-f` looks for more of the log and whether its job has ended.
const FOLLOW_EVERY: Duration = Duration::from_millis(200);

/// Writes the log `select` picks to `out`, unchanged. With `follow`, a running job's log is
/// followed until the job ends. An `Err` (exit 1) when nothing matches, the job has not started,
/// the run predates logs, or retention deleted its log.
pub fn logs(
    socket: &Path,
    select: &LogSelect,
    follow: bool,
    out: &mut impl Write,
) -> Result<(), Error> {
    let mut daemon = Connection::open(socket)?;
    let found = match daemon.ask(&ClientMessage::Log {
        select: select.clone(),
    })? {
        DaemonMessage::Log { found } => found,
        other => return Err(Error::new(format!("unexpected reply {other:?}"))),
    };
    let Some(found) = found else {
        return Err(Error::new(match select {
            LogSelect::Job { job } => format!("no job {job}"),
            LogSelect::Run { run } => format!("no run {run}"),
            LogSelect::Last { key } => format!("no run of {key}"),
            LogSelect::Failed { key: None } => "no failed run".to_owned(),
            LogSelect::Failed { key: Some(key) } => format!("no failed run of {key}"),
        }));
    };
    let written = match found {
        LogFound {
            job: Some(job),
            log_path: Some(path),
            ..
        } => running(socket, job, Path::new(&path), follow, out),
        LogFound {
            job: Some(job),
            log_path: None,
            ..
        } => return Err(Error::new(format!("job {job} has not started"))),
        LogFound {
            run,
            log_path: None,
            ..
        } => {
            return Err(Error::new(format!(
                "run {} has no log",
                run.unwrap_or_default()
            )));
        }
        LogFound {
            run,
            log_path: Some(path),
            ..
        } => {
            let run = run.unwrap_or_default();
            let mut file = File::open(&path).map_err(|e| match e.kind() {
                io::ErrorKind::NotFound => Error::new(format!("the log of run {run} has expired")),
                _ => Error::new(format!("cannot read {path}: {e}")),
            })?;
            io::copy(&mut file, out).and_then(|_| out.flush())
        }
    };
    match written {
        // A reader that stopped early (`| head`) is not a failure.
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(Error::new(format!("cannot write the log: {e}"))),
        Ok(()) => Ok(()),
    }
}

/// Writes what job `job`'s log at `path` holds; with `follow`, then what is added until the job
/// ends. A log its client has not created yet reads as empty.
fn running(
    socket: &Path,
    job: u64,
    path: &Path,
    follow: bool,
    out: &mut impl Write,
) -> io::Result<()> {
    let mut file = File::open(path).ok();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        // Asked before reading, so whatever the job wrote before it ended is read after.
        let ended = !follow || !holds(socket, job);
        if file.is_none() {
            file = File::open(path).ok();
        }
        if let Some(file) = file.as_mut() {
            loop {
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])?;
            }
            out.flush()?;
        }
        if ended {
            return Ok(());
        }
        thread::sleep(FOLLOW_EVERY);
    }
}

/// Whether the daemon still holds job `job`. A daemon that cannot be asked holds nothing.
fn holds(socket: &Path, job: u64) -> bool {
    let Ok(mut daemon) = Connection::open(socket) else {
        return false;
    };
    !matches!(
        daemon.ask(&ClientMessage::Why { job }),
        Ok(DaemonMessage::Why {
            state: JobState::NotFound,
            ..
        }) | Err(_)
    )
}
