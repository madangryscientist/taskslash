# taskslash-floor

The floor: how fast a command can run through a pass-through that does what `taskslash run` does at
the process level, with none of TaskSlash's own work. TaskSlash's overhead is read against it.

`taskslash-floor -- <command> [args...]` starts the command with stdout and stderr on pipes and
stdin inherited, copies each pipe to its own matching stream on one thread each, waits for the
command, and exits with its code (128 plus the signal number when a signal killed it, 127 when there
is no command or it cannot be started). No daemon, no admission, no leases, no log, no
pseudo-terminal, no process group, no signal forwarding.

`floor-bench` times starts of three rows, spawn to exit, and prints min, median, p95, p99 and max in
milliseconds.

## Run it

```bash
bunx moon run floor:bench
```

It builds both binaries in release and runs `floor-bench` with 100 starts per row. Run it by hand
for other options: `target/release/floor-bench --runs 500 --taskslash ~/.cargo/bin/taskslash`.
It is not part of the gate.

Example output (100 runs each, load 3.39):

```
bare       min    0.399  median    0.426  p95    0.546  p99    0.598  max    0.610  ms (100 runs)
floor      min    1.000  median    1.081  p95    1.211  p99    1.243  max    1.263  ms (100 runs)
taskslash  min   26.185  median  128.698  p95  136.851  p99  139.474  max  142.996  ms (100 runs)
{"runs":100,"floor_version":"0.1.0","taskslash_version":"taskslash 0.1.0","commit":"4c389bd","load_1m":3.39,"rows":[...]}
```

## Reading it

- `bare` runs `true` directly: what starting a process costs on this machine.
- `floor` runs `taskslash-floor -- true`: that cost plus a bare Rust pass-through (one extra exec,
  two pipes and two copy threads).
- `taskslash` runs `taskslash run --name overhead-probe --cpu 0.1 --mem 10 -- true` against the
  running daemon. When `taskslash status --json` does not exit 0 the row reads
  `skipped: daemon not answering` and the run still succeeds.

`taskslash` minus `floor` is TaskSlash's own overhead: everything it does beyond passing the command
through. The command's output goes to null, so `taskslash run` takes its pipe path, as the floor
does.

The last line is JSON with every row, both versions, the repo commit and the 1-minute load average.
Record a run as a new row in the project's benchmark log with its load: a load average above about 2
means the machine was busy and the numbers read high.
