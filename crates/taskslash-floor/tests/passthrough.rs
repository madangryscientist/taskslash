//! `taskslash-floor` passes its command's exit code and output through unchanged, and
//! `floor-bench` skips the `taskslash` row when no daemon answers. No network, no daemon.

use std::process::{Command, Output, Stdio};

const FLOOR: &str = env!("CARGO_BIN_EXE_taskslash-floor");
const BENCH: &str = env!("CARGO_BIN_EXE_floor-bench");

fn floor(args: &[&str]) -> Output {
    Command::new(FLOOR)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("taskslash-floor runs")
}

#[test]
fn exit_codes_come_back_unchanged() {
    for code in [0, 3, 42] {
        let out = floor(&["--", "sh", "-c", &format!("exit {code}")]);
        assert_eq!(out.status.code(), Some(code));
    }
}

#[test]
fn a_signal_death_exits_128_plus_the_signal() {
    let out = floor(&["--", "sh", "-c", "kill -TERM $$"]);
    assert_eq!(out.status.code(), Some(128 + 15));
}

#[test]
fn stdout_and_stderr_arrive_on_their_own_streams() {
    let out = floor(&["--", "sh", "-c", "echo out; echo err >&2; exit 3"]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(out.stdout, b"out\n");
    assert_eq!(out.stderr, b"err\n");
}

#[test]
fn a_mebibyte_on_each_stream_at_once_arrives_intact() {
    // Both streams are written at the same time, each far past a pipe's buffer: a pass-through
    // that drained one before the other would deadlock here.
    let out = floor(&[
        "--",
        "sh",
        "-c",
        "(yes err | head -c 1048576) >&2 & yes out | head -c 1048576; wait",
    ]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(out.stdout.len(), 1 << 20);
    assert_eq!(out.stderr.len(), 1 << 20);
    assert!(out.stdout.chunks(4).all(|c| b"out\n".starts_with(c)));
    assert!(out.stderr.chunks(4).all(|c| b"err\n".starts_with(c)));
}

#[test]
fn a_missing_command_exits_127() {
    let out = floor(&["--", "taskslash-floor-no-such-command"]);
    assert_eq!(out.status.code(), Some(127));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot start"));
}

#[test]
fn no_command_exits_127() {
    assert_eq!(floor(&[]).status.code(), Some(127));
    assert_eq!(floor(&["--"]).status.code(), Some(127));
}

#[test]
fn the_taskslash_row_is_skipped_when_no_daemon_answers() {
    // `false status --json` exits 1, as `taskslash status --json` does with no daemon.
    let out = Command::new(BENCH)
        .args(["--runs", "3", "--taskslash", "false"])
        .stdin(Stdio::null())
        .output()
        .expect("floor-bench runs");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 4, "{stdout}");
    for (line, name) in lines.iter().zip(["bare", "floor"]) {
        assert!(line.starts_with(name), "{line}");
        for field in ["min", "median", "p95", "p99", "max"] {
            assert!(line.contains(field), "{line}");
        }
    }
    assert!(lines[2].starts_with("taskslash"), "{}", lines[2]);
    assert!(
        lines[2].contains("skipped: daemon not answering"),
        "{}",
        lines[2]
    );
    let json = lines[3];
    assert!(json.starts_with('{') && json.ends_with('}'), "{json}");
    for key in [
        "\"runs\":3",
        "\"floor_version\":",
        "\"taskslash_version\":null",
        "\"commit\":",
        "\"load_1m\":",
        "\"skipped\":\"skipped: daemon not answering\"",
    ] {
        assert!(json.contains(key), "{key} not in {json}");
    }
}
