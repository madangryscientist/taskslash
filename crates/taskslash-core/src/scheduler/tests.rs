//! The queue and `admit` against a fake `seen`.

use super::*;
use crate::admission::tests::machine;
use crate::estimate::estimate;
use std::sync::mpsc::{self, Receiver};

fn submit(s: &mut Scheduler, id: u64, key: &str) -> Receiver<()> {
    let (start, started) = mpsc::channel();
    let job = Job {
        id,
        key: key.to_owned(),
        kind: Kind::Command,
        estimate: estimate(&[], Kind::Command),
        vouched: false,
    };
    s.submit(job, start);
    started
}

#[test]
fn one_never_seen_job_runs_at_a_time_and_seen_ones_pass_it() {
    let seen = |key: &str| key.starts_with("seen");
    let mut s = Scheduler::new(Limits::default(), machine());
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
    let (reason, _, position) = s.waiting(2).unwrap();
    assert_eq!(
        reason.to_string(),
        "another never-seen job is running: new-a (job 1)"
    );
    assert_eq!(position, 1);
    assert!(s.waiting(1).is_none());

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
    let mut s = Scheduler::new(Limits::default(), machine());
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
    let mut s = Scheduler::new(Limits::default(), machine());
    let _a = submit(&mut s, 1, "a");
    let mut machine = machine();
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

#[test]
fn an_unmeasured_job_is_not_counted_again_as_unmanaged() {
    // 10 threads, an 8.5 thread budget. A plain job estimated at 4 threads really uses 4, so the
    // machine shows 4 busy threads; a second 4 thread job still fits (4 + 4 = 8).
    let mut s = Scheduler::new(Limits::default(), machine());
    let submit4 = |s: &mut Scheduler, id: u64| {
        let (start, started) = mpsc::channel();
        let mut estimate = estimate(&[], Kind::Command);
        estimate.cpu_threads = 4.0;
        let job = Job {
            id,
            key: format!("k{id}"),
            kind: Kind::Command,
            estimate,
            vouched: true,
        };
        s.submit(job, start);
        started
    };
    let first = submit4(&mut s, 1);
    s.admit(|_| true);
    assert!(first.try_recv().is_ok());
    let mut busy = machine();
    busy.cpu.busy = 0.4;
    for _ in 0..30 {
        s.sample(&busy);
    }
    let second = submit4(&mut s, 2);
    s.admit(|_| true);
    assert!(
        second.try_recv().is_ok(),
        "{:?}",
        s.waiting(2).map(|w| w.0.to_string())
    );
}
