//! The queue's order through a real `taskslash daemon` against a fake machine
//! (`TASKSLASH_FAKE_MACHINE`, 10 threads). `TASKSLASH_CPU_SHARE=0.5` makes the CPU budget 5.0
//! threads, so one 4.0 thread job runs at a time and every start is in a pass of its own: the
//! history's start times are then the order the queue gave. `sleep` uses next to nothing, so this
//! holds on either launch path.

mod common;

use serde_json::Value;
use std::fs;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_taskslash");

struct Daemon {
    dir: PathBuf,
    child: Child,
}

impl Daemon {
    fn start() -> Self {
        let dir = std::env::temp_dir().join(format!("tko-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        common::write_machine(&dir.join("machine.json"), 0.0);
        let log = fs::File::create(dir.join("daemon.log")).unwrap();
        let mut child = command(&dir, &["daemon"])
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .expect("the daemon starts");
        let deadline = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(dir.join("d.sock")).is_err() {
            assert!(child.try_wait().unwrap().is_none(), "the daemon exited");
            assert!(
                Instant::now() < deadline,
                "the daemon did not accept in 5 s"
            );
            thread::sleep(Duration::from_millis(20));
        }
        Self { dir, child }
    }

    /// `run --name <name> --class <class> --cpu 4.0 --mem 100 -- <argv>`.
    fn spawn(&self, name: &str, class: &str, argv: &[&str]) -> Child {
        let mut args = vec![
            "run", "--name", name, "--class", class, "--cpu", "4.0", "--mem", "100", "--",
        ];
        args.extend_from_slice(argv);
        command(&self.dir, &args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("taskslash run starts")
    }

    fn json(&self, args: &[&str]) -> Value {
        let out = command(&self.dir, args).output().unwrap();
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// Polls `status --json` until `done` holds, for at most 5 s.
    fn wait_for(&self, done: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let status = self.json(&["status", "--json"]);
            if done(&status) {
                return status;
            }
            let log = fs::read_to_string(self.dir.join("daemon.log")).unwrap_or_default();
            assert!(Instant::now() < deadline, "gave up: {status}\n{log}");
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// The `started_at` of `key`'s one recorded run.
    fn started(&self, key: &str) -> String {
        let history = self.json(&["history", key, "--json"]);
        let runs = history["runs"].as_array().unwrap();
        assert_eq!(runs.len(), 1, "{history}");
        runs[0]["started_at"].as_str().unwrap().to_owned()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn command(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(BIN);
    command
        .args(args)
        .env("TASKSLASH_SOCKET", dir.join("d.sock"))
        .env("TASKSLASH_DB", dir.join("runs.db"))
        .env("TASKSLASH_FAKE_MACHINE", dir.join("machine.json"))
        .env("TASKSLASH_CPU_SHARE", "0.5")
        .env_remove("TASKSLASH_CLASS")
        .stdin(Stdio::null());
    command
}

fn waiting(status: &Value) -> usize {
    status["waiting"].as_array().map_or(0, Vec::len)
}

#[test]
fn an_urgent_job_submitted_behind_five_normal_ones_starts_first() {
    let daemon = Daemon::start();
    let pid = daemon.dir.join("holder.pid");
    let script = format!("echo $$ > {}; exec sleep 30", pid.display());
    let mut holder = daemon.spawn("holder", "normal", &["sh", "-c", &script]);
    daemon.wait_for(|s| s["jobs"].as_array().is_some_and(|j| j.len() == 1));

    let mut jobs = Vec::new();
    for n in 1..=5 {
        jobs.push(daemon.spawn(&format!("normal-{n}"), "normal", &["sleep", "1"]));
        daemon.wait_for(|s| waiting(s) == n);
    }
    jobs.push(daemon.spawn("urgent", "urgent", &["sleep", "1"]));
    let status = daemon.wait_for(|s| waiting(s) == 6);
    let first = &status["waiting"][0];
    assert_eq!(first["key"], "urgent", "{status}");
    assert_eq!(first["class"], "urgent", "{status}");
    assert_eq!(first["position"], 1, "{status}");

    // End the holder once its shell has written its pid.
    let deadline = Instant::now() + Duration::from_secs(5);
    let holder_pid = loop {
        let text = fs::read_to_string(&pid).unwrap_or_default();
        if text.ends_with('\n') {
            break text.trim().to_owned();
        }
        assert!(Instant::now() < deadline, "the holder wrote no pid");
        thread::sleep(Duration::from_millis(20));
    };
    Command::new("kill")
        .args(["-KILL", &holder_pid])
        .status()
        .unwrap();
    holder.wait().unwrap();
    for mut job in jobs {
        assert_eq!(job.wait().unwrap().code(), Some(0));
    }

    let urgent = daemon.started("urgent");
    for n in 1..=5 {
        let normal = daemon.started(&format!("normal-{n}"));
        assert!(urgent < normal, "urgent {urgent} vs normal-{n} {normal}");
    }
}
