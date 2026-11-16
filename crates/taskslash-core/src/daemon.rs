//! `taskslash daemon`: hands out job ids and records each job's start and end. One thread per
//! connection; a connection is one job.

use crate::Error;
use crate::protocol::{ClientMessage, DaemonMessage, receive, send};
use std::fs::{self, DirBuilder};
use std::io::BufReader;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

/// Binds the socket at `path` and serves until the process is killed.
pub fn serve(path: &Path) -> Result<(), Error> {
    let listener = bind(path)?;
    eprintln!("taskslash daemon listening on {}", path.display());
    let next_job = Arc::new(AtomicU64::new(1));
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let next_job = Arc::clone(&next_job);
                thread::spawn(move || serve_job(stream, &next_job));
            }
            Err(e) => eprintln!("taskslash: accept failed: {e}"),
        }
    }
    Ok(())
}

/// Binds `path`, creating its parent with mode 0700 (the socket's permissions are the
/// authentication). A socket file nobody answers on is stale and is replaced; anything else at
/// `path` is left alone.
fn bind(path: &Path) -> Result<UnixListener, Error> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .map_err(|e| Error::new(format!("cannot create {}: {e}", parent.display())))?;
    }
    if let Ok(meta) = fs::symlink_metadata(path) {
        if !meta.file_type().is_socket() {
            return Err(Error::new(format!(
                "cannot bind {}: not a socket",
                path.display()
            )));
        }
        if UnixStream::connect(path).is_ok() {
            return Err(Error::new(format!(
                "a daemon is already running on {}",
                path.display()
            )));
        }
        fs::remove_file(path)
            .map_err(|e| Error::new(format!("cannot remove stale {}: {e}", path.display())))?;
    }
    UnixListener::bind(path).map_err(|e| Error::new(format!("cannot bind {}: {e}", path.display())))
}

/// One connection: `submit`, `start`, then `exited` and `recorded`. An EOF or a bad message after
/// `start` ends the job as dropped.
fn serve_job(stream: UnixStream, next_job: &AtomicU64) {
    let Ok(read_half) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(read_half);
    let mut writer = stream;
    let argv = match receive(&mut reader) {
        // A connection that closes without asking is a probe (the stale check, a readiness wait).
        Ok(None) => return,
        Ok(Some(ClientMessage::Submit { argv, .. })) if !argv.is_empty() => argv,
        Ok(Some(ClientMessage::Submit { .. })) => {
            return refuse(&mut writer, "submit needs a non-empty argv");
        }
        Ok(Some(ClientMessage::Exited { .. })) => {
            return refuse(&mut writer, "expected submit, got exited");
        }
        Err(e) => return refuse(&mut writer, &format!("bad request: {e}")),
    };
    let job = next_job.fetch_add(1, Ordering::Relaxed);
    eprintln!("job {job} started: {}", argv.join(" "));
    let start = DaemonMessage::Start {
        job,
        unit: format!("taskslash-job-{job}"),
    };
    if send(&mut writer, &start).is_err() {
        eprintln!("job {job} dropped");
        return;
    }
    match receive(&mut reader) {
        Ok(Some(ClientMessage::Exited { job: id, code })) if id == job => {
            eprintln!("job {job} exited {code}");
            // The client may already be gone; the job is recorded either way.
            let _ = send(&mut writer, &DaemonMessage::Recorded { job });
        }
        _ => eprintln!("job {job} dropped"),
    }
}

fn refuse(writer: &mut UnixStream, message: &str) {
    let _ = send(
        writer,
        &DaemonMessage::Error {
            message: message.to_owned(),
        },
    );
}
