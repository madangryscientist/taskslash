//! What a key will cost, from its recorded runs (spike 4.4): memory from the worst recent run, CPU
//! from a typical calm one, and each figure with no run to feed it from the job's kind instead.

use crate::protocol::Kind;
use serde::{Deserialize, Serialize};

const MIB: i64 = 1 << 20;
/// A run taken while the machine's CPU `some avg10` was above this is starved (the gate of 5.1).
const CALM_CPU_SOME: f64 = 40.0;
/// How many measured runs the memory figure looks at.
const MEM_RUNS: usize = 5;
/// How many runs the CPU, I/O and duration figures look at.
const RUNS: usize = 10;

/// What the estimate reads of one recorded run. `None` is not measured.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunCost {
    pub wall_ms: i64,
    pub cpu_usec: Option<i64>,
    pub peak_cpu_5s: Option<f64>,
    pub peak_mem_bytes: Option<i64>,
    pub written_bytes: Option<i64>,
    pub exit_code: Option<i32>,
    /// The mean of the machine's CPU `some avg10` over the run; `None` when no sample fell inside.
    pub machine_cpu_some: Option<f64>,
}

impl RunCost {
    /// Measured: CPU seconds, peak memory and an exit code were all recorded.
    fn measured(&self) -> bool {
        self.cpu_usec.is_some() && self.peak_mem_bytes.is_some() && self.exit_code.is_some()
    }

    /// Calm: measured, and the machine's CPU pressure during it is known and at most the gate.
    fn calm(&self) -> bool {
        self.measured() && self.machine_cpu_some.is_some_and(|p| p <= CALM_CPU_SOME)
    }

    /// The wall time in seconds, never 0 (so a rate over it is finite).
    fn wall_s(&self) -> f64 {
        self.wall_ms.max(1) as f64 / 1000.0
    }
}

/// Where a figure came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    History,
    Default,
    /// The caller gave it (`run --cpu`, `--mem`).
    Given,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Samples {
    /// Measured runs among those given.
    pub measured: usize,
    /// Calm runs among those given.
    pub calm: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sources {
    pub cpu: Source,
    pub mem: Source,
    pub io: Source,
    pub duration: Source,
}

/// A key's expected cost: what admission reserves for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Estimate {
    /// Mean CPU over the job's life, in threads.
    pub cpu_threads: f64,
    /// The most CPU a calm run used over any 5 s, in threads; `None` from a default.
    pub cpu_peak_5s: Option<f64>,
    pub mem_mib: u64,
    pub write_mib_s: f64,
    /// `None` when no calm run gives one; there is no default.
    pub duration_s: Option<f64>,
    pub samples: Samples,
    pub source: Sources,
}

/// The estimate from `runs`, a key's recorded runs newest first, falling back to `kind`'s defaults.
pub fn estimate(runs: &[RunCost], kind: Kind) -> Estimate {
    let measured: Vec<&RunCost> = runs.iter().filter(|r| r.measured()).collect();
    let calm: Vec<&RunCost> = measured
        .iter()
        .copied()
        .filter(|r| r.calm())
        .take(RUNS)
        .collect();
    let source = |history: bool| {
        if history {
            Source::History
        } else {
            Source::Default
        }
    };

    // Memory: the worst of the newest 5 measured runs, plus 20%, rounded up to a whole MiB.
    let peak_mem = measured
        .iter()
        .take(MEM_RUNS)
        .filter_map(|r| r.peak_mem_bytes)
        .max();
    let mem_mib = peak_mem.map_or(kind.default_mem_mib(), |bytes| {
        (u64::try_from(bytes).unwrap_or(0) * 6).div_ceil(5 << 20)
    });

    // CPU: the nearest rank 75th percentile of mean threads over the newest 10 calm runs.
    let mut threads: Vec<f64> = calm
        .iter()
        .map(|r| r.cpu_usec.unwrap_or(0) as f64 / (r.wall_ms.max(1) as f64 * 1000.0))
        .collect();
    threads.sort_by(f64::total_cmp);
    let cpu_threads = match threads.len() {
        0 => kind.default_cpu_threads(),
        n => round(threads[(0.75 * n as f64).ceil() as usize - 1], 2),
    };
    let cpu_peak_5s = calm
        .iter()
        .filter_map(|r| r.peak_cpu_5s)
        .reduce(f64::max)
        .map(|p| round(p, 2));

    // I/O: the mean write rate over the newest 10 measured runs that recorded one.
    let rates: Vec<f64> = measured
        .iter()
        .take(RUNS)
        .filter_map(|r| r.written_bytes.map(|w| w as f64 / MIB as f64 / r.wall_s()))
        .collect();
    let write_mib_s = if rates.is_empty() {
        0.0
    } else {
        round(rates.iter().sum::<f64>() / rates.len() as f64, 2)
    };

    // Duration: the median wall time of the newest 10 calm runs.
    let mut walls: Vec<i64> = calm.iter().map(|r| r.wall_ms).collect();
    walls.sort_unstable();
    let duration_s = match walls.len() {
        0 => None,
        n if n % 2 == 1 => Some(walls[n / 2] as f64 / 1000.0),
        n => Some((walls[n / 2 - 1] + walls[n / 2]) as f64 / 2000.0),
    };

    Estimate {
        cpu_threads,
        cpu_peak_5s,
        mem_mib,
        write_mib_s,
        duration_s,
        samples: Samples {
            measured: measured.len(),
            calm: runs.iter().filter(|r| r.calm()).count(),
        },
        source: Sources {
            cpu: source(!threads.is_empty()),
            mem: source(peak_mem.is_some()),
            io: source(!rates.is_empty()),
            duration: source(duration_s.is_some()),
        },
    }
}

fn round(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

#[cfg(test)]
mod tests;
