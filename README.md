<!-- template:begin self-test -->
# slash-template

The template for a new Slash product: a Bun and moon monorepo with one or more engines (TypeScript,
Rust, or both side by side), an optional web app on the Hack Cove stack, and CI that passes with no
secrets. Everything below the line is the README the product starts with.

> **Status: alpha.** A product made from it is alpha too: anything may break.

## Create a product

```bash
gh repo create madangryscientist/<name> --private --template madangryscientist/slash-template --clone
cd <name>
bun scripts/init.ts --name <Name> --licence mit|elastic --engine ts|rust [--engine ...] [--web]
bun install                      # writes bun.lock for the parts you kept
bun run typecheck && bun run lint && bun run format:check && bun run test && bun run build
git add -A && git commit -m "chore(repo): start <name>"
```

- `--name`: the product's name in PascalCase (`TaskSlash`). It replaces `SlashProduct` everywhere,
  in each spelling, file and directory names included.
- `--licence mit` writes the MIT License, `--licence elastic` the Elastic License 2.0, into
  `LICENSE`, `NOTICE` and the `license` field of every `package.json` and `Cargo.toml`.
- `--engine ts`: `packages/engine` (the TypeScript library) and `packages/cli` (the npm CLI, with
  `release.yml` for npm trusted publishing).
- `--engine rust`: a cargo workspace at the root, `crates/<name>-core` (the library) and
  `crates/<name>` (the CLI binary). With a Rust engine the CLI is the Rust binary, so `packages/cli`
  is left out.
- `--engine ts --engine rust`: both side by side. The CLI is the Rust binary and `packages/engine`
  stays as the TypeScript library the web app imports.
- `--web`: `apps/web` (TanStack Start on Cloudflare, `@hackcove/ui-react`) and the UI review
  workflow. It imports `packages/engine`, so `--web` with only `--engine rust` adds it.

init removes what you did not choose, the template's own tests and CI job, and itself.

**A product whose CLI is the Rust binary has no release workflow yet.** Only the npm CLI
(`--engine ts` alone) ships with `release.yml`.

## Add an engine later

Each engine is one moon project tagged with its language, and engines sit side by side:

- **Another crate:** `cargo new --lib crates/<name>-<part>`, give it a `moon.yml` with
  `tags: ["rust"]`, and list it under `projects` in `.moon/workspace.yml`. `.moon/tasks/rust.yml`
  gives it `build`, `test`, `lint` and `format-check`.
- **Another TypeScript package:** copy `packages/engine`'s `package.json`, `tsconfig.json`,
  `vitest.config.ts` and `moon.yml` (tagged `ts`) into `packages/<part>` and list it.
- **Another language:** add `.moon/tasks/<language>.yml` with `inheritedBy: { tags: [<language>] }`
  and its tasks, tag the project with it, pin the toolchain in `.tool-versions`, and list it.

---
<!-- template:end self-test -->
# SlashProduct

<!-- template:begin mit -->
SlashProduct, under the MIT License.
<!-- template:end mit -->
<!-- template:begin elastic -->
Source-available SlashProduct you can run yourself, under the Elastic License 2.0.
<!-- template:end elastic -->

> **Status: alpha.** Changing fast. Expect breaking changes between versions.

## Layout

| Path | What it is |
| --- | --- |
<!-- template:begin engine -->
| `packages/engine` | the engine, a TypeScript library |
<!-- template:end engine -->
<!-- template:begin cli -->
| `packages/cli` | the `slashproduct` command (npm) |
<!-- template:end cli -->
<!-- template:begin rust -->
| `crates/slashproduct-core` | the engine, a Rust library |
| `crates/slashproduct` | the `slashproduct` command (the Rust binary) |
<!-- template:end rust -->
<!-- template:begin web -->
| `apps/web` | the web app: TanStack Start on Cloudflare Workers |
<!-- template:end web -->
| `scripts` | the repo checks |

## Develop

```bash
bun install
bun run slashproduct --version
bun run slashproduct greet Ada
bun run typecheck && bun run lint && bun run format:check && bun run test && bun run build
<!-- template:begin web -->
bun run test:e2e
<!-- template:end web -->
```

<!-- template:begin rust -->
The Rust crates need [rustup](https://rustup.rs): `rust-toolchain.toml` pins the toolchain, and
`rustup toolchain install` installs it, once per machine. There is no release workflow for the
binary yet.

<!-- template:end rust -->
See [`AGENTS.md`](AGENTS.md) for the full gate and [`CONTRIBUTING.md`](CONTRIBUTING.md) for how to
send a change.

## Licence

<!-- template:begin self-test -->
The template is MIT, see [`LICENSE`](LICENSE). A product carries the licence chosen at creation.
<!-- template:end self-test -->
<!-- template:begin mit -->
MIT, see [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE).
<!-- template:end mit -->
<!-- template:begin elastic -->
Elastic License 2.0, see [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE). SlashProduct is a name of
Jacobus Langner; the licence does not grant it.
<!-- template:end elastic -->
