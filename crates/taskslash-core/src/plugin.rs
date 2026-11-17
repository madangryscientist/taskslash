//! Command plugins: `taskslash <name> [args...]` runs the first `taskslash-<name>` executable on
//! `PATH` in place of taskslash, as git and cargo do. Built-ins always win; the caller checks them.
//!
//! The plugin's environment carries the contract: `TASKSLASH_PROTOCOL` (the protocol version),
//! `TASKSLASH_SOCKET` (the daemon socket, when it resolves) and `TASKSLASH_BIN` (this binary).

use crate::Error;
use crate::protocol::PROTOCOL_VERSION;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// What a plugin's program is named: `taskslash-<name>`.
pub const PREFIX: &str = "taskslash-";

/// A plugin name found on `PATH`.
#[derive(Debug, PartialEq, Eq)]
pub struct Plugin {
    pub name: String,
    /// The first one on `PATH`: the one that runs.
    pub path: PathBuf,
    /// Later ones with the same name, in `PATH` order. They never run.
    pub shadowed: Vec<PathBuf>,
    /// A built-in has this name, so the plugin never runs.
    pub builtin: bool,
}

/// A regular file (after following links) with an execute bit.
fn executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The directories of a `PATH` value, in order, without empty entries or repeats.
fn dirs(path: &OsStr) -> Vec<PathBuf> {
    let mut seen = Vec::new();
    for dir in std::env::split_paths(path) {
        if !dir.as_os_str().is_empty() && !seen.contains(&dir) {
            seen.push(dir);
        }
    }
    seen
}

/// The program `taskslash <name>` runs: the first `taskslash-<name>` on `path` that is an
/// executable regular file. None for a name that is empty or holds a `/`.
pub fn find(name: &OsStr, path: &OsStr) -> Option<PathBuf> {
    if name.is_empty() || name.as_bytes().contains(&b'/') {
        return None;
    }
    let mut file = OsString::from(PREFIX);
    file.push(name);
    dirs(path)
        .into_iter()
        .map(|dir| dir.join(&file))
        .find(|candidate| executable(candidate))
}

/// Every plugin on `path`, by name; `builtins` are the names a plugin can never take.
pub fn list(path: &OsStr, builtins: &[String]) -> Vec<Plugin> {
    let mut found: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for dir in dirs(path) {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut here: Vec<(String, PathBuf)> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let file = entry.file_name();
                let name = file.to_str()?.strip_prefix(PREFIX)?.to_owned();
                let path = entry.path();
                (!name.is_empty() && executable(&path)).then_some((name, path))
            })
            .collect();
        here.sort();
        for (name, path) in here {
            found.entry(name).or_default().push(path);
        }
    }
    found
        .into_iter()
        .map(|(name, mut paths)| {
            let path = paths.remove(0);
            Plugin {
                builtin: builtins.contains(&name),
                name,
                path,
                shadowed: paths,
            }
        })
        .collect()
}

/// One line per plugin: `<name>  <path>`, then what shadows or blocks it.
pub fn list_text(plugins: &[Plugin]) -> String {
    let width = plugins.iter().map(|p| p.name.len()).max().unwrap_or(0);
    let mut text = String::new();
    for plugin in plugins {
        text.push_str(&format!(
            "{:width$}  {}",
            plugin.name,
            plugin.path.display()
        ));
        if !plugin.shadowed.is_empty() {
            let shadowed: Vec<String> = plugin
                .shadowed
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            text.push_str(&format!("  (shadows {})", shadowed.join(", ")));
        }
        if plugin.builtin {
            text.push_str("  (a built-in has this name: never run)");
        }
        text.push('\n');
    }
    text
}

/// The plugins as one JSON array, on one line. A path that is not UTF-8 is written lossily.
pub fn list_json(plugins: &[Plugin]) -> String {
    let lossy = |p: &PathBuf| p.to_string_lossy().into_owned();
    let array: Vec<serde_json::Value> = plugins
        .iter()
        .map(|p| {
            serde_json::json!({
                "name": p.name,
                "path": lossy(&p.path),
                "shadowed": p.shadowed.iter().map(lossy).collect::<Vec<_>>(),
                "builtin": p.builtin,
            })
        })
        .collect();
    format!("{}\n", serde_json::Value::Array(array))
}

/// Runs `program` with `args` in place of this process, with the plugin contract in its
/// environment. Returns only when it could not be started.
pub fn exec(program: &Path, args: &[OsString]) -> Error {
    let mut command = Command::new(program);
    command
        .args(args)
        .env("TASKSLASH_PROTOCOL", PROTOCOL_VERSION.to_string());
    match crate::socket_path() {
        Ok(socket) => command.env("TASKSLASH_SOCKET", socket),
        Err(_) => command.env_remove("TASKSLASH_SOCKET"),
    };
    match std::env::current_exe() {
        Ok(bin) => command.env("TASKSLASH_BIN", bin),
        Err(e) => return Error::new(format!("cannot find the taskslash binary: {e}")),
    };
    let e = command.exec();
    Error::new(format!("cannot run {}: {e}", program.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("tkp-{}-{tag}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn sub(&self, name: &str) -> PathBuf {
            let dir = self.0.join(name);
            fs::create_dir_all(&dir).unwrap();
            dir
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(path: &Path, mode: u32) {
        fs::write(path, "#!/bin/sh\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    fn join(dirs: &[&Path]) -> OsString {
        std::env::join_paths(dirs).unwrap()
    }

    #[test]
    fn find_takes_the_first_executable_and_skips_the_rest() {
        let tmp = Dir::new("find");
        let (a, b, c) = (tmp.sub("a"), tmp.sub("b"), tmp.sub("c"));
        write(&a.join("taskslash-x"), 0o644);
        fs::create_dir(b.join("taskslash-x")).unwrap();
        write(&c.join("taskslash-x"), 0o755);
        let path = join(&[&a, &b, &c]);
        assert_eq!(find(OsStr::new("x"), &path), Some(c.join("taskslash-x")));
        assert_eq!(find(OsStr::new("y"), &path), None);
        assert_eq!(find(OsStr::new(""), &path), None);
        assert_eq!(find(OsStr::new("../c/taskslash-x"), &path), None);
    }

    #[test]
    fn find_follows_a_link_to_an_executable() {
        let tmp = Dir::new("link");
        let (a, b) = (tmp.sub("a"), tmp.sub("b"));
        write(&a.join("real"), 0o755);
        symlink(a.join("real"), b.join("taskslash-x")).unwrap();
        symlink(a.join("missing"), a.join("taskslash-y")).unwrap();
        let path = join(&[&a, &b]);
        assert_eq!(find(OsStr::new("x"), &path), Some(b.join("taskslash-x")));
        assert_eq!(find(OsStr::new("y"), &path), None);
    }

    #[test]
    fn list_groups_by_name_marks_shadows_and_builtins() {
        let tmp = Dir::new("list");
        let (a, b) = (tmp.sub("a"), tmp.sub("b"));
        write(&a.join("taskslash-x"), 0o755);
        write(&b.join("taskslash-x"), 0o755);
        write(&b.join("taskslash-run"), 0o700);
        write(&b.join("taskslash-off"), 0o644);
        write(&b.join("taskslash-"), 0o755);
        write(&b.join("other"), 0o755);
        let path = join(&[&a, &b, &a, &tmp.0.join("missing")]);
        let plugins = list(&path, &["run".to_owned()]);
        assert_eq!(
            plugins,
            vec![
                Plugin {
                    name: "run".into(),
                    path: b.join("taskslash-run"),
                    shadowed: vec![],
                    builtin: true,
                },
                Plugin {
                    name: "x".into(),
                    path: a.join("taskslash-x"),
                    shadowed: vec![b.join("taskslash-x")],
                    builtin: false,
                },
            ]
        );
        let text = list_text(&plugins);
        assert_eq!(
            text,
            format!(
                "run  {}  (a built-in has this name: never run)\nx    {}  (shadows {})\n",
                b.join("taskslash-run").display(),
                a.join("taskslash-x").display(),
                b.join("taskslash-x").display()
            )
        );
        let json: serde_json::Value = serde_json::from_str(&list_json(&plugins)).unwrap();
        assert_eq!(json[1]["name"], "x");
        assert_eq!(
            json[1]["shadowed"][0],
            b.join("taskslash-x").to_str().unwrap()
        );
        assert_eq!(json[0]["builtin"], true);
    }
}
