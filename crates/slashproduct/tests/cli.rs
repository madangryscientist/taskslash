use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_slashproduct"))
        .args(args)
        .output()
        .expect("the binary runs")
}

#[test]
fn prints_its_version() {
    let out = run(&["--version"]);
    assert!(out.status.success());
    let expected = format!("slashproduct {}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!(String::from_utf8_lossy(&out.stdout), expected);
}

#[test]
fn greets_by_name() {
    let out = run(&["greet", "Ada"]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "Hello, Ada, from SlashProduct.\n"
    );
}

#[test]
fn a_usage_error_exits_2() {
    let out = run(&["nope"]);
    assert_eq!(out.status.code(), Some(2));
}
