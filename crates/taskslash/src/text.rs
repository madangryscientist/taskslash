//! The plain text forms of `status` and `history`.

use taskslash_core::admission::ordinal;
use taskslash_core::estimate::{Estimate, Source};
use taskslash_core::protocol::{Run, Status};

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

/// One header line and one line per run, `-` for a cost that was not measured.
pub fn table(runs: &[Run]) -> String {
    const MIB: f64 = (1u64 << 20) as f64;
    let mut out = format!(
        "{:>6}  {:<24}  {:>8}  {:>8}  {:>11}  {:>12}  {:>11}  {:>4}  {:>6}\n",
        "id",
        "started_at",
        "wall",
        "cpu_s",
        "peak_cpu_5s",
        "peak_mem_mib",
        "written_mib",
        "exit",
        "cached"
    );
    for run in runs {
        let or_dash = |v: Option<String>| v.unwrap_or_else(|| "-".to_owned());
        let mib = |v: Option<i64>| or_dash(v.map(|b| format!("{:.1}", b as f64 / MIB)));
        out.push_str(&format!(
            "{:>6}  {:<24}  {:>8}  {:>8}  {:>11}  {:>12}  {:>11}  {:>4}  {:>6}\n",
            run.id,
            run.started_at,
            format!("{:.1}s", run.wall_ms as f64 / 1000.0),
            or_dash(run.cpu_usec.map(|u| format!("{:.2}", u as f64 / 1e6))),
            or_dash(run.peak_cpu_5s.map(|p| format!("{p:.2}"))),
            mib(run.peak_mem_bytes),
            mib(run.written_bytes),
            or_dash(run.exit_code.map(|c| c.to_string())),
            or_dash(run.cached.map(|c| if c { "yes" } else { "no" }.to_owned())),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use taskslash_core::estimate::estimate;
    use taskslash_core::protocol::{Class, Kind, Launch};

    #[test]
    fn a_plain_run_shows_a_dash_for_each_cost() {
        let run = Run {
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
        };
        let text = table(&[run]);
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
                "-"
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
}
