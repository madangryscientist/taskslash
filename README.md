# TaskSlash

TaskSlash is admission control for builds and tests on one machine. Several gates and browser runs
started at once (by you, or by coding agents in several worktrees) overload a desktop until tests
time out for reasons that have nothing to do with the code. TaskSlash puts a small daemon in front of
them: each command asks it for a slot, and it keeps the queue and the record of what ran.

This first version hands out job ids and records each job's start and end. Admission, cost records
and the queue come next.

## Use

```bash
taskslash daemon                          # run the daemon (normally under systemd, below)
taskslash run -- bun run test             # run a command as a job
taskslash run --name gate -- sh -c 'exit 3'; echo $?   # 3
```

`taskslash run` runs the command itself, in your directory, with your environment and terminal, and
exits with its exit code (128+N when signal N killed it, 127 when it cannot be started). With no
daemon running it does not run the command and exits 1.

The daemon listens on `$TASKSLASH_SOCKET`, or `$XDG_RUNTIME_DIR/taskslash/daemon.sock` when that is
unset. The socket's directory is created with mode 0700: only you can submit jobs.

### Install the daemon as a systemd user unit

```bash
cargo install --locked --path crates/taskslash
cp crates/taskslash/systemd/taskslash.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now taskslash
journalctl --user -u taskslash -f         # one line per job start and end
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
