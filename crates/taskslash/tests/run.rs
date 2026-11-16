//! `taskslash run` through a real `taskslash daemon`, each test on its own socket.

use std::fs;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_taskslash");

/// A directory of its own under the temp dir, holding the socket path. Removed on drop.
struct Scratch {
    dir: PathBuf,
    socket: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tks-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let socket = dir.join("d.sock");
        assert!(
            socket.as_os_str().len() < 108,
            "a unix socket path must stay under 108 bytes: {}",
            socket.display()
        );
        Self { dir, socket }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(BIN);
        command
            .args(args)
            .env("TASKSLASH_SOCKET", &self.socket)
            .env("TASKSLASH_DB", self.dir.join("runs.db"))
            .stdin(Stdio::null());
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("the binary runs")
    }

    /// Starts `taskslash daemon` on this socket and waits up to 5 s for it to accept.
    fn daemon(self) -> Daemon {
        let child = self
            .command(&["daemon"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the daemon starts");
        let mut daemon = Daemon {
            child,
            scratch: self,
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(&daemon.scratch.socket).is_err() {
            if let Some(status) = daemon.child.try_wait().unwrap() {
                panic!("the daemon exited early: {status}");
            }
            assert!(
                Instant::now() < deadline,
                "the daemon did not accept in 5 s"
            );
            thread::sleep(Duration::from_millis(20));
        }
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

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn run_true_exits_0() {
    let daemon = Scratch::new().daemon();
    let out = daemon.scratch.run(&["run", "--", "true"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
}

#[test]
fn run_passes_the_exit_code_through() {
    let daemon = Scratch::new().daemon();
    let out = daemon.scratch.run(&["run", "--", "sh", "-c", "exit 3"]);
    assert_eq!(out.status.code(), Some(3), "{out:?}");
}

#[test]
fn a_command_that_cannot_start_exits_127() {
    let daemon = Scratch::new().daemon();
    let out = daemon.scratch.run(&["run", "--", "./no-such-command"]);
    assert_eq!(out.status.code(), Some(127), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot run ./no-such-command"));
}

#[test]
fn run_with_no_daemon_exits_1() {
    let scratch = Scratch::new();
    let out = scratch.run(&["run", "--", "true"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("no daemon at"));
}

#[test]
fn a_second_daemon_refuses_a_live_socket() {
    let daemon = Scratch::new().daemon();
    let out = daemon.scratch.run(&["daemon"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("already running"));
}

#[test]
fn a_daemon_replaces_a_stale_socket() {
    let scratch = Scratch::new();
    drop(UnixListener::bind(&scratch.socket).unwrap());
    assert!(scratch.socket.exists());
    let daemon = scratch.daemon();
    let out = daemon.scratch.run(&["run", "--", "true"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
}

#[test]
fn a_daemon_leaves_a_file_that_is_not_a_socket() {
    let scratch = Scratch::new();
    fs::write(&scratch.socket, "notes").unwrap();
    let out = scratch.run(&["daemon"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("not a socket"));
    assert_eq!(fs::read_to_string(&scratch.socket).unwrap(), "notes");
}

#[test]
fn run_with_no_command_exits_2() {
    let scratch = Scratch::new();
    assert_eq!(scratch.run(&["run"]).status.code(), Some(2));
    assert_eq!(scratch.run(&["run", "--"]).status.code(), Some(2));
}
