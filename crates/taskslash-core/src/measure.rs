//! Measuring a scoped job from its cgroup while it runs: CPU time, the peak CPU over 5 s, peak
//! memory and bytes written.
//!
//! The user manager delegates `cpu` and `memory`, not `io`, so bytes written come from
//! `/proc/<pid>/io` of each process in the cgroup.

use crate::launch::SLICE;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How often a running job is read.
const INTERVAL: Duration = Duration::from_millis(250);
/// How long the cgroup may take to appear after `start`.
const APPEAR: Duration = Duration::from_secs(2);
/// How often to look for it meanwhile.
const APPEAR_POLL: Duration = Duration::from_millis(20);
/// The window of the peak CPU figure, in seconds.
const PEAK_WINDOW: f64 = 5.0;

/// What a job cost. A field is `None` when it was never read (the job ended before its cgroup was
/// seen, or the file never gave a good value), never 0 for unknown.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Cost {
    pub cpu_usec: Option<u64>,
    pub peak_cpu_5s: Option<f64>,
    pub peak_mem_bytes: Option<u64>,
    pub written_bytes: Option<u64>,
}

/// The cgroup directory of the scope `<unit>.scope` under `taskslash.slice`, for this user. `None`
/// when the uid cannot be read.
pub fn cgroup_dir(unit: &str) -> Option<PathBuf> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let uid: u32 = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some(PathBuf::from(format!(
        "/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service/{SLICE}/{unit}.scope"
    )))
}

/// A thread reading one job's cgroup every 250 ms until `finish`.
pub struct Sampler {
    stop: Sender<()>,
    thread: JoinHandle<Cost>,
}

impl Sampler {
    pub fn start(dir: PathBuf) -> Self {
        let (stop, stopped) = mpsc::channel();
        let thread = thread::spawn(move || {
            let t0 = Instant::now();
            let mut readings = Readings::default();
            let mut seen = false;
            let mut stopping = false;
            loop {
                seen = seen || dir.is_dir();
                // Once seen, a missing directory means the job's last process has exited.
                if seen && !readings.take(&dir, t0.elapsed().as_secs_f64()) {
                    break;
                }
                if stopping || (!seen && t0.elapsed() >= APPEAR) {
                    break;
                }
                let wait = if seen { INTERVAL } else { APPEAR_POLL };
                if stopped.recv_timeout(wait) != Err(RecvTimeoutError::Timeout) {
                    // One more reading on the way out, in case the cgroup is still there.
                    stopping = true;
                }
            }
            readings.cost()
        });
        Self { stop, thread }
    }

    /// Stops sampling and returns the last good readings.
    pub fn finish(self) -> Cost {
        drop(self.stop);
        self.thread.join().unwrap_or_default()
    }
}

/// The last good value of each file, kept because the files vanish with the job's last process.
#[derive(Default)]
struct Readings {
    /// `(seconds since sampling began, usage_usec)`, starting at `(0, 0)`: the cgroup is created
    /// empty after sampling begins.
    cpu: Vec<(f64, u64)>,
    peak_mem: Option<u64>,
    written: Option<u64>,
}

impl Readings {
    /// Reads every file once. False when the cgroup directory is gone.
    fn take(&mut self, dir: &Path, t: f64) -> bool {
        if let Some(usage) = read(&dir.join("cpu.stat")).and_then(|s| field(&s, "usage_usec")) {
            if self.cpu.is_empty() {
                self.cpu.push((0.0, 0));
            }
            self.cpu.push((t, usage));
        }
        if let Some(peak) = read(&dir.join("memory.peak")).and_then(|s| s.trim().parse().ok()) {
            self.peak_mem = Some(peak);
        }
        if let Some(procs) = read(&dir.join("cgroup.procs")) {
            let alive: BTreeMap<u32, u64> = procs
                .lines()
                .filter_map(|pid| pid.trim().parse::<u32>().ok())
                .filter_map(|pid| {
                    let io = read(Path::new(&format!("/proc/{pid}/io")))?;
                    Some((pid, field(&io, "write_bytes:")?))
                })
                .collect();
            self.written = self.written.max(written_bytes([&alive]));
        }
        dir.is_dir()
    }

    fn cost(&self) -> Cost {
        Cost {
            cpu_usec: self.cpu.last().map(|&(_, usage)| usage),
            peak_cpu_5s: peak_cpu(&self.cpu, PEAK_WINDOW),
            peak_mem_bytes: self.peak_mem,
            written_bytes: self.written,
        }
    }
}

fn read(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

/// The number after `name` on the line that starts with it (`usage_usec 12`, `write_bytes: 12`).
fn field(text: &str, name: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix(name))
        .and_then(|rest| rest.trim().parse().ok())
}

/// The most CPU used over any `window` seconds, in threads, from `(t seconds, usage_usec)` samples
/// in time order. Each sample is paired with the latest one at least `window` before it (or the
/// first, for a job shorter than that), and the usage between them is divided by the larger of
/// their distance and `window`: the job used nothing before it began.
pub fn peak_cpu(series: &[(f64, u64)], window: f64) -> Option<f64> {
    if series.len() < 2 {
        return None;
    }
    let mut peak: f64 = 0.0;
    let mut j = 0;
    for (i, &(t, usage)) in series.iter().enumerate().skip(1) {
        while j + 1 < i && series[j + 1].0 <= t - window {
            j += 1;
        }
        let (t0, usage0) = series[j];
        let used = usage.saturating_sub(usage0) as f64 / 1e6;
        peak = peak.max(used / (t - t0).max(window));
    }
    Some(peak)
}

/// The bytes a job wrote, from per-sample `{pid: write_bytes}` maps of the processes alive at each
/// sample: the largest sum at one sample. A process's counter includes children it has reaped, so
/// summing each pid's last value would count a reaped child's writes twice.
pub fn written_bytes<'a>(samples: impl IntoIterator<Item = &'a BTreeMap<u32, u64>>) -> Option<u64> {
    samples.into_iter().map(|alive| alive.values().sum()).max()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1 << 20;

    #[test]
    fn peak_cpu_is_the_busiest_5_s() {
        // 1 thread for 4 s, 3 threads for 6 s, then idle for 4 s; a sample every second.
        let mut series = vec![(0.0, 0)];
        let mut usage = 0;
        for t in 1..=14 {
            usage += match t {
                1..=4 => 1_000_000,
                5..=10 => 3_000_000,
                _ => 0,
            };
            series.push((f64::from(t), usage));
        }
        let peak = peak_cpu(&series, 5.0).unwrap();
        assert!((peak - 3.0).abs() < 1e-9, "{peak}");
    }

    #[test]
    fn a_job_shorter_than_the_window_is_averaged_over_the_window() {
        // 2 threads for 2.5 s: 5 CPU seconds in any 5 s window is 1 thread.
        let series = [(0.0, 0), (1.0, 2_000_000), (2.5, 5_000_000)];
        let peak = peak_cpu(&series, 5.0).unwrap();
        assert!((peak - 1.0).abs() < 1e-9, "{peak}");
    }

    #[test]
    fn peak_cpu_needs_two_samples() {
        assert_eq!(peak_cpu(&[], 5.0), None);
        assert_eq!(peak_cpu(&[(0.0, 0)], 5.0), None);
    }

    #[test]
    fn bytes_written_is_the_largest_alive_sum_not_the_sum_of_lasts() {
        // pid 10 runs the job; its child 11 writes 200 MiB and is reaped, and 10's counter then
        // includes the child's 200 MiB. 10 writes 4 MiB of its own at the end.
        let samples: Vec<BTreeMap<u32, u64>> = vec![
            BTreeMap::from([(10, 0), (11, 0)]),
            BTreeMap::from([(10, 0), (11, 120 * MIB)]),
            BTreeMap::from([(10, 0), (11, 200 * MIB)]),
            BTreeMap::from([(10, 200 * MIB)]),
            BTreeMap::from([(10, 204 * MIB)]),
        ];
        // The sum of each pid's last value would say 404 MiB.
        assert_eq!(written_bytes(&samples), Some(204 * MIB));
        assert_eq!(written_bytes(&[]), None);
    }

    #[test]
    fn a_cgroup_never_seen_costs_nulls() {
        let dir = std::env::temp_dir().join("taskslash-no-such-cgroup.scope");
        let sampler = Sampler::start(dir);
        assert_eq!(sampler.finish(), Cost::default());
    }
}
