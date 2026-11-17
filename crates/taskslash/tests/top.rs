//! `taskslash top`'s two exits before it takes the screen, through the real binary: no daemon
//! answering is exit 1 with the message `status` prints, checked first; stdout not a terminal (as
//! under `cargo test`) is exit 2.

use std::fs;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

mod common;

const BIN: &str = env!("CARGO_BIN_EXE_taskslash");

fn dir(tag: &str) -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tktop-{}-{tag}-{n}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn taskslash(dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(BIN);
    command
        .args(args)
        .env("TASKSLASH_SOCKET", dir.join("d.sock"))
        .env("TASKSLASH_DB", dir.join("runs.db"))
        .env("TASKSLASH_FAKE_MACHINE", dir.join("machine.json"))
        .stdin(Stdio::null());
    command
}

fn output(dir: &Path, args: &[&str]) -> Output {
    taskslash(dir, args).output().expect("the binary runs")
}

/// A daemon on a fake machine in a directory of its own; killed and removed on drop.
struct Daemon {
    dir: PathBuf,
    child: Child,
}

impl Daemon {
    fn start() -> Self {
        let dir = dir("up");
        common::write_machine(&dir.join("machine.json"), 0.0);
        let mut child = taskslash(&dir, &["daemon"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the daemon starts");
        let deadline = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(dir.join("d.sock")).is_err() {
            assert!(
                child.try_wait().unwrap().is_none(),
                "the daemon exited early"
            );
            assert!(
                Instant::now() < deadline,
                "the daemon did not accept in 5 s"
            );
            thread::sleep(Duration::from_millis(20));
        }
        Self { dir, child }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn top_with_no_daemon_exits_1_with_the_message_status_prints() {
    let dir = dir("none");
    let top = output(&dir, &["top"]);
    let status = output(&dir, &["status"]);
    fs::remove_dir_all(&dir).unwrap();
    assert_eq!(top.status.code(), Some(1), "{top:?}");
    let stderr = String::from_utf8_lossy(&top.stderr);
    assert!(stderr.contains("no daemon at"), "{stderr}");
    assert_eq!(top.stderr, status.stderr);
    assert!(top.stdout.is_empty(), "nothing drawn: {top:?}");
}

#[test]
fn top_with_stdout_not_a_terminal_exits_2_before_drawing() {
    let daemon = Daemon::start();
    let top = output(&daemon.dir, &["top"]);
    assert_eq!(top.status.code(), Some(2), "{top:?}");
    assert_eq!(
        String::from_utf8_lossy(&top.stderr),
        "taskslash: top needs a terminal, and stdout is not one\n"
    );
    assert!(top.stdout.is_empty(), "nothing drawn: {top:?}");
}
