//! The order and the pass, against a fixed machine (10 threads, so a CPU budget of 8.5, no
//! pressure) and a fixed clock: `base` is the pass's `now` minus however long each job waited.

use super::*;
use crate::admission::tests::machine;

const MIN: u64 = 60;

/// A clock where `now` is an hour after `base`, so a job can have waited up to an hour.
struct Clock {
    base: Instant,
}

impl Clock {
    fn new() -> Self {
        Self {
            base: Instant::now(),
        }
    }

    fn now(&self) -> Instant {
        self.base + Duration::from_secs(60 * MIN)
    }

    /// The `submitted_at` of a job that has waited `secs` at `now`.
    fn waited(&self, secs: u64) -> Instant {
        self.now() - Duration::from_secs(secs)
    }
}

fn demand(cpu: f64) -> Demand {
    Demand {
        estimate: Use {
            cpu,
            mem_mib: 100.0,
            write_mib_s: 0.0,
        },
        never_seen: false,
    }
}

fn queued(id: u64, class: Class, submitter: &str, at: Instant, cpu: f64) -> Queued {
    Queued {
        id,
        key: format!("j{id}"),
        class,
        submitter: submitter.to_owned(),
        submitted_at: at,
        demand: demand(cpu),
    }
}

fn active(id: u64, submitter: &str, cpu: f64) -> Active {
    Active {
        held: admission::Running {
            job: id,
            key: format!("r{id}"),
            estimate: demand(cpu).estimate,
            observed: None,
            never_seen: false,
        },
        submitter: submitter.to_owned(),
    }
}

fn planned(waiting: &[Queued], running: &[Active], now: Instant) -> Vec<u64> {
    plan(
        waiting,
        running,
        &machine(),
        &Use::default(),
        &Limits::default(),
        now,
    )
}

#[test]
fn an_urgent_job_submitted_behind_five_normal_ones_starts_first() {
    let clock = Clock::new();
    let running = [active(1, "/a", 4.0)];
    let mut waiting: Vec<Queued> = (2..=6)
        .map(|id| queued(id, Class::Normal, "/a", clock.waited(100 - id), 4.0))
        .collect();
    waiting.push(queued(7, Class::Urgent, "/a", clock.waited(10), 4.0));
    assert_eq!(planned(&waiting, &running, clock.now()), [7]);

    let pass = pass(
        &waiting,
        &running,
        &machine(),
        &Use::default(),
        &Limits::default(),
        clock.now(),
    );
    let places: Vec<(u64, usize)> = pass.waiting.iter().map(|w| (w.0, w.1)).collect();
    assert_eq!(places, [(2, 1), (3, 2), (4, 3), (5, 4), (6, 5)]);
}

#[test]
fn the_worktree_holding_less_of_the_machine_goes_first() {
    let clock = Clock::new();
    let running = [active(1, "/a", 4.0)];
    let waiting = [
        queued(2, Class::Normal, "/a", clock.waited(30), 4.0),
        queued(3, Class::Normal, "/b", clock.waited(20), 4.0),
    ];
    assert_eq!(planned(&waiting, &running, clock.now()), [3]);
}

#[test]
fn a_job_waiting_over_5_minutes_is_not_passed_and_before_that_a_smaller_one_backfills() {
    let clock = Clock::new();
    let running = [active(1, "/a", 4.0)];
    let x = |waited| queued(2, Class::Normal, "/a", clock.waited(waited), 6.0);
    let y = queued(3, Class::Normal, "/a", clock.waited(30), 2.0);

    let starved = [x(6 * MIN), y.clone()];
    assert_eq!(planned(&starved, &running, clock.now()), [] as [u64; 0]);
    let pass = pass(
        &starved,
        &running,
        &machine(),
        &Use::default(),
        &Limits::default(),
        clock.now(),
    );
    assert_eq!(
        pass.waiting[1],
        (
            3,
            2,
            Reason::Behind {
                job: 2,
                key: "j2".into()
            }
        )
    );

    let young = [x(4 * MIN), y];
    assert_eq!(planned(&young, &running, clock.now()), [3]);
}

#[test]
fn a_never_seen_job_that_waited_long_does_not_hold_back_seen_ones() {
    let clock = Clock::new();
    let mut running = [active(1, "/a", 2.0)];
    running[0].held.never_seen = true;
    let mut x = queued(2, Class::Normal, "/a", clock.waited(6 * MIN), 2.0);
    x.demand.never_seen = true;
    let y = queued(3, Class::Normal, "/a", clock.waited(30), 2.0);
    assert_eq!(planned(&[x, y], &running, clock.now()), [3]);
}

#[test]
fn a_job_moves_up_one_class_per_15_minutes_waited() {
    let at = |m: u64| effective(Class::Background, Duration::from_secs(m * MIN));
    assert_eq!(at(14), Class::Background);
    assert_eq!(at(16), Class::Normal);
    assert_eq!(at(31), Class::Interactive);
    assert_eq!(at(46), Class::Flake);
    assert_eq!(at(61), Class::Urgent);
    assert_eq!(at(600), Class::Urgent);

    // In the order: a background job waiting 16 minutes ties with a normal one and, older, wins.
    let clock = Clock::new();
    let running = [active(1, "/a", 4.0)];
    let waiting = [
        queued(2, Class::Normal, "/a", clock.waited(MIN), 4.0),
        queued(3, Class::Background, "/a", clock.waited(16 * MIN), 4.0),
    ];
    assert_eq!(planned(&waiting, &running, clock.now()), [3]);
    // At 31 minutes it ties with an interactive job and, older, wins; at 29 it does not.
    let waiting = |bg: u64| {
        [
            queued(2, Class::Interactive, "/a", clock.waited(10 * MIN), 4.0),
            queued(3, Class::Background, "/a", clock.waited(bg * MIN), 4.0),
        ]
    };
    assert_eq!(planned(&waiting(31), &running, clock.now()), [3]);
    assert_eq!(planned(&waiting(29), &running, clock.now()), [2]);
}

#[test]
fn shares_update_within_a_pass() {
    let clock = Clock::new();
    let waiting = [
        queued(1, Class::Normal, "/a", clock.waited(30), 4.0),
        queued(2, Class::Normal, "/a", clock.waited(20), 4.0),
        queued(3, Class::Normal, "/b", clock.waited(10), 4.0),
    ];
    assert_eq!(planned(&waiting, &[], clock.now()), [1, 3]);
}

#[test]
fn a_gate_holds_every_job() {
    let clock = Clock::new();
    let mut m = machine();
    m.cpu.pressure.some_avg10 = 50.0;
    let waiting = [
        queued(1, Class::Urgent, "/a", clock.waited(10), 0.5),
        queued(2, Class::Normal, "/b", clock.waited(10), 0.5),
    ];
    let pass = pass(
        &waiting,
        &[],
        &m,
        &Use::default(),
        &Limits::default(),
        clock.now(),
    );
    assert!(pass.start.is_empty());
    assert_eq!(pass.waiting.len(), 2);
    assert!(
        pass.waiting
            .iter()
            .all(|w| matches!(w.2, Reason::Gate { .. }))
    );
}
