//! The words: the sentence a `Reason` reads as, and the waiting line.

use super::{Gate, Reason, Resource};
use std::fmt;

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
                let names: Vec<String> = held_by
                    .iter()
                    .map(|h| format!("{} (job {})", h.key, h.job))
                    .collect();
                f.write_str(&names.join(", "))
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

/// The stderr line a waiting `taskslash run` prints.
pub fn waiting_line(reason: &Reason, waited_s: u64, position: usize) -> String {
    format!(
        "taskslash: waiting {waited_s}s ({} in line): {reason}",
        ordinal(position)
    )
}
