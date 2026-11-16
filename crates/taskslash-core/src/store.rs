//! The record of runs: one SQLite table, written only by the daemon.

use crate::Error;
use crate::estimate::RunCost;
use crate::measure::Cost;
use crate::protocol::{Kind, Launch, Run};
use rusqlite::types::Type;
use rusqlite::{Connection, Row, params};
use std::env;
use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// How many runs `history` returns.
pub const HISTORY_LIMIT: usize = 20;
/// How many measured runs the estimate is given (it reads at most the newest 10 calm ones).
const COSTS_LIMIT: i64 = 100;

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

/// The mean of the machine's `some avg10` pressures over a run; `None` when no sample fell inside.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RunPressure {
    pub cpu: Option<f64>,
    pub mem: Option<f64>,
    pub io: Option<f64>,
}

/// A finished job, as the daemon knows it.
pub struct Finished<'a> {
    pub key: &'a str,
    pub kind: Kind,
    pub argv: &'a [String],
    pub cwd: &'a str,
    pub launch: Launch,
    pub started_at: SystemTime,
    pub wall_ms: u64,
    pub cost: Cost,
    pub exit_code: Option<i32>,
    pub pressure: RunPressure,
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
                kind TEXT NOT NULL,
                argv TEXT NOT NULL,
                cwd TEXT NOT NULL,
                launch TEXT NOT NULL,
                started_at TEXT NOT NULL,
                wall_ms INTEGER NOT NULL,
                cpu_usec INTEGER,
                peak_cpu_5s REAL,
                peak_mem_bytes INTEGER,
                written_bytes INTEGER,
                exit_code INTEGER,
                machine_cpu_some REAL,
                machine_mem_some REAL,
                machine_io_some REAL
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
                "INSERT INTO runs (key, kind, argv, cwd, launch, started_at, wall_ms, cpu_usec,
                    peak_cpu_5s, peak_mem_bytes, written_bytes, exit_code, machine_cpu_some,
                    machine_mem_some, machine_io_some)
                 VALUES (?1, ?2, ?3, ?4, ?5,
                    strftime('%Y-%m-%dT%H:%M:%fZ', ?6 / 1000.0, 'unixepoch'),
                    ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    run.key,
                    run.kind.as_str(),
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
                    run.pressure.cpu,
                    run.pressure.mem,
                    run.pressure.io,
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
                "SELECT id, key, kind, argv, cwd, launch, started_at, wall_ms, cpu_usec,
                    peak_cpu_5s, peak_mem_bytes, written_bytes, exit_code, machine_cpu_some,
                    machine_mem_some, machine_io_some
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

    /// The newest measured runs of `key` (at most 100), newest first, for its estimate.
    pub fn costs(&self, key: &str) -> Result<Vec<RunCost>, Error> {
        let mut statement = self
            .conn
            .prepare(
                "SELECT wall_ms, cpu_usec, peak_cpu_5s, peak_mem_bytes, written_bytes, exit_code,
                    machine_cpu_some
                 FROM runs WHERE key = ?1 AND cpu_usec IS NOT NULL
                    AND peak_mem_bytes IS NOT NULL AND exit_code IS NOT NULL
                 ORDER BY started_at DESC, id DESC LIMIT ?2",
            )
            .map_err(sql)?;
        statement
            .query_map(params![key, COSTS_LIMIT], |row| {
                Ok(RunCost {
                    wall_ms: row.get(0)?,
                    cpu_usec: row.get(1)?,
                    peak_cpu_5s: row.get(2)?,
                    peak_mem_bytes: row.get(3)?,
                    written_bytes: row.get(4)?,
                    exit_code: row.get(5)?,
                    machine_cpu_some: row.get(6)?,
                })
            })
            .map_err(sql)?
            .collect::<Result<_, _>>()
            .map_err(sql)
    }

    /// Whether `key` is seen: at least one of its runs finished (has an exit code), measured or not.
    pub fn seen(&self, key: &str) -> Result<bool, Error> {
        self.conn
            .query_row(
                "SELECT EXISTS (SELECT 1 FROM runs WHERE key = ?1 AND exit_code IS NOT NULL)",
                params![key],
                |row| row.get(0),
            )
            .map_err(sql)
    }
}

fn from_row(row: &Row) -> rusqlite::Result<Run> {
    // A cell that does not parse fails the query by name rather than reading as another run.
    let bad = |column: usize, e: serde_json::Error| {
        rusqlite::Error::FromSqlConversionFailure(column, Type::Text, Box::new(e))
    };
    let kind: String = row.get(2)?;
    let argv: String = row.get(3)?;
    let launch: String = row.get(5)?;
    Ok(Run {
        id: row.get(0)?,
        key: row.get(1)?,
        kind: serde_json::from_value(kind.into()).map_err(|e| bad(2, e))?,
        argv: serde_json::from_str(&argv).map_err(|e| bad(3, e))?,
        cwd: row.get(4)?,
        launch: serde_json::from_value(launch.into()).map_err(|e| bad(5, e))?,
        started_at: row.get(6)?,
        wall_ms: row.get(7)?,
        cpu_usec: row.get(8)?,
        peak_cpu_5s: row.get(9)?,
        peak_mem_bytes: row.get(10)?,
        written_bytes: row.get(11)?,
        exit_code: row.get(12)?,
        machine_cpu_some: row.get(13)?,
        machine_mem_some: row.get(14)?,
        machine_io_some: row.get(15)?,
    })
}

fn sql(e: rusqlite::Error) -> Error {
    Error::new(format!("database: {e}"))
}

#[cfg(test)]
mod tests;
