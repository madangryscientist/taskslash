//! The pure figures and an absent cgroup.

use super::*;

const MIB: u64 = 1 << 20;

#[test]
fn peak_cpu_is_the_busiest_5_s() {
    // 1 thread for 4 s, 3 threads for 6 s, then idle for 4 s; a sample every second.
    let mut series = vec![(0.0, 0)];
    let mut usage = 0;
    for t in 1..=14 {
        usage += match t {
            1..=4 => 1_000_000,
            5..=10 => 3_000_000,
            _ => 0,
        };
        series.push((f64::from(t), usage));
    }
    let peak = peak_cpu(&series, 5.0).unwrap();
    assert!((peak - 3.0).abs() < 1e-9, "{peak}");
}

#[test]
fn a_job_shorter_than_the_window_is_averaged_over_the_window() {
    // 2 threads for 2.5 s: 5 CPU seconds in any 5 s window is 1 thread.
    let series = [(0.0, 0), (1.0, 2_000_000), (2.5, 5_000_000)];
    let peak = peak_cpu(&series, 5.0).unwrap();
    assert!((peak - 1.0).abs() < 1e-9, "{peak}");
}

#[test]
fn peak_cpu_needs_two_samples() {
    assert_eq!(peak_cpu(&[], 5.0), None);
    assert_eq!(peak_cpu(&[(0.0, 0)], 5.0), None);
}

#[test]
fn bytes_written_is_the_largest_alive_sum_not_the_sum_of_lasts() {
    // pid 10 runs the job; its child 11 writes 200 MiB and is reaped, and 10's counter then
    // includes the child's 200 MiB. 10 writes 4 MiB of its own at the end.
    let samples: Vec<BTreeMap<u32, u64>> = vec![
        BTreeMap::from([(10, 0), (11, 0)]),
        BTreeMap::from([(10, 0), (11, 120 * MIB)]),
        BTreeMap::from([(10, 0), (11, 200 * MIB)]),
        BTreeMap::from([(10, 200 * MIB)]),
        BTreeMap::from([(10, 204 * MIB)]),
    ];
    // The sum of each pid's last value would say 404 MiB.
    assert_eq!(written_bytes(&samples), Some(204 * MIB));
    assert_eq!(written_bytes(&[]), None);
}

#[test]
fn a_cgroup_never_seen_costs_nulls() {
    let dir = std::env::temp_dir().join("taskslash-no-such-cgroup.scope");
    let sampler = Sampler::start(dir);
    assert_eq!(sampler.finish(), Cost::default());
}

#[test]
fn the_recent_rate_is_over_the_newest_window() {
    // 1 MiB/s for 10 s, then 3 MiB/s for 5 s; a sample every second.
    let mut series = vec![(0.0, 0)];
    let mut n = 0;
    for t in 1..=15 {
        n += if t <= 10 { MIB } else { 3 * MIB };
        series.push((f64::from(t), n));
    }
    let rate = recent_rate(&series, 5.0) / MIB as f64;
    assert!((rate - 3.0).abs() < 1e-9, "{rate}");
    // Shorter than the window: 2 MiB over the whole 5 s window.
    let rate = recent_rate(&series[..3], 5.0) / MIB as f64;
    assert!((rate - 0.4).abs() < 1e-9, "{rate}");
    // A start-up burst, 50 ms of CPU in the first 20 ms, is not 2.5 threads.
    let rate = recent_rate(&[(0.0, 0), (0.02, 50_000)], 5.0) / 1e6;
    assert!((rate - 0.01).abs() < 1e-9, "{rate}");
    assert_eq!(recent_rate(&series[..1], 5.0), 0.0);
    assert_eq!(recent_rate(&[], 5.0), 0.0);
}
