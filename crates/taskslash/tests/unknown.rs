//! One never-seen job at a time, through a real `taskslash daemon` and real `taskslash run`s. The
//! rule does not depend on measurement, so this runs on whichever launch path the daemon chose:
//! `scope` where there is a systemd user session (this desktop), `plain` where there is not (CI).

use serde_json::Value;
use std::fs;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

mod common;

const BIN: &str = env!("CARGO_BIN_EXE_taskslash");

/// A daemon on a socket and database in a directory of its own; killed and removed on drop.
struct Daemon {
    dir: PathBuf,
    child: Child,
}

impl Daemon {
    fn command(dir: &Path, args: &[&str]) -> Command {
        let mut command = Command::new(BIN);
        command
            .args(args)
            .env("TASKSLASH_SOCKET", dir.join("d.sock"))
            .env("TASKSLASH_DB", dir.join("runs.db"))
            .env("TASKSLASH_FAKE_MACHINE", dir.join("machine.json"))
            .stdin(Stdio::null());
        command
    }

    fn start(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("tku-{}-{tag}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        common::write_machine(&dir.join("machine.json"), 0.0);
        let log = fs::File::create(dir.join("daemon.log")).unwrap();
        let mut child = Self::command(&dir, &["daemon"])
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

    /// Starts `taskslash run --name <name>` of a job that stamps the time into `<name>.times`, sleeps
    /// `secs`, and stamps it again.
    fn spawn(&self, name: &str, secs: u32) -> Child {
        let file = self.dir.join(format!("{name}.times"));
        let script = format!(
            "date +%s%N >> {0}; sleep {secs}; date +%s%N >> {0}",
            file.display()
        );
        Self::command(
            &self.dir,
            &["run", "--name", name, "--", "sh", "-c", &script],
        )
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("taskslash run starts")
    }

    /// The job's start and end, in nanoseconds since the epoch.
    fn interval(&self, name: &str) -> (u128, u128) {
        let text = fs::read_to_string(self.dir.join(format!("{name}.times"))).unwrap();
        let stamps: Vec<u128> = text.lines().map(|l| l.trim().parse().unwrap()).collect();
        assert_eq!(stamps.len(), 2, "{name}: {text}");
        (stamps[0], stamps[1])
    }

    /// Polls `status --json` every 50 ms until `done` holds, for at most `secs`.
    fn wait_for(&self, secs: u64, done: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let status = self.status();
            if done(&status) {
                return status;
            }
            assert!(Instant::now() < deadline, "gave up waiting: {status}");
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn status(&self) -> Value {
        let out = Self::command(&self.dir, &["status", "--json"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn log(&self) -> String {
        fs::read_to_string(self.dir.join("daemon.log")).unwrap_or_default()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn finish(child: Child) -> (Output, String) {
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    (out, stderr)
}

fn overlap(a: (u128, u128), b: (u128, u128)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

#[test]
fn two_never_seen_jobs_never_overlap_but_a_seen_one_runs_beside_one() {
    let daemon = Daemon::start("overlap");
    let a = daemon.spawn("new-a", 1);
    let b = daemon.spawn("new-b", 1);

    // While one runs, status lists the other as waiting, with its reason and estimate.
    let status = daemon.wait_for(3, |s| {
        s["waiting"].as_array().is_some_and(|w| !w.is_empty())
    });
    assert_eq!(status["jobs"].as_array().map(Vec::len), Some(1), "{status}");
    let waiting = &status["waiting"][0];
    eprintln!("waiting: {waiting}");
    let reason = waiting["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("another never-seen job is running: new-"),
        "{waiting}"
    );
    assert_eq!(waiting["kind"], "command", "{waiting}");
    assert_eq!(waiting["estimate"]["mem_mib"], 2048, "{waiting}");
    assert_eq!(waiting["estimate"]["source"]["cpu"], "default", "{waiting}");

    let (_, a_err) = finish(a);
    let (_, b_err) = finish(b);
    let (first, second, second_err) = if b_err.contains(" waiting ") {
        ("new-a", "new-b", &b_err)
    } else {
        ("new-b", "new-a", &a_err)
    };
    let first_err = if first == "new-a" { &a_err } else { &b_err };
    assert!(!first_err.contains(" waiting "), "{first}: {first_err}");
    assert!(
        second_err.contains(&format!(
            " waiting 0s (1st in line): another never-seen job is running: {first} (job "
        )),
        "{second}: {second_err}"
    );
    assert!(
        second_err.contains("(never run before: assumed 2.0 threads; --cpu 0.1 says otherwise)"),
        "{second}: {second_err}"
    );
    let (one, two) = (daemon.interval(first), daemon.interval(second));
    assert!(
        !overlap(one, two) && one.1 <= two.0,
        "{first} {one:?} and {second} {two:?} overlap\n{}",
        daemon.log()
    );

    // new-a has finished a run, so it is seen: it may run beside a never-seen one.
    fs::remove_file(daemon.dir.join("new-a.times")).unwrap();
    let a = daemon.spawn("new-a", 2);
    let c = daemon.spawn("new-c", 2);
    let (_, a_err) = finish(a);
    let (_, c_err) = finish(c);
    assert!(!a_err.contains("taskslash: waiting "), "new-a: {a_err}");
    assert!(!c_err.contains("taskslash: waiting "), "new-c: {c_err}");
    let (a, c) = (daemon.interval("new-a"), daemon.interval("new-c"));
    assert!(
        overlap(a, c),
        "new-a {a:?} and new-c {c:?} did not overlap\n{}",
        daemon.log()
    );
}

#[test]
fn a_run_killed_while_waiting_leaves_the_queue_and_never_starts() {
    let daemon = Daemon::start("drop");
    let a = daemon.spawn("new-a", 2);
    // b submits only once a runs, so b is the one that queues.
    daemon.wait_for(3, |s| s["jobs"].as_array().is_some_and(|j| j.len() == 1));
    let mut b = daemon.spawn("new-b", 0);
    daemon.wait_for(3, |s| s["waiting"].as_array().is_some_and(|w| w.len() == 1));
    b.kill().unwrap();
    b.wait().unwrap();
    let status = daemon.wait_for(3, |s| s["waiting"] == serde_json::json!([]));
    assert_eq!(status["jobs"][0]["key"], "new-a", "{status}");
    finish(a);
    assert!(
        daemon.log().contains("job 2 left the queue"),
        "{}",
        daemon.log()
    );
    assert!(
        !daemon.dir.join("new-b.times").exists(),
        "new-b ran: {}",
        daemon.log()
    );
}
