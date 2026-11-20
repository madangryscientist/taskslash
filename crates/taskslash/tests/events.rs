//! `taskslash events` and the `subscribe` stream through a real `taskslash daemon`, each test with
//! its own socket, database and fake machine (so admission does not depend on this machine's real pressure and use).

use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

mod common;

const BIN: &str = env!("CARGO_BIN_EXE_taskslash");

/// A daemon on a socket, database and fake machine in a directory of its own; killed and removed
/// on drop.
struct Daemon {
    dir: PathBuf,
    child: Child,
}

impl Daemon {
    fn start() -> Self {
        Self::under(0.0)
    }

    /// A daemon whose fake machine has CPU `some avg10` at `cpu_some_avg10` from its first sample.
    fn under(cpu_some_avg10: f64) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tke-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        common::write_machine(&dir.join("machine.json"), cpu_some_avg10);
        let log = fs::File::create(dir.join("daemon.log")).unwrap();
        let mut child = command(&dir, &["daemon"])
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .expect("the daemon starts");
        let deadline = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(dir.join("d.sock")).is_err() {
            if let Some(status) = child.try_wait().unwrap() {
                let log = fs::read_to_string(dir.join("daemon.log")).unwrap_or_default();
                panic!("the daemon exited early: {status}: {log}");
            }
            assert!(
                Instant::now() < deadline,
                "the daemon did not accept in 5 s"
            );
            thread::sleep(Duration::from_millis(20));
        }
        Self { dir, child }
    }

    fn command(&self, args: &[&str]) -> Command {
        command(&self.dir, args)
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("the binary runs")
    }

    fn log(&self) -> String {
        fs::read_to_string(self.dir.join("daemon.log")).unwrap_or_default()
    }

    /// Waits up to 5 s for the daemon's log to say `line` `count` times.
    fn wait_for(&self, line: &str, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.log().lines().filter(|l| *l == line).count() < count {
            assert!(
                Instant::now() < deadline,
                "no {count} x {line:?} in 5 s: {}",
                self.log()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Starts `taskslash events` and waits until the daemon has it subscribed (`count` subscribers
    /// in all). Its stdout lines arrive on the receiver.
    fn events(&self, count: usize) -> (Child, Receiver<String>) {
        let mut child = self
            .command(&["events"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("taskslash events starts");
        let stdout = child.stdout.take().unwrap();
        let (lines, received) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if lines.send(line).is_err() {
                    break;
                }
            }
        });
        self.wait_for("subscriber connected", count);
        (child, received)
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.kill();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn command(dir: &std::path::Path, args: &[&str]) -> Command {
    let mut command = Command::new(BIN);
    command
        .args(args)
        .env("TASKSLASH_SOCKET", dir.join("d.sock"))
        .env("TASKSLASH_DB", dir.join("runs.db"))
        .env("TASKSLASH_FAKE_MACHINE", dir.join("machine.json"))
        .stdin(Stdio::null());
    command
}

/// The next `n` event lines, parsed, each within 10 s.
fn next(events: &Receiver<String>, n: usize) -> Vec<Value> {
    (0..n)
        .map(|_| {
            let line = events
                .recv_timeout(Duration::from_secs(10))
                .expect("an event line within 10 s");
            serde_json::from_str(&line).unwrap_or_else(|e| panic!("{line:?}: {e}"))
        })
        .collect()
}

fn text<'a>(value: &'a Value, path: &str) -> &'a str {
    value
        .pointer(path)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{path} is not a string in {value}"))
}

fn int(value: &Value, path: &str) -> i64 {
    value
        .pointer(path)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("{path} is not an integer in {value}"))
}

/// Waits up to 5 s for `child` to exit and returns its code.
fn exit_code(child: &mut Child) -> Option<i32> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code();
        }
        assert!(Instant::now() < deadline, "it did not exit in 5 s");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn events_shows_a_job_queued_started_and_finished_and_exits_0_when_the_daemon_goes() {
    let mut daemon = Daemon::start();
    let (mut events, lines) = daemon.events(1);
    let out = daemon.run(&["run", "--name", "ev", "--", "true"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");

    let got = next(&lines, 3);
    let types: Vec<&str> = got.iter().map(|e| text(e, "/event/type")).collect();
    assert_eq!(types, ["queued", "started", "finished"], "{got:?}");
    for event in &got {
        assert_eq!(text(event, "/type"), "event", "{event}");
        assert_eq!(int(event, "/protocol"), 1, "{event}");
        assert!(text(event, "/event/at").ends_with('Z'), "{event}");
    }
    let seqs: Vec<i64> = got.iter().map(|e| int(e, "/seq")).collect();
    assert_eq!(seqs, [1, 2, 3]);
    let job = int(&got[0], "/event/job");
    assert!(got.iter().all(|e| int(e, "/event/job") == job), "{got:?}");

    let (queued, started, finished) = (&got[0], &got[1], &got[2]);
    assert_eq!(text(queued, "/event/key"), "ev");
    assert_eq!(text(queued, "/event/kind"), "command");
    assert_eq!(text(queued, "/event/class"), "normal");
    assert_eq!(
        queued.pointer("/event/argv"),
        Some(&serde_json::json!(["true"]))
    );
    assert!(!text(queued, "/event/cwd").is_empty());
    assert!(!text(queued, "/event/submitter").is_empty());
    assert!(!text(started, "/event/unit").is_empty());
    assert!(!text(started, "/event/launch").is_empty());
    assert_eq!(text(started, "/event/log"), text(finished, "/event/log"));
    assert_eq!(text(finished, "/event/key"), "ev");
    assert_eq!(int(finished, "/event/exit_code"), 0);
    assert!(int(finished, "/event/wall_ms") >= 0);

    let out = daemon.run(&["history", "ev", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let history: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(int(finished, "/event/run"), int(&history, "/runs/0/id"));
    assert_eq!(
        text(finished, "/event/log"),
        text(&history, "/runs/0/log_path")
    );

    daemon.kill();
    assert_eq!(exit_code(&mut events), Some(0));
}

#[test]
fn a_job_that_leaves_the_queue_still_gets_its_finished() {
    // CPU pressure over its pressure gate holds every job in the queue.
    let daemon = Daemon::under(90.0);
    let (mut events, lines) = daemon.events(1);
    let mut held = daemon
        .command(&["run", "--name", "held", "--", "true"])
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let queued = next(&lines, 1);
    assert_eq!(text(&queued[0], "/event/type"), "queued", "{queued:?}");
    let job = int(&queued[0], "/event/job");
    daemon.wait_for(
        &format!("job {job} waiting (1st): cpu pressure 90% over the 40% gate"),
        1,
    );
    held.kill().unwrap();
    held.wait().unwrap();

    let finished = &next(&lines, 1)[0];
    assert_eq!(text(finished, "/event/type"), "finished", "{finished}");
    assert_eq!(int(finished, "/event/job"), int(&queued[0], "/event/job"));
    for field in ["exit_code", "run", "log"] {
        assert_eq!(
            finished.pointer(&format!("/event/{field}")),
            Some(&Value::Null),
            "{finished}"
        );
    }
    assert_eq!(int(finished, "/event/wall_ms"), 0);
    events.kill().unwrap();
    events.wait().unwrap();
}

/// The wiring: a subscriber that never reads is connected while a job runs. A few events fit in
/// the socket's buffer, so this does not fill its queue; the `Bus` unit tests in the daemon prove
/// that a full queue never makes `publish` wait.
#[test]
fn a_subscriber_that_never_reads_does_not_hold_up_a_job() {
    let daemon = Daemon::start();
    let mut stuck = UnixStream::connect(daemon.dir.join("d.sock")).unwrap();
    stuck.write_all(b"{\"type\":\"subscribe\"}\n").unwrap();
    daemon.wait_for("subscriber connected", 1);
    let began = Instant::now();
    let out = daemon.run(&["run", "--", "true"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert!(
        began.elapsed() < Duration::from_secs(5),
        "the job took {:?}",
        began.elapsed()
    );
    drop(stuck);
}

#[test]
fn a_raw_subscriber_gets_subscribed_first() {
    let daemon = Daemon::start();
    let stream = UnixStream::connect(daemon.dir.join("d.sock")).unwrap();
    let mut writer = stream.try_clone().unwrap();
    writer.write_all(b"{\"type\":\"subscribe\"}\n").unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert_eq!(line, "{\"type\":\"subscribed\",\"protocol\":1}\n");
}

#[test]
fn events_exits_1_when_no_daemon_answers() {
    let dir = std::env::temp_dir().join(format!("tke-{}-none", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let out = command(&dir, &["events"]).output().unwrap();
    let _ = fs::remove_dir_all(&dir);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no daemon at"), "{stderr}");
    assert!(out.stdout.is_empty());
}
