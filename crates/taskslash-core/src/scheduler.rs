//! The queue of waiting jobs and the set of running ones. `admit` is the one place a waiting job
//! starts: it runs `order::pass` over them and starts the jobs it picks. Between passes the queue
//! is kept in the pass's order, so a job's index is its place in line.

use crate::admission::{self, Demand, Limits, Reason, Use};
use crate::estimate::Estimate;
use crate::measure::{Live, LiveReading};
use crate::order;
use crate::order::{Active, Class, Queued};
use crate::protocol::{JobState, Kind};
use crate::sampler::MachineSnapshot;
use crate::store::RunPressure;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;
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
    pub class: Class,
    /// The worktree (or directory) it was submitted from, which fair share is counted by.
    pub submitter: String,
    /// The file its client copies its output into.
    pub log: PathBuf,
}

/// How many machine samples the unmanaged figure is the mean of.
const UNMANAGED_SAMPLES: usize = 30;

struct Waiting {
    job: Job,
    submitted_at: Instant,
    /// Why it is still waiting, from the last `admit`.
    reason: Option<Reason>,
    /// The connection thread waits on the other end for its start.
    start: Sender<Admitted>,
}

/// What `admit` sends a job's connection thread when the job starts.
#[derive(Debug, Clone, PartialEq)]
pub struct Admitted {
    /// The reason that still held it, when it started past its wait cap.
    pub capped: Option<Reason>,
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
    pub fn submit(&mut self, job: Job, start: Sender<Admitted>) {
        self.queue.push(Waiting {
            job,
            submitted_at: Instant::now(),
            reason: None,
            start,
        });
    }

    /// Starts every waiting job `order::pass` lets start, as of now, and puts the rest in its
    /// order with their reasons. `seen` says whether a key has a finished run.
    pub fn admit(&mut self, seen: impl Fn(&str) -> bool) {
        let now = Instant::now();
        let running: Vec<Active> = self
            .running
            .values()
            .map(|r| Active {
                held: self.held_by(r),
                submitter: r.job.submitter.clone(),
            })
            .collect();
        let queued: Vec<Queued> = self
            .queue
            .iter()
            .map(|w| Queued {
                id: w.job.id,
                key: w.job.key.clone(),
                class: w.job.class,
                submitter: w.job.submitter.clone(),
                submitted_at: w.submitted_at,
                demand: Demand {
                    estimate: reserved(&w.job.estimate),
                    never_seen: !w.job.vouched && !seen(&w.job.key),
                },
            })
            .collect();
        let pass = order::pass(
            &queued,
            &running,
            &self.machine,
            &self.unmanaged(),
            &self.limits,
            now,
        );
        let mut queue: BTreeMap<u64, Waiting> =
            self.queue.drain(..).map(|w| (w.job.id, w)).collect();
        for id in pass.start {
            let Some(waiting) = queue.remove(&id) else {
                continue;
            };
            let unknown = queued.iter().any(|q| q.id == id && q.demand.never_seen);
            // The connection thread is gone only if it panicked; nothing then runs the job.
            let capped = pass
                .capped
                .iter()
                .find(|(capped, _)| *capped == id)
                .map(|(_, reason)| reason.clone());
            if waiting.start.send(Admitted { capped }).is_ok() {
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
        for (id, _, reason) in pass.waiting {
            if let Some(mut waiting) = queue.remove(&id) {
                waiting.reason = Some(reason);
                self.queue.push(waiting);
            }
        }
    }

    /// Why job `id` waits, how long it has, and its 1-based place in the queue; `None` when it
    /// is not waiting.
    pub fn waiting(&self, id: u64) -> Option<(Reason, u64, usize)> {
        let at = self.queue.iter().position(|w| w.job.id == id)?;
        let w = &self.queue[at];
        let reason = w.reason.clone()?;
        Some((reason, w.submitted_at.elapsed().as_secs(), at + 1))
    }

    /// Where job `id` is and, when it waits, why, how long and its place in line.
    pub fn whereabouts(&self, id: u64) -> (JobState, Option<(Reason, u64, usize)>) {
        if self.running.contains_key(&id) {
            return (JobState::Running, None);
        }
        match self.waiting(id) {
            Some(waiting) => (JobState::Waiting, Some(waiting)),
            None => (JobState::NotFound, None),
        }
    }

    /// A running job as admission counts it.
    fn held_by(&self, r: &Running) -> admission::Running {
        admission::Running {
            job: r.job.id,
            key: r.job.key.clone(),
            estimate: reserved(&r.job.estimate),
            observed: r.live.as_ref().and_then(|l| *lock(l)).map(observed),
            never_seen: r.unknown,
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

    /// The log of job `id` and whether it runs (else it waits); `None` when it is not held.
    pub fn log(&self, id: u64) -> Option<(PathBuf, bool)> {
        if let Some(running) = self.running.get(&id) {
            return Some((running.job.log.clone(), true));
        }
        self.queue
            .iter()
            .find(|w| w.job.id == id)
            .map(|w| (w.job.log.clone(), false))
    }

    /// The running job of `key` that started last: its number and start.
    pub fn last_running(&self, key: &str) -> Option<(u64, SystemTime)> {
        self.running
            .values()
            .filter(|r| r.job.key == key)
            .max_by_key(|r| (r.started_at, r.job.id))
            .map(|r| (r.job.id, r.started_at))
    }

    /// The logs of the running jobs, which retention never deletes.
    pub fn running_logs(&self) -> HashSet<PathBuf> {
        self.running.values().map(|r| r.job.log.clone()).collect()
    }

    /// Ends job `id`, returning it.
    pub fn finish(&mut self, id: u64) -> Option<Running> {
        self.running.remove(&id)
    }

    /// Takes a machine sample: the snapshot admission reads, one more unmanaged figure, and one
    /// more reading of every running job's pressure.
    /// Unmanaged is the machine minus what the running jobs use now: a measured job's live
    /// reading (memory as current, not peak), and an unmeasured job's estimate, since its real use
    /// is in the machine's figure and admission already counts it at its estimate.
    pub fn sample(&mut self, machine: &MachineSnapshot) {
        let mut managed = Use::default();
        for r in self.running.values() {
            let now = match r.live.as_ref().and_then(|l| *lock(l)) {
                Some(live) => Use {
                    cpu: live.cpu_threads,
                    mem_mib: live.mem_mib as f64,
                    write_mib_s: live.write_mib_s,
                },
                None => reserved(&r.job.estimate),
            };
            managed.cpu += now.cpu;
            managed.mem_mib += now.mem_mib;
            managed.write_mib_s += now.write_mib_s;
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
