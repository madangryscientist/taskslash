//! How a job's command is started: in a transient systemd user scope when this machine has a user
//! session, else as a plain child.

use crate::protocol::Launch;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};

/// The slice every scoped job runs under. `systemd-run --slice=` creates it on first use.
pub const SLICE: &str = "taskslash.slice";

/// `Scope` when `systemd-run --user --scope --quiet -- true` exits 0, else `Plain`. The daemon asks
/// once, at startup.
pub fn probe() -> Launch {
    let ok = Command::new("systemd-run")
        .args(["--user", "--scope", "--quiet", "--", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if ok { Launch::Scope } else { Launch::Plain }
}

/// The command that runs `argv` the way `launch` says. stdio, cwd and env are inherited either way;
/// `systemd-run --scope` runs the command in this process's foreground and passes its exit through.
pub fn command(launch: Launch, unit: &str, argv: &[OsString]) -> Command {
    match launch {
        Launch::Plain => {
            let mut command = Command::new(&argv[0]);
            command.args(&argv[1..]);
            command
        }
        Launch::Scope => {
            let mut command = Command::new("systemd-run");
            command
                .args(["--user", "--scope", "--quiet", "--collect"])
                .arg(format!("--slice={SLICE}"))
                .arg(format!("--unit={unit}"))
                .arg("--")
                .args(argv);
            command
        }
    }
}

/// Whether `program` names an executable file, found the way `execvp` finds it: as a path when it
/// holds a `/`, else on `$PATH`. A scoped command that cannot start makes `systemd-run` exit 1,
/// which a command's own failure also means, so the client checks first and exits 127 as a shell
/// does.
pub fn can_start(program: &Path) -> bool {
    if program.as_os_str().is_empty() {
        return false;
    }
    if program.to_string_lossy().contains('/') {
        return is_executable(program);
    }
    env::var_os("PATH")
        .is_some_and(|path| env::split_paths(&path).any(|dir| is_executable(&dir.join(program))))
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scope_wraps_the_command_in_systemd_run() {
        let argv: Vec<OsString> = vec!["sh".into(), "-c".into(), "exit 4".into()];
        let command = command(Launch::Scope, "taskslash-9-1", &argv);
        assert_eq!(command.get_program(), "systemd-run");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            args,
            [
                "--user",
                "--scope",
                "--quiet",
                "--collect",
                "--slice=taskslash.slice",
                "--unit=taskslash-9-1",
                "--",
                "sh",
                "-c",
                "exit 4"
            ]
        );
    }

    #[test]
    fn can_start_finds_programs_like_execvp() {
        assert!(can_start(Path::new("sh")));
        assert!(can_start(Path::new("/bin/sh")));
        assert!(!can_start(Path::new("./no-such-command")));
        assert!(!can_start(Path::new("no-such-command-anywhere")));
        assert!(!can_start(Path::new("/etc/passwd")));
        assert!(!can_start(Path::new("")));
    }
}
