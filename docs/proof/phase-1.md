# Phase 1 proof: three gates and two browser runs on one desktop

Phase 1 of TaskSlash is admission control on one machine: concurrent gates and browser runs should
stop overloading the desktop, without making the work slower overall. Every rule before this was
tested against a fake machine. This is the run on a real one, under the load that caused the
trouble: the same five pieces started together, once with today's commands and once through
TaskSlash.

**Result: every pass line holds on the TaskSlash run, with the default limits.** CPU pressure peaked
at 19.5% (64.7% without TaskSlash), and the whole load finished in 191 s, against 215 s without it.
No limit needed tuning.

## The machine

| | |
| --- | --- |
| CPU | AMD Ryzen 5 5600X, 6 cores, 12 threads, all 12 allowed to user jobs (4 of them are shared with a separate slice for flaky-test reproduction, idle during the runs) |
| RAM | 31.3 GiB (32,008 MiB) |
| Swap | 31.3 GiB zram plus a 31.3 GiB swap file |
| Disk | one Samsung 850 EVO 500 GB SATA SSD, LUKS, btrfs |
| TaskSlash | commit `258cc8b`, installed as the systemd user service with `bun scripts/install-service.ts` |
| Limits | the defaults: `TASKSLASH_CPU_SHARE=0.85` (10.2 of 12 threads), `TASKSLASH_MEM_SHARE=0.80`, `TASKSLASH_WRITE_MIB_S=150`; no drop-in |

## The load

A private Bun and moon monorepo: 28 typecheck and test targets per gate, one test suite on a local
Postgres, and a visual tool that starts Postgres, two dev servers and a Playwright Chromium.

Three clones of it, all at the same commit, stand for three worktrees. The five pieces start within
2 s of each other:

- in each clone, the full gate (`:typecheck :test`) with a forced cache miss;
- from clone 1 and clone 2, one visual run each.

| | Without TaskSlash (baseline) | Through TaskSlash |
| --- | --- | --- |
| Gate | `moon run --concurrency 3 --force :typecheck :test` | `taskslash moon :typecheck :test --force` |
| Visual run | the existing one-slot browser lease, around the visual tool | the same lease command, which now runs `taskslash run --kind browser` while the daemon answers |
| Daemon | stopped | running |

Before the counted TaskSlash run, one warm-up pass of all five pieces ran through TaskSlash (not
counted), so every key had a finished run. Without it, every key is never seen and runs one at a
time; the warm-up took 17 minutes for exactly that reason.

`scripts/record-machine.ts` recorded the machine once a second across each counted run, from just
before the first piece started to just after the last ended.

## The pass lines

| Line | Pass when | Baseline | TaskSlash | TaskSlash result |
| --- | --- | --- | --- | --- |
| No load-caused failures | every gate and both visual runs pass, zero test timeouts | 0 timeouts; both visual runs pass; each gate fails one test, the same one (below) | 0 timeouts; both visual runs pass; each gate fails the same one test | pass |
| CPU pressure | `some avg10` never above 50% | 64.69% | 19.53% | pass |
| I/O pressure | `full avg10` never above 10% | 0.18% | 3.17% | pass |
| Memory | `MemAvailable` never under 10% of RAM, swap growth at most 1 GiB | min 42.5%, swap +0 MiB | min 50.1%, swap +0 MiB | pass |
| Not slower overall | wall time at most 1.25 times the baseline's | 215.2 s | 191.0 s (0.89 times) | pass |
| Explains itself | every job that waited printed what held it | no waiting line (the gates do not queue) | 76 jobs waited; all 76 printed a waiting line naming the jobs holding the CPU it needed | pass |

The baseline fails the CPU line, as expected: three moon gates at `--concurrency 3` and two
browser runs put up to 9 moon tasks and two browser stacks on 12 threads at once, and CPU pressure
went past 50%.

**The one failing test is not load-caused.** It fails the same way in all three gates of every run
(baseline, warm-up and TaskSlash) and also fails when run alone on an idle machine, 3 times out of 3.
It checks how the kernel treats a process under a real-time CPU limit, and the shell these runs
started from has a limit that the test does not expect. It counts as a failure of its own, not of
the load, and it is reported to the monorepo's maintainers.

**Explains itself, checked two ways.** The daemon's journal (`journalctl --user -u taskslash`) lists
76 jobs that waited during the counted run. For each, the matching gate or visual log has a
waiting line: `taskslash: cli-db:test: waiting 30s (3rd in line): <reason>` from
`taskslash moon` (the target named first), or `taskslash: waiting 30s (2nd in line): <reason>` from
`taskslash run`. All 76 reasons
were CPU: `cpu needs 2.9, free 0.5; held by <the jobs holding it>`. No job waited without a reason.

## Wall time per piece

| Piece | Baseline | TaskSlash | Exit (both runs) |
| --- | --- | --- | --- |
| Gate 1 | 194.4 s | 184.5 s | 1 (the one test above) |
| Gate 2 | 187.7 s | 179.4 s | 1 (the one test above) |
| Gate 3 | 215.2 s | 191.0 s | 1 (the one test above) |
| Visual run 1 | 67.4 s | 187.0 s | 0 |
| Visual run 2 | 65.6 s | 174.4 s | 0 |
| All five, first start to last end | **215.2 s** | **191.0 s** | |

TaskSlash makes the browser runs wait and the gates faster. Each visual run waited 135 s to 150 s
for 2.9 threads of room, then captured in 13 s to 15 s instead of 23 s to 24 s. Each gate finished
sooner because its tests no longer competed with two browsers and two other gates for the same cores.
The total is set by the slowest gate, so the load as a whole finished 24 s earlier.

## Timeouts

A timeout is a line in a gate or visual log matching `timed out` or `timeout` from the test runner.

| Run | Gate 1 | Gate 2 | Gate 3 | Visual 1 | Visual 2 |
| --- | --- | --- | --- | --- | --- |
| Baseline | 0 | 0 | 0 | 0 | 0 |
| TaskSlash | 0 | 0 | 0 | 0 | 0 |

This machine, on this evening, did not time a test out even without TaskSlash. What the baseline
shows instead is the pressure that leads there: CPU `some` at 64.7% means that, at the worst point,
some runnable task was waiting for a core for 64.7% of a 10 s window.

## Tuning

None. Every line passed with the default limits on the first counted run, so the defaults in
`crates/taskslash-core/src/admission.rs` stay as they are and no drop-in is installed.

## What happened along the way

- **The first baseline was discarded.** It started while the machine was still busy with the
  release build of TaskSlash (load 7.6 at the start), and another session held the browser slot
  for its first seconds. It ran in 201.7 s with CPU pressure at 52.4%. The baseline above was
  rerun after the load average fell under 2, with nothing else running.
- **The two visual runs each got their own `TMPDIR`.** The visual tool keeps a base checkout and a
  warm-up folder at fixed paths under `$TMPDIR`. Two runs from two clones at the same commit
  collided on them in the dry run (`git worktree add` failed with a lock held by the other run).
  Each run got `TMPDIR=<scratch>/tk8-tmp-<n>`, which changes no argv and so no TaskSlash key. With
  that, the two runs took separate free ports and separate Postgres sockets, as designed.
- **In the baseline, both visual runs got the one-slot browser lease at once.** The lease is meant to
  make the second run wait for the first; started in the same instant, both acquired it, and one
  printed that it had lost the lease mid-run. That made the baseline faster than today's commands
  normally are (65 s each instead of about 65 s plus 65 s), so the wall time comparison is a harder
  test for TaskSlash than intended, not an easier one. It is reported to the lease's maintainers.

## What this does not show

One desktop, one load mix and one evening's runs. It says nothing yet about the pool of machines or
CI (phases 2 and 3), or about a different machine. The longer proof, the count of flaky-test
reports that turn out to be timeouts under load, two weeks before and two weeks after, is collected
separately.

## Rerun it

```bash
# 1. Record the machine across the load, in its own terminal:
bun scripts/record-machine.ts "$TMPDIR/run.jsonl"
# 2. Start the five pieces within 2 s of each other, each with its own log; note start, end, exit.
# 3. Ctrl-C the recorder after the last piece ends. It prints one summary line:
#    max cpu some, io some, io full and memory some (avg10, %), min MemAvailable as % of MemTotal,
#    and swap growth in MiB.
```

`record-machine.ts` reads `/proc/pressure/*` and `/proc/meminfo` itself, not the daemon, so a run
without TaskSlash is recorded the same way as a run with it. It writes one JSON line per second
(`t`, `cpu_some`, `io_some`, `io_full`, `mem_some`, `mem_available_mib`, `mem_total_mib`,
`swap_used_mib`) and exits 2 on any argument count but one.

For a run through TaskSlash: install the service (`bun scripts/install-service.ts`), run one warm-up
pass of the same load so every key has a finished run, then the counted run. For the baseline:
`systemctl --user stop taskslash` first, and start it again after.
