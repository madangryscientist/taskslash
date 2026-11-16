# TaskSlash

TaskSlash is admission control for builds and tests on one machine. Several gates and browser runs
started at once (by you, or by coding agents in several worktrees) overload a desktop until tests
time out for reasons that have nothing to do with the code. TaskSlash puts a small daemon in front of
them: each command asks it for a slot, and it keeps the queue and the record of what ran.

This version runs each job in its own systemd scope, measures it, and records what it cost, and the
daemon samples the whole machine once a second. Admission and the queue come next, computed from
those records and readings.

## Use

```bash
taskslash daemon                          # run the daemon (normally under systemd, below)
taskslash run -- bun run test             # run a command as a job
taskslash run --name gate -- sh -c 'exit 3'; echo $?   # 3
taskslash history gate                    # what the last 20 runs of "gate" cost
taskslash history gate --json             # the same, as a JSON array
taskslash status                          # is this machine busy, and with what
taskslash status --json                   # the same, as one line of JSON
```

`taskslash run` runs the command itself, in your directory, with your environment and terminal, and
exits with its exit code (128+N when signal N killed it, 127 when it cannot be started). With no
daemon running it does not run the command and exits 1; so do `taskslash history` and
`taskslash status`.

### What a run records

When `taskslash run` ends, the daemon writes a row: the key, the command, its directory, the start
time, wall time, CPU seconds, the peak CPU over any 5 seconds (in threads), peak memory, MiB written
and the exit code. The key is `--name` when given, else the first two words of the command
(`bunx moon`). `taskslash history <key>` prints the newest 20 runs of a key, `-` for a value that
was not measured; a run whose `taskslash run` went away before the command ended has no exit code.

**Measurement needs a systemd user session.** The daemon checks once at startup whether
`systemd-run --user --scope` works. When it does, every job runs in a transient scope under
`taskslash.slice`, and the daemon reads the scope's cgroup every 250 ms while it runs. When it does
not (CI, a container, macOS), the daemon logs `taskslash: no systemd user session, jobs run
unmeasured`, runs jobs as plain children, and records wall time and exit code only. Jobs are
measured, never capped.

The database is SQLite, at `$TASKSLASH_DB`, else `$XDG_STATE_HOME/taskslash/taskslash.db`, else
`~/.local/state/taskslash/taskslash.db`. Only the daemon writes it.

The daemon listens on `$TASKSLASH_SOCKET`, or `$XDG_RUNTIME_DIR/taskslash/daemon.sock` when that is
unset. The socket's directory is created with mode 0700: only you can submit jobs.

### What status shows

`taskslash status` answers from the daemon's latest machine reading, taken once a second (free
space every 30 seconds), and lists the running jobs with their last live reading. The plain form is
a few lines: CPU, the `some avg10` pressures, memory, each disk, each mount, then one line per job.

| Field (`--json`) | What it is |
| --- | --- |
| `machine.sampled_at` | when the reading was taken, RFC 3339 UTC |
| `cpu.threads` | the CPUs the daemon may run on (its `Cpus_allowed_list`): the budget jobs can use, which can be fewer than are online |
| `cpu.threads_online` | every CPU the kernel has online |
| `cpu.busy` | the busy share (0 to 1) of the `threads` CPUs over the last second, from `/proc/stat`; iowait counts as idle |
| `*.pressure` | `/proc/pressure/{cpu,memory,io}`: the share of time, in percent, that some (`some`) or all (`full`) runnable work waited, averaged over 10 and 60 seconds. A missing `full` line is 0 |
| `memory.*_mib` | `/proc/meminfo`: `MemTotal`, `MemAvailable` (what can be handed out without swapping), `SwapTotal`, `SwapFree` |
| `io.disks[]` | each physical disk in `/sys/block` (not `loop`, `ram`, `zram` or `dm-` devices): `busy` is the share of time it had I/O in flight, `write_mib_s` its write rate |
| `mounts[]` | `$HOME`, `$TMPDIR` and `/tmp`, one per file system: size, space free to you, and the share used |
| `load` | `/proc/loadavg`, for people only |
| `jobs[]` | each running job: `job`, `key`, `started_at`, and its `cpu_threads` and `mem_mib` now, null until the first reading and always on an unmeasured run |

Shares and rates are 0 in the first second after the daemon starts, before there are two readings.

### Install the daemon as a systemd user unit

```bash
cargo install --locked --path crates/taskslash
cp crates/taskslash/systemd/taskslash.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now taskslash
journalctl --user -u taskslash -f         # the launch path, then one line per job start and end
```

> **Status: alpha.** Changing fast. Expect breaking changes between versions.

## Layout

| Path | What it is |
| --- | --- |
| `packages/engine` | the engine, a TypeScript library |
| `crates/taskslash-core` | the engine, a Rust library |
| `crates/taskslash` | the `taskslash` command (the Rust binary) |
| `scripts` | the repo checks |

## Develop

```bash
bun install
bun run taskslash --version
bun run taskslash --help
bun run typecheck && bun run lint && bun run format:check && bun run test && bun run build
```

The Rust crates need [rustup](https://rustup.rs): `rust-toolchain.toml` pins the toolchain, and
`rustup toolchain install` installs it, once per machine. There is no release workflow for the
binary yet.

See [`AGENTS.md`](AGENTS.md) for the full gate and [`CONTRIBUTING.md`](CONTRIBUTING.md) for how to
send a change.

## Licence

MIT, see [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE).
