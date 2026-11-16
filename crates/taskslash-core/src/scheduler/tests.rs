//! The queue and `admit` against a fake `seen`.

use super::*;
use crate::estimate::estimate;
use crate::sampler::{MachineSampler, ProcSampler};
use std::sync::mpsc::{self, Receiver};

fn submit(s: &mut Scheduler, id: u64, key: &str) -> Receiver<()> {
    let (start, started) = mpsc::channel();
    let job = Job {
        id,
        key: key.to_owned(),
        kind: Kind::Command,
        estimate: estimate(&[], Kind::Command),
    };
    s.submit(job, start);
    started
}

#[test]
fn one_never_seen_job_runs_at_a_time_and_seen_ones_pass_it() {
    let seen = |key: &str| key.starts_with("seen");
    let mut s = Scheduler::default();
    let a = submit(&mut s, 1, "new-a");
    let b = submit(&mut s, 2, "new-b");
    let c = submit(&mut s, 3, "seen-c");
    s.admit(seen);
    assert!(a.try_recv().is_ok());
    assert!(b.try_recv().is_err());
    assert!(
        c.try_recv().is_ok(),
        "a seen job behind a waiting one starts"
    );
    assert_eq!(
        s.reason(2).as_deref(),
        Some("another never-seen job is running: new-a")
    );
    assert_eq!(s.reason(1), None);

    let (_, waiting) = s.status();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].key, "new-b");

    assert!(s.finish(1).is_some());
    s.admit(seen);
    assert!(b.try_recv().is_ok());
    assert!(!s.leave(2), "a started job is not on the queue");
}

#[test]
fn a_job_that_leaves_the_queue_never_starts() {
    let mut s = Scheduler::default();
    let _a = submit(&mut s, 1, "new-a");
    let b = submit(&mut s, 2, "new-b");
    s.admit(|_| false);
    assert!(s.leave(2));
    s.finish(1);
    s.admit(|_| false);
    assert!(b.try_recv().is_err());
    assert!(s.status().1.is_empty());
}

#[test]
fn a_run_keeps_the_mean_of_the_samples_taken_while_it_ran() {
    let mut s = Scheduler::default();
    let _a = submit(&mut s, 1, "a");
    let mut machine = ProcSampler::new().sample();
    s.sample(&machine); // before it started: not counted
    s.admit(|_| true);
    for (cpu, mem) in [(10.0, 1.0), (50.0, 0.0), (0.0, 0.5)] {
        machine.cpu.pressure.some_avg10 = cpu;
        machine.memory.pressure.some_avg10 = mem;
        machine.io.pressure.some_avg10 = 3.0;
        s.sample(&machine);
    }
    let pressure = s.finish(1).unwrap().pressure();
    assert_eq!(
        pressure,
        RunPressure {
            cpu: Some(20.0),
            mem: Some(0.5),
            io: Some(3.0)
        }
    );

    let _b = submit(&mut s, 2, "b");
    s.admit(|_| true);
    assert_eq!(s.finish(2).unwrap().pressure(), RunPressure::default());
}
