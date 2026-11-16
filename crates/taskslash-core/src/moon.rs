//! A moon workspace as `taskslash moon` sees it: where it is, which moon runs it, the repository's
//! name, and the graph of targets to run, read from `moon query tasks` (its JSON: `tasks`, by
//! project then task, each with a `target` and `deps` that each have a `target`).

use crate::Error;
use crate::protocol::Kind;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// The file that marks a moon workspace's root.
const WORKSPACE_FILE: &str = ".moon/workspace.yml";

/// The first directory from `cwd` up, `cwd` itself first, that holds `.moon/workspace.yml`.
pub fn find_root(cwd: &Path) -> Option<&Path> {
    cwd.ancestors()
        .find(|dir| dir.join(WORKSPACE_FILE).is_file())
}

/// `<root>/node_modules/.bin/moon` when it exists, else `moon` on `PATH`.
pub fn moon_binary(root: &Path) -> Result<PathBuf, Error> {
    let local = root.join("node_modules/.bin/moon");
    if local.is_file() {
        return Ok(local);
    }
    env::var_os("PATH")
        .and_then(|path| {
            env::split_paths(&path)
                .map(|dir| dir.join("moon"))
                .find(|moon| moon.is_file())
        })
        .ok_or_else(|| {
            Error::new(format!(
                "no moon: neither {} nor moon on PATH",
                local.display()
            ))
        })
}

/// The repository's name, the same for every worktree of it: the basename of `root` in a clone
/// (`.git` a directory), or of the main checkout a worktree's `.git` file points to
/// (`gitdir: <main>/.git/worktrees/<name>`). Anything else is the basename of `root`.
pub fn repo_name(root: &Path) -> String {
    let main = fs::read_to_string(root.join(".git"))
        .ok()
        .and_then(|text| {
            let gitdir = root.join(text.strip_prefix("gitdir:")?.trim());
            let worktrees = gitdir.parent()?;
            let dot_git = worktrees.parent()?;
            (worktrees.file_name()? == "worktrees" && dot_git.file_name()? == ".git")
                .then(|| dot_git.parent().map(Path::to_path_buf))?
        })
        .unwrap_or_else(|| root.to_path_buf());
    main.file_name().map_or_else(
        || main.to_string_lossy().into_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// The cost class of a moon task, by its name.
pub fn kind(task: &str) -> Kind {
    match task {
        "test" => Kind::Test,
        "typecheck" => Kind::Typecheck,
        "build" => Kind::Build,
        _ => Kind::Command,
    }
}

#[derive(Deserialize)]
struct Query {
    tasks: BTreeMap<String, BTreeMap<String, QueryTask>>,
}

#[derive(Deserialize)]
struct QueryTask {
    target: String,
    #[serde(default)]
    deps: Vec<QueryDep>,
}

#[derive(Deserialize)]
struct QueryDep {
    target: String,
}

/// One task of the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub project: String,
    pub task: String,
    pub target: String,
    pub deps: Vec<String>,
}

/// The tasks in `moon query tasks`'s output, by project then task.
pub fn parse(json: &str) -> Result<Vec<Task>, Error> {
    let query: Query = serde_json::from_str(json)
        .map_err(|e| Error::new(format!("cannot read moon query tasks: {e}")))?;
    Ok(query
        .tasks
        .into_iter()
        .flat_map(|(project, tasks)| {
            tasks.into_iter().map(move |(task, t)| Task {
                project: project.clone(),
                task,
                target: t.target,
                deps: t.deps.into_iter().map(|d| d.target).collect(),
            })
        })
        .collect())
}

/// One target to run, and the nodes (indices into the graph) it runs after.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub project: String,
    pub task: String,
    pub target: String,
    pub deps: Vec<usize>,
}

/// The targets `requested` names (`:task` is that task in every project that has it,
/// `project:task` exactly that one) and every target they depend on, transitively, in dependency
/// order: each node after all of its `deps`. A target that is neither form, or matches nothing, is
/// a usage error naming it; a dependency on a target moon does not list, or a cycle, is an error.
pub fn graph(tasks: &[Task], requested: &[String]) -> Result<Vec<Node>, Error> {
    let by_target: BTreeMap<&str, &Task> = tasks.iter().map(|t| (t.target.as_str(), t)).collect();
    let mut wanted = BTreeSet::new();
    for target in requested {
        let (project, task) = target
            .split_once(':')
            .filter(|(p, t)| !t.is_empty() && !t.contains(':') && !p.starts_with(['#', '^', '~']))
            .ok_or_else(|| {
                Error::usage(format!(
                    "{target:?} is not a target: give :<task> or <project>:<task>"
                ))
            })?;
        let mut matched = false;
        for t in tasks
            .iter()
            .filter(|t| t.task == task && (project.is_empty() || t.project == project))
        {
            matched = true;
            wanted.insert(t.target.as_str());
        }
        if !matched {
            return Err(Error::usage(format!("{target} matches no task")));
        }
    }

    // Every dependency, transitively.
    let mut stack: Vec<&str> = wanted.iter().copied().collect();
    while let Some(target) = stack.pop() {
        for dep in &by_target[target].deps {
            if !by_target.contains_key(dep.as_str()) {
                return Err(Error::new(format!(
                    "{target} depends on {dep}, which moon does not list"
                )));
            }
            if wanted.insert(dep.as_str()) {
                stack.push(dep.as_str());
            }
        }
    }

    // Kahn's order, the smallest ready target first, so the order is the same every time.
    let mut left: BTreeMap<&str, BTreeSet<&str>> = wanted
        .iter()
        .map(|&t| (t, by_target[t].deps.iter().map(String::as_str).collect()))
        .collect();
    let mut order: Vec<&str> = Vec::with_capacity(left.len());
    while !left.is_empty() {
        let ready: Vec<&str> = left
            .iter()
            .filter(|(_, deps)| deps.is_empty())
            .map(|(&t, _)| t)
            .collect();
        if ready.is_empty() {
            let stuck: Vec<&str> = left.keys().copied().collect();
            return Err(Error::new(format!(
                "moon reports a dependency cycle among {}",
                stuck.join(", ")
            )));
        }
        for target in ready {
            left.remove(target);
            for deps in left.values_mut() {
                deps.remove(target);
            }
            order.push(target);
        }
    }

    let index: BTreeMap<&str, usize> = order.iter().enumerate().map(|(i, &t)| (t, i)).collect();
    Ok(order
        .iter()
        .map(|&target| {
            let task = by_target[target];
            Node {
                project: task.project.clone(),
                task: task.task.clone(),
                target: task.target.clone(),
                deps: task.deps.iter().map(|d| index[d.as_str()]).collect(),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests;
