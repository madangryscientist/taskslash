//! The jobs that ended since the daemon started, by number, so `logs <n>` still finds one after it
//! ends. Kept in memory only: a restart forgets them, and only the newest are kept.

use std::collections::BTreeMap;

/// How many ended jobs the daemon remembers.
pub(super) const ENDED_JOBS: usize = 1000;

/// What became of an ended job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Ended {
    /// It ran and its row is run `id`.
    Run(i64),
    /// It ran and its row is being written: remembered before it leaves the scheduler, so a
    /// `logs <n>` in between finds it.
    Recording,
    /// It ran, but its row could not be written.
    Unrecorded,
    /// It never ran: it left the queue, or its client went before it started.
    NoRun,
}

/// The newest `cap` ended jobs. Job numbers only grow, so the smallest is the oldest.
pub(super) struct EndedJobs {
    cap: usize,
    jobs: BTreeMap<u64, Ended>,
}

impl EndedJobs {
    pub(super) fn new(cap: usize) -> Self {
        Self {
            cap,
            jobs: BTreeMap::new(),
        }
    }

    /// Remembers that job `job` ended as `ended`, forgetting the oldest past the cap.
    pub(super) fn insert(&mut self, job: u64, ended: Ended) {
        self.jobs.insert(job, ended);
        while self.jobs.len() > self.cap {
            self.jobs.pop_first();
        }
    }

    /// What became of job `job`; `None` when it is not remembered.
    pub(super) fn get(&self, job: u64) -> Option<Ended> {
        self.jobs.get(&job).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::{Ended, EndedJobs};

    #[test]
    fn it_resolves_the_newest_and_forgets_the_oldest_past_its_cap() {
        let mut ended = EndedJobs::new(3);
        ended.insert(1, Ended::Run(10));
        ended.insert(2, Ended::NoRun);
        ended.insert(3, Ended::Run(30));
        assert_eq!(ended.get(1), Some(Ended::Run(10)));
        assert_eq!(ended.get(2), Some(Ended::NoRun));
        ended.insert(4, Ended::Run(40));
        assert_eq!(ended.get(1), None, "the oldest is forgotten");
        assert_eq!(ended.get(4), Some(Ended::Run(40)));
        assert_eq!(ended.jobs.len(), 3);
        assert_eq!(ended.get(99), None);
    }
}
