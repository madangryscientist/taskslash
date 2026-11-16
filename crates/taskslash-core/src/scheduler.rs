//! The queue of waiting jobs and the set of running ones. `admit` is the one place a waiting job
//! starts: it walks the queue in `order` and starts every job `admission::check` lets start,
//! counting each one it starts against the jobs after it.

use crate::admission::{self, Decision, Demand, Limits, Reason, Use};
use crate::estimate::Estimate;
use crate::measure::{Live, LiveReading};
use crate::protocol::Kind;
use crate::sampler::MachineSnapshot;
use crate::store::RunPressure;
use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Instant, SystemTime};

/// A submitted job: what admission knows about it.
pub struct Job {
    pub id: u64,
    pub key: String,
    pub kind: Kind,
    pub estimate: Estimate,
    /// The caller gave both `--cpu` and `--mem`: never treated as never seen.
    pub vouched: bool,
}

/// How many machine samples the unmanaged figure is the mean of.
const UNMANAGED_SAMPLES: usize = 30;

struct Waiting {
    job: Job,
    queued_at: Instant,
    /// Why it is still waiting, from the last `admit`.
    reason: Option<Reason>,
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

pub struct Scheduler {
    queue: Vec<Waiting>,
    running: BTreeMap<u64, Running>,
    limits: Limits,
    /// The latest machine sample.
    machine: MachineSnapshot,
    /// The unmanaged use of the newest samples, newest last.
    unmanaged: VecDeque<Use>,
}

/// Where a job is, as `why` answers.
pub enum Whereabouts {
    /// Waiting: its reason, seconds waited and 1-based place in the queue.
    Waiting(Reason, u64, usize),
    Running,
    NotFound,
}

impl Scheduler {
    pub fn new(limits: Limits, machine: MachineSnapshot) -> Self {
        Self {
            queue: Vec::new(),
            running: BTreeMap::new(),
            limits,
            machine,
            unmanaged: VecDeque::new(),
        }
    }

    /// Queues `job`; `admit` sends on `start` when it may run.
    pub fn submit(&mut self, job: Job, start: Sender<()>) {
        self.queue.push(Waiting {
            job,
            queued_at: Instant::now(),
            reason: None,
            start,
        });
    }

    /// Starts every waiting job, in `order`, that `admission::check` allows, each against the
    /// running set as it grows. `seen` says whether a key has a finished run.
    pub fn admit(&mut self, seen: impl Fn(&str) -> bool) {
        let mut held: Vec<admission::Running> = self
            .running
            .values()
            .map(|r| admission::Running {
                job: r.job.id,
                key: r.job.key.clone(),
                estimate: reserved(&r.job.estimate),
                observed: r.live.as_ref().and_then(|l| *lock(l)).map(observed),
                never_seen: r.unknown,
            })
            .collect();
        let unmanaged = self.unmanaged();
        for id in self.order() {
            let Some(at) = self.queue.iter().position(|w| w.job.id == id) else {
                continue;
            };
            let job = &self.queue[at].job;
            let unknown = !job.vouched && !seen(&job.key);
            let demand = Demand {
                estimate: reserved(&job.estimate),
                never_seen: unknown,
            };
            let decision =
                admission::check(&self.machine, &unmanaged, &held, &demand, &self.limits);
            if let Decision::Wait(reason) = decision {
                self.queue[at].reason = Some(reason);
                continue;
            }
            held.push(admission::Running {
                job: id,
                key: job.key.clone(),
                estimate: demand.estimate,
                observed: None,
                never_seen: unknown,
            });
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

    /// Why job `id` waits, how long it has, and its 1-based place in the queue; `None` when it
    /// is not waiting.
    pub fn waiting(&self, id: u64) -> Option<(Reason, u64, usize)> {
        let at = self.queue.iter().position(|w| w.job.id == id)?;
        let w = &self.queue[at];
        let reason = w.reason.clone()?;
        Some((reason, w.queued_at.elapsed().as_secs(), at + 1))
    }

    /// Where job `id` is.
    pub fn whereabouts(&self, id: u64) -> Whereabouts {
        if self.running.contains_key(&id) {
            return Whereabouts::Running;
        }
        match self.waiting(id) {
            Some((reason, waited_s, position)) => Whereabouts::Waiting(reason, waited_s, position),
            None => Whereabouts::NotFound,
        }
    }

    /// The mean unmanaged use over the newest samples; nothing before the first.
    fn unmanaged(&self) -> Use {
        let n = self.unmanaged.len().max(1) as f64;
        let sum = |f: fn(&Use) -> f64| self.unmanaged.iter().map(f).sum::<f64>() / n;
        Use {
            cpu: sum(|u| u.cpu),
            mem_mib: sum(|u| u.mem_mib),
            write_mib_s: sum(|u| u.write_mib_s),
        }
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

    /// Takes a machine sample: the snapshot admission reads, one more unmanaged figure, and one
    /// more reading of every running job's pressure.
    pub fn sample(&mut self, machine: &MachineSnapshot) {
        let mut managed = Use::default();
        for live in self
            .running
            .values()
            .filter_map(|r| *lock(r.live.as_ref()?))
        {
            managed.cpu += live.cpu_threads;
            managed.mem_mib += live.mem_mib as f64;
            managed.write_mib_s += live.write_mib_s;
        }
        if self.unmanaged.len() == UNMANAGED_SAMPLES {
            self.unmanaged.pop_front();
        }
        self.unmanaged
            .push_back(admission::unmanaged(machine, &managed));
        self.machine = machine.clone();
        for running in self.running.values_mut() {
            let p = &mut running.pressure;
            p.cpu += machine.cpu.pressure.some_avg10;
            p.mem += machine.memory.pressure.some_avg10;
            p.io += machine.io.pressure.some_avg10;
            p.samples += 1;
        }
    }
}

/// What admission reserves for an estimate.
fn reserved(e: &Estimate) -> Use {
    Use {
        cpu: e.cpu_threads,
        mem_mib: e.mem_mib as f64,
        write_mib_s: e.write_mib_s,
    }
}

/// What admission counts a live reading as: memory at its peak so far.
fn observed(live: Live) -> Use {
    Use {
        cpu: live.cpu_threads,
        mem_mib: live.peak_mem_mib as f64,
        write_mib_s: live.write_mib_s,
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

mod status;
#[cfg(test)]
mod tests;
