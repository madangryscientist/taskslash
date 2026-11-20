//! The admission rule (spike 5.1), a pure function: may a waiting job start now, and if not, the
//! first thing that holds it. Pressure gates first, then the never-seen rule, then the three resources.
//! The order jobs are asked in, and the starvation guard, are `order`'s.

use crate::Error;
use crate::sampler::MachineSnapshot;
use serde::{Deserialize, Serialize};

/// An amount of each resource: CPU in threads, memory in MiB, disk writes in MiB/s.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Use {
    pub cpu: f64,
    pub mem_mib: f64,
    pub write_mib_s: f64,
}

impl Use {
    fn get(&self, resource: Resource) -> f64 {
        match resource {
            Resource::Cpu => self.cpu,
            Resource::Memory => self.mem_mib,
            Resource::Write => self.write_mib_s,
        }
    }
}

/// A running managed job, as admission counts it.
#[derive(Debug, Clone, PartialEq)]
pub struct Running {
    pub job: u64,
    pub key: String,
    pub estimate: Use,
    /// What it uses now (CPU and writes over the last 5 s, memory as its peak so far); `None` on
    /// the plain launch path, where it counts at its estimate.
    pub observed: Option<Use>,
    /// Started while its key was never seen: the never-seen rule counts it.
    pub never_seen: bool,
}

impl Running {
    /// `max(estimate, observed)` of `resource`.
    pub fn holds(&self, resource: Resource) -> f64 {
        let observed = self.observed.map_or(0.0, |o| o.get(resource));
        self.estimate.get(resource).max(observed)
    }
}

/// The job asking to start.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Demand {
    pub estimate: Use,
    /// Its key never finished a run and the caller did not give both `--cpu` and `--mem`.
    pub never_seen: bool,
}

/// The share of the machine jobs may commit, and the disk write budget.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    pub cpu_share: f64,
    pub mem_share: f64,
    pub write_mib_s: f64,
}

impl Limits {
    /// The defaults, each overridden by `TASKSLASH_CPU_SHARE`, `TASKSLASH_MEM_SHARE` or
    /// `TASKSLASH_WRITE_MIB_S` when set. A value that is not a positive number is an error.
    pub fn from_env() -> Result<Self, Error> {
        let mut limits = Self::default();
        for (name, field) in [
            ("TASKSLASH_CPU_SHARE", &mut limits.cpu_share),
            ("TASKSLASH_MEM_SHARE", &mut limits.mem_share),
            ("TASKSLASH_WRITE_MIB_S", &mut limits.write_mib_s),
        ] {
            let Some(text) = std::env::var_os(name).filter(|v| !v.is_empty()) else {
                continue;
            };
            let text = text.to_string_lossy();
            *field = text
                .parse()
                .ok()
                .filter(|v: &f64| v.is_finite() && *v > 0.0)
                .ok_or_else(|| Error::new(format!("{name}={text} is not a positive number")))?;
        }
        Ok(limits)
    }
}

impl Limits {
    /// What jobs may commit on `machine`: the CPU and memory shares of it, and the write budget.
    pub fn budget(&self, machine: &MachineSnapshot) -> Use {
        Use {
            cpu: self.cpu_share * machine.cpu.threads as f64,
            mem_mib: self.mem_share * machine.memory.total_mib as f64,
            write_mib_s: self.write_mib_s,
        }
    }
}

impl Default for Limits {
    /// The spike's starting values, tuned in the proof run.
    fn default() -> Self {
        Self {
            cpu_share: 0.85,
            mem_share: 0.80,
            write_mib_s: 150.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Start,
    Wait(Reason),
}

/// Why a job waits: the first thing that blocked it, pressure gate before never-seen before resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reason {
    /// A pressure or space gate holds: `value` is past `limit` (both in percent).
    Gate {
        gate: Gate,
        value: f64,
        limit: f64,
        /// The mount, for the `mount` gate.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mount: Option<String>,
    },
    /// Another job whose key was never seen is running.
    NeverSeen { job: u64, key: String },
    /// `resource` has `free` left of its budget and the job `needs` more.
    Resource {
        resource: Resource,
        needs: f64,
        free: f64,
        held_by: Vec<Held>,
    },
    /// Job `job` is ahead in line, does not fit, and has waited too long to be passed.
    Behind { job: u64, key: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gate {
    CpuPressure,
    MemoryPressure,
    IoFullPressure,
    IoPressure,
    MemoryAvailable,
    Mount,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resource {
    Cpu,
    Memory,
    Write,
}

const RESOURCES: [Resource; 3] = [Resource::Cpu, Resource::Memory, Resource::Write];

/// A running job's hold on a resource.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Held {
    pub job: u64,
    pub key: String,
    pub amount: f64,
}

/// Whether `candidate` may start now on `machine`, beside `running` and `unmanaged` use.
pub fn check(
    machine: &MachineSnapshot,
    unmanaged: &Use,
    running: &[Running],
    candidate: &Demand,
    limits: &Limits,
) -> Decision {
    if let Some(gate) = gate(machine) {
        return Decision::Wait(gate);
    }
    if candidate.never_seen
        && let Some(other) = running.iter().find(|r| r.never_seen)
    {
        return Decision::Wait(Reason::NeverSeen {
            job: other.job,
            key: other.key.clone(),
        });
    }
    let budget = limits.budget(machine);
    let too_big = RESOURCES
        .iter()
        .any(|&r| candidate.estimate.get(r) > budget.get(r));
    if too_big && running.is_empty() {
        return Decision::Start;
    }
    for resource in RESOURCES {
        let committed = committed(running).get(resource) + unmanaged.get(resource);
        let needs = candidate.estimate.get(resource);
        if committed + needs > budget.get(resource) {
            let mut held_by: Vec<Held> = running
                .iter()
                .map(|r| Held {
                    job: r.job,
                    key: r.key.clone(),
                    amount: r.holds(resource),
                })
                .filter(|h| h.amount > 0.0)
                .collect();
            held_by.sort_by(|a, b| b.amount.total_cmp(&a.amount).then(a.job.cmp(&b.job)));
            return Decision::Wait(Reason::Resource {
                resource,
                needs,
                free: (budget.get(resource) - committed).max(0.0),
                held_by,
            });
        }
    }
    Decision::Start
}

/// The first pressure gate that holds, if any.
fn gate(m: &MachineSnapshot) -> Option<Reason> {
    gates(m).into_iter().next()
}

/// Every pressure gate that holds, in the order `check` tries them.
pub(crate) fn gates(m: &MachineSnapshot) -> Vec<Reason> {
    let over = |gate, value: f64, limit| {
        (value > limit).then_some(Reason::Gate {
            gate,
            value,
            limit,
            mount: None,
        })
    };
    let available = if m.memory.total_mib == 0 {
        100.0
    } else {
        100.0 * m.memory.available_mib as f64 / m.memory.total_mib as f64
    };
    let mut held: Vec<Reason> = [
        over(Gate::CpuPressure, m.cpu.pressure.some_avg10, 40.0),
        over(Gate::MemoryPressure, m.memory.pressure.some_avg10, 5.0),
        over(Gate::IoFullPressure, m.io.pressure.full_avg10, 5.0),
        over(Gate::IoPressure, m.io.pressure.some_avg10, 25.0),
        // Under the limit holds here, so the comparison is turned around.
        (available < 10.0).then_some(Reason::Gate {
            gate: Gate::MemoryAvailable,
            value: available,
            limit: 10.0,
            mount: None,
        }),
    ]
    .into_iter()
    .flatten()
    .collect();
    held.extend(
        m.mounts
            .iter()
            .find(|mount| mount.used > 0.9)
            .map(|mount| Reason::Gate {
                gate: Gate::Mount,
                value: mount.used * 100.0,
                limit: 90.0,
                mount: Some(mount.path.clone()),
            }),
    );
    held
}

/// What the running jobs hold, per resource: the sum of `max(estimate, observed)`.
fn committed(running: &[Running]) -> Use {
    let sum = |resource| running.iter().map(|r| r.holds(resource)).sum();
    Use {
        cpu: sum(Resource::Cpu),
        mem_mib: sum(Resource::Memory),
        write_mib_s: sum(Resource::Write),
    }
}

/// What the machine shows in use minus `managed`, what the managed jobs use now, per resource,
/// floored at 0.
pub fn unmanaged(machine: &MachineSnapshot, managed: &Use) -> Use {
    let shown = Use {
        cpu: machine.cpu.busy * machine.cpu.threads as f64,
        mem_mib: machine
            .memory
            .total_mib
            .saturating_sub(machine.memory.available_mib) as f64,
        write_mib_s: machine.io.disks.iter().map(|d| d.write_mib_s).sum(),
    };
    Use {
        cpu: (shown.cpu - managed.cpu).max(0.0),
        mem_mib: (shown.mem_mib - managed.mem_mib).max(0.0),
        write_mib_s: (shown.write_mib_s - managed.write_mib_s).max(0.0),
    }
}

mod text;
pub use text::{Waiting, ordinal, waiting_line};

#[cfg(test)]
pub(crate) mod tests;
