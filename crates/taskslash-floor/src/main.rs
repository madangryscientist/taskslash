//! `taskslash-floor -- <command> [args...]`: runs the command the way `taskslash run` does at the
//! process level (stdout and stderr on pipes, each copied to this process's matching stream, its
//! exit code returned) and does nothing else. No daemon, no admission, no log, no process group,
//! no signal forwarding. It is the floor TaskSlash's own overhead is measured against.

use std::env;
use std::io::{self, Read, Write};
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitCode, Stdio};
use std::thread;

/// The exit code when there is no command to run, or it cannot be started.
const CANNOT_START: u8 = 127;

/// Copies `from` to `to` until `from` ends or `to` is gone. A destination that is gone stops the
/// copy and drops the pipe, so the command gets SIGPIPE as it would writing there itself.
fn copy(mut from: impl Read, mut to: impl Write) {
    let mut buf = [0u8; 64 * 1024];
    loop {
        match from.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => {
                if to.write_all(&buf[..n]).and_then(|()| to.flush()).is_err() {
                    return;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

fn main() -> ExitCode {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--") {
        args.remove(0);
    }
    let Some((program, rest)) = args.split_first() else {
        eprintln!("taskslash-floor: usage: taskslash-floor -- <command> [args...]");
        return ExitCode::from(CANNOT_START);
    };
    let mut child = match Command::new(program)
        .args(rest)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            eprintln!("taskslash-floor: cannot start {program}: {e}");
            return ExitCode::from(CANNOT_START);
        }
    };
    let out = child
        .stdout
        .take()
        .map(|pipe| thread::spawn(move || copy(pipe, io::stdout())));
    let err = child
        .stderr
        .take()
        .map(|pipe| thread::spawn(move || copy(pipe, io::stderr())));
    let status = child.wait();
    for copier in [out, err].into_iter().flatten() {
        // A copier only panics if writing panics; the command's code still stands.
        let _ = copier.join();
    }
    match status {
        Ok(status) => match (status.code(), status.signal()) {
            (Some(code), _) => ExitCode::from(u8::try_from(code & 0xff).unwrap_or(1)),
            (None, Some(signal)) => ExitCode::from(u8::try_from(128 + signal).unwrap_or(255)),
            (None, None) => ExitCode::FAILURE,
        },
        Err(e) => {
            eprintln!("taskslash-floor: waiting for {program}: {e}");
            ExitCode::FAILURE
        }
    }
}
