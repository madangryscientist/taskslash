//! Where job logs live and how long the daemon keeps them. The daemon names each job's log and
//! deletes old ones; the client writes them.

use crate::Error;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs::{self, DirBuilder};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How long an ended job's log is kept, by default.
const KEEP_DAYS: f64 = 7.0;
/// How large the directory may grow, by default, in MiB.
const KEEP_MIB: u64 = 1024;
const DAY: f64 = 86_400.0;

/// `$TASKSLASH_LOGS` when set, else `logs` beside the database at `db`.
pub fn logs_dir(db: &Path) -> PathBuf {
    match env::var_os("TASKSLASH_LOGS").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => db.parent().unwrap_or_else(|| Path::new(".")).join("logs"),
    }
}

/// Creates `dir` (and its parents) with mode 0700 when missing: logs hold whatever a command
/// printed. A directory that is already there (`TASKSLASH_LOGS` may name one of yours) is left as
/// it is.
pub fn create_dir(dir: &Path) -> Result<(), Error> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| Error::new(format!("cannot create {}: {e}", dir.display())))
}

/// The log of the job running as systemd unit `unit`. The unit name holds the daemon's pid, so it
/// is unique across daemon restarts, which a job number alone is not.
pub fn log_path(dir: &Path, unit: &str) -> PathBuf {
    dir.join(format!("{unit}.log"))
}

/// How long ended jobs' logs are kept and how large the directory may grow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Retention {
    pub max_age: Duration,
    pub max_bytes: u64,
}

impl Retention {
    /// 7 days and 1 GiB, or `TASKSLASH_LOG_DAYS` (a number of days, 0 or more) and
    /// `TASKSLASH_LOG_MIB` (a whole number of MiB).
    pub fn from_env() -> Result<Self, Error> {
        let var = |name: &str| env::var(name).ok().filter(|v| !v.is_empty());
        let days = match var("TASKSLASH_LOG_DAYS") {
            Some(text) => text
                .parse::<f64>()
                .ok()
                .filter(|d| d.is_finite() && *d >= 0.0)
                .ok_or_else(|| {
                    Error::usage(format!(
                        "TASKSLASH_LOG_DAYS: {text} is not a number of days"
                    ))
                })?,
            None => KEEP_DAYS,
        };
        let mib = match var("TASKSLASH_LOG_MIB") {
            Some(text) => text.parse::<u64>().map_err(|_| {
                Error::usage(format!(
                    "TASKSLASH_LOG_MIB: {text} is not a whole number of MiB"
                ))
            })?,
            None => KEEP_MIB,
        };
        Ok(Self {
            max_age: Duration::from_secs_f64(days * DAY),
            max_bytes: mib.saturating_mul(1 << 20),
        })
    }
}

/// A file in the log directory: its size, and when its job ended (`None` while it runs).
#[derive(Debug, Clone, PartialEq)]
pub struct LogFile {
    pub path: PathBuf,
    pub bytes: u64,
    pub ended: Option<SystemTime>,
}

/// The logs to delete as of `now`: every ended one older than `keep.max_age`, then the oldest
/// ended ones until the rest fit in `keep.max_bytes`. Never a running job's.
pub fn expired(files: &[LogFile], now: SystemTime, keep: &Retention) -> Vec<PathBuf> {
    let mut ended: Vec<(&LogFile, SystemTime)> = files
        .iter()
        .filter_map(|f| f.ended.map(|at| (f, at)))
        .collect();
    ended.sort_by_key(|&(f, at)| (at, f.path.clone()));
    let mut total: u64 = files.iter().map(|f| f.bytes).sum();
    let mut gone = Vec::new();
    for (file, at) in ended {
        let old = now.duration_since(at).unwrap_or_default() > keep.max_age;
        if old || total > keep.max_bytes {
            total -= file.bytes;
            gone.push(file.path.clone());
        }
    }
    gone
}

/// Deletes the expired logs in `dir`. A file is running when it is in `running`; else it
/// ended when `ended` says (from its run's row), or at its last write when no row names it.
/// Returns the deleted paths.
pub fn sweep(
    dir: &Path,
    running: &HashSet<PathBuf>,
    ended: &HashMap<PathBuf, SystemTime>,
    keep: &Retention,
    now: SystemTime,
) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let files: Vec<LogFile> = entries
        .flatten()
        .filter(|e| is_log_name(&e.file_name().to_string_lossy()))
        .filter_map(|e| {
            let meta = e.metadata().ok().filter(fs::Metadata::is_file)?;
            let path = e.path();
            let at = if running.contains(&path) {
                None
            } else {
                Some(
                    ended
                        .get(&path)
                        .copied()
                        .or_else(|| meta.modified().ok())
                        .unwrap_or(now),
                )
            };
            Some(LogFile {
                bytes: meta.len(),
                ended: at,
                path,
            })
        })
        .collect();
    expired(&files, now, keep)
        .into_iter()
        .filter(|path| match fs::remove_file(path) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("taskslash: cannot delete {}: {e}", path.display());
                false
            }
        })
        .collect()
}

/// Whether `name` is a log `log_path` names: `taskslash-<...>.log`. Retention touches nothing else.
fn is_log_name(name: &str) -> bool {
    name.starts_with("taskslash-") && name.ends_with(".log")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const HOUR: Duration = Duration::from_secs(3600);

    fn file(name: &str, bytes: u64, ended_hours_ago: Option<u64>, now: SystemTime) -> LogFile {
        LogFile {
            path: PathBuf::from(name),
            bytes,
            ended: ended_hours_ago.map(|h| now - HOUR * u32::try_from(h).unwrap()),
        }
    }

    #[test]
    fn an_ended_log_past_the_age_limit_goes_and_a_running_one_stays() {
        let now = SystemTime::now();
        let keep = Retention {
            max_age: HOUR * 24,
            max_bytes: 1000,
        };
        let files = [
            file("old.log", 10, Some(25), now),
            file("new.log", 10, Some(23), now),
            file("running.log", 10, None, now),
        ];
        assert_eq!(expired(&files, now, &keep), [PathBuf::from("old.log")]);
    }

    #[test]
    fn the_oldest_ended_logs_go_until_the_rest_fit() {
        let now = SystemTime::now();
        let keep = Retention {
            max_age: HOUR * 1000,
            max_bytes: 250,
        };
        // 400 bytes in all: dropping the two oldest ended (100 + 100) leaves 200.
        let files = [
            file("running.log", 150, None, now),
            file("c.log", 50, Some(1), now),
            file("a.log", 100, Some(3), now),
            file("b.log", 100, Some(2), now),
        ];
        assert_eq!(
            expired(&files, now, &keep),
            [PathBuf::from("a.log"), PathBuf::from("b.log")]
        );
    }

    #[test]
    fn a_running_log_stays_even_over_the_size_limit() {
        let now = SystemTime::now();
        let keep = Retention {
            max_age: HOUR,
            max_bytes: 10,
        };
        let files = [file("running.log", 500, None, now)];
        assert!(expired(&files, now, &keep).is_empty());
    }

    #[test]
    fn sweep_deletes_from_the_directory_and_keeps_running_and_other_files() {
        let dir = env::temp_dir().join(format!("tks-logs-{}", std::process::id()));
        create_dir(&dir).unwrap();
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let names = [
            "taskslash-1-1.log",
            "taskslash-1-2.log",
            "taskslash-1-3.log",
            "build.log",
            "notes.txt",
        ];
        for name in names {
            fs::write(dir.join(name), "x").unwrap();
        }
        let now = SystemTime::now();
        // Job 1 ended long ago, job 2 runs, job 3 has no row, so its last write counts (now).
        let running = HashSet::from([dir.join("taskslash-1-2.log")]);
        let ended = HashMap::from([
            (dir.join("taskslash-1-1.log"), now - HOUR * 200),
            (dir.join("taskslash-1-2.log"), now - HOUR * 200),
        ]);
        // Another tool's old log in the same directory is not ours to delete.
        let old = fs::File::options()
            .write(true)
            .open(dir.join("build.log"))
            .unwrap();
        old.set_modified(now - HOUR * 1000).unwrap();
        let keep = Retention {
            max_age: HOUR * 168,
            max_bytes: 1 << 30,
        };
        let gone = sweep(&dir, &running, &ended, &keep, now);
        assert_eq!(gone, [dir.join("taskslash-1-1.log")]);
        let mut left: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "build.log",
                "notes.txt",
                "taskslash-1-2.log",
                "taskslash-1-3.log"
            ]
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
