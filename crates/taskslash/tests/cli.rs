use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_taskslash"))
        .args(args)
        .output()
        .expect("the binary runs")
}

#[test]
fn prints_its_version() {
    let out = run(&["--version"]);
    assert!(out.status.success());
    let expected = format!("taskslash {}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}

#[test]
fn a_usage_error_exits_2() {
    let out = run(&["nope"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn an_unknown_class_is_a_usage_error_from_the_flag_or_the_environment() {
    let out = run(&["run", "--class", "high", "--", "true"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let out = Command::new(env!("CARGO_BIN_EXE_taskslash"))
        .args(["run", "--", "true"])
        .env("TASKSLASH_CLASS", "high")
        .env("TASKSLASH_SOCKET", "/nonexistent/taskslash.sock")
        .output()
        .expect("the binary runs");
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("TASKSLASH_CLASS"), "{err}");
}
