//! Where the daemon's socket lives.

use crate::Error;
use std::env;
use std::path::PathBuf;

/// `$TASKSLASH_SOCKET` when set, else `$XDG_RUNTIME_DIR/taskslash/daemon.sock`.
pub fn socket_path() -> Result<PathBuf, Error> {
    if let Some(path) = env::var_os("TASKSLASH_SOCKET").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    match env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir).join("taskslash").join("daemon.sock")),
        None => Err(Error::new("set XDG_RUNTIME_DIR or TASKSLASH_SOCKET")),
    }
}
