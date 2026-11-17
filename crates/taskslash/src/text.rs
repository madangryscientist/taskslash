//! The plain text forms of `status` and `history`.

/// How wide the `where` column of the run table is.
const WHERE_WIDTH: usize = 12;

/// `name` in at most `width` characters: a longer one cut from the left to `..` and its last
/// `width - 2`.
fn cut_left(name: &str, width: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= width {
        return name.to_owned();
    }
    let tail: String = chars[chars.len() - (width - 2)..].iter().collect();
    format!("..{tail}")
}

/// One header line and one line per key: when it last ran, how many runs, the last exit code (`-`
/// when its client went away), the worktree it last ran in, and the key, whole, last.
pub fn keys_text(keys: &[KeySummary]) -> String {
    let mut out = format!(
        "{:<24}  {:>6}  {:>4}  {:<12}  key\n",
        "last_run", "runs", "exit", "where"
    );
    for k in keys {
        out.push_str(&format!(
            "{:<24}  {:>6}  {:>4}  {:<12}  {}\n",
            k.last_started_at,
            k.runs,
            k.last_exit_code.map_or("-".to_owned(), |c| c.to_string()),
            cut_left(worktree(&k.last_submitter), WHERE_WIDTH),
            k.key
        ));
    }
    out
}

use taskslash_core::admission::ordinal;
use taskslash_core::estimate::{Estimate, Source};
use taskslash_core::protocol::{KeySummary, Run, Status, worktree};

/// A few lines: CPU, the three `some avg10` pressures, memory, each disk, each mount, each job.
pub fn status_text(status: &Status) -> String {
    let m = &status.machine;
    let pct = |share: f64| format!("{:.0}%", share * 100.0);
    let mut out = format!(
        "cpu       {} busy on {} threads ({} online), load {:.2} {:.2} {:.2}\n",
        pct(m.cpu.busy),
        m.cpu.threads,
        m.cpu.threads_online,
        m.load[0],
        m.load[1],
        m.load[2]
    );
    out.push_str(&format!(
        "pressure  cpu {:.2}  memory {:.2}  io {:.2}  (some avg10, %)\n",
        m.cpu.pressure.some_avg10, m.memory.pressure.some_avg10, m.io.pressure.some_avg10
    ));
    out.push_str(&format!(
        "memory    {} MiB available of {} MiB, swap {} of {} MiB free\n",
        m.memory.available_mib, m.memory.total_mib, m.memory.swap_free_mib, m.memory.swap_total_mib
    ));
    for disk in &m.io.disks {
        out.push_str(&format!(
            "disk      {} {} busy, {:.1} MiB/s written\n",
            disk.name,
            pct(disk.busy),
            disk.write_mib_s
        ));
    }
    for mount in &m.mounts {
        out.push_str(&format!(
            "mount     {} {} MiB free of {} MiB ({} used)\n",
            mount.path,
            mount.free_mib,
            mount.total_mib,
            pct(mount.used)
        ));
    }
    for job in &status.jobs {
        let cpu = job
            .cpu_threads
            .map_or("-".to_owned(), |t| format!("{t:.2}"));
        let mem = job.mem_mib.map_or("-".to_owned(), |m| m.to_string());
        out.push_str(&format!(
            "job {}     {} ({}) since {}, {cpu} threads, {mem} MiB\n",
            job.job, job.key, job.class, job.started_at
        ));
    }
    for w in &status.waiting {
        let class = match w.effective_class {
            Some(raised) => format!("{}, as {raised}", w.class),
            None => w.class.to_string(),
        };
        out.push_str(&format!(
            "waiting {} {} ({}, {class}) {:.0}s ({} in line): {}\n",
            w.job,
            w.key,
            w.kind,
            w.waited_s,
            ordinal(w.position),
            w.reason
        ));
    }
    out
}

/// Two lines: each figure with where it came from, then how many runs fed it.
pub fn estimate_text(e: &Estimate) -> String {
    let from = |source: Source| match source {
        Source::History => "history",
        Source::Default => "default",
        Source::Given => "given",
    };
    let peak = e
        .cpu_peak_5s
        .map_or(String::new(), |p| format!(", peak {p:.2} over 5s"));
    let duration = e.duration_s.map_or("-".to_owned(), |d| format!("{d:.1}s"));
    format!(
        "estimate  cpu {:.2} threads{peak} ({}), memory {} MiB ({}), write {:.2} MiB/s ({}), \
         duration {duration} ({})\nsamples   {} measured, {} calm\n",
        e.cpu_threads,
        from(e.source.cpu),
        e.mem_mib,
        from(e.source.mem),
        e.write_mib_s,
        from(e.source.io),
        from(e.source.duration),
        e.samples.measured,
        e.samples.calm,
    )
}

/// One header line and one line per run, `-` for a cost that was not measured, and the worktree
/// it ran in (cut to 12 characters), so no line is over 120 columns.
pub fn table(runs: &[Run]) -> String {
    const MIB: f64 = (1u64 << 20) as f64;
    let mut out = format!(
        "{:>6}  {:<24}  {:>8}  {:>8}  {:>11}  {:>12}  {:>11}  {:>4}  {:>6}  {}\n",
        "id",
        "started_at",
        "wall",
        "cpu_s",
        "peak_cpu_5s",
        "peak_mem_mib",
        "written_mib",
        "exit",
        "cached",
        "where"
    );
    for run in runs {
        let or_dash = |v: Option<String>| v.unwrap_or_else(|| "-".to_owned());
        let mib = |v: Option<i64>| or_dash(v.map(|b| format!("{:.1}", b as f64 / MIB)));
        out.push_str(&format!(
            "{:>6}  {:<24}  {:>8}  {:>8}  {:>11}  {:>12}  {:>11}  {:>4}  {:>6}  {}\n",
            run.id,
            run.started_at,
            format!("{:.1}s", run.wall_ms as f64 / 1000.0),
            or_dash(run.cpu_usec.map(|u| format!("{:.2}", u as f64 / 1e6))),
            or_dash(run.peak_cpu_5s.map(|p| format!("{p:.2}"))),
            mib(run.peak_mem_bytes),
            mib(run.written_bytes),
            or_dash(run.exit_code.map(|c| c.to_string())),
            or_dash(run.cached.map(|c| if c { "yes" } else { "no" }.to_owned())),
            cut_left(worktree(&run.submitter), WHERE_WIDTH),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use taskslash_core::estimate::estimate;
    use taskslash_core::protocol::{Class, Kind, Launch};

    fn run() -> Run {
        Run {
            id: 1,
            key: "t1".into(),
            kind: Kind::Command,
            argv: vec!["sh".into(), "-c".into(), "exit 4".into()],
            cwd: "/work".into(),
            class: Class::Normal,
            submitter: "/work".into(),
            launch: Launch::Plain,
            started_at: "2026-11-16T02:17:44.464Z".into(),
            wall_ms: 1234,
            cpu_usec: None,
            peak_cpu_5s: None,
            peak_mem_bytes: None,
            written_bytes: None,
            exit_code: Some(4),
            machine_cpu_some: None,
            machine_mem_some: None,
            machine_io_some: None,
            cached: None,
            moon_hash: None,
            log_path: None,
        }
    }

    #[test]
    fn a_plain_run_shows_a_dash_for_each_cost() {
        let text = table(&[run()]);
        let line: Vec<&str> = text.lines().nth(1).unwrap().split_whitespace().collect();
        assert_eq!(
            line,
            [
                "1",
                "2026-11-16T02:17:44.464Z",
                "1.2s",
                "-",
                "-",
                "-",
                "-",
                "4",
                "-",
                "work"
            ]
        );
    }

    #[test]
    fn an_estimate_with_no_history_says_default_for_each_figure() {
        let text = estimate_text(&estimate(&[], Kind::Typecheck));
        assert_eq!(
            text,
            "estimate  cpu 2.00 threads (default), memory 3072 MiB (default), \
             write 0.00 MiB/s (default), duration - (default)\n\
             samples   0 measured, 0 calm\n"
        );
    }

    #[test]
    fn a_run_table_line_stays_within_120_columns_for_long_values() {
        let long = Run {
            id: 999_999,
            key: "a-very-long-repository-name/with-a-project:and-a-long-task-name".into(),
            submitter: "/home/someone/dev/a-very-long-worktree-directory-name".into(),
            wall_ms: 9_999_999,
            cpu_usec: Some(99_999_000_000),
            peak_cpu_5s: Some(64.0),
            peak_mem_bytes: Some(99_999 << 20),
            written_bytes: Some(99_999 << 20),
            exit_code: Some(255),
            cached: Some(false),
            ..run()
        };
        let text = table(&[long]);
        for line in text.lines() {
            assert!(
                line.chars().count() <= 120,
                "{} columns: {line}",
                line.len()
            );
        }
        assert!(
            text.lines().nth(1).unwrap().ends_with("  ..ctory-name"),
            "{text}"
        );
    }

    #[test]
    fn a_short_name_is_kept_and_a_long_one_cut_from_the_left() {
        assert_eq!(cut_left("shop", 12), "shop");
        assert_eq!(cut_left("taskslash.tk", 12), "taskslash.tk");
        assert_eq!(cut_left("taskslash.tk-35", 12), "..lash.tk-35");
    }

    #[test]
    fn keys_text_puts_the_key_last_and_whole() {
        let keys = [KeySummary {
            key: "shop/web:test with a long name past every column".into(),
            runs: 12,
            last_started_at: "2026-11-17T09:00:00.000Z".into(),
            last_exit_code: None,
            last_submitter: "/dev/shop".into(),
        }];
        let text = keys_text(&keys);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[0].split_whitespace().collect::<Vec<_>>(),
            ["last_run", "runs", "exit", "where", "key"]
        );
        assert_eq!(
            lines[1],
            "2026-11-17T09:00:00.000Z      12     -  shop          \
             shop/web:test with a long name past every column"
        );
    }
}
