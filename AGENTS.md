# SlashProduct: repo conventions

This file is the operating agreement for contributors and agents working here: the gate, the landing
rule and the invariants. Keep it accurate.

<!-- template:begin self-test -->
**This is the template.** `scripts/init.ts` turns a copy of it into a product (README.md says how).
Every part is present here, so the gate below runs on all of them. Two rules for changing the template:

- A file or line that belongs to one part sits inside `template:begin <part>` and
  `template:end <part>` marker lines (`#`, `//` or `<!-- -->` comments), or under that part in
  `PART_PATHS` (`scripts/template/product.ts`). The parts are `rust`, `ts` engine (`engine`), the npm
  CLI (`cli`), `web`, the two licences (`mit`, `elastic`) and `self-test` (the template's own tests,
  always removed). JSON has no comments, so init edits the root `package.json` scripts in code.
- The placeholder name is `SlashProduct` (`slashproduct`, `SLASHPRODUCT`). Use it wherever the
  product's name goes; init swaps all three spellings, in paths too.

The `init` job in `ci.yml` creates a product for every choice (12 of them), checks it with
`scripts/template/assert.ts` and runs its gate. `bunx moon run scripts:test` runs the same checks
locally (`scripts/template/init.test.ts`).

<!-- template:end self-test -->
## The gate

A moon workspace (moon 2.6.5, pinned in the root `package.json`) on Bun workspaces.
`.moon/workspace.yml` lists the projects, `.moon/tasks/*.yml` holds what they inherit (shared inputs,
the Vitest `test` task for the projects tagged `ts`, and the cargo tasks for the ones tagged `rust`),
and each project has a `moon.yml`. There is no toolchain file: every task runs on the system
toolchain, so moon never downloads Bun or runs `bun install`. Keep it that way (no
`.moon/toolchains.yml`, `pipeline.installDependencies: false`).

| Project | Path | What it is |
| --- | --- | --- |
<!-- template:begin engine -->
| `engine` | `packages/engine` | the engine, a TypeScript library |
<!-- template:end engine -->
<!-- template:begin cli -->
| `cli` | `packages/cli` | the `slashproduct` bin: argv, stdout and exit codes, over the engine |
<!-- template:end cli -->
<!-- template:begin rust -->
| `core` | `crates/slashproduct-core` | the engine, a Rust library |
| `bin` | `crates/slashproduct` | the `slashproduct` binary, over `slashproduct-core` |
<!-- template:end rust -->
<!-- template:begin web -->
| `web` | `apps/web` | the web app: TanStack Start on Cloudflare Workers |
<!-- template:end web -->
| `scripts` | `scripts` | the repo checks (typecheck and lint only) |

The `gate` job in `.github/workflows/ci.yml` runs these on GitHub-hosted runners (`ubuntu-latest`),
and a change is green when all of them pass **from the worktree**:

```bash
bun scripts/check-workflows.ts .github/workflows/*.yml   # no unguarded self-hosted job
bun install --frozen-lockfile     # a fresh worktree has no node_modules
bun run typecheck                 # moon run :typecheck  (tsc, strict)
bun run lint                      # moon run :lint, then check-dashes
bun run format:check              # oxfmt --check (`bun run format` fixes)
bun run test                      # moon run :test
bun run build                     # moon run :build
```

<!-- template:begin rust -->
For the crates, `lint` is `cargo clippy --all-targets -- -D warnings`, `test` is `cargo test` and
`build` is `cargo build`, all `--locked`; `format:check` also runs `moon run :format-check`
(`cargo fmt --check`, and `cargo fmt` fixes it). On a new machine run `rustup toolchain install`
first: it installs the toolchain `rust-toolchain.toml` pins (the `gate` job does the same).

<!-- template:end rust -->
<!-- template:begin web -->
The `apps` job runs when the apps or packages change: `bunx moon run web:test-e2e` (Playwright
against a local production build), then `bunx moon run web:build --force` and the deploy dry run
(`(cd apps/web && bunx wrangler deploy --dry-run --outdir "$TMPDIR/web-dry")`, no Cloudflare
credentials needed). First time on a machine: `(cd apps/web && bunx playwright install
chromium-headless-shell)`. `ui-review.yml` checks the UI is built the Hack Cove way.

<!-- template:end web -->
One project: `bunx moon run scripts:typecheck`. moon caches in `.moon/cache` (gitignored); add `--force` to
rerun a cached task. A shared moon remote cache sits behind a bearer token read from
`MOON_REMOTE_TOKEN`; unset, moon warns about a 401 and runs on the local cache, so CI passes with no
secret. Lint rules are all errors; fix the code, do not disable a rule. A TODO carries an issue:
`TODO(#123)`.

`bunfig.toml` sets `[install] linker = "isolated"`: a package sees its own declared dependencies and
the root's, not another package's. Declare what a package imports in that package's `package.json`.

**Generated files are not formatted.** Anything a tool writes is excluded in `.oxfmtrc.json`'s
`ignorePatterns`. `**/*.md` is ignored too.

**/tmp is a quota'd tmpfs; do not fill it.** Every test run goes through `scripts/test-tmp-scope.ts`
(a Vitest and Playwright `globalSetup`), which gives it a fresh `TMPDIR` and removes it after. A new
project's `vitest.config.ts` lists the same `globalSetup`. Scratch goes under `$TMPDIR`, never a
hardcoded `/tmp`.

## Landing

- Code changes land through pull requests; nobody pushes to `main`. Branch, gate, rebase on
  `origin/main`, push, open the PR.
- Commit subjects are Conventional Commits: `<type>(<scope>): <subject>`, with type one of `feat fix
  docs chore refactor test perf build ci style revert`. The squash keeps the subject.
- Stage files by name or `git add -A`; never `git commit -am` (it drops new files).
- No `Co-Authored-By` trailer and no AI attribution line, in a commit or a pull request.
- No em dash or en dash anywhere (`scripts/check-dashes.ts` fails the lint on one).

## Invariants

1. **Exit codes are a contract.** `slashproduct` exits 0 on success, 1 when the run failed for a
   reason that is not usage, 2 on a usage error.
2. **The TypeScript code writes through `process.stdout` / `process.stderr`**, not `console` (the
   lint bans it outside `scripts/`).
3. **A secret is read from the environment, never from the tree.**
<!-- template:begin web -->
4. **UI is composed from `@hackcove/ui-react`.** A custom style or component is a flag, not a fix; a
   reusable gap is filed against the hackcove repo. Keep the Catppuccin credit comment in each
   `styles.css`.
<!-- template:end web -->
