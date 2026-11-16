//! The TaskSlash engine. The binary in `crates/taskslash` is its command line.
//!
//! The caller runs the command and the daemon admits and records: `taskslash run` asks the daemon
//! over a unix socket for a job, runs the command as its own child, and reports the exit code. The
//! connection stays open for the job's whole life and is its lease.

mod client;
mod daemon;
pub mod protocol;
mod socket;

pub use client::run;
pub use daemon::serve;
pub use socket::socket_path;

use std::fmt;

/// A failure the command line prints as `taskslash: <message>` and exits 1 on.
#[derive(Debug)]
pub struct Error(String);

impl Error {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
