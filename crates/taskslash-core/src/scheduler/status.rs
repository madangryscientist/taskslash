//! What `taskslash status` shows of the scheduler.

use super::{Scheduler, lock, round};
use crate::procfs;
use crate::protocol::{JobStatus, WaitingStatus};

impl Scheduler {
    /// The running jobs, oldest first, and the waiting ones in queue order.
    pub fn status(&self) -> (Vec<JobStatus>, Vec<WaitingStatus>) {
        let jobs = self
            .running
            .values()
            .map(|r| {
                let live = r.live.as_ref().and_then(|live| *lock(live));
                JobStatus {
                    job: r.job.id,
                    key: r.job.key.clone(),
                    kind: r.job.kind,
                    started_at: procfs::rfc3339(r.started_at),
                    cpu_threads: live.map(|l| round(l.cpu_threads, 2)),
                    mem_mib: live.map(|l| l.mem_mib),
                    estimate: r.job.estimate.clone(),
                }
            })
            .collect();
        let waiting = self
            .queue
            .iter()
            .enumerate()
            .map(|(at, w)| WaitingStatus {
                job: w.job.id,
                key: w.job.key.clone(),
                kind: w.job.kind,
                waited_s: round(w.queued_at.elapsed().as_secs_f64(), 1),
                position: at + 1,
                reason: w
                    .reason
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
                why: w.reason.clone(),
                estimate: w.job.estimate.clone(),
            })
            .collect();
        (jobs, waiting)
    }
}
