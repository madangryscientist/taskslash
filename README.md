# TaskSlash

TaskSlash is admission control for builds and tests on one machine. Several gates and browser runs
started at once (by you, or by coding agents in several worktrees) overload a desktop until tests
time out for reasons that have nothing to do with the code. TaskSlash puts a small daemon in front of
them: each command asks it for a slot, and it keeps the queue and the record of what ran.

This version runs each job in its own systemd scope, measures it, and records what it cost, and the
daemon samples the whole machine once a second. From those records it estimates what each key will
cost, and a job starts only when its estimate fits in the room the machine has left. A job that
waits says what it waits for.

## Use

```bash
taskslash daemon                          # run the daemon (normally under systemd, below)
taskslash run -- bun run test             # run a command as a job
taskslash run --name gate -- sh -c 'exit 3'; echo $?   # 3
taskslash run --kind browser --name visual -- bun run visual   # a cost class for its defaults
taskslash run --cpu 4 --mem 3000 -- bun run visual             # say what it costs (threads, MiB)
taskslash run --class urgent -- bun run test                     # first in line (default: normal)
taskslash moon :typecheck :test          # moon targets, one job per target
taskslash why 7                           # why job 7 waits (or "running")
taskslash why 7 --json                    # the reason as an object
taskslash history gate                    # the estimate of "gate", then what its last 20 runs cost
taskslash history gate --json             # the same, as {"estimate": ..., "runs": [...]}
taskslash status                          # is this machine busy, and with what
taskslash status --json                   # the same, as one line of JSON
taskslash logs 7                          # what running job 7 has printed so far
taskslash logs 7 -f                       # the same, followed until it ends
taskslash logs --last gate                # the newest run of "gate", running or not
taskslash logs --failed                   # the newest run that failed
taskslash plugins                         # the plugins on PATH (below)
```

`taskslash run` runs the command itself, in your directory, with your environment and terminal, and
exits with its exit code (128+N when signal N killed it, 127 when it cannot be started). With no
daemon running it does not run the command and exits 1; so do `taskslash history`,
`taskslash status`, `taskslash why` and `taskslash logs`.

### Every job's output is kept

`taskslash run` copies everything the command writes into a log of the job's own, and still writes
it to you exactly as before. So `taskslash run -- bun run test | tail -20` shows you 20 lines, and
the whole output is still there afterwards: `taskslash logs --last 'bun run'`.

- **On a terminal** (your stdout is one), the command runs on a pseudo-terminal of its own, so it
  sees a terminal and keeps its colours and progress bars. Its window size follows yours, and what
  you type reaches it. Your terminal is put back as it was when the command ends.
- **Otherwise** (a pipe, a file, a background run), stdout and stderr stay separate: each goes to
  your matching stream and into the log, so `> out 2> err` still splits them.
- **Signals.** The command runs in a process group of its own. SIGINT, SIGTERM and SIGHUP sent to
  `taskslash run` (Ctrl-C included) are passed on to that group, so the command ends the way it
  would have without TaskSlash, and the exit code is still its own.

`taskslash moon` does the same for each target: its stdout and stderr go to you and into that
target's own log.

A log holds stdout and stderr together, in the order the client read them, with nothing added. It
is a file per job in `logs` beside the database (`~/.local/state/taskslash/logs/` by default, so
`TASKSLASH_DB` moves it too), or in `$TASKSLASH_LOGS`. The daemon creates the directory with mode 0700 when it is missing (one that is already there keeps its mode), and each log is 0600.
A log is named after the job's systemd unit, `taskslash-<daemon pid>-<job>.log`: job numbers start
again at 1 when the daemon restarts, and the pid keeps the names apart. `taskslash status --json`
shows each running job's `log_path`, and `taskslash history --json` each run's (null for a run
recorded before logs were kept).

**Retention.** At startup and once an hour, the daemon deletes the logs (`taskslash-*.log`, nothing
else in the directory) of jobs that ended over 7 days ago, then the oldest ended ones until the directory is under 1 GiB. A running job's log is never
deleted. `TASKSLASH_LOG_DAYS` (days, fractions allowed) and `TASKSLASH_LOG_MIB` change the limits.
The run's row keeps the path of a deleted log.

**Reading a log.** `taskslash logs` prints one log to stdout, unchanged:

| Command | Picks |
| --- | --- |
| `taskslash logs <job>` | the running job with that number, as `taskslash status` shows it |
| `taskslash logs --run <id>` | the recorded run with that id, as `taskslash history` shows it |
| `taskslash logs --last <key>` | the newest run of the key, running or not |
| `taskslash logs --failed [<key>]` | the newest run (of the key) with an exit code that is not 0; a signal's 128+N counts, a run with no exit code (its client went away) does not |

`-f` follows a running job's log until the job ends, then exits. It exits 0 when it printed the log,
and 1 with one line on stderr when there is no such job or run (`taskslash: no job 7`,
`taskslash: no run of gate`), when the run was recorded before logs were kept
(`taskslash: run 12 has no log`), or when retention deleted it
(`taskslash: the log of run 12 has expired`); 2 on a usage error.

### moon targets

```bash
taskslash moon :typecheck :test           # every project's typecheck and test
taskslash moon cli:test --force           # one target, with moon's cache bypassed
taskslash moon :test --class urgent       # the jobs' class (default: TASKSLASH_CLASS, else normal)
```

`taskslash moon` replaces `moon run --concurrency 3 :typecheck :test`. A fixed `--concurrency`
is blind to the other gates on the machine: three gates at once are nine heavy tasks. Here each
target is its own job, so the daemon's budget decides how many run at once, across every worktree.

It finds the workspace (the first directory from here up with `.moon/workspace.yml`) and its moon
(`node_modules/.bin/moon` there, else `moon` on `PATH`), and reads `moon query tasks`. A target is
`:<task>` (that task in every project that has it) or `<project>:<task>`; anything else, or one that
matches nothing, is a usage error (exit 2). The requested targets and every target they depend on,
requested or not, each run **exactly once**, after all their dependencies passed, as
`moon run <target> --upstream none` (plus `--force` when given), so moon never re-runs a dependency
TaskSlash already ran. That holds for a task with its cache off too: it runs once, before what needs
it. A target whose dependency failed, directly or not, is skipped; the others keep running. Each
job's output is passed through as moon prints it.

Each job is named `<repo>/<target>`, where `<repo>` is the repository's name, the same for every
worktree of it (`shop` for both `shop` and its worktree `shop.feature-x`), so they share one
history. Its kind is the task's name when that is `test`, `typecheck` or `build`, else `command`.
It ends with one line per target, in dependency order:

```
a:gen pass 0.1s
b:test pass 0.1s cached
b:typecheck fail 0.1s
b:lint skipped -
```

`cached` marks a target moon replayed from its cache. That run is recorded with `cached` set and its
moon hash, and is left out of the estimate: replaying a cache costs nothing like the task. Exits 0
when every target passed, else 1.

A `moon run` typed by hand still works as it always did. The daemon does not see it as a job; it
is counted as unmanaged load, like anything else on the machine.

### What a run records

When `taskslash run` ends, the daemon writes a row: the key, the kind, the class, the command, its
directory, its submitter (below), the start time, wall time, CPU seconds, the peak CPU over any 5 seconds (in threads), peak memory,
MiB written, the exit code, and the machine's pressure while it ran (`machine_cpu_some`,
`machine_mem_some`, `machine_io_some`: the mean of the `some avg10` readings taken during the run,
null when none was). A `taskslash moon` job also records `cached` (moon replayed it from cache) and
`moon_hash`; both are null for any other run. The key is `--name` when given, else the first two words of the command
(`bunx moon`). `taskslash history <key>` prints the newest 20 runs of a key, `-` for a value that
was not measured; a run whose `taskslash run` went away before the command ended has no exit code.

**Measurement needs a systemd user session.** The daemon checks once at startup whether
`systemd-run --user --scope` works. When it does, every job runs in a transient scope under
`taskslash.slice` (on systemd 254 and newer with `--expand-environment=no`, so `$` in your command
is left alone), and the daemon reads the scope's cgroup every 250 ms while it runs. When it does
not (CI, a container, macOS), the daemon logs `taskslash: no systemd user session, jobs run
unmeasured`, runs jobs as plain children, and records wall time and exit code only. Jobs are
measured, never capped. `TASKSLASH_LAUNCH=plain` makes the daemon use plain children even where a
user session exists (the tests use it to run both paths on one machine).

The database is SQLite, at `$TASKSLASH_DB`, else `$XDG_STATE_HOME/taskslash/taskslash.db`, else
`~/.local/state/taskslash/taskslash.db`. Only the daemon writes it.

The daemon listens on `$TASKSLASH_SOCKET`, or `$XDG_RUNTIME_DIR/taskslash/daemon.sock` when that is
unset. The socket's directory is created with mode 0700: only you can submit jobs.

### Kinds and estimates

`--kind` is a job's cost class: `test`, `typecheck`, `build`, `browser` or `command` (the default).
It only matters until the key has history. `taskslash history <key>` prints the key's estimate above
its runs, each figure marked `history` or `default` (or `given` in `status`, for a run's `--cpu`
or `--mem`):

| Figure | From the key's runs | Default |
| --- | --- | --- |
| `mem_mib` | the largest peak memory of the newest 5 measured runs, plus 20% | `typecheck` 3 GiB, `build` and `browser` 4 GiB, else 2 GiB |
| `cpu_threads` | the 75th percentile (nearest rank) of mean threads, CPU seconds over wall time, over the newest 10 calm runs; `cpu_peak_5s` is the largest 5 second peak of the same runs | `build` and `browser` 4, else 2 |
| `write_mib_s` | the mean of MiB written per second over the newest 10 measured runs | 0 |
| `duration_s` | the median wall time of the newest 10 calm runs | none |

A run is **measured** when it has CPU seconds, peak memory and an exit code (the scope path) and is
not a moon cache hit, and
**calm** when it is measured and the machine's CPU `some avg10` during it is known and at most 40.
A run on a starved machine still tells the truth about its memory and bytes written, but not about
its CPU share or how long it takes, so those two use calm runs only. The kind for the defaults is
the newest run's.

### Admission

The daemon decides, for each waiting job in the queue's order (below), whether it starts now.
Three resources count: CPU (threads), memory (MiB) and disk writes (MiB/s).

- **Budget:** CPU is 85% of `cpu.threads` (the CPUs the daemon may use), memory 80% of
  `memory.total_mib`, disk writes 150 MiB/s. Override them in the daemon's environment with
  `TASKSLASH_CPU_SHARE`, `TASKSLASH_MEM_SHARE` and `TASKSLASH_WRITE_MIB_S`.
- **Committed:** each running job counts at the larger of its estimate and what it uses now (CPU
  and writes over the last 5 seconds, memory at its peak so far; an unmeasured job counts at its
  estimate), plus **unmanaged** use: what the machine shows minus what the jobs use now (an
  unmeasured job at its estimate, so it is not counted twice), floored at 0, the mean of the last
  30 one second samples. Machine CPU is `cpu.busy` times `cpu.threads`,
  memory `total_mib - available_mib`, writes the sum over the disks.
- **A job fits** when, for every resource, committed plus its estimate is within the budget.
- **Gates:** while any holds nothing starts: CPU `some avg10` over 40, memory `some avg10` over 5,
  I/O `full avg10` over 5 or `some avg10` over 25, `available_mib` under 10% of `total_mib`, a
  watched mount over 90% used.
- **Never seen:** a key is seen once one of its runs has finished. At most one job whose key was
  never seen runs at a time, unless the caller gave both `--cpu` and `--mem`.
- **Too big to ever fit:** a job whose estimate alone is over a budget starts when no other job is
  running and no gate holds.

`--cpu <threads>` and `--mem <MiB>` on `run` replace the learned estimate for that run.

### Order

`--class` gives a job its priority class: `urgent`, `flake`, `interactive`, `normal` or
`background`, highest first. Without it the class is `$TASKSLASH_CLASS` (so a launcher can set it
once for everything it runs), else `normal`. An unknown class, from either, is a usage error
(exit 2).

A job's **submitter** is the nearest directory, from its working directory up, that holds a `.git`
(a directory in a clone, a file in a worktree), else the working directory itself. Fair share is
counted per submitter.

Each pass over the queue (on every submit, end and machine sample) takes the waiting jobs in this
order:

1. **Effective class**, highest first. A job that has waited more than 15 minutes moves up one
   class, and one more for every further 15 minutes, never past `urgent`.
2. **Within a class**, the submitter holding the least of the machine first: for each resource, the
   sum of `max(estimate, observed)` of its running jobs over that resource's budget, and the
   largest of the three (Dominant Resource Fairness). A submitter with nothing running holds 0. A
   job the pass starts counts at once, so two jobs from one idle worktree do not both go first.
3. Then the job that has waited longest, then the lower job number.

A job that fits starts. A job that does not is passed over and the pass goes on, so a later,
smaller job may start (**backfill**), unless the one passed over waits for room and has waited more
than 5 minutes:
then nothing behind it starts until it has (**the starvation guard**), and those behind it wait
with `behind <key> (job <n>), which has waited over 5 minutes and is not passed`. A gate holds every
job. A higher class never stops, kills or freezes a running job; it is only first in line.

**Weights.** On the scope path a job runs with `-p CPUWeight=<w>` from its class: `urgent` 400,
`flake` 300, `interactive` 200, `normal` 100, `background` 20. Weights share out a busy CPU between
running jobs; they do not admit. The plain path has no weight.

A job that waits is told why on stderr when it is queued and every 15 seconds after. The line names
the first thing that holds it, checked in this order: a gate, the never-seen rule, a resource. A
resource line lists the running jobs that hold the most of it first:

```
taskslash: waiting 45s (2nd in line): cpu needs 4.8, free 1.8; held by cli-db:test (job 3), visual (job 5)
taskslash: waiting 30s (1st in line): cpu pressure 52% over the 40% gate
```

`taskslash why <job>` prints the same sentence, `running` for a running job, and exits 1 for a job
the daemon does not hold (an ended job's record is in `history`). With `--json` it prints the
reason: `{"kind":"resource","resource":"cpu","needs":4.8,"free":1.8,"held_by":[{"job":3,"key":
"cli-db:test","amount":3.0}]}`, `{"kind":"gate","gate":"cpu_pressure","value":52.0,"limit":40.0}`,
`{"kind":"never_seen","job":3,"key":"..."}`, `{"kind":"behind","job":3,"key":"..."}`, or
`{"kind":"running"}`. A `taskslash run` stopped
while it waits leaves the queue and its command never runs.

**A fake machine, for tests.** With `TASKSLASH_FAKE_MACHINE=<file>` in its environment the daemon
reads the machine from that file (a `machine` object as `status --json` prints it) on every sample
instead of from `/proc`, keeping the last good reading when the file cannot be read. It exists so
tests can set the machine's load and pressure; do not use it otherwise.

### What status shows

`taskslash status` answers from the daemon's latest machine reading, taken once a second (free
space every 30 seconds), and lists the running jobs with their last live reading. The plain form is
a few lines: CPU, the `some avg10` pressures, memory, each disk, each mount, then one line per
running job and one per waiting job (`waiting 9 visual (browser, background, as normal) 960s (2nd in
line): ...` for a job whose class waiting has raised).

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
| `jobs[]` | each running job: `job`, `key`, `kind`, `class`, `submitter`, `started_at`, its `cpu_threads` and `mem_mib` now (null until the first reading and always on an unmeasured run), and its `estimate` |
| `waiting[]` | each waiting job, in the order of the last pass: `job`, `key`, `kind`, `class`, `effective_class` (only when waiting has raised it), `submitter`, `waited_s`, `position` (1-based), `reason` (the sentence), `why` (the reason object) and its `estimate` |

Shares and rates are 0 in the first second after the daemon starts, before there are two readings.

### Plugins

Anything else can plug into TaskSlash as a program of its own, in any language. A plugin is an
executable named `taskslash-<name>` on `PATH`, found the way git and cargo find theirs (so a repo can
ship one in its `node_modules/.bin`, and a Rust one installs to `~/.cargo/bin`).

```bash
taskslash plugins                         # every taskslash-<name> on PATH, and the path that runs
taskslash plugins --json                  # [{"name", "path", "shadowed", "builtin"}, ...]
taskslash hello world                     # runs taskslash-hello world
```

There are three kinds:

| Kind | What it does | |
| --- | --- | --- |
| command | `taskslash <name> [args...]` runs `taskslash-<name> [args...]` | here now |
| listener | follows the daemon's event stream (job queued, started, ended) | coming |
| filter | rewrites a job's output on its way to the log and the terminal (secret redaction) | coming |

A command plugin replaces the `taskslash` process, so its exit code and signals are its own. Built-in
commands always win: a `taskslash-run` never runs. With no `taskslash-<name>` on `PATH`,
`taskslash <name>` is a usage error (exit 2) that names the program it looked for. Only regular files
(or links to one) with an execute bit count. When two directories on `PATH` hold the same name, the
first one runs and `taskslash plugins` lists the later ones as shadowed.

A plugin needs no daemon to start, and gets this contract in its environment:

| Variable | What it holds |
| --- | --- |
| `TASKSLASH_PROTOCOL` | the version of the socket protocol, now `1`; it goes up on any breaking change |
| `TASKSLASH_SOCKET` | the daemon's socket path, as `taskslash` resolves it; unset when it does not resolve (no `XDG_RUNTIME_DIR`) |
| `TASKSLASH_BIN` | the absolute path of the running `taskslash`, to call back into it (`"$TASKSLASH_BIN" run -- ...`) |

An example: save this as an executable `taskslash-hello` on `PATH`, then run `taskslash hello world`.

<!-- plugin-example:start -->
```sh
#!/bin/sh
# taskslash-hello: greets, then says whether the daemon answers.
set -u
echo "hello, ${1:-you}"
echo "protocol $TASKSLASH_PROTOCOL"
if [ -S "${TASKSLASH_SOCKET:-}" ] && "$TASKSLASH_BIN" status >/dev/null 2>&1; then
  echo "the daemon is up"
else
  echo "no daemon"
fi
```
<!-- plugin-example:end -->

### Install the daemon as a systemd user unit

```bash
bun scripts/install-service.ts
journalctl --user -u taskslash -f         # the launch path, then one line per job start and end
```

The same command also upgrades: rerun it after pulling a new `main`, and a job running during the restart keeps running but is not recorded.

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
