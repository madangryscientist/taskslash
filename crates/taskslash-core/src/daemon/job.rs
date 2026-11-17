//! One job's connection: queued, started, measured and recorded.

use super::Daemon;
use super::ended::Ended;
use crate::admission::{Reason, ordinal};
use crate::estimate::{Source, estimate};
use crate::logs;
use crate::measure::{self, Cost, Sampler};
use crate::order::WAIT_CAP;
use crate::procfs::rfc3339;
use crate::protocol::{Class, ClientMessage, DaemonMessage, JobEvent, Kind, Launch, receive, send};
use crate::scheduler::Job;
use crate::store::{self, Finished};
use std::io::BufReader;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::process;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime};

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
        // The daemon's pid keeps unit names apart across restarts and daemons on other sockets.
        let unit = format!("taskslash-{}-{id}", process::id());
        let log = logs::log_path(&self.logs, &unit);
        let key = store::key(name, argv);
        self.events.publish(JobEvent::Queued {
            job: id,
            key: key.clone(),
            kind,
            class,
            submitter: submitter.to_owned(),
            cwd: cwd.to_owned(),
            argv: argv.to_vec(),
            at: now(),
        });
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
            log: log.clone(),
        };
        let (start, started) = mpsc::channel();
        let waiting = {
            let mut scheduler = self.scheduler();
            scheduler.submit(job, start);
            self.admit_locked(&mut scheduler);
            scheduler.waiting(id)
        };
        // Started in the pass above: `admit` already sent its `Admitted`. A waiting job takes it in
        // the loop below, which a pass on another thread can send to at any moment.
        let mut admitted = if waiting.is_none() {
            started.try_recv().ok()
        } else {
            None
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
                    Ok(sent) => {
                        admitted = Some(sent);
                        break;
                    }
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
                            self.never_started(id, &key);
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        self.never_started(id, &key);
                        return;
                    }
                }
            }
        }

        let capped = admitted.and_then(|a| a.capped);
        eprintln!("{}", started_line(id, class, argv, capped.as_ref()));
        let sampler = match self.launch {
            Launch::Scope => measure::cgroup_dir(&unit).map(Sampler::start),
            Launch::Plain => None,
        };
        if let Some(sampler) = &sampler {
            self.scheduler().set_live(id, sampler.live());
        }
        let log = log.to_string_lossy().into_owned();
        self.events.publish(JobEvent::Started {
            job: id,
            unit: unit.clone(),
            launch: self.launch,
            log: log.clone(),
            at: now(),
        });
        let start = DaemonMessage::Start {
            job: id,
            unit,
            launch: self.launch,
            log: log.clone(),
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
        // Remembered as ending while the scheduler is still locked, so `logs <n>` always finds it.
        let finished = {
            let mut scheduler = self.scheduler();
            let finished = scheduler.finish(id);
            if finished.is_some() {
                self.job_ended(id, Ended::Recording);
            }
            finished
        };
        let Some(running) = finished else {
            self.never_started(id, &key);
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
            moon_hash: moon_hash.clone(),
            log_path: Some(&log),
        };
        let recorded = self.store().record(&finished);
        self.job_ended(
            id,
            recorded
                .as_ref()
                .map_or(Ended::Unrecorded, |&run| Ended::Run(run)),
        );
        self.events.publish(JobEvent::Finished {
            job: id,
            key: key.clone(),
            exit_code,
            wall_ms,
            cached,
            moon_hash,
            run: recorded.as_ref().ok().copied(),
            log: Some(log.clone()),
            at: now(),
        });
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

impl Daemon {
    /// The `finished` of a job that leaves before it runs, so every `queued` has one.
    fn never_started(&self, id: u64, key: &str) {
        self.job_ended(id, Ended::NoRun);
        self.events.publish(JobEvent::Finished {
            job: id,
            key: key.to_owned(),
            exit_code: None,
            wall_ms: 0,
            cached: None,
            moon_hash: None,
            run: None,
            log: None,
            at: now(),
        });
    }
}

/// The daemon's line for a job that starts, which says so when the wait cap let it past what
/// still held it.
fn started_line(id: u64, class: Class, argv: &[String], capped: Option<&Reason>) -> String {
    let argv = argv.join(" ");
    match capped {
        None => format!("job {id} started ({class}): {argv}"),
        Some(reason) => format!(
            "job {id} started ({class}) past its {} min wait cap: {reason}: {argv}",
            WAIT_CAP.as_secs() / 60
        ),
    }
}

fn now() -> String {
    rfc3339(SystemTime::now())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admission::Gate;

    #[test]
    fn the_start_line_says_when_a_job_started_past_its_wait_cap_and_why() {
        let argv = ["cargo".to_owned(), "test".to_owned()];
        assert_eq!(
            started_line(12, Class::Normal, &argv, None),
            "job 12 started (normal): cargo test"
        );
        let held = Reason::Gate {
            gate: Gate::CpuPressure,
            value: 52.0,
            limit: 40.0,
            mount: None,
        };
        assert_eq!(
            started_line(12, Class::Normal, &argv, Some(&held)),
            "job 12 started (normal) past its 3 min wait cap: \
             cpu pressure 52% over the 40% gate: cargo test"
        );
    }
}
