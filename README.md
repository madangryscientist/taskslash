# TaskSlash

TaskSlash, under the MIT License.

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
bun run taskslash greet Ada
bun run typecheck && bun run lint && bun run format:check && bun run test && bun run build
```

The Rust crates need [rustup](https://rustup.rs): `rust-toolchain.toml` pins the toolchain, and
`rustup toolchain install` installs it, once per machine. There is no release workflow for the
binary yet.

See [`AGENTS.md`](AGENTS.md) for the full gate and [`CONTRIBUTING.md`](CONTRIBUTING.md) for how to
send a change.

## Licence

MIT, see [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE).
