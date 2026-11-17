//! Admission through a real `taskslash daemon` against a fake machine (`TASKSLASH_FAKE_MACHINE`):
//! 10 threads, so the CPU budget is 8.5 threads and 10% of the machine is 1.0 thread. Each job is
//! a `sleep 30` with `--cpu <x> --mem 100`, killed by the test. `sleep` uses next to nothing, so
//! `max(estimate, observed)` is the estimate on either launch path (scope here, plain in CI).

mod common;

use serde_json::Value;
use std::fs;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_taskslash");

/// A daemon on its own socket, database and fake machine; it and its jobs are killed on drop.
struct Daemon {
    dir: PathBuf,
    child: Child,
    jobs: Vec<(String, Child)>,
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
        let dir = std::env::temp_dir().join(format!("tka-{}-{tag}", std::process::id()));
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
        Self {
            dir,
            child,
            jobs: Vec::new(),
        }
    }

    /// Starts `run --name <name> --cpu <cpu> --mem 100 -- sleep 30`, its stderr in `<name>.err`.
    /// The shell writes its pid to `<name>.pid` and becomes the `sleep`, so the test can end it.
    fn spawn(&mut self, name: &str, cpu: &str) {
        self.spawn_with(name, &["--cpu", cpu, "--mem", "100"]);
    }

    /// `spawn` with `cost` in place of `--cpu <cpu> --mem 100`.
    fn spawn_with(&mut self, name: &str, cost: &[&str]) {
        let pid = self.dir.join(format!("{name}.pid"));
        let script = format!("echo $$ > {}; exec sleep 30", pid.display());
        let err = fs::File::create(self.dir.join(format!("{name}.err"))).unwrap();
        let mut args = vec!["run", "--name", name];
        args.extend_from_slice(cost);
        args.extend_from_slice(&["--", "sh", "-c", &script]);
        let child = Self::command(&self.dir, &args)
            .stdout(Stdio::null())
            .stderr(err)
            .spawn()
            .expect("taskslash run starts");
        self.jobs.push((name.to_owned(), child));
    }

    /// Kills job `name`'s `sleep` and waits for its `taskslash run` to exit. The daemon counts a
    /// job as running before its shell has written its pid, so this waits for the pid first.
    fn end(&mut self, name: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while pid(&self.dir, name).is_none() {
            assert!(Instant::now() < deadline, "{name} wrote no pid in 5 s");
            thread::sleep(Duration::from_millis(20));
        }
        kill_sleep(&self.dir, name);
        let at = self.jobs.iter().position(|(n, _)| n == name).unwrap();
        let (_, mut child) = self.jobs.remove(at);
        child.wait().unwrap();
    }

    fn stderr(&self, name: &str) -> String {
        fs::read_to_string(self.dir.join(format!("{name}.err"))).unwrap_or_default()
    }

    /// Polls `status --json` every 50 ms until `done` holds, for at most `secs`.
    fn wait_for(&self, secs: u64, done: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let status = self.status();
            if done(&status) {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "gave up waiting: {status}\n{}",
                self.log()
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn status(&self) -> Value {
        let out = self.run(&["status", "--json"]);
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn run(&self, args: &[&str]) -> Output {
        Self::command(&self.dir, args).output().unwrap()
    }

    /// The job number of the waiting job `name`, once it waits.
    fn waiting(&self, name: &str) -> u64 {
        let status = self.wait_for(5, |s| listed(s, "waiting", name));
        let waiting = status["waiting"].as_array().unwrap();
        let w = waiting.iter().find(|w| w["key"] == name).unwrap();
        w["job"].as_u64().unwrap()
    }

    fn running(&self, count: usize) {
        self.wait_for(5, |s| {
            s["jobs"].as_array().is_some_and(|j| j.len() == count)
        });
    }

    /// `why <job> --json`.
    fn why(&self, job: u64) -> Value {
        let out = self.run(&["why", &job.to_string(), "--json"]);
        assert_eq!(out.status.code(), Some(0), "{out:?}");
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn log(&self) -> String {
        fs::read_to_string(self.dir.join("daemon.log")).unwrap_or_default()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        for (name, mut child) in self.jobs.drain(..) {
            kill_sleep(&self.dir, &name);
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Kills the `sleep` of job `name` when its pid file is there (a waiting job has none yet).
fn kill_sleep(dir: &Path, name: &str) {
    if let Some(pid) = pid(dir, name) {
        let _ = Command::new("kill").args(["-KILL", &pid]).status();
    }
}

/// The pid job `name`'s shell wrote, once the whole line is there.
fn pid(dir: &Path, name: &str) -> Option<String> {
    let text = fs::read_to_string(dir.join(format!("{name}.pid"))).ok()?;
    text.ends_with('\n').then(|| text.trim().to_owned())
}

/// Whether `status` lists job `name` among `what` (`jobs` or `waiting`).
fn listed(status: &Value, what: &str, name: &str) -> bool {
    status[what]
        .as_array()
        .is_some_and(|list| list.iter().any(|j| j["key"] == name))
}

#[test]
fn spike_5_3_a_40_percent_job_waits_until_two_of_three_builds_end() {
    let mut daemon = Daemon::start("spike");
    for name in ["build-1", "build-2", "build-3"] {
        daemon.spawn(name, "2.33");
    }
    daemon.running(3);
    daemon.spawn("visual", "4.0");
    let job = daemon.waiting("visual");

    // The waiting line is printed when the job is queued.
    let deadline = Instant::now() + Duration::from_secs(3);
    while !daemon.stderr("visual").contains("waiting") {
        assert!(
            Instant::now() < deadline,
            "no waiting line: {}",
            daemon.log()
        );
        thread::sleep(Duration::from_millis(20));
    }
    let line = daemon.stderr("visual");
    eprintln!("visual's stderr: {}", line.trim());
    let start =
        format!("taskslash: job {job} waiting 0s (1st in line): cpu needs 4.0, free 1.5; held by ");
    assert!(line.starts_with(&start), "{line}");
    // It gave --cpu and --mem, so no never-run hint.
    assert!(!line.contains("never run before"), "{line}");
    assert!(line.contains("build-1 (job "), "{line}");
    let why = daemon.why(job);
    assert_eq!(why["kind"], "resource", "{why}");
    assert_eq!(why["resource"], "cpu", "{why}");
    assert_eq!(why["held_by"].as_array().map(Vec::len), Some(3), "{why}");

    // Two builds hold 4.66; 4.66 + 4.0 = 8.66 is still over 8.5.
    daemon.end("build-1");
    daemon.running(2);
    thread::sleep(Duration::from_millis(1500));
    let status = daemon.status();
    assert!(listed(&status, "waiting", "visual"), "{status}");
    assert_eq!(daemon.why(job)["resource"], "cpu");

    // One build holds 2.33; 2.33 + 4.0 = 6.33 fits.
    daemon.end("build-2");
    daemon.wait_for(3, |s| listed(s, "jobs", "visual"));
    let out = daemon.run(&["why", &job.to_string()]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "running\n", "{out:?}");
}

#[test]
fn two_40_percent_jobs_start_together_and_a_third_waits() {
    let mut daemon = Daemon::start("two");
    daemon.spawn("visual-a", "4.0");
    daemon.spawn("visual-b", "4.0");
    daemon.running(2);
    daemon.spawn("visual-c", "4.0");
    let job = daemon.waiting("visual-c");
    let why = daemon.why(job);
    assert_eq!(why["resource"], "cpu", "{why}");
    let out = daemon.run(&["why", &job.to_string()]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("(1st in line): cpu needs 4.0, free 0.5"),
        "{text}"
    );

    // A job the daemon does not hold is not found.
    let out = daemon.run(&["why", "999"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("job 999 not found"),
        "{out:?}"
    );
}

#[test]
fn a_tripped_cpu_pressure_gate_holds_a_small_job_until_it_clears() {
    let mut daemon = Daemon::start("gate");
    let machine = daemon.dir.join("machine.json");
    common::write_machine(&machine, 50.0);
    daemon.wait_for(3, |s| s["machine"]["cpu"]["pressure"]["some_avg10"] == 50.0);
    daemon.spawn("small", "0.5");
    let job = daemon.waiting("small");
    let why = daemon.why(job);
    assert_eq!(why["kind"], "gate", "{why}");
    assert_eq!(why["gate"], "cpu_pressure", "{why}");
    let out = daemon.run(&["why", &job.to_string()]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("cpu pressure 50% over the 40% gate"),
        "{text}"
    );

    common::write_machine(&machine, 0.0);
    daemon.wait_for(3, |s| listed(s, "jobs", "small"));
}

#[test]
fn a_never_run_key_says_its_number_and_assumed_cost_and_why_lists_every_waiting_job() {
    let mut daemon = Daemon::start("never");
    let out = daemon.run(&["why"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "nothing is waiting\n");
    let out = daemon.run(&["why", "--json"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout), "[]\n", "{out:?}");

    let machine = daemon.dir.join("machine.json");
    common::write_machine(&machine, 50.0);
    daemon.wait_for(3, |s| s["machine"]["cpu"]["pressure"]["some_avg10"] == 50.0);
    daemon.spawn_with("fresh", &[]);
    let fresh = daemon.waiting("fresh");

    let deadline = Instant::now() + Duration::from_secs(3);
    while !daemon.stderr("fresh").contains("waiting") {
        assert!(
            Instant::now() < deadline,
            "no waiting line: {}",
            daemon.log()
        );
        thread::sleep(Duration::from_millis(20));
    }
    let line = daemon.stderr("fresh");
    assert_eq!(
        line.trim_end(),
        format!(
            "taskslash: job {fresh} waiting 0s (1st in line): cpu pressure 50% over the 40% gate \
             (never run before: assumed 2.0 threads; --cpu 0.1 says otherwise)"
        ),
        "{line}"
    );

    daemon.spawn("vouched", "0.5");
    let vouched = daemon.waiting("vouched");
    let out = daemon.run(&["why"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    let cwd = std::env::current_dir().unwrap();
    let submitter = taskslash_core::submitter(&cwd);
    let worktree = submitter.file_name().unwrap().to_string_lossy();
    let mut lines: Vec<&str> = text.lines().collect();
    lines.sort_unstable();
    let mut want = [
        format!("job {fresh} {worktree} fresh: cpu pressure 50% over the 40% gate"),
        format!("job {vouched} {worktree} vouched: cpu pressure 50% over the 40% gate"),
    ];
    want.sort_unstable();
    assert_eq!(lines, want, "{text}");
    let out = daemon.run(&["why", "--json"]);
    let all: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(all.as_array().map(Vec::len), Some(2), "{all}");
    assert_eq!(all[0]["reason"]["kind"], "gate", "{all}");
    assert!(all[0]["submitter"].is_string(), "{all}");

    // An ended job is named with its run.
    common::write_machine(&machine, 0.0);
    daemon.wait_for(3, |s| listed(s, "jobs", "vouched"));
    daemon.end("vouched");
    let deadline = Instant::now() + Duration::from_secs(5);
    let text = loop {
        let out = daemon.run(&["why", &vouched.to_string()]);
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        if text.contains("ended: run") {
            assert_eq!(out.status.code(), Some(0), "{out:?}");
            break text;
        }
        assert!(Instant::now() < deadline, "{out:?}");
        thread::sleep(Duration::from_millis(50));
    };
    assert!(
        text.starts_with(&format!("job {vouched} ended: run ")),
        "{text}"
    );
    assert!(
        text.trim_end().ends_with(&format!(
            "of vouched, exit 137 (taskslash logs {vouched} shows its log)"
        )),
        "{text}"
    );
    let out = daemon.run(&["why", &vouched.to_string(), "--json"]);
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["kind"], "ended", "{json}");
    assert_eq!(json["ended"]["as"], "run", "{json}");
}
