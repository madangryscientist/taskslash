//! The TaskSlash engine. The binary in `crates/taskslash` is its command line.
//!
//! The caller runs the command and the daemon admits and records: `taskslash run` asks the daemon
//! over a unix socket for a job, runs the command as its own child (in a transient systemd scope
//! when the machine has a user session), and reports the exit code. The connection stays open for
//! the job's whole life and is its lease. The daemon measures the scope while it runs and writes a
//! row of what the job cost when it ends. A thread samples the whole machine once a second for
//! `taskslash status` and admission.
//!
//! A job that may not start yet waits in the daemon's queue. `admit` decides, from each key's
//! estimate (its past runs, else its kind's defaults); so far its one rule is that at most one job
//! whose key has never finished a run runs at a time.

mod client;
mod daemon;
pub mod estimate;
mod launch;
mod measure;
pub mod procfs;
pub mod protocol;
pub mod sampler;
mod scheduler;
mod socket;
mod store;

pub use client::{history, run, status};
pub use daemon::serve;
pub use socket::socket_path;
pub use store::db_path;

use std::fmt;

/// A failure the command line prints as `taskslash: <message>` and exits 1 on.
#[derive(Debug)]
pub struct Error(String);

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
