//! One job's connection: queued, started, measured and recorded.

use super::Daemon;
use crate::estimate::estimate;
use crate::measure::{self, Cost, Sampler};
use crate::protocol::{ClientMessage, DaemonMessage, Kind, Launch, receive, send};
use crate::scheduler::Job;
use crate::store::{self, Finished};
use std::io::BufReader;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::process;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

/// How often a waiting connection checks that its client is still there.
const WAIT_POLL: Duration = Duration::from_millis(100);

impl Daemon {
    /// A job: queued until `admit` starts it (`waiting` once if it does not start at once), then
    /// `start`, sampling while it runs, and a row when `exited` arrives (and `recorded`) or when the
    /// connection drops (no exit code). A connection that drops while waiting leaves the queue.
    pub(super) fn serve_job(
        &self,
        reader: &mut BufReader<UnixStream>,
        writer: &mut UnixStream,
        name: Option<&str>,
        kind: Kind,
        argv: &[String],
        cwd: &str,
    ) {
        let id = self.next_job.fetch_add(1, Ordering::Relaxed);
        let key = store::key(name, argv);
        let costs = self.store().costs(&key).unwrap_or_default();
        let job = Job {
            id,
            key: key.clone(),
            kind,
            estimate: estimate(&costs, kind),
        };
        let (start, started) = mpsc::channel();
        let waiting = {
            let mut scheduler = self.scheduler();
            scheduler.submit(job, start);
            self.admit_locked(&mut scheduler);
            scheduler.reason(id)
        };
        if let Some(reason) = waiting {
            eprintln!("job {id} waiting: {reason}");
            let _ = send(writer, &DaemonMessage::Waiting { reason });
            loop {
                match started.recv_timeout(WAIT_POLL) {
                    Ok(()) => break,
                    Err(RecvTimeoutError::Timeout) if hung_up(writer) => {
                        // Not on the queue any more means it started meanwhile: run it as dropped.
                        if self.scheduler().leave(id) {
                            eprintln!("job {id} left the queue");
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        }

        // The daemon's pid keeps unit names apart across restarts and daemons on other sockets.
        let unit = format!("taskslash-{}-{id}", process::id());
        eprintln!("job {id} started: {}", argv.join(" "));
        let sampler = match self.launch {
            Launch::Scope => measure::cgroup_dir(&unit).map(Sampler::start),
            Launch::Plain => None,
        };
        if let Some(sampler) = &sampler {
            self.scheduler().set_live(id, sampler.live());
        }
        let start = DaemonMessage::Start {
            job: id,
            unit,
            launch: self.launch,
        };
        let exit_code = if send(writer, &start).is_ok() {
            match receive(reader) {
                Ok(Some(ClientMessage::Exited { job, code })) if job == id => Some(code),
                _ => None,
            }
        } else {
            None
        };
        let Some(running) = self.scheduler().finish(id) else {
            return;
        };
        let wall_ms = u64::try_from(running.clock.elapsed().as_millis()).unwrap_or(u64::MAX);
        let cost = sampler.map(Sampler::finish).unwrap_or_else(Cost::default);
        match exit_code {
            Some(code) => eprintln!("job {id} exited {code}"),
            None => eprintln!("job {id} dropped"),
        }
        let finished = Finished {
            key: &key,
            kind,
            argv,
            cwd,
            launch: self.launch,
            started_at: running.started_at,
            wall_ms,
            cost,
            exit_code,
            pressure: running.pressure(),
        };
        let recorded = self.store().record(&finished);
        // After the row, so a job that finished counts as seen when the next one is considered.
        self.admit();
        match recorded {
            // The client may already be gone; the row is written either way.
            Ok(_) if exit_code.is_some() => {
                let _ = send(writer, &DaemonMessage::Recorded { job: id });
            }
            Ok(_) => {}
            Err(e) => eprintln!("taskslash: job {id} not recorded: {e}"),
        }
    }
}

/// Whether the client on `stream` hung up (or sent something, which a waiting client never does).
fn hung_up(stream: &UnixStream) -> bool {
    let mut fd = libc::pollfd {
        fd: stream.as_raw_fd(),
        events: libc::POLLIN | libc::POLLRDHUP,
        revents: 0,
    };
    // SAFETY: `fd` is one valid `pollfd` for the duration of the call, and the timeout is 0.
    let ready = unsafe { libc::poll(&raw mut fd, 1, 0) };
    ready != 0
}
