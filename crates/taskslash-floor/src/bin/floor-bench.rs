//! `floor-bench [--runs N] [--taskslash <path>]`: times N starts of each row, spawn to exit, and
//! prints min, median, p95, p99 and max in milliseconds, one line per row, then one JSON line with
//! every row, the versions, the repo commit and the 1-minute load average.
//!
//! Rows: `bare` runs `true` directly, `floor` runs it through `taskslash-floor` (the sibling binary
//! in this target directory), `taskslash` through `taskslash run`. The command's output goes to
//! null, so `taskslash run` takes its pipe path, as `taskslash-floor` always does. When
//! `taskslash status --json` does not exit 0 the `taskslash` row is skipped, not failed.

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::time::Instant;

const DEFAULT_RUNS: usize = 100;
const USAGE: &str = "usage: floor-bench [--runs N] [--taskslash <path>]";

/// The value at percentile `p` (0 to 100) of `sorted` (ascending), by nearest rank: the smallest
/// sample with at least `p`% of the samples at or below it. `sorted` is never empty: `--runs` is
/// at least 1 and a failed run ends the benchmark.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    let n = sorted.len();
    // The rank is ceil(p * n / 100), at least 1, multiplied before the divide so a whole-number
    // rank (95 of 100) stays whole.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "sample counts are far below 2^52, so the casts are exact"
    )]
    let rank = (p * n as f64 / 100.0).ceil() as usize;
    sorted[rank.clamp(1, n) - 1]
}

struct Stats {
    min: f64,
    median: f64,
    p95: f64,
    p99: f64,
    max: f64,
}

impl Stats {
    fn of(samples: &mut [f64]) -> Stats {
        samples.sort_by(f64::total_cmp);
        Stats {
            min: percentile(samples, 0.0),
            median: percentile(samples, 50.0),
            p95: percentile(samples, 95.0),
            p99: percentile(samples, 99.0),
            max: percentile(samples, 100.0),
        }
    }
}

enum Outcome {
    Timed(Stats),
    Skipped(&'static str),
}

struct Row {
    name: &'static str,
    outcome: Outcome,
}

/// Times `runs` starts of `argv`, spawn to exit, in milliseconds. Any run that does not exit 0 is
/// an error: a timing of a failed start is not a start.
fn time(argv: &[String], runs: usize) -> Result<Stats, String> {
    let (program, args) = argv.split_first().ok_or("empty command")?;
    let mut samples = Vec::with_capacity(runs);
    for _ in 0..runs {
        let started = Instant::now();
        let status = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| format!("cannot start {program}: {e}"))?;
        let elapsed = started.elapsed();
        if !status.success() {
            return Err(format!("{} exited with {status}", argv.join(" ")));
        }
        samples.push(elapsed.as_secs_f64() * 1000.0);
    }
    Ok(Stats::of(&mut samples))
}

/// The first line `argv` prints on stdout when it exits 0, else `None`.
fn first_line(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Some(text.lines().next().unwrap_or("").trim().to_string())
}

fn answers(program: &str) -> bool {
    Command::new(program)
        .args(["status", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn load_1m() -> Option<f64> {
    fs::read_to_string("/proc/loadavg")
        .ok()?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_or_null(value: Option<&str>) -> String {
    value.map_or_else(|| "null".to_string(), json_string)
}

struct Options {
    runs: usize,
    taskslash: String,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        runs: DEFAULT_RUNS,
        taskslash: "taskslash".to_string(),
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--runs" => {
                let value = args.next().ok_or("--runs needs a number")?;
                options.runs = value
                    .parse()
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or(format!("--runs: not a positive number: {value}"))?;
            }
            "--taskslash" => options.taskslash = args.next().ok_or("--taskslash needs a path")?,
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(options)
}

fn floor_binary() -> Result<PathBuf, String> {
    let me = env::current_exe().map_err(|e| format!("cannot find this binary: {e}"))?;
    let floor = me.with_file_name("taskslash-floor");
    if floor.is_file() {
        Ok(floor)
    } else {
        Err(format!(
            "no {} (build it first: cargo build --release --bins)",
            floor.display()
        ))
    }
}

fn run(options: &Options) -> Result<(), String> {
    let floor = floor_binary()?;
    let strings = |argv: &[&str]| argv.iter().map(ToString::to_string).collect::<Vec<_>>();
    let mut rows = vec![
        Row {
            name: "bare",
            outcome: Outcome::Timed(time(&strings(&["true"]), options.runs)?),
        },
        Row {
            name: "floor",
            outcome: Outcome::Timed(time(
                &strings(&[&floor.to_string_lossy(), "--", "true"]),
                options.runs,
            )?),
        },
    ];
    let taskslash = options.taskslash.as_str();
    let answering = answers(taskslash);
    rows.push(Row {
        name: "taskslash",
        outcome: if answering {
            Outcome::Timed(time(
                &strings(&[
                    taskslash,
                    "run",
                    "--name",
                    "overhead-probe",
                    "--cpu",
                    "0.1",
                    "--mem",
                    "10",
                    "--",
                    "true",
                ]),
                options.runs,
            )?)
        } else {
            Outcome::Skipped("daemon not answering")
        },
    });

    let mut json_rows = Vec::new();
    for row in &rows {
        match &row.outcome {
            Outcome::Timed(s) => {
                println!(
                    "{:<10} min {:>8.3}  median {:>8.3}  p95 {:>8.3}  p99 {:>8.3}  max {:>8.3}  ms ({} runs)",
                    row.name, s.min, s.median, s.p95, s.p99, s.max, options.runs
                );
                json_rows.push(format!(
                    "{{\"name\":{},\"min_ms\":{:.3},\"median_ms\":{:.3},\"p95_ms\":{:.3},\"p99_ms\":{:.3},\"max_ms\":{:.3}}}",
                    json_string(row.name),
                    s.min,
                    s.median,
                    s.p95,
                    s.p99,
                    s.max
                ));
            }
            Outcome::Skipped(why) => {
                println!("{:<10} skipped: {why}", row.name);
                json_rows.push(format!(
                    "{{\"name\":{},\"skipped\":{}}}",
                    json_string(row.name),
                    json_string(&format!("skipped: {why}"))
                ));
            }
        }
    }
    let taskslash_version = answering
        .then(|| first_line(taskslash, &["--version"]))
        .flatten();
    let commit = first_line("git", &["rev-parse", "--short", "HEAD"])
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let load = load_1m().map_or_else(|| "null".to_string(), |l| format!("{l:.2}"));
    println!(
        "{{\"runs\":{},\"floor_version\":{},\"taskslash_version\":{},\"commit\":{},\"load_1m\":{},\"rows\":[{}]}}",
        options.runs,
        json_string(env!("CARGO_PKG_VERSION")),
        json_or_null(taskslash_version.as_deref()),
        json_string(&commit),
        load,
        json_rows.join(",")
    );
    Ok(())
}

fn main() -> ExitCode {
    let options = match parse(env::args().skip(1)) {
        Ok(options) => options,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("floor-bench: {e}");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("floor-bench: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::percentile;

    #[test]
    fn single_sample_is_every_percentile() {
        for p in [0.0, 50.0, 95.0, 99.0, 100.0] {
            assert_eq!(percentile(&[7.0], p), 7.0);
        }
    }

    #[test]
    fn odd_count_median_is_the_middle() {
        let s = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile(&s, 50.0), 3.0);
        assert_eq!(percentile(&s, 95.0), 5.0);
        assert_eq!(percentile(&s, 0.0), 1.0);
    }

    #[test]
    fn even_count_median_is_the_lower_middle() {
        let s = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(percentile(&s, 50.0), 2.0);
        assert_eq!(percentile(&s, 75.0), 3.0);
        assert_eq!(percentile(&s, 99.0), 4.0);
    }

    #[test]
    fn hundred_samples_rank_exactly() {
        let s: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&s, 95.0), 95.0);
        assert_eq!(percentile(&s, 99.0), 99.0);
        assert_eq!(percentile(&s, 100.0), 100.0);
    }
}
