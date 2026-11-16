//! `taskslash history` through a real `taskslash daemon`, each test with its own socket and
//! database. Each environment asserts the launch path it has: the scope path where there is a
//! systemd user session (this desktop), the plain path where there is not (CI).

use serde_json::Value;
use std::fs;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_taskslash");
const MIB: f64 = (1u64 << 20) as f64;

/// A directory of its own under the temp dir, holding the socket and the database. Removed on drop.
struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tkh-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(BIN);
        command
            .args(args)
            .env("TASKSLASH_SOCKET", self.dir.join("d.sock"))
            .env("TASKSLASH_DB", self.dir.join("runs.db"))
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("the binary runs")
    }

    /// The runs of `key`, from `history --json`.
    fn history(&self, key: &str) -> Vec<Value> {
        let out = self.run(&["history", key, "--json"]);
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        match serde_json::from_slice(&out.stdout).expect("history prints JSON") {
            Value::Array(runs) => runs,
            other => panic!("history printed {other}"),
        }
    }

    /// Starts `taskslash daemon` with its stderr in `daemon.log` and waits up to 5 s for it.
    fn daemon(self) -> Daemon {
        let log = fs::File::create(self.dir.join("daemon.log")).unwrap();
        let child = self
            .command(&["daemon"])
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .expect("the daemon starts");
        let mut daemon = Daemon {
            child,
            scratch: self,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(daemon.scratch.dir.join("d.sock")).is_err() {
            if let Some(status) = daemon.child.try_wait().unwrap() {
                panic!("the daemon exited early: {status}: {}", daemon.log());
            }
            assert!(
                Instant::now() < deadline,
                "the daemon did not accept in 5 s"
            );
            thread::sleep(Duration::from_millis(20));
        }
        eprintln!("daemon: {}", daemon.log().trim());
        daemon
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// A running daemon, killed on drop (before its scratch directory is removed).
struct Daemon {
    child: Child,
    scratch: Scratch,
}

impl Daemon {
    fn log(&self) -> String {
        fs::read_to_string(self.scratch.dir.join("daemon.log")).unwrap_or_default()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Whether this machine can start a transient systemd user scope, asked the way the daemon asks.
fn has_user_session() -> bool {
    Command::new("systemd-run")
        .args(["--user", "--scope", "--quiet", "--", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn number(run: &Value, field: &str) -> f64 {
    run[field]
        .as_f64()
        .unwrap_or_else(|| panic!("{field} is not a number in {run}"))
}

fn assert_between(what: &str, value: f64, low: f64, high: f64) {
    assert!(
        (low..=high).contains(&value),
        "{what} is {value}, not between {low} and {high}"
    );
}

#[test]
fn a_run_is_recorded_with_its_exit_code_and_wall_time() {
    let daemon = Scratch::new().daemon();
    let out = daemon
        .scratch
        .run(&["run", "--name", "t1", "--", "sh", "-c", "exit 4"]);
    assert_eq!(out.status.code(), Some(4), "{out:?}");
    let runs = daemon.scratch.history("t1");
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0]["exit_code"], 4, "{}", runs[0]);
    assert!(number(&runs[0], "wall_ms") > 0.0, "{}", runs[0]);
    assert_eq!(runs[0]["argv"], serde_json::json!(["sh", "-c", "exit 4"]));
}

#[test]
fn a_run_costs_what_it_used_on_the_path_this_machine_has() {
    let scoped = has_user_session();
    eprintln!("launch path: {}", if scoped { "scope" } else { "plain" });
    let daemon = Scratch::new().daemon();
    let probe = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/probe.py");
    let dir = daemon.scratch.dir.to_string_lossy().into_owned();
    let out = daemon.scratch.run(&[
        "run",
        "--name",
        "t2",
        "--",
        "python3",
        &probe.to_string_lossy(),
        "2",
        "200",
        "50",
        &dir,
    ]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let runs = daemon.scratch.history("t2");
    assert_eq!(runs.len(), 1, "{runs:?}");
    let run = &runs[0];
    eprintln!("t2: {run}");
    assert_eq!(run["exit_code"], 0, "{run}");

    if !scoped {
        assert_eq!(run["launch"], "plain", "{run}");
        assert!(
            daemon
                .log()
                .contains("no systemd user session, jobs run unmeasured"),
            "{}",
            daemon.log()
        );
        for field in ["cpu_usec", "peak_cpu_5s", "peak_mem_bytes", "written_bytes"] {
            assert!(run[field].is_null(), "{field} is not null in {run}");
        }
        return;
    }
    assert_eq!(run["launch"], "scope", "{run}");
    assert_between("CPU seconds", number(run, "cpu_usec") / 1e6, 3.4, 4.6);
    assert_between(
        "peak memory MiB",
        number(run, "peak_mem_bytes") / MIB,
        200.0,
        300.0,
    );
    assert_between(
        "MiB written",
        number(run, "written_bytes") / MIB,
        40.0,
        80.0,
    );
}

#[test]
fn history_with_no_daemon_exits_1() {
    let scratch = Scratch::new();
    let out = scratch.run(&["history", "t1"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("no daemon at"));
}

#[test]
fn plain_history_prints_a_header_and_a_line_per_run() {
    let daemon = Scratch::new().daemon();
    for code in ["0", "3"] {
        let script = format!("exit {code}");
        daemon
            .scratch
            .run(&["run", "--name", "t3", "--", "sh", "-c", &script]);
    }
    let out = daemon.scratch.run(&["history", "t3"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    let header: Vec<&str> = lines[0].split_whitespace().collect();
    assert_eq!(
        header,
        [
            "started_at",
            "wall",
            "cpu_s",
            "peak_cpu_5s",
            "peak_mem_mib",
            "written_mib",
            "exit"
        ]
    );
    // Newest first.
    assert!(lines[1].ends_with(" 3"), "{text}");
    assert!(lines[2].ends_with(" 0"), "{text}");
}
