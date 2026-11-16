//! The store against an in-memory database.

use super::*;
use std::time::Duration;

fn words(argv: &[&str]) -> Vec<String> {
    argv.iter().map(|&a| a.to_owned()).collect()
}

#[test]
fn the_key_is_the_name_else_two_words() {
    let argv = words(&["bunx", "moon", "run", ":test"]);
    assert_eq!(key(Some("gate"), &argv), "gate");
    assert_eq!(key(None, &argv), "bunx moon");
    assert_eq!(key(None, &words(&["true"])), "true");
}

fn finished<'a>(argv: &'a [String], launch: Launch, at: u64, cost: Cost) -> Finished<'a> {
    Finished {
        key: "k",
        kind: Kind::Test,
        argv,
        cwd: "/work",
        launch,
        started_at: UNIX_EPOCH + Duration::from_millis(at),
        wall_ms: 12,
        cost,
        exit_code: Some(4),
        pressure: RunPressure::default(),
    }
}

#[test]
fn a_plain_run_is_recorded_with_null_costs() {
    let store = Store::in_memory();
    let argv = words(&["sh", "-c", "exit 4"]);
    store
        .record(&finished(
            &argv,
            Launch::Plain,
            1_763_290_000_123,
            Cost::default(),
        ))
        .unwrap();
    let runs = store.history("k").unwrap();
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(run.argv, argv);
    assert_eq!(run.launch, Launch::Plain);
    assert_eq!(run.started_at, "2025-11-16T10:46:40.123Z");
    assert_eq!((run.wall_ms, run.exit_code), (12, Some(4)));
    assert_eq!(run.cpu_usec, None);
    assert_eq!(run.peak_cpu_5s, None);
    assert_eq!(run.peak_mem_bytes, None);
    assert_eq!(run.written_bytes, None);
}

#[test]
fn history_is_newest_first_and_at_most_20() {
    let store = Store::in_memory();
    let argv = words(&["true"]);
    let cost = Cost {
        cpu_usec: Some(5),
        peak_cpu_5s: Some(0.5),
        peak_mem_bytes: Some(6),
        written_bytes: Some(0),
    };
    for at in 0..25 {
        store
            .record(&finished(&argv, Launch::Scope, at * 1000, cost.clone()))
            .unwrap();
    }
    let runs = store.history("k").unwrap();
    assert_eq!(runs.len(), HISTORY_LIMIT);
    assert_eq!(runs[0].started_at, "1970-01-01T00:00:24.000Z");
    assert_eq!(runs[0].cpu_usec, Some(5));
    assert_eq!(runs[0].written_bytes, Some(0));
    assert!(store.history("other").unwrap().is_empty());
}

#[test]
fn a_key_is_seen_once_a_run_finished_and_costs_are_its_measured_runs() {
    let store = Store::in_memory();
    let argv = words(&["true"]);
    assert!(!store.seen("k").unwrap());
    let mut dropped = finished(&argv, Launch::Scope, 1000, Cost::default());
    dropped.exit_code = None;
    store.record(&dropped).unwrap();
    assert!(
        !store.seen("k").unwrap(),
        "a dropped run does not make a key seen"
    );
    store
        .record(&finished(&argv, Launch::Plain, 2000, Cost::default()))
        .unwrap();
    assert!(store.seen("k").unwrap(), "an unmeasured finished run does");
    assert!(store.costs("k").unwrap().is_empty());

    let mut measured = finished(
        &argv,
        Launch::Scope,
        3000,
        Cost {
            cpu_usec: Some(7),
            peak_cpu_5s: Some(0.5),
            peak_mem_bytes: Some(8),
            written_bytes: None,
        },
    );
    measured.pressure = RunPressure {
        cpu: Some(1.5),
        mem: Some(0.25),
        io: None,
    };
    store.record(&measured).unwrap();
    let costs = store.costs("k").unwrap();
    assert_eq!(costs.len(), 1);
    assert_eq!(
        (costs[0].cpu_usec, costs[0].machine_cpu_some),
        (Some(7), Some(1.5))
    );
    let run = &store.history("k").unwrap()[0];
    assert_eq!(run.kind, Kind::Test);
    assert_eq!(
        (
            run.machine_cpu_some,
            run.machine_mem_some,
            run.machine_io_some
        ),
        (Some(1.5), Some(0.25), None)
    );
}
