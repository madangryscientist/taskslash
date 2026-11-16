//! `taskslash moon` through a real daemon and the repo's own moon, on a copy of the fixture
//! workspace in `tests/fixtures/moon`: `a:gen` (cache off) appends a line to `a/gen.log` and both
//! `a:typecheck` and `a:test` depend on it; `b:test` passes, `b:typecheck` fails and `b:lint`
//! depends on it.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Output;

mod common;
#[path = "moon/fixture.rs"]
mod fixture;

use fixture::{BIN, Scratch};

/// The summary at the end of `taskslash moon`'s stdout: target to (state, cached).
fn summary(out: &Output) -> BTreeMap<String, (String, bool)> {
    let text = String::from_utf8_lossy(&out.stdout);
    let block = text.rsplit("\n\n").next().unwrap_or_default();
    block
        .lines()
        .map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            assert!(
                (3..=4).contains(&words.len()),
                "a summary line: {line:?}\n{text}"
            );
            (
                words[0].to_owned(),
                (words[1].to_owned(), words.get(3) == Some(&"cached")),
            )
        })
        .collect()
}

fn states(summary: &BTreeMap<String, (String, bool)>) -> BTreeMap<&str, &str> {
    summary
        .iter()
        .map(|(t, (s, _))| (t.as_str(), s.as_str()))
        .collect()
}

/// What a plain `moon run` reports per target it finished: a target with a finished line
/// (`<target> (<time>, <hash>)`) passed, unless moon also says `Task <target> failed to run`.
fn moon_states(out: &Output) -> BTreeMap<String, &'static str> {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let mut states = BTreeMap::new();
    for line in text.lines() {
        let Some(rest) = line
            .trim()
            .strip_prefix("\u{25ae}\u{25ae}\u{25ae}\u{25ae} ")
        else {
            continue;
        };
        if let Some((target, inside)) = rest.split_once(" (")
            && inside.contains(", ")
        {
            states.insert(target.to_owned(), "pass");
        }
    }
    for line in text.lines() {
        if let Some(target) = line
            .split_once("Task ")
            .and_then(|(_, rest)| rest.strip_suffix(" failed to run."))
        {
            states.insert(target.to_owned(), "fail");
        }
    }
    states
}

fn gen_lines(workspace: &Path) -> usize {
    fs::read_to_string(workspace.join("a/gen.log"))
        .map(|text| text.lines().count())
        .unwrap_or(0)
}

#[test]
fn a_shared_uncached_dependency_runs_once_and_a_failure_exits_1() {
    let daemon = Scratch::new().daemon();
    let out = daemon.scratch.taskslash(&["moon", ":typecheck", ":test"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(gen_lines(&daemon.scratch.workspace), 1, "a:gen ran once");
    let summary = summary(&out);
    assert_eq!(
        states(&summary),
        BTreeMap::from([
            ("a:gen", "pass"),
            ("a:test", "pass"),
            ("a:typecheck", "pass"),
            ("b:test", "pass"),
            ("b:typecheck", "fail"),
        ])
    );
    // The summary is in dependency order: a:gen before what needs it.
    let text = String::from_utf8_lossy(&out.stdout);
    let at = |t: &str| text.rfind(&format!("\n{t} ")).unwrap();
    assert!(
        at("a:gen") < at("a:test") && at("a:gen") < at("a:typecheck"),
        "{text}"
    );
}

#[test]
fn a_failure_skips_what_depends_on_it() {
    let daemon = Scratch::new().daemon();
    let out = daemon.scratch.taskslash(&["moon", "b:lint"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(
        states(&summary(&out)),
        BTreeMap::from([("b:lint", "skipped"), ("b:typecheck", "fail")])
    );
}

/// moon 2.6.5 stops at its first failure, so on `:typecheck :test` it finishes only some targets:
/// each of those has the same result here. On `:test`, where nothing fails, the sets are equal.
#[test]
fn the_results_match_moon_run_on_a_fresh_copy() {
    let daemon = Scratch::new().daemon();
    let ours = daemon.scratch.taskslash(&["moon", ":typecheck", ":test"]);
    let ours = summary(&ours);
    let fresh = Scratch::new();
    let theirs = fresh
        .command(
            "node_modules/.bin/moon",
            &["run", "--concurrency", "3", ":typecheck", ":test"],
        )
        .output()
        .unwrap();
    assert_eq!(theirs.status.code(), Some(1), "{theirs:?}");
    let theirs_states = moon_states(&theirs);
    assert_eq!(
        theirs_states.get("b:typecheck"),
        Some(&"fail"),
        "{theirs:?}"
    );
    for (target, state) in &theirs_states {
        assert_eq!(states(&ours).get(target.as_str()), Some(state), "{target}");
    }

    let ours = daemon.scratch.taskslash(&["moon", ":test", "--force"]);
    assert_eq!(ours.status.code(), Some(0), "{ours:?}");
    let fresh = Scratch::new();
    let theirs = fresh
        .command(
            "node_modules/.bin/moon",
            &["run", "--concurrency", "3", ":test"],
        )
        .output()
        .unwrap();
    assert_eq!(theirs.status.code(), Some(0), "{theirs:?}");
    let ours = summary(&ours);
    let ours: BTreeMap<String, &str> = states(&ours)
        .into_iter()
        .map(|(t, s)| (t.to_owned(), s))
        .collect();
    assert_eq!(ours, moon_states(&theirs));
}

#[test]
fn a_cache_hit_is_recorded_as_cached_with_its_hash() {
    let daemon = Scratch::new().daemon();
    let first = daemon.scratch.taskslash(&["moon", "b:test"]);
    let second = daemon.scratch.taskslash(&["moon", "b:test"]);
    assert_eq!(
        summary(&first)["b:test"],
        ("pass".to_owned(), false),
        "{first:?}"
    );
    assert_eq!(
        summary(&second)["b:test"],
        ("pass".to_owned(), true),
        "{second:?}"
    );

    let out = daemon
        .scratch
        .taskslash(&["history", "fixture/b:test", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let history: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let runs = history["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(
        (runs[0]["cached"].as_bool(), runs[1]["cached"].as_bool()),
        (Some(true), Some(false))
    );
    let hash = runs[0]["moon_hash"].as_str().unwrap();
    assert_eq!(hash.len(), 64, "{hash}");
    assert_eq!(
        runs[1]["moon_hash"].as_str(),
        Some(hash),
        "the same inputs, the same hash"
    );
    assert_eq!(runs[0]["kind"], "test");
}

#[test]
fn outside_a_workspace_or_with_a_bad_target_is_a_usage_error() {
    let daemon = Scratch::new().daemon();
    let out = daemon.scratch.taskslash(&["moon", "a:nope"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("a:nope matches no task"));
    let out = daemon
        .scratch
        .command(BIN, &["moon", ":test"])
        .current_dir(&daemon.scratch.dir)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stderr).contains("no moon workspace"));
}
