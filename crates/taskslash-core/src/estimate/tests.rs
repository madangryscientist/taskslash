//! The estimate rule against fixed runs, each expected figure worked out by hand.

use super::*;

/// A finished run: wall ms, CPU seconds, peak 5 s threads, peak MiB, MiB written, CPU some.
fn run(wall: i64, cpu_s: f64, peak: f64, mem: i64, written: i64, some: Option<f64>) -> RunCost {
    RunCost {
        wall_ms: wall,
        cpu_usec: Some((cpu_s * 1e6) as i64),
        peak_cpu_5s: Some(peak),
        peak_mem_bytes: Some(mem * MIB),
        written_bytes: Some(written * MIB),
        exit_code: Some(0),
        machine_cpu_some: some,
        cached: false,
    }
}

/// A run on the plain launch path: wall time and exit code only.
fn plain(wall: i64) -> RunCost {
    RunCost {
        wall_ms: wall,
        exit_code: Some(0),
        ..RunCost::default()
    }
}

/// Twelve runs, newest first, and the estimate worked out by hand.
///
/// Measured: r0 r2 r3 r4 r6 r7 r8 r9 r11 (9). r1 and r10 are plain; r5 was dropped (no exit
/// code), so its 999 MiB never counts. Calm: r0 r3 r6 r7 r8 r11 (6); r2 (75) and r9 (40.1) are
/// starved, r4 had no sample, and r3 sits on the pressure gate (40.0), which is still calm.
///
/// - Memory: newest 5 measured are r0 r2 r3 r4 r6 = 100 300 200 150 250 MiB; the max is 300
///   (r2, starved but honest), times 1.2 = 360 MiB. r7's 800 MiB is the 6th measured: out.
/// - CPU: mean threads of the calm runs, r0 2/2 = 1.0, r3 12/4 = 3.0, r6 6/3 = 2.0,
///   r7 2.5/5 = 0.5, r8 9/6 = 1.5, r11 2.5/1 = 2.5. Sorted 0.5 1.0 1.5 2.0 2.5 3.0; n = 6,
///   index ceil(4.5) - 1 = 4, so 2.5 (linear interpolation would say 2.375).
/// - Peak 5 s: the max over those calm runs is r3's 3.5; starved r9's 4.0 does not count.
/// - I/O: MiB/s of the 9 measured, r0 20/2 = 10, r2 40/8 = 5, r3 0, r4 10/1 = 10, r6 15/3 = 5,
///   r7 30/5 = 6, r8 0, r9 10/2 = 5, r11 4/1 = 4; the sum is 45, over 9 = 5.0.
/// - Duration: calm walls sorted 1 2 3 4 5 6 s; an even count, so (3 + 4) / 2 = 3.5 s.
#[test]
fn twelve_mixed_runs_give_the_hand_worked_estimate() {
    let mut dropped = run(500, 0.5, 9.0, 999, 1, Some(0.0));
    dropped.exit_code = None;
    let runs = [
        run(2000, 2.0, 1.5, 100, 20, Some(5.0)),  // r0
        plain(1000),                              // r1
        run(8000, 8.0, 1.2, 300, 40, Some(75.0)), // r2
        run(4000, 12.0, 3.5, 200, 0, Some(40.0)), // r3
        run(1000, 1.0, 1.0, 150, 10, None),       // r4
        dropped,                                  // r5
        run(3000, 6.0, 2.5, 250, 15, Some(20.0)), // r6
        run(5000, 2.5, 0.8, 800, 30, Some(12.0)), // r7
        run(6000, 9.0, 2.0, 50, 0, Some(1.0)),    // r8
        run(2000, 8.0, 4.0, 60, 10, Some(40.1)),  // r9
        plain(1000),                              // r10
        run(1000, 2.5, 2.2, 70, 4, Some(0.5)),    // r11
    ];
    let history = Sources {
        cpu: Source::History,
        mem: Source::History,
        io: Source::History,
        duration: Source::History,
    };
    assert_eq!(
        estimate(&runs, Kind::Test),
        Estimate {
            cpu_threads: 2.5,
            cpu_peak_5s: Some(3.5),
            mem_mib: 360,
            write_mib_s: 5.0,
            duration_s: Some(3.5),
            samples: Samples {
                measured: 9,
                calm: 6
            },
            source: history,
        }
    );
}

#[test]
fn only_the_newest_ten_calm_runs_count() {
    // Newest 10: 1 thread for 1 s. The two oldest: 10 threads for 100 s, and 2 GiB.
    let mut runs = vec![run(1000, 1.0, 1.0, 10, 1, Some(0.0)); 10];
    runs.extend(vec![run(100_000, 1000.0, 10.0, 2048, 0, Some(0.0)); 2]);
    let got = estimate(&runs, Kind::Command);
    assert_eq!(
        (got.cpu_threads, got.cpu_peak_5s, got.duration_s),
        (1.0, Some(1.0), Some(1.0))
    );
    assert_eq!((got.mem_mib, got.write_mib_s), (12, 1.0));
    assert_eq!((got.samples.measured, got.samples.calm), (12, 12));
}

#[test]
fn no_measured_run_gives_the_kind_defaults() {
    let mut dropped = run(500, 1.0, 1.0, 100, 1, Some(0.0));
    dropped.exit_code = None;
    let defaults = Sources {
        cpu: Source::Default,
        mem: Source::Default,
        io: Source::Default,
        duration: Source::Default,
    };
    for (kind, cpu, mem) in [
        (Kind::Test, 2.0, 2048),
        (Kind::Typecheck, 2.0, 3072),
        (Kind::Build, 4.0, 4096),
        (Kind::Browser, 4.0, 4096),
        (Kind::Command, 2.0, 2048),
    ] {
        let got = estimate(&[plain(1000), dropped.clone(), plain(3000)], kind);
        assert_eq!(
            got,
            Estimate {
                cpu_threads: cpu,
                cpu_peak_5s: None,
                mem_mib: mem,
                write_mib_s: 0.0,
                duration_s: None,
                samples: Samples {
                    measured: 0,
                    calm: 0
                },
                source: defaults,
            },
            "{kind:?}"
        );
    }
    assert_eq!(estimate(&[], Kind::Test).source, defaults);
}

#[test]
fn all_starved_runs_feed_memory_and_io_only() {
    // 10 MiB over 2 s = 5 MiB/s and 20 MiB over 4 s = 5 MiB/s; the worst peak is 500 MiB.
    let runs = [
        run(2000, 8.0, 4.0, 500, 10, Some(80.0)),
        run(4000, 16.0, 4.0, 400, 20, Some(41.0)),
    ];
    let got = estimate(&runs, Kind::Build);
    assert_eq!(
        (got.cpu_threads, got.cpu_peak_5s, got.duration_s),
        (4.0, None, None)
    );
    assert_eq!((got.mem_mib, got.write_mib_s), (600, 5.0));
    assert_eq!(
        got.source,
        Sources {
            cpu: Source::Default,
            mem: Source::History,
            io: Source::History,
            duration: Source::Default,
        }
    );
    assert_eq!((got.samples.measured, got.samples.calm), (2, 0));
}

/// A cache hit costs what moon's replay cost, not the task: it feeds no figure (spike 4.4).
#[test]
fn a_cache_hit_is_not_a_cost_sample() {
    let mut hit = run(200, 0.1, 0.2, 40, 0, Some(0.0));
    hit.cached = true;
    let real = run(4000, 8.0, 2.5, 500, 40, Some(0.0));
    let both = estimate(&[hit.clone(), real.clone()], Kind::Test);
    assert_eq!(
        both,
        estimate(&[real], Kind::Test),
        "the hit changes nothing"
    );
    assert_eq!(
        both.samples,
        Samples {
            measured: 1,
            calm: 1
        }
    );
    assert_eq!((both.cpu_threads, both.mem_mib), (2.0, 600));

    let only_hits = estimate(&[hit.clone(), hit], Kind::Typecheck);
    assert_eq!(
        only_hits,
        estimate(&[], Kind::Typecheck),
        "the kind's defaults"
    );
}
