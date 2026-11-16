//! A fake machine for the daemon (`TASKSLASH_FAKE_MACHINE`), so admission does not depend on how
//! busy the machine running the tests is: 10 threads, 0 busy, 64 GiB total and available, a roomy
//! disk, and every pressure 0 except CPU `some avg10`.

use std::fs;
use std::path::Path;

/// Writes the fake machine to `path`, replacing it in one rename so the daemon never reads half.
pub fn write_machine(path: &Path, cpu_some_avg10: f64) {
    let calm = r#"{"some_avg10":0.0,"some_avg60":0.0,"full_avg10":0.0,"full_avg60":0.0}"#;
    let cpu = format!(
        r#"{{"some_avg10":{cpu_some_avg10:.1},"some_avg60":0.0,"full_avg10":0.0,"full_avg60":0.0}}"#
    );
    let json = format!(
        r#"{{"sampled_at":"2026-11-16T00:00:00.000Z",
"cpu":{{"threads":10,"threads_online":10,"busy":0.0,"pressure":{cpu}}},
"memory":{{"total_mib":65536,"available_mib":65536,"swap_total_mib":0,"swap_free_mib":0,"pressure":{calm}}},
"io":{{"pressure":{calm},"disks":[{{"name":"fake0","busy":0.0,"write_mib_s":0.0}}]}},
"mounts":[{{"path":"/fake","total_mib":1048576,"free_mib":1000000,"used":0.05}}],
"load":[0.0,0.0,0.0]}}"#
    );
    let partial = path.with_extension("partial");
    fs::write(&partial, json).unwrap();
    fs::rename(&partial, path).unwrap();
}
