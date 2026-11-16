//! The `/proc` parsers on files copied from the desktop they were written on (12 threads online,
//! user work confined to `0-3,6-9`, one SATA disk `sda` under a LUKS mapping `dm-0`, and zram swap).

use taskslash_core::procfs::{self, Pressure};

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/",
            $name
        ))
    };
}

#[test]
fn the_allowed_cpu_list_0_3_6_9_is_8_threads() {
    let cpus = procfs::allowed_cpus(fixture!("self-status"));
    assert_eq!(cpus, [0, 1, 2, 3, 6, 7, 8, 9]);
    assert_eq!(procfs::cpu_list("0-11").len(), 12);
    assert_eq!(procfs::cpu_list("5\n"), [5]);
    assert!(procfs::allowed_cpus("Name: x\n").is_empty());
}

#[test]
fn busy_is_measured_over_the_allowed_cpus_only() {
    let before = procfs::cpu_times(fixture!("stat-1"));
    let after = procfs::cpu_times(fixture!("stat-2"));
    // Every cpuN line, not the `cpu` total line.
    assert_eq!(after.len(), 12);
    let allowed = procfs::allowed_cpus(fixture!("self-status"));
    // Worked out by hand from the two files: 288 busy of 814 jiffies on CPUs 0-3 and 6-9.
    let busy = procfs::busy_share(&before, &after, &allowed);
    assert!((busy - 288.0 / 814.0).abs() < 1e-9, "{busy}");
    // The same reading twice is no time at all, not a division by zero.
    assert_eq!(procfs::busy_share(&after, &after, &allowed), 0.0);
}

#[test]
fn busy_counts_iowait_as_idle() {
    // user nice system idle iowait irq softirq steal: 10 busy, 30 idle, 60 iowait.
    let before = procfs::cpu_times("cpu0 0 0 0 0 0 0 0 0 0 0\n");
    let after = procfs::cpu_times("cpu0 10 0 0 30 60 0 0 0 0 0\n");
    let busy = procfs::busy_share(&before, &after, &[0]);
    assert!((busy - 0.1).abs() < 1e-9, "{busy}");
}

#[test]
fn sda_is_picked_and_dm_zram_and_partitions_are_skipped() {
    let disks: Vec<String> = fixture!("sys-block")
        .lines()
        .filter(|name| procfs::is_physical_disk(name))
        .map(str::to_owned)
        .collect();
    assert_eq!(disks, ["sda"]);
    for skipped in ["loop0", "ram0", "zram0", "dm-0"] {
        assert!(!procfs::is_physical_disk(skipped), "{skipped}");
    }
    assert!(procfs::is_physical_disk("nvme0n1"));
    // `sda1` and `sda2` are lines of /proc/diskstats too; only the named disk is read.
    let counters = procfs::diskstats(fixture!("diskstats-1"), &disks);
    assert_eq!(counters.len(), 1);
    assert_eq!(counters[0].name, "sda");
    assert_eq!(counters[0].sectors_written, 13_718_251_327);
    assert_eq!(counters[0].io_ticks_ms, 203_766_623);
}

#[test]
fn pressure_reads_some_and_full_avg10_and_avg60() {
    assert_eq!(
        procfs::pressure(fixture!("pressure-io")),
        Pressure {
            some_avg10: 0.47,
            some_avg60: 0.65,
            full_avg10: 0.30,
            full_avg60: 0.45,
        }
    );
    assert_eq!(procfs::pressure(fixture!("pressure-cpu")).some_avg10, 7.12);
    assert_eq!(
        procfs::pressure(fixture!("pressure-memory")).full_avg60,
        0.61
    );
}

#[test]
fn a_pressure_file_with_no_full_line_has_full_0() {
    let some_only = "some avg10=5.47 avg60=6.54 avg300=5.10 total=93460110717\n";
    assert_eq!(
        procfs::pressure(some_only),
        Pressure {
            some_avg10: 5.47,
            some_avg60: 6.54,
            full_avg10: 0.0,
            full_avg60: 0.0,
        }
    );
    // The irq file has the other half only.
    let irq = procfs::pressure(fixture!("pressure-irq"));
    assert_eq!((irq.some_avg10, irq.some_avg60), (0.0, 0.0));
    assert_eq!(procfs::pressure(""), Pressure::default());
}

#[test]
fn meminfo_is_read_in_mib() {
    let memory = procfs::meminfo(fixture!("meminfo"));
    assert_eq!(memory.total_mib, 32_775_700 / 1024);
    assert_eq!(memory.available_mib, 21_467_064 / 1024);
    assert_eq!(memory.swap_total_mib, 65_556_448 / 1024);
    assert_eq!(memory.swap_free_mib, 62_214_080 / 1024);
}

#[test]
fn loadavg_is_the_first_three_numbers() {
    assert_eq!(procfs::loadavg(fixture!("loadavg")), [4.31, 4.04, 5.23]);
}

#[test]
fn rfc3339_formats_utc_with_milliseconds() {
    use std::time::{Duration, UNIX_EPOCH};
    let at = UNIX_EPOCH + Duration::from_millis(1_794_795_464_464);
    assert_eq!(procfs::rfc3339(at), "2026-11-16T02:17:44.464Z");
    assert_eq!(procfs::rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
    // A leap day.
    let leap = UNIX_EPOCH + Duration::from_secs(1_709_208_000);
    assert_eq!(procfs::rfc3339(leap), "2024-02-29T12:00:00.000Z");
}
