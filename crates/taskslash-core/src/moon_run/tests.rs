//! The runner's order, skip rule and summary, with a stand-in for the daemon job.

use super::*;
use std::fs;
use std::path::PathBuf;

fn node(target: &str, deps: &[usize]) -> Node {
    let (project, task) = target.split_once(':').unwrap();
    Node {
        project: project.into(),
        task: task.into(),
        target: target.into(),
        deps: deps.to_vec(),
    }
}

fn end(state: State) -> End {
    End {
        state,
        wall: Duration::from_millis(1200),
        cached: false,
    }
}

#[test]
fn a_failure_skips_only_what_depends_on_it() {
    let nodes = [
        node("a:gen", &[]),
        node("b:typecheck", &[]),
        node("a:test", &[0]),
        node("b:lint", &[1]),
        node("b:docs", &[3]),
        node("b:test", &[]),
    ];
    let ran = std::sync::Mutex::new(Vec::new());
    let ends = schedule(&nodes, |n| {
        ran.lock().unwrap().push(n.target.clone());
        end(if n.target == "b:typecheck" {
            State::Fail
        } else {
            State::Pass
        })
    });
    let states: Vec<&str> = ends.iter().map(|e| e.state.as_str()).collect();
    assert_eq!(
        states,
        ["pass", "fail", "pass", "skipped", "skipped", "pass"]
    );
    let mut ran = ran.into_inner().unwrap();
    ran.sort();
    assert_eq!(
        ran,
        ["a:gen", "a:test", "b:test", "b:typecheck"],
        "each once"
    );
}

#[test]
fn a_summary_line_says_how_long_and_whether_cached() {
    let mut e = end(State::Pass);
    assert_eq!(summary_line("a:test", &e), "a:test pass 1.2s\n");
    e.cached = true;
    assert_eq!(summary_line("a:test", &e), "a:test pass 1.2s cached\n");
    assert_eq!(
        summary_line("b:lint", &end(State::Skipped)),
        "b:lint skipped -\n"
    );
}

#[test]
fn escapes_are_taken_out_before_matching() {
    let line = b"\x1b[1m\xe2\x96\xae\xe2\x96\xae b:test\x1b[0m (\x1b[2mcached\x1b[0m, b8b2)";
    assert!(plain(line).contains("b:test (cached"));
}

#[test]
fn a_node_that_panics_fails_and_the_rest_still_end() {
    let nodes = [
        node("a:gen", &[]),
        node("a:test", &[0]),
        node("b:test", &[]),
    ];
    let ends = schedule(&nodes, |n| {
        assert_ne!(n.target, "a:gen", "a write to a closed stderr");
        end(State::Pass)
    });
    let states: Vec<&str> = ends.iter().map(|e| e.state.as_str()).collect();
    assert_eq!(states, ["fail", "skipped", "pass"]);
}

/// A fresh directory under `TMPDIR` (not a hardcoded `/tmp`), removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let dir = env::temp_dir().join(format!("tkgate-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_workspace_bin_goes_in_front_when_it_exists() {
    let root = Dir::new();
    fs::create_dir_all(root.0.join("node_modules/.bin")).unwrap();
    let inherited = env::join_paths(["/usr/bin", "/bin"]).unwrap();
    let path = gate_path(&root.0, Some(inherited)).expect("a path");
    let dirs: Vec<PathBuf> = env::split_paths(&path).collect();
    assert_eq!(
        dirs,
        [
            root.0.join("node_modules/.bin"),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
        ]
    );
}

#[test]
fn the_path_is_left_as_it_was_without_a_workspace_bin() {
    let root = Dir::new();
    let inherited = OsString::from("/usr/bin:/bin");
    assert_eq!(gate_path(&root.0, Some(inherited.clone())), None);
    assert_eq!(gate_path(&root.0, None), None);
}

#[test]
fn an_unset_path_is_left_unset_even_with_a_workspace_bin() {
    let root = Dir::new();
    fs::create_dir_all(root.0.join("node_modules/.bin")).unwrap();
    assert_eq!(gate_path(&root.0, None), None);
}
