# The CLI for a person at a terminal: a review

Every TaskSlash subcommand was built for gates and agents. This review walks each one as a person
would on first use, at a terminal, on a machine running the daemon, and lists what they cannot do,
cannot find, or cannot read. It changes no code: the fixes are chosen from the table and built
afterwards.

## How it was walked

- **Build:** the daemon and the CLI were reinstalled from `main` at `ccdb25a` with
  `bun scripts/install-service.ts` (no job was running at the time), and `systemctl --user
  is-active taskslash` answered `active`. `taskslash --version` prints `taskslash 0.1.0`.
- **Machine:** a 12-thread desktop whose database held 3,177 runs under 413 distinct keys, most
  of them from moon gates across about twenty worktrees of a monorepo and of several smaller repos.
- **Order:** `taskslash --help`, then each subcommand's `--help`, then the first real use of each
  against the live daemon, then the failure cases (no argument, an unknown key or job, no daemon,
  no moon workspace). The code was read only to explain a finding. The database was read with
  `sqlite3 -readonly` only to check what keys exist.
- **No daemon** was simulated by pointing `TASKSLASH_SOCKET` at a path with nothing behind it, so
  the real daemon kept running.

In the quotes below, `<mono>` stands for the monorepo's name and `~` for the home directory.

## Findings

Fix size: **S** is under an hour, **M** about a day, **L** more than a day.

| # | Command | What happened | What a person needed | Suggested fix | Size |
| --- | --- | --- | --- | --- | --- |
| 1 | `taskslash history` | Usage error, exit 2: `the following required arguments were not provided: <KEY>`. There is no other command that lists keys; the only way to learn them was `sqlite3` on the database. | To see which keys exist before picking one. | `history` with no key lists the keys, most recent first, with run count, last run time, last exit code and the worktree it last ran in. `--json` too. | M |
| 2 | `taskslash history nonsense-key` | Prints a full default estimate (`cpu 2.00 threads (default), memory 2048 MiB (default) ...`), `samples 0 measured, 0 calm` and an empty table, exit 0. It looks exactly like a real key that has no runs yet. | To be told the key is unknown, and what keys are close to it. | `taskslash: no runs of "nonsense-key"`, then up to five keys that contain the text, exit 1. | S |
| 3 | `taskslash history --help`, README `## Use` | The help's only key example is `"bunx moon"`, and it returns nothing: 0 of 413 keys is `bunx moon`, because `taskslash moon` names each job `<repo>/<project>:<task>`. The README's examples are `gate` and `'bun run'`. No help text and no README example shows a moon key a person can copy. | A real moon key in the help, since moon jobs are most of the history. | Help: `The key: a --name, the first two words of a command ("bun test"), or for taskslash moon, <repo>/<project>:<task> ("shop/web:test")`. Same example in the README's `## Use` block. | S |
| 4 | `taskslash history cli:test` (inside the monorepo) | Empty default answer, exit 0, as in row 2. The key is `<mono>/cli:test`, and nothing on screen says so. | moon calls the target `cli:test`; a person types that. | When run inside a moon workspace, a key shaped like a moon target (`cli:test`) resolves to `<repo>/cli:test`, and the header line names the key it used. | S |
| 5 | moon keys and worktrees (`select distinct key from runs`) | The README is right for worktrees: 20 worktrees of the monorepo, including ones checked out outside its parent folder, all record under `<mono>/...`. The folder name shows up only for a **standalone clone**: three clones of the monorepo in `$TMPDIR` each got their own 28 keys under their folder names (`<folder>/cli:test`), with none of the monorepo's history behind them. | One key per repository, whatever folder it is cloned into. | For a clone, take the repo name from the `origin` remote's last path segment (minus `.git`) before falling back to the folder name. Say in the README which one wins. | S |
| 6 | `taskslash history <mono>/cli:test` | The table has `id started_at wall cpu_s peak_cpu_5s peak_mem_mib written_mib exit cached`. A row with exit 101 cannot be traced to the worktree or command that ran it; `--json` has `cwd` and `argv`, the table does not. | To see where a run came from, above all a failed one. | Add a `where` column (the submitter's directory name, as `top` shows) and keep the table under 120 columns. | S |
| 7 | `taskslash history <key>` | `samples 45 measured, 45 calm` and `(history)` / `(default)` after each figure. "Calm" is not explained in `--help`. A key with one finished run still shows `cpu 2.00 threads (default)` because that run was not calm, with no reason given. | To know why the estimate still uses defaults. | One line in the help: a calm run is one that ran while the machine was not under pressure, and only calm runs feed the CPU figure. | S |
| 8 | `taskslash history <key>` | Always the newest 20, with no way to ask for more, for one worktree, or for today. | To look further back or narrow to one worktree. | `--limit <n>` and `--where <dir>`. | S |
| 9 | Keys from `taskslash run` without `--name` | A key is the first two words of the command. `sh -c '...'`, `bash -c '...'` and wrapper tools that pass a whole command line as `--name` make keys a person cannot guess or type: 73 of 413 keys are over 60 characters, several hold scratch directory paths, and 85 keys have exactly one run, so they never get an estimate. | Keys short enough to type, and runs of the same thing landing on the same key. | In `run --help`, recommend `--name` for anything run more than once and say a `sh -c` key includes the script. Longer term, cap a key's length and drop path-like words. | M |
| 10 | `taskslash moon scripts:typecheck` | Ends with `scripts:typecheck pass 0.5s`. The job's key is `taskslash/scripts:typecheck`, which is printed nowhere, so the next step (`taskslash logs --last <key>`, `taskslash history <key>`) needs a guess. | The key, and for a failure, the command that shows its log. | Print the key in the summary, and after a `fail` line, `log: taskslash logs --last <key>`. | S |
| 11 | `taskslash moon nosuch:test` | `taskslash: nosuch:test matches no task`, exit 2. | To see which projects or tasks exist. | List the projects that have the task (or the tasks of the project) on the next line. | S |
| 12 | `taskslash moon :test` in a fresh worktree | `taskslash: no moon: neither ~/.../node_modules/.bin/moon nor moon on PATH`, exit 1. | The next step. | Add `run bun install (or your package manager's install) first`. | S |
| 13 | `taskslash run -- sleep 2` | Waited, with one line on stderr: `taskslash: waiting 0s (1st in line): cpu needs 2.0, free 0.4; held by taskslash/core:lint (job 11), taskslash/bin:build (job 7), ...` (11 jobs on one line). The waiting job's own number is not printed, so `taskslash why <job>` cannot be used for it, and a two-second `sleep` waited because a never-seen command is assumed to take 2 threads. | Its job number, a readable reason, and a hint for a cheap command. | `waiting as job 18 ...`, the held-by list cut to the three biggest plus `and 8 more`, and on the first wait of a never-seen key: `(never run before: assumed 2 threads; --cpu 0.1 says otherwise)`. | S |
| 14 | `taskslash status` | With nothing running it prints the machine lines and stops: no line says there are no jobs. A running job reads `job 3     <mono>/cli:typecheck (normal) since 2026-11-17T20:34:19.814Z, 1.31 threads, 706 MiB`: the worktree is missing (it is in `--json` as `submitter`), and the start time is a UTC timestamp, not how long it has run. | To see at a glance whether anything runs, whose it is, and for how long. | `no jobs running or waiting` when empty; `job 3  12s  <worktree>  <key> (normal) 1.31 threads 706 MiB`. | S |
| 15 | `taskslash why` | Usage error, exit 2: it needs a job number. `taskslash why 1` for a job that ended says `job 1 not found (an ended job's record is in history)`, but `history` needs a key, not a job number, and job numbers start again at 1 when the daemon restarts. | To ask "why is anything waiting" without first finding a number. | `why` with no job explains every waiting job, one line each, or says nothing waits. For an ended job, name its key and run id if the daemon still remembers it. | S |
| 16 | `taskslash logs --failed` | Printed a TypeScript error log with no header, exit 0. It was the newest failure **on the whole machine**, from another worktree, and nothing said which key, run, worktree or time it was. | To know whose failure it is, and by default to see the ones from here. | A header on stderr (`run 3140 <key> in <worktree>, exit 2, 20:22`), and a `--here` (or default) scope of the current worktree. | S |
| 17 | `taskslash logs 1` | `taskslash: no job 1`, exit 1. The job had ended; the log exists. | The next step. | `no running job 1; an ended job's log is under taskslash logs --run <id> (see taskslash history <key>)`, or look the job up by number while the daemon remembers it. | S |
| 18 | (no command) cancel | There is no way to take a waiting job out of the queue or stop a running one, except Ctrl-C in the terminal that submitted it. A job submitted by a script or another terminal cannot be stopped from here. | To cancel a stuck or unwanted job. | `taskslash cancel <job>`: a waiting job leaves the queue, a running one gets SIGTERM, then SIGKILL after a grace period. Only the submitting user. | M |
| 19 | `taskslash top` | On an idle machine it opens on `no jobs since top opened` and `no job selected`: finished jobs from before it opened are never shown. Keys are only q, j/k and arrows: no full log, no cancel, no help key. | Something useful on first open: the last few finished runs. | Seed the finished list from the newest runs in the database. Later: `enter` opens the full log in a pager, `?` lists keys, `c` cancels once row 18 exists. | M |
| 20 | `taskslash events` | Prints nothing until a job event happens. A person cannot tell it is connected and waiting. The help is written for plugin authors (the JSON shape), which is right for its purpose. | A sign of life. | When stderr is a terminal, one line there: `listening for job events (Ctrl-C to stop)`. | S |
| 21 | `taskslash plugins` | Prints nothing, exit 0, when no plugin is installed. | To know the list is empty, and what a plugin is. | `no plugins on PATH (a plugin is an executable named taskslash-<name>)`. | S |
| 22 | `taskslash daemon` | With one already running: `taskslash: a daemon is already running on /run/user/1000/taskslash/daemon.sock`, exit 1. Clear. The help (`Runs until killed`) does not say it normally runs as the `taskslash` systemd user unit. | Where the daemon normally lives. | One help line: `Normally runs as the systemd user unit: systemctl --user status taskslash`. | S |
| 23 | `taskslash --version`, `taskslash status` | `taskslash 0.1.0`, with no commit. Nothing shows which build the running daemon is or when it started, so after an install a person cannot confirm the daemon is the new one. | To tell whether the daemon is current. | Put the commit in `--version` (`0.1.0 (ccdb25a)`), and the daemon's version and start time in `status`. | S |
| 24 | `history`, `logs` with no daemon | Both exit 1 with `no daemon at ... Start it with 'systemctl --user start taskslash' or 'taskslash daemon'.` The message is good, but past runs and logs are files on this machine that could be read without the daemon. | To read what already happened while the daemon is down. | Fall back to reading the database (read-only) and the log directory when no daemon answers. | M |
| 25 | All `--help` | `TASKSLASH_SOCKET` appears only in `events --help`; `TASKSLASH_DB` in no help at all; `TASKSLASH_CLASS`, `TASKSLASH_LOGS`, `TASKSLASH_LOG_DAYS` and `TASKSLASH_LOG_MIB` each appear in one subcommand's help. | One place listing the settings. | An `Environment:` section in the top-level help (clap `after_help`). | S |
| 26 | `taskslash --help` | Lists `daemon` first and gives no example. A person who wants "what is happening" or "what ran" has to read every line. Typing `taskslash` alone prints the same help but exits 2. | A starting point. | Order the commands by use (`top`, `status`, `history`, `logs`, `why`, `run`, `moon`, then `events`, `plugins`, `daemon`) and add three examples under them. | S |

`run` needed nothing beyond row 13 and the key advice in row 9: a plain command ran and kept its
exit code (`-- sh -c 'echo hi; exit 3'` printed `hi` and exited 3), a missing command said
`cannot run nosuchcmd: no such executable` with exit 127, and a wrong `--kind` listed the valid ones.
The no-daemon message is clear and says what to do, for every command.

## Top five, in fix order

1. **Rows 1 and 2: `history` lists keys, and an unknown key says so.** This is the dead end that
   started the review. Listing keys is the only way to discover them, and the unknown-key answer
   turns a silent wrong answer into a next step. Rows 3 and 7 (help text) go in the same change.
2. **Rows 4 and 10: moon targets as keys.** Accept `cli:test` inside a workspace, and print the
   key (and the log command for a failure) in `taskslash moon`'s summary. moon jobs are most of the
   history, and today a person cannot get from a failed target to its log without guessing the key.
3. **Row 16: `logs --failed` says whose failure it is and defaults to this worktree.** On a shared
   machine the newest failure is usually someone else's, and today it is printed with no header.
4. **Rows 13, 14 and 15: job numbers and worktrees where a person looks.** `run` prints its job
   number while waiting, `status` says when nothing runs and shows each job's worktree and age, and
   `why` with no argument explains every waiting job.
5. **Row 18: `taskslash cancel <job>`.** The only missing action rather than missing text: a job
   submitted elsewhere cannot be stopped today. It is M because it touches the daemon and needs
   its own tests.

Rows 5 (clone names), 19 (`top` opening empty) and 23 (version and commit) are next in line.
