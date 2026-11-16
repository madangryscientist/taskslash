//! The fake machine sampler, for tests.

use super::{MachineSampler, MachineSnapshot};
use std::fs;
use std::path::{Path, PathBuf};

/// A fake machine for tests (`TASKSLASH_FAKE_MACHINE`): reads a `MachineSnapshot` as JSON from
/// a file on every sample. A file that cannot be read or parsed keeps the last good snapshot.
pub struct FileSampler {
    path: PathBuf,
    last: MachineSnapshot,
}

impl FileSampler {
    /// Fails when the first read fails, so a daemon with a bad file does not start.
    pub fn new(path: PathBuf) -> Result<Self, String> {
        let first = read_snapshot(&path)?;
        Ok(Self { path, last: first })
    }
}

impl MachineSampler for FileSampler {
    fn sample(&mut self) -> MachineSnapshot {
        match read_snapshot(&self.path) {
            Ok(snapshot) => self.last = snapshot,
            Err(e) => eprintln!("taskslash: fake machine kept: {e}"),
        }
        self.last.clone()
    }
}

fn read_snapshot(path: &Path) -> Result<MachineSnapshot, String> {
    let text =
        fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}
