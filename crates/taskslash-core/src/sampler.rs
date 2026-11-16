//! The machine sampler: what the whole machine is doing, read once a second by the daemon and
//! answered to `taskslash status`. Behind a trait so admission can be tested against a fake.

use crate::procfs::{self, CpuTimes, DiskCounters, Pressure};
use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// How often the watched mounts' free space is read (the rest is read every sample).
const MOUNTS_EVERY: Duration = Duration::from_secs(30);
const MIB: f64 = (1u64 << 20) as f64;

/// Something that can say what the machine is doing now.
pub trait MachineSampler: Send {
    fn sample(&mut self) -> MachineSnapshot;
}

/// One reading of the machine. A share is 0 to 1; a pressure is the kernel's percent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineSnapshot {
    /// RFC 3339, UTC.
    pub sampled_at: String,
    pub cpu: Cpu,
    pub memory: Memory,
    pub io: Io,
    pub mounts: Vec<Mount>,
    pub load: [f64; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cpu {
    /// The CPUs this daemon may run on (its `Cpus_allowed_list`): the budget jobs can use.
    pub threads: usize,
    /// Every CPU the kernel has online.
    pub threads_online: usize,
    /// The busy share of the `threads` CPUs since the last sample.
    pub busy: f64,
    pub pressure: Pressure,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Memory {
    pub total_mib: u64,
    pub available_mib: u64,
    pub swap_total_mib: u64,
    pub swap_free_mib: u64,
    pub pressure: Pressure,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Io {
    pub pressure: Pressure,
    pub disks: Vec<Disk>,
}

/// A physical disk, since the last sample.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Disk {
    pub name: String,
    /// The share of the time the device had I/O in flight.
    pub busy: f64,
    pub write_mib_s: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mount {
    pub path: String,
    pub total_mib: u64,
    /// What an unprivileged user can still write.
    pub free_mib: u64,
    pub used: f64,
}

/// The real sampler, over `/proc`, `/sys/block` and `statvfs`.
pub struct ProcSampler {
    allowed: Vec<usize>,
    disks: Vec<String>,
    mount_paths: Vec<PathBuf>,
    previous: Option<(Instant, Vec<CpuTimes>, Vec<DiskCounters>)>,
    mounts: Vec<Mount>,
    mounts_read: Option<Instant>,
}

impl ProcSampler {
    /// Reads which CPUs this process may use, which disks are physical, and which mounts to watch
    /// (`$HOME`, `$TMPDIR` and `/tmp`, one per file system).
    pub fn new() -> Self {
        let allowed = procfs::allowed_cpus(&read("/proc/self/status"));
        let mut disks: Vec<String> = fs::read_dir("/sys/block")
            .map(|dir| {
                dir.filter_map(|e| e.ok()?.file_name().into_string().ok())
                    .filter(|name| procfs::is_physical_disk(name))
                    .collect()
            })
            .unwrap_or_default();
        disks.sort();
        let wanted = ["HOME", "TMPDIR"]
            .iter()
            .filter_map(|name| std::env::var_os(name).filter(|v| !v.is_empty()))
            .map(PathBuf::from)
            .chain([PathBuf::from("/tmp")]);
        Self {
            allowed,
            disks,
            mount_paths: one_per_device(wanted),
            previous: None,
            mounts: Vec::new(),
            mounts_read: None,
        }
    }
}

impl Default for ProcSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl MachineSampler for ProcSampler {
    fn sample(&mut self) -> MachineSnapshot {
        let now = Instant::now();
        let times = procfs::cpu_times(&read("/proc/stat"));
        let counters = procfs::diskstats(&read("/proc/diskstats"), &self.disks);
        // The allowed list can name CPUs that are possible but not online; only online ones count.
        // A process with no affinity line is allowed every online CPU.
        let allowed: Vec<usize> = times
            .iter()
            .map(|t| t.cpu)
            .filter(|cpu| self.allowed.is_empty() || self.allowed.contains(cpu))
            .collect();
        let (busy, disks) = match &self.previous {
            Some((then, old_times, old_counters)) => {
                let secs = now.duration_since(*then).as_secs_f64();
                let busy = procfs::busy_share(old_times, &times, &allowed);
                (busy, disk_rates(old_counters, &counters, secs))
            }
            None => (0.0, disk_rates(&counters, &counters, 0.0)),
        };
        if self
            .mounts_read
            .is_none_or(|at| now.duration_since(at) >= MOUNTS_EVERY)
        {
            self.mounts = self.mount_paths.iter().filter_map(|p| statvfs(p)).collect();
            self.mounts_read = Some(now);
        }
        let memory = procfs::meminfo(&read("/proc/meminfo"));
        let pressure = |name: &str| procfs::pressure(&read(&format!("/proc/pressure/{name}")));
        let snapshot = MachineSnapshot {
            sampled_at: procfs::rfc3339(SystemTime::now()),
            cpu: Cpu {
                threads: allowed.len(),
                threads_online: times.len(),
                busy: round(busy, 3),
                pressure: pressure("cpu"),
            },
            memory: Memory {
                total_mib: memory.total_mib,
                available_mib: memory.available_mib,
                swap_total_mib: memory.swap_total_mib,
                swap_free_mib: memory.swap_free_mib,
                pressure: pressure("memory"),
            },
            io: Io {
                pressure: pressure("io"),
                disks,
            },
            mounts: self.mounts.clone(),
            load: procfs::loadavg(&read("/proc/loadavg")),
        };
        self.previous = Some((now, times, counters));
        snapshot
    }
}

/// Each disk's busy share and write rate over `secs` seconds; 0 for a disk with no earlier reading
/// or when no time passed.
fn disk_rates(before: &[DiskCounters], after: &[DiskCounters], secs: f64) -> Vec<Disk> {
    after
        .iter()
        .map(|now| {
            let then = before.iter().find(|c| c.name == now.name);
            let (busy, write_mib_s) = match then {
                Some(then) if secs > 0.0 => {
                    let ticks = now.io_ticks_ms.saturating_sub(then.io_ticks_ms) as f64;
                    let sectors = now.sectors_written.saturating_sub(then.sectors_written) as f64;
                    (
                        (ticks / (secs * 1000.0)).clamp(0.0, 1.0),
                        sectors * 512.0 / MIB / secs,
                    )
                }
                _ => (0.0, 0.0),
            };
            Disk {
                name: now.name.clone(),
                busy: round(busy, 3),
                write_mib_s: round(write_mib_s, 2),
            }
        })
        .collect()
}

/// The paths that exist, keeping the first of those on one device.
fn one_per_device(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut seen = Vec::new();
    let mut kept = Vec::new();
    for path in paths {
        if let Ok(meta) = fs::metadata(&path)
            && !seen.contains(&meta.dev())
        {
            seen.push(meta.dev());
            kept.push(path);
        }
    }
    kept
}

/// The size and free space of the file system holding `path`. `None` when it cannot be read.
fn statvfs(path: &Path) -> Option<Mount> {
    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: an all zero `statvfs` is a valid value of the plain C struct, `c_path` is a valid
    // NUL terminated string, and `buf` outlives the call.
    let mut buf: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &raw mut buf) } != 0 {
        return None;
    }
    let block = buf.f_frsize;
    let total = buf.f_blocks * block;
    let free = buf.f_bavail * block;
    let used = if total == 0 {
        0.0
    } else {
        1.0 - free as f64 / total as f64
    };
    Some(Mount {
        path: path.to_string_lossy().into_owned(),
        total_mib: total >> 20,
        free_mib: free >> 20,
        used: round(used.clamp(0.0, 1.0), 3),
    })
}

fn read(path: &str) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn round(value: f64, places: i32) -> f64 {
    let scale = 10f64.powi(places);
    (value * scale).round() / scale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_sample_has_no_rates() {
        let mut sampler = ProcSampler::new();
        let first = sampler.sample();
        assert_eq!(first.cpu.busy, 0.0);
        assert!(
            first
                .io
                .disks
                .iter()
                .all(|d| d.busy == 0.0 && d.write_mib_s == 0.0)
        );
    }

    #[test]
    fn one_mount_is_kept_per_device_first_path_first() {
        let dir = std::env::temp_dir();
        let kept = one_per_device([dir.clone(), dir.join("."), PathBuf::from("/no/such/path")]);
        assert_eq!(kept, [dir]);
    }
}
