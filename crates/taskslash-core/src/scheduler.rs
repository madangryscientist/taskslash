//! The queue of waiting jobs and the set of running ones. `admit` is the one place a waiting job
//! starts: it walks the queue in `order` and starts every job `refusal` has nothing against.

use crate::estimate::Estimate;
use crate::measure::LiveReading;
use crate::procfs;
use crate::protocol::{JobStatus, Kind, WaitingStatus};
use crate::sampler::MachineSnapshot;
use crate::store::RunPressure;
use std::collections::BTreeMap;
use std::sync::mpsc::Sender;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Instant, SystemTime};

/// A submitted job: what admission knows about it.
pub struct Job {
    pub id: u64,
    pub key: String,
    pub kind: Kind,
    pub estimate: Estimate,
}

struct Waiting {
    job: Job,
    queued_at: Instant,
    /// Why it is still waiting, from the last `admit`.
    reason: String,
    /// The connection thread waits on the other end for its start.
    start: Sender<()>,
}

/// A job between its start and its end.
pub struct Running {
    pub job: Job,
    pub started_at: SystemTime,
    pub clock: Instant,
    /// Never seen when it started: the rule of one at a time counts it.
    unknown: bool,
    /// `None` on the plain launch path, where nothing measures the job.
    live: Option<LiveReading>,
    pressure: PressureSum,
}

/// The sums of the machine's `some avg10` pressures over the samples taken while a job ran.
#[derive(Default)]
struct PressureSum {
    cpu: f64,
    mem: f64,
    io: f64,
    samples: u32,
}

impl Running {
    /// The mean pressures over the job's samples; `None` each when no sample fell inside.
    pub fn pressure(&self) -> RunPressure {
        let p = &self.pressure;
        let mean = |sum: f64| (p.samples > 0).then(|| round(sum / f64::from(p.samples), 2));
        RunPressure {
            cpu: mean(p.cpu),
            mem: mean(p.mem),
            io: mean(p.io),
        }
    }
}

#[derive(Default)]
pub struct Scheduler {
    queue: Vec<Waiting>,
    running: BTreeMap<u64, Running>,
}

impl Scheduler {
    /// Queues `job`; `admit` sends on `start` when it may run.
    pub fn submit(&mut self, job: Job, start: Sender<()>) {
        self.queue.push(Waiting {
            job,
            queued_at: Instant::now(),
            reason: String::new(),
            start,
        });
    }

    /// Starts every waiting job, in `order`, that `refusal` allows. `seen` says whether a key has a
    /// finished run.
    pub fn admit(&mut self, seen: impl Fn(&str) -> bool) {
        for id in self.order() {
            let Some(at) = self.queue.iter().position(|w| w.job.id == id) else {
                continue;
            };
            let unknown = !seen(&self.queue[at].job.key);
            if let Some(reason) = self.refusal(unknown) {
                self.queue[at].reason = reason;
                continue;
            }
            let waiting = self.queue.remove(at);
            // The connection thread is gone only if it panicked; nothing then runs the job.
            if waiting.start.send(()).is_ok() {
                self.running.insert(
                    id,
                    Running {
                        job: waiting.job,
                        started_at: SystemTime::now(),
                        clock: Instant::now(),
                        unknown,
                        live: None,
                        pressure: PressureSum::default(),
                    },
                );
            }
        }
    }

    /// The order the queue is considered in: submit order.
    fn order(&self) -> Vec<u64> {
        self.queue.iter().map(|w| w.job.id).collect()
    }

    /// Why a job may not start now, or `None` when it may. `unknown` is whether its key was never
    /// seen. The only rule so far: at most one never-seen job runs at a time.
    fn refusal(&self, unknown: bool) -> Option<String> {
        if !unknown {
            return None;
        }
        self.running
            .values()
            .find(|r| r.unknown)
            .map(|r| format!("another never-seen job is running: {}", r.job.key))
    }

    /// Why job `id` is waiting, or `None` when it is not (it started).
    pub fn reason(&self, id: u64) -> Option<String> {
        self.queue
            .iter()
            .find(|w| w.job.id == id)
            .map(|w| w.reason.clone())
    }

    /// Takes job `id` off the queue. False when it is not there (it started).
    pub fn leave(&mut self, id: u64) -> bool {
        let before = self.queue.len();
        self.queue.retain(|w| w.job.id != id);
        self.queue.len() < before
    }

    pub fn set_live(&mut self, id: u64, live: LiveReading) {
        if let Some(running) = self.running.get_mut(&id) {
            running.live = Some(live);
        }
    }

    /// Ends job `id`, returning it.
    pub fn finish(&mut self, id: u64) -> Option<Running> {
        self.running.remove(&id)
    }

    /// Adds a machine sample to every running job's pressure.
    pub fn sample(&mut self, machine: &MachineSnapshot) {
        for running in self.running.values_mut() {
            let p = &mut running.pressure;
            p.cpu += machine.cpu.pressure.some_avg10;
            p.mem += machine.memory.pressure.some_avg10;
            p.io += machine.io.pressure.some_avg10;
            p.samples += 1;
        }
    }

    /// The running jobs, oldest first, and the waiting ones in queue order.
    pub fn status(&self) -> (Vec<JobStatus>, Vec<WaitingStatus>) {
        let jobs = self
            .running
            .values()
            .map(|r| {
                let live = r.live.as_ref().and_then(|live| *lock(live));
                JobStatus {
                    job: r.job.id,
                    key: r.job.key.clone(),
                    kind: r.job.kind,
                    started_at: procfs::rfc3339(r.started_at),
                    cpu_threads: live.map(|l| round(l.cpu_threads, 2)),
                    mem_mib: live.map(|l| l.mem_mib),
                    estimate: r.job.estimate.clone(),
                }
            })
            .collect();
        let waiting = self
            .queue
            .iter()
            .map(|w| WaitingStatus {
                job: w.job.id,
                key: w.job.key.clone(),
                kind: w.job.kind,
                waited_s: round(w.queued_at.elapsed().as_secs_f64(), 1),
                reason: w.reason.clone(),
                estimate: w.job.estimate.clone(),
            })
            .collect();
        (jobs, waiting)
    }
}

/// A panic while holding a lock leaves that connection, not the shared state, broken.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn round(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

#[cfg(test)]
mod tests;
