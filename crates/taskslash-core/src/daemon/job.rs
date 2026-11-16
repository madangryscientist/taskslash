//! One job's connection: queued, started, measured and recorded.

use super::Daemon;
use crate::admission::ordinal;
use crate::estimate::{Source, estimate};
use crate::measure::{self, Cost, Sampler};
use crate::protocol::{Class, ClientMessage, DaemonMessage, Kind, Launch, receive, send};
use crate::scheduler::Job;
use crate::store::{self, Finished};
use std::io::BufReader;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::process;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

/// How often a waiting connection checks that its client is still there.
const WAIT_POLL: Duration = Duration::from_millis(100);
/// How often a waiting client is told again why.
const TELL_EVERY: Duration = Duration::from_secs(15);

/// A `submit`: the job's name, kind, command and directory, and what the caller said it costs
/// (`run --cpu`, `--mem`), each of which replaces the estimate's figure; its class and submitter.
pub(super) struct Submitted<'a> {
    pub name: Option<&'a str>,
    pub kind: Kind,
    pub argv: &'a [String],
    pub cwd: &'a str,
    pub cpu: Option<f64>,
    pub mem_mib: Option<u64>,
    pub class: Class,
    pub submitter: &'a str,
}

impl Daemon {
    /// A job: queued until `admit` starts it (`waiting` when queued and every 15 s), then
    /// `start`, sampling while it runs, and a row when `exited` arrives (and `recorded`) or when the
    /// connection drops (no exit code). A connection that drops while waiting leaves the queue.
    pub(super) fn serve_job(
        &self,
        reader: &mut BufReader<UnixStream>,
        writer: &mut UnixStream,
        submitted: Submitted<'_>,
    ) {
        let Submitted {
            name,
            kind,
            argv,
            cwd,
            cpu,
            mem_mib,
            class,
            submitter,
        } = submitted;
        let id = self.next_job.fetch_add(1, Ordering::Relaxed);
        let key = store::key(name, argv);
        let costs = self.store().costs(&key).unwrap_or_else(|e| {
            eprintln!("taskslash: job {id} estimated from defaults: {e}");
            Vec::new()
        });
        let mut estimate = estimate(&costs, kind);
        if let Some(cpu) = cpu {
            estimate.cpu_threads = cpu;
            estimate.source.cpu = Source::Given;
        }
        if let Some(mem_mib) = mem_mib {
            estimate.mem_mib = mem_mib;
            estimate.source.mem = Source::Given;
        }
        let job = Job {
            id,
            key: key.clone(),
            kind,
            estimate,
            vouched: cpu.is_some() && mem_mib.is_some(),
            class,
            submitter: submitter.to_owned(),
        };
        let (start, started) = mpsc::channel();
        let waiting = {
            let mut scheduler = self.scheduler();
            scheduler.submit(job, start);
            self.admit_locked(&mut scheduler);
            scheduler.waiting(id)
        };
        if let Some((reason, _, position)) = waiting {
            eprintln!("job {id} waiting ({}): {reason}", ordinal(position));
            let _ = send(
                writer,
                &DaemonMessage::Waiting {
                    reason,
                    waited_s: 0,
                    position,
                },
            );
            let mut told = Instant::now();
            loop {
                match started.recv_timeout(WAIT_POLL) {
                    Ok(()) => break,
                    Err(RecvTimeoutError::Timeout) if told.elapsed() >= TELL_EVERY => {
                        told = Instant::now();
                        let now = self.scheduler().waiting(id);
                        if let Some((reason, waited_s, position)) = now {
                            let _ = send(
                                writer,
                                &DaemonMessage::Waiting {
                                    reason,
                                    waited_s,
                                    position,
                                },
                            );
                        }
                    }
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
        eprintln!("job {id} started ({class}): {}", argv.join(" "));
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
        let (exit_code, cached, moon_hash) = match send(writer, &start)
            .ok()
            .and_then(|()| receive(reader).ok().flatten())
        {
            Some(ClientMessage::Exited {
                job,
                code,
                cached,
                moon_hash,
            }) if job == id => (Some(code), cached, moon_hash),
            _ => (None, None, None),
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
            class,
            submitter,
            launch: self.launch,
            started_at: running.started_at,
            wall_ms,
            cost,
            exit_code,
            pressure: running.pressure(),
            cached,
            moon_hash,
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
    // -1 (EINTR and the like) is not a hang up; the next poll asks again.
    ready > 0
}
