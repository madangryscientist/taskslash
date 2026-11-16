//! The record of runs: one SQLite table, written only by the daemon.

use crate::Error;
use crate::measure::Cost;
use crate::protocol::{Launch, Run};
use rusqlite::types::Type;
use rusqlite::{Connection, Row, params};
use std::env;
use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// How many runs `history` returns.
pub const HISTORY_LIMIT: usize = 20;

/// `$TASKSLASH_DB` when set, else `$XDG_STATE_HOME/taskslash/taskslash.db`, else
/// `~/.local/state/taskslash/taskslash.db`.
pub fn db_path() -> Result<PathBuf, Error> {
    let set = |name| {
        env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(path) = set("TASKSLASH_DB") {
        return Ok(path);
    }
    let state = set("XDG_STATE_HOME")
        .or_else(|| set("HOME").map(|home| home.join(".local").join("state")))
        .ok_or_else(|| Error::new("set TASKSLASH_DB, XDG_STATE_HOME or HOME"))?;
    Ok(state.join("taskslash").join("taskslash.db"))
}

/// A run's key: `--name` when given, else the first two words of its argv.
pub fn key(name: Option<&str>, argv: &[String]) -> String {
    match name {
        Some(name) => name.to_owned(),
        None => argv
            .iter()
            .take(2)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" "),
    }
}

/// A finished job, as the daemon knows it.
pub struct Finished<'a> {
    pub key: &'a str,
    pub argv: &'a [String],
    pub cwd: &'a str,
    pub launch: Launch,
    pub started_at: SystemTime,
    pub wall_ms: u64,
    pub cost: Cost,
    pub exit_code: Option<i32>,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (creating it and its directory when missing) the database at `path`.
    pub fn open(path: &Path) -> Result<Self, Error> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)
                .map_err(|e| Error::new(format!("cannot create {}: {e}", parent.display())))?;
        }
        let conn = Connection::open(path)
            .map_err(|e| Error::new(format!("cannot open {}: {e}", path.display())))?;
        Self::with(conn)
    }

    #[cfg(test)]
    fn in_memory() -> Self {
        Self::with(Connection::open_in_memory().unwrap()).unwrap()
    }

    fn with(conn: Connection) -> Result<Self, Error> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS runs (
                id INTEGER PRIMARY KEY,
                key TEXT NOT NULL,
                argv TEXT NOT NULL,
                cwd TEXT NOT NULL,
                launch TEXT NOT NULL,
                started_at TEXT NOT NULL,
                wall_ms INTEGER NOT NULL,
                cpu_usec INTEGER,
                peak_cpu_5s REAL,
                peak_mem_bytes INTEGER,
                written_bytes INTEGER,
                exit_code INTEGER
            );
            CREATE INDEX IF NOT EXISTS runs_key ON runs (key, started_at);",
        )
        .map_err(sql)?;
        Ok(Self { conn })
    }

    /// Writes one run and returns its id.
    pub fn record(&self, run: &Finished) -> Result<i64, Error> {
        let argv = serde_json::to_string(run.argv).map_err(|e| Error::new(e.to_string()))?;
        let started_ms = run
            .started_at
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
        let int = |v: Option<u64>| v.map(|v| i64::try_from(v).unwrap_or(i64::MAX));
        self.conn
            .execute(
                "INSERT INTO runs (key, argv, cwd, launch, started_at, wall_ms, cpu_usec,
                    peak_cpu_5s, peak_mem_bytes, written_bytes, exit_code)
                 VALUES (?1, ?2, ?3, ?4, strftime('%Y-%m-%dT%H:%M:%fZ', ?5 / 1000.0, 'unixepoch'),
                    ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    run.key,
                    argv,
                    run.cwd,
                    run.launch.as_str(),
                    started_ms,
                    int(Some(run.wall_ms)),
                    int(run.cost.cpu_usec),
                    run.cost.peak_cpu_5s,
                    int(run.cost.peak_mem_bytes),
                    int(run.cost.written_bytes),
                    run.exit_code,
                ],
            )
            .map_err(sql)?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The newest runs of `key`, at most 20, newest first.
    pub fn history(&self, key: &str) -> Result<Vec<Run>, Error> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT id, key, argv, cwd, launch, started_at, wall_ms, cpu_usec, peak_cpu_5s,
                    peak_mem_bytes, written_bytes, exit_code
                 FROM runs WHERE key = ?1 ORDER BY started_at DESC, id DESC LIMIT ?2",
            )
            .map_err(sql)?;
        let limit = i64::try_from(HISTORY_LIMIT).unwrap_or(i64::MAX);
        statement
            .query_map(params![key, limit], from_row)
            .map_err(sql)?
            .collect::<Result<_, _>>()
            .map_err(sql)
    }
}

fn from_row(row: &Row) -> rusqlite::Result<Run> {
    // A cell that does not parse fails the query by name rather than reading as another run.
    let bad = |column: usize, e: serde_json::Error| {
        rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(e))
    };
    let argv: String = row.get(2)?;
    let launch: String = row.get(4)?;
    Ok(Run {
        id: row.get(0)?,
        key: row.get(1)?,
        argv: serde_json::from_str(&argv).map_err(|e| bad(2, e))?,
        cwd: row.get(3)?,
        launch: serde_json::from_value(launch.into()).map_err(|e| bad(4, e))?,
        started_at: row.get(5)?,
        wall_ms: row.get(6)?,
        cpu_usec: row.get(7)?,
        peak_cpu_5s: row.get(8)?,
        peak_mem_bytes: row.get(9)?,
        written_bytes: row.get(10)?,
        exit_code: row.get(11)?,
    })
}

fn sql(e: rusqlite::Error) -> Error {
    Error::new(format!("database: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn words(argv: &[&str]) -> Vec<String> {
        argv.iter().map(|&a| a.to_owned()).collect()
    }

    #[test]
    fn the_key_is_the_name_else_two_words() {
        let argv = words(&["bunx", "moon", "run", ":test"]);
        assert_eq!(key(Some("gate"), &argv), "gate");
        assert_eq!(key(None, &argv), "bunx moon");
        assert_eq!(key(None, &words(&["true"])), "true");
    }

    fn finished<'a>(argv: &'a [String], launch: Launch, at: u64, cost: Cost) -> Finished<'a> {
        Finished {
            key: "k",
            argv,
            cwd: "/work",
            launch,
            started_at: UNIX_EPOCH + Duration::from_millis(at),
            wall_ms: 12,
            cost,
            exit_code: Some(4),
        }
    }

    #[test]
    fn a_plain_run_is_recorded_with_null_costs() {
        let store = Store::in_memory();
        let argv = words(&["sh", "-c", "exit 4"]);
        store
            .record(&finished(
                &argv,
                Launch::Plain,
                1_763_290_000_123,
                Cost::default(),
            ))
            .unwrap();
        let runs = store.history("k").unwrap();
        assert_eq!(runs.len(), 1);
        let run = &runs[0];
        assert_eq!(run.argv, argv);
        assert_eq!(run.launch, Launch::Plain);
        assert_eq!(run.started_at, "2025-11-16T10:46:40.123Z");
        assert_eq!((run.wall_ms, run.exit_code), (12, Some(4)));
        assert_eq!(run.cpu_usec, None);
        assert_eq!(run.peak_cpu_5s, None);
        assert_eq!(run.peak_mem_bytes, None);
        assert_eq!(run.written_bytes, None);
    }

    #[test]
    fn history_is_newest_first_and_at_most_20() {
        let store = Store::in_memory();
        let argv = words(&["true"]);
        let cost = Cost {
            cpu_usec: Some(5),
            peak_cpu_5s: Some(0.5),
            peak_mem_bytes: Some(6),
            written_bytes: Some(0),
        };
        for at in 0..25 {
            store
                .record(&finished(&argv, Launch::Scope, at * 1000, cost.clone()))
                .unwrap();
        }
        let runs = store.history("k").unwrap();
        assert_eq!(runs.len(), HISTORY_LIMIT);
        assert_eq!(runs[0].started_at, "1970-01-01T00:00:24.000Z");
        assert_eq!(runs[0].cpu_usec, Some(5));
        assert_eq!(runs[0].written_bytes, Some(0));
        assert!(store.history("other").unwrap().is_empty());
    }
}
