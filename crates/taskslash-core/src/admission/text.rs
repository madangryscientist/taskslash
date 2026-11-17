//! The words: the sentence a `Reason` reads as, and the waiting line.

use super::{Gate, Reason, Resource};
use crate::order::GUARD_AFTER;
use std::fmt;

/// How many holders a resource reason names, the biggest first; the rest are counted.
const HOLDERS_NAMED: usize = 3;

/// The plain sentence `taskslash why`, the waiting line and `status` print.
impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gate {
                gate,
                value,
                limit,
                mount,
            } => {
                let what = match gate {
                    Gate::CpuPressure => "cpu pressure",
                    Gate::MemoryPressure => "memory pressure",
                    Gate::IoFullPressure => "io full pressure",
                    Gate::IoPressure => "io pressure",
                    Gate::MemoryAvailable => {
                        return write!(
                            f,
                            "memory available {value:.0}% under the {limit:.0}% gate"
                        );
                    }
                    Gate::Mount => {
                        let path = mount.as_deref().unwrap_or("?");
                        return write!(f, "{path} {value:.0}% used, over the {limit:.0}% gate");
                    }
                };
                write!(f, "{what} {value:.0}% over the {limit:.0}% gate")
            }
            Self::NeverSeen { job, key } => {
                write!(f, "another never-seen job is running: {key} (job {job})")
            }
            Self::Behind { job, key } => write!(
                f,
                "behind {key} (job {job}), which has waited over {} minutes and is not passed",
                GUARD_AFTER.as_secs() / 60
            ),
            Self::Resource {
                resource,
                needs,
                free,
                held_by,
            } => {
                let (name, unit) = match resource {
                    Resource::Cpu => ("cpu", ""),
                    Resource::Memory => ("memory", " MiB"),
                    Resource::Write => ("write", " MiB/s"),
                };
                let n = |v: f64| match resource {
                    Resource::Memory => format!("{v:.0}{unit}"),
                    _ => format!("{v:.1}{unit}"),
                };
                write!(f, "{name} needs {}, free {}; held by ", n(*needs), n(*free))?;
                if held_by.is_empty() {
                    return f.write_str("work taskslash does not manage");
                }
                // The biggest holders explain the wait; the rest are only counted.
                let names: Vec<String> = held_by
                    .iter()
                    .take(HOLDERS_NAMED)
                    .map(|h| format!("{} (job {})", h.key, h.job))
                    .collect();
                f.write_str(&names.join(", "))?;
                match held_by.len().saturating_sub(HOLDERS_NAMED) {
                    0 => Ok(()),
                    more => write!(f, " and {more} more"),
                }
            }
        }
    }
}

/// `1st`, `2nd`, `3rd`, `4th`, ..., `11th`, `12th`, `13th`, `21st`.
pub fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{n}{suffix}")
}

/// One waiting line, as the daemon reports it.
#[derive(Debug, Clone, Copy)]
pub struct Waiting<'a> {
    /// The job's number; `None` from a daemon older than it, and the line then leaves it out.
    pub job: Option<u64>,
    pub reason: &'a Reason,
    pub waited_s: u64,
    pub position: usize,
    /// The threads its kind is assumed to take, when the hint that its key never ran is due.
    pub never_run: Option<f64>,
}

/// The stderr line a waiting `taskslash run` prints:
/// `taskslash: job 18 waiting 0s (1st in line): <reason>`, and the never-run hint when it is due.
pub fn waiting_line(w: &Waiting<'_>) -> String {
    let job = w.job.map_or(String::new(), |job| format!("job {job} "));
    let hint = w.never_run.map_or(String::new(), |threads| {
        format!(" (never run before: assumed {threads:.1} threads; --cpu 0.1 says otherwise)")
    });
    format!(
        "taskslash: {job}waiting {}s ({} in line): {}{hint}",
        w.waited_s,
        ordinal(w.position),
        w.reason
    )
}
