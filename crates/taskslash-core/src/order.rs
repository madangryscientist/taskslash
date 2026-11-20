//! The order the waiting jobs are considered in, and the pass over them (spike 5.2), as pure
//! functions of the waiting jobs, the running ones, the machine and a monotonic `now`.
//!
//! Highest effective class first; within a class, the submitter holding the smallest dominant
//! share of the budget first (Dominant Resource Fairness); then the job that has waited longest,
//! then the lower id. The pass starts each job that fits and walks past one that does not
//! (backfill), unless that one has waited more than 5 minutes: then nothing behind it starts.
//! When no managed job runs, the first job the walk reaches that has waited 3 minutes on a CPU or
//! I/O pressure gate or on the budget starts anyway (the wait cap); a pressure gate stops the walk at the
//! first job in line, so under one only that job can be capped. Memory and mount gates never are.

use crate::admission::{self, Decision, Demand, Gate, Limits, Reason, Resource, Use};
use crate::sampler::MachineSnapshot;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;
use std::time::{Duration, Instant};

/// How long a job waits before it moves up one class, and again for each further step.
const RAISE_EVERY: Duration = Duration::from_secs(15 * 60);
/// How long a job waits before it stops later jobs from passing it.
pub(crate) const GUARD_AFTER: Duration = Duration::from_secs(5 * 60);
/// How long a job waits on load TaskSlash does not manage (a CPU or I/O pressure gate, or a
/// resource budget nothing managed holds) before it starts anyway, when no managed job runs.
pub(crate) const WAIT_CAP: Duration = Duration::from_secs(3 * 60);

/// A job's priority class, highest first.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    Urgent,
    Flake,
    Interactive,
    #[default]
    Normal,
    Background,
}

impl Class {
    const ALL: [Self; 5] = [
        Self::Urgent,
        Self::Flake,
        Self::Interactive,
        Self::Normal,
        Self::Background,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Urgent => "urgent",
            Self::Flake => "flake",
            Self::Interactive => "interactive",
            Self::Normal => "normal",
            Self::Background => "background",
        }
    }

    /// The `CPUWeight` a scoped job of this class runs with.
    pub fn cpu_weight(self) -> u32 {
        match self {
            Self::Urgent => 400,
            Self::Flake => 300,
            Self::Interactive => 200,
            Self::Normal => 100,
            Self::Background => 20,
        }
    }

    /// The class one step higher; `Urgent` stays.
    fn up(self) -> Self {
        match self {
            Self::Urgent | Self::Flake => Self::Urgent,
            Self::Interactive => Self::Flake,
            Self::Normal => Self::Interactive,
            Self::Background => Self::Normal,
        }
    }
}

impl fmt::Display for Class {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Class {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|class| class.as_str() == s)
            .ok_or_else(|| {
                format!(
                    "unknown class {s:?}: one of urgent, flake, interactive, normal, background"
                )
            })
    }
}

/// `class` raised one step for every whole `RAISE_EVERY` past the first that the job has waited
/// more than: over 15 minutes is one step, over 30 two, never past `Urgent`.
pub fn effective(class: Class, waited: Duration) -> Class {
    let mut class = class;
    let mut past = RAISE_EVERY;
    while waited > past && class != Class::Urgent {
        class = class.up();
        past += RAISE_EVERY;
    }
    class
}

/// A waiting job, as the pass sees it.
#[derive(Debug, Clone)]
pub struct Queued {
    pub id: u64,
    pub key: String,
    pub class: Class,
    pub submitter: String,
    pub submitted_at: Instant,
    pub demand: Demand,
}

/// A running job and who submitted it.
#[derive(Debug, Clone)]
pub struct Active {
    pub held: admission::Running,
    pub submitter: String,
}

/// What a pass decided: the jobs to start, in the order they start, and every job left waiting
/// with its 1-based place in line and why it waits. `capped` names the started jobs that the
/// wait cap let past what still held them, and that reason.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pass {
    pub start: Vec<u64>,
    pub waiting: Vec<(u64, usize, Reason)>,
    pub capped: Vec<(u64, Reason)>,
}

/// The jobs a pass starts, in order. See `pass`.
pub fn plan(
    waiting: &[Queued],
    running: &[Active],
    machine: &MachineSnapshot,
    unmanaged: &Use,
    limits: &Limits,
    now: Instant,
) -> Vec<u64> {
    pass(waiting, running, machine, unmanaged, limits, now).start
}

/// One pass: repeatedly takes the first job in order (shares as they stand, counting the jobs this
/// pass has started), asks `admission::check` against the running set as it grows, and starts it
/// or passes over it. A pressure gate stops the walk for every job. A job that does not fit (a resource
/// holds it, not the never-seen rule) and has waited more than `GUARD_AFTER` stops it too, and
/// every job behind it waits on it. With nothing managed running, a job held past `WAIT_CAP` by
/// a reason `cappable` allows starts anyway, and is reported in `capped`.
pub fn pass(
    waiting: &[Queued],
    running: &[Active],
    machine: &MachineSnapshot,
    unmanaged: &Use,
    limits: &Limits,
    now: Instant,
) -> Pass {
    let budget = limits.budget(machine);
    let mut held: Vec<admission::Running> = running.iter().map(|a| a.held.clone()).collect();
    let mut holds: HashMap<&str, Use> = HashMap::new();
    for a in running {
        add(holds.entry(&a.submitter).or_default(), &a.held);
    }
    let mut left: Vec<&Queued> = waiting.iter().collect();
    let mut out = Pass::default();
    let mut place = 0;
    while !left.is_empty() {
        let next = first(&left, &holds, &budget, now);
        let job = left.remove(next);
        let decision = admission::check(machine, unmanaged, &held, &job.demand, limits);
        let reason = match decision {
            Decision::Start => None,
            Decision::Wait(reason) => Some(reason),
        };
        let capped = reason.as_ref().is_some_and(|reason| {
            held.is_empty()
                && now.saturating_duration_since(job.submitted_at) >= WAIT_CAP
                && cappable(reason)
                && admission::gates(machine).iter().all(cappable)
        });
        let reason = match reason {
            Some(reason) if capped => {
                out.capped.push((job.id, reason));
                None
            }
            reason => reason,
        };
        let Some(reason) = reason else {
            let started = admission::Running {
                job: job.id,
                key: job.key.clone(),
                estimate: job.demand.estimate,
                observed: None,
                never_seen: job.demand.never_seen,
            };
            add(holds.entry(&job.submitter).or_default(), &started);
            held.push(started);
            out.start.push(job.id);
            continue;
        };
        place += 1;
        let gate = matches!(reason, Reason::Gate { .. });
        // Only room can come free for it; holding others back helps no other wait.
        let guard = matches!(reason, Reason::Resource { .. })
            && now.saturating_duration_since(job.submitted_at) > GUARD_AFTER;
        out.waiting.push((job.id, place, reason.clone()));
        if gate || guard {
            let behind = if gate {
                reason
            } else {
                Reason::Behind {
                    job: job.id,
                    key: job.key.clone(),
                }
            };
            while !left.is_empty() {
                let next = first(&left, &holds, &budget, now);
                place += 1;
                out.waiting
                    .push((left.remove(next).id, place, behind.clone()));
            }
        }
    }
    out
}

/// Whether the wait cap may start a job past `reason`: load TaskSlash does not manage, which a
/// wait cannot be shown to end. Memory and mount gates guard against harm, and are never capped.
fn cappable(reason: &Reason) -> bool {
    match reason {
        Reason::Resource { .. } => true,
        Reason::Gate { gate, .. } => matches!(
            gate,
            Gate::CpuPressure | Gate::IoPressure | Gate::IoFullPressure
        ),
        Reason::NeverSeen { .. } | Reason::Behind { .. } => false,
    }
}

/// The index in `left` of the job that goes first.
fn first(left: &[&Queued], holds: &HashMap<&str, Use>, budget: &Use, now: Instant) -> usize {
    let share = |submitter: &str| holds.get(submitter).map_or(0.0, |h| dominant(h, budget));
    let class = |q: &Queued| effective(q.class, now.saturating_duration_since(q.submitted_at));
    let mut best = 0;
    for at in 1..left.len() {
        let (a, b) = (left[at], left[best]);
        let earlier = class(a)
            .cmp(&class(b))
            .then(share(&a.submitter).total_cmp(&share(&b.submitter)))
            .then(a.submitted_at.cmp(&b.submitted_at))
            .then(a.id.cmp(&b.id))
            .is_lt();
        if earlier {
            best = at;
        }
    }
    best
}

/// Adds what `job` holds, `max(estimate, observed)` per resource, to `sum`.
fn add(sum: &mut Use, job: &admission::Running) {
    sum.cpu += job.holds(Resource::Cpu);
    sum.mem_mib += job.holds(Resource::Memory);
    sum.write_mib_s += job.holds(Resource::Write);
}

/// The largest of a submitter's three shares of the budget.
fn dominant(holds: &Use, budget: &Use) -> f64 {
    let share = |held: f64, budget: f64| if budget > 0.0 { held / budget } else { 0.0 };
    share(holds.cpu, budget.cpu)
        .max(share(holds.mem_mib, budget.mem_mib))
        .max(share(holds.write_mib_s, budget.write_mib_s))
}

#[cfg(test)]
mod tests;
