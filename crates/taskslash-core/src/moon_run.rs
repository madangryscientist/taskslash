//! `taskslash moon`: runs moon targets one job per target, each once, in dependency order, so the
//! daemon decides how many run at once instead of moon's `--concurrency`.

use crate::Error;
use crate::client::{Demand, Job, Report, run_job};
use crate::moon::{self, Node};
use crate::protocol::Class;
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// How a node ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Pass,
    Fail,
    /// A node it depends on, directly or not, failed, so it never ran.
    Skipped,
}

impl State {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Skipped => "skipped",
        }
    }
}

/// A node's end: how, how long its command ran, and whether moon replayed it from cache.
#[derive(Debug, Clone, Copy)]
struct End {
    state: State,
    wall: Duration,
    cached: bool,
}

/// Runs `targets` (and every target they depend on) of the moon workspace above `cwd` through the
/// daemon at `socket`, `force` passing `--force` to each. Prints a summary line per node and
/// returns 0 when every node passed, else 1. A usage error when there is no workspace or a target
/// is wrong; an `Err` when moon cannot be found or asked.
pub fn moon(
    socket: &Path,
    cwd: &Path,
    targets: &[String],
    force: bool,
    class: Class,
) -> Result<i32, Error> {
    let root = moon::find_root(cwd)
        .ok_or_else(|| {
            Error::usage(format!(
                "no moon workspace: no .moon/workspace.yml in {} or above",
                cwd.display()
            ))
        })?
        .to_path_buf();
    let binary = moon::moon_binary(&root)?;
    let output = Command::new(&binary)
        .args(["query", "tasks"])
        .current_dir(&root)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| Error::new(format!("cannot run {}: {e}", binary.display())))?;
    if !output.status.success() {
        return Err(Error::new(format!(
            "{} query tasks failed: {}",
            binary.display(),
            output.status
        )));
    }
    let tasks = moon::parse(&String::from_utf8_lossy(&output.stdout))?;
    let nodes = moon::graph(&tasks, targets)?;
    let repo = moon::repo_name(&root);

    let ends = schedule(&nodes, |node| {
        run_node(socket, &root, &binary, &repo, node, force, class)
    });

    let mut out = io::stdout().lock();
    let mut text = String::from("\n");
    for (node, end) in nodes.iter().zip(&ends) {
        text.push_str(&summary_line(&node.target, end));
    }
    out.write_all(text.as_bytes())
        .and_then(|()| out.flush())
        .map_err(|e| Error::new(format!("cannot write the summary: {e}")))?;
    Ok(i32::from(ends.iter().any(|e| e.state != State::Pass)))
}

/// `<target> <pass|fail|skipped> <wall>[ cached]`, a newline.
fn summary_line(target: &str, end: &End) -> String {
    let wall = match end.state {
        State::Skipped => "-".to_owned(),
        _ => format!("{:.1}s", end.wall.as_secs_f64()),
    };
    let cached = if end.cached { " cached" } else { "" };
    format!("{target} {} {wall}{cached}\n", end.state.as_str())
}

/// Runs each node once on a thread of its own, as soon as every node it depends on has passed,
/// and returns how each ended, in `nodes`' order. A node whose dependency failed, directly or not,
/// is never run and ends `Skipped`. `nodes` is in dependency order (each after its `deps`).
fn schedule(nodes: &[Node], run: impl Fn(&Node) -> End + Sync) -> Vec<End> {
    let mut ends: Vec<Option<End>> = vec![None; nodes.len()];
    let mut started = vec![false; nodes.len()];
    thread::scope(|scope| {
        let (done, results) = mpsc::channel();
        let mut running = 0;
        loop {
            for (i, node) in nodes.iter().enumerate() {
                let ready = !started[i]
                    && node
                        .deps
                        .iter()
                        .all(|&d| ends[d].is_some_and(|e| e.state == State::Pass));
                if ready {
                    started[i] = true;
                    running += 1;
                    let done = done.clone();
                    let run = &run;
                    scope.spawn(move || {
                        // A panic (a write to a closed stderr) is that node's failure, never a
                        // node that never reports and leaves the loop waiting.
                        let end =
                            panic::catch_unwind(AssertUnwindSafe(|| run(node))).unwrap_or(End {
                                state: State::Fail,
                                wall: Duration::ZERO,
                                cached: false,
                            });
                        let _ = done.send((i, end));
                    });
                }
            }
            if running == 0 {
                break;
            }
            let Ok((i, end)) = results.recv() else { break };
            running -= 1;
            ends[i] = Some(end);
        }
    });
    ends.into_iter()
        .map(|end| {
            end.unwrap_or(End {
                state: State::Skipped,
                wall: Duration::ZERO,
                cached: false,
            })
        })
        .collect()
}

/// Runs one node as a daemon job: `<moon> run <target> --upstream none [--force]`.
fn run_node(
    socket: &Path,
    root: &Path,
    binary: &Path,
    repo: &str,
    node: &Node,
    force: bool,
    class: Class,
) -> End {
    let mut argv: Vec<OsString> = vec![
        binary.into(),
        "run".into(),
        node.target.clone().into(),
        "--upstream".into(),
        "none".into(),
    ];
    if force {
        argv.push("--force".into());
    }
    let job = Job {
        name: Some(format!("{repo}/{}", node.target)),
        kind: moon::kind(&node.task),
        class,
        demand: Demand::default(),
        argv: &argv,
        cwd: root,
        label: Some(&node.target),
    };
    let mut wall = Duration::ZERO;
    let result = run_job(socket, &job, |command| {
        let clock = Instant::now();
        let ran = exec(command, &node.target);
        wall = clock.elapsed();
        let (status, cached) = ran?;
        let report = Report {
            cached: Some(cached),
            moon_hash: last_hash(root, node),
        };
        Ok((status, report))
    });
    match result {
        Ok((code, report)) => End {
            state: if code == 0 { State::Pass } else { State::Fail },
            wall,
            cached: report.cached == Some(true),
        },
        Err(e) => {
            eprintln!("taskslash: {}: {e}", node.target);
            End {
                state: State::Fail,
                wall,
                cached: false,
            }
        }
    }
}

/// Runs `command` with its stdout passed through line by line, and says whether moon replayed
/// `target` from its cache: moon prints `<target> (cached, <hash>)` then. (The target's
/// `lastRun.json` cannot say: moon rewrites its `lastRunTime` on a cache hit too.)
fn exec(mut command: Command, target: &str) -> io::Result<(ExitStatus, bool)> {
    let mut child = command.stdout(Stdio::piped()).spawn()?;
    let mut cached = false;
    if let Some(stdout) = child.stdout.take() {
        let marker = format!("{target} (cached");
        for line in BufReader::new(stdout).split(b'\n') {
            let Ok(mut line) = line else { break };
            cached |= plain(&line).contains(&marker);
            line.push(b'\n');
            // A closed stdout is no reason to stop the job; keep reading so it never blocks.
            let _ = io::stdout().lock().write_all(&line);
        }
    }
    Ok((child.wait()?, cached))
}

/// `line` as text with its terminal escape sequences (`ESC [ ... <letter>`) taken out.
fn plain(line: &[u8]) -> String {
    let text = String::from_utf8_lossy(line);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The hash in `node`'s `.moon/cache/states/<project>/<task>/lastRun.json`, when it names this
/// target and the hash is not empty (an uncached task has none).
fn last_hash(root: &Path, node: &Node) -> Option<String> {
    let path: PathBuf = [
        ".moon",
        "cache",
        "states",
        &node.project,
        &node.task,
        "lastRun.json",
    ]
    .iter()
    .fold(root.to_path_buf(), |path, part| path.join(part));
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    (value.get("target")?.as_str()? == node.target).then_some(())?;
    let hash = value.get("hash")?.as_str()?;
    (!hash.is_empty()).then(|| hash.to_owned())
}

#[cfg(test)]
mod tests;
