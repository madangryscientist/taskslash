# TaskSlash facts

Checked against main at 95af709 on 2026-11-17.

## What it is
TaskSlash stops builds and tests that start at the same time from overloading one computer: each one waits in a queue until the machine has room for it. It is early (alpha), version 0.1.0.

## Today
- `daemon-queue`: A daemon, `taskslash daemon`, holds the queue and decides when each submitted job starts.
- `run-command`: `taskslash run -- <command>` runs a command as a job, in your directory with your environment and terminal, and exits with the command's own exit code.
- `admission-by-room`: A job starts when its estimated CPU, memory and disk writes fit in what the machine has left, and waits while the machine's CPU, memory or I/O pressure is over a set line. A job too big to ever fit starts when nothing else runs, and a job held only by load TaskSlash does not run starts after a set wait.
- `learned-estimates`: The daemon estimates what a job will cost from the recorded runs of the same name, and falls back to a default for its kind (test, typecheck, build, browser, command) until there is history.
- `measured-runs`: With a systemd user session, each job runs in its own systemd scope and the daemon records its wall time, CPU, peak memory, disk writes and exit code in a SQLite database.
- `priority-classes`: A job has a priority class (urgent, flake, interactive, normal, background), and within a class the worktree using the least of the machine goes first.
- `waiting-reason`: A job that waits says why on stderr, and `taskslash why <job>` prints the same reason, also as JSON.
- `moon-targets`: `taskslash moon <targets>` runs moon targets as one job per target, each once, after its dependencies passed.
- `job-logs`: Every job's output is kept in a log file of its own until it passes a set age or the logs pass a set size, and `taskslash logs` prints it, by job, run, name or the last failure.
- `history`: `taskslash history <name>` prints a job's estimate and what its last runs cost.
- `status`: `taskslash status` shows the machine's load, pressure, memory, disks and the running and waiting jobs, also as JSON.
- `events`: `taskslash events` prints one JSON line per job queued, started and finished, for other tools to follow.
- `command-plugins`: Any program named `taskslash-<name>` on `PATH` runs as `taskslash <name>`, and `taskslash plugins` lists them.
- `desktop-proof`: On one desktop, three gates and two browser runs started together stayed under the pressure lines through TaskSlash and finished sooner than without it (docs/proof/phase-1.md).
- `no-release`: There is no release you can install yet: you build it from source with cargo.

## Planned
- `filter-plugins`: Filter plugins that rewrite a job's output on its way to the log and the terminal, such as secret redaction.
- `machine-pool`: Scheduling across a pool of machines.
- `ci-runners`: Scheduling for CI.

## Licence
- `licence`: open source under the MIT licence

## Where it runs
- `linux-systemd`: Linux with a systemd user session, where jobs are measured; `bun scripts/install-service.ts` builds it from the repo with cargo and installs the daemon as a systemd user service.
- `linux-no-systemd`: Linux without a systemd user session (a container or a CI runner), where jobs run unmeasured and only wall time and exit code are recorded.
