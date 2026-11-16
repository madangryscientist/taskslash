# Contributing to SlashProduct

Thanks for helping. This page is the short version of [`AGENTS.md`](AGENTS.md), the full operating
agreement for people and coding agents working here. Read it before a large change.

By taking part you agree to the [Code of Conduct](CODE_OF_CONDUCT.md). A security problem goes to
[`SECURITY.md`](SECURITY.md), not to a public issue. Your contribution is licensed under the licence
the rest of the repo carries (see [`LICENSE`](LICENSE)); there is no CLA.

## Start with an issue

For a bug, open an issue with the version and the steps. For anything bigger than a fix, open an
issue first so we can agree on it before you build it.

## The gate

A change is ready when all of these pass from a fresh checkout:

```bash
bun install
bun run typecheck       # tsc, strict, on every project
bun run lint            # oxlint (type-aware, --deny-warnings), check-slop, check-dashes
bun run format:check    # oxfmt; `bun run format` fixes it
bun run test            # Vitest under Bun
bun run build
```

<!-- template:begin rust -->
For the Rust crates, `lint` also runs clippy, `format:check` runs `cargo fmt --check` (`cargo fmt`
fixes it) and `test` runs `cargo test`.

<!-- template:end rust -->

Lint rules are errors: fix the code, do not disable a rule.

## Pull requests

- Branch from `main`, keep the change small, and rebase on `origin/main` before you push.
- Write the commit subject as a [Conventional Commit](https://www.conventionalcommits.org/)
  (`feat(scope): ...`, `fix(scope): ...`; `docs`, `chore`, `refactor`, `test`, `perf`, `build`,
  `ci`, `style`, `revert`). The scope is the area (`cli`, `engine`, `web`, `ci`) and the issue goes
  at the end: `fix(engine): handle an empty list (#12)`. The squash keeps your subject.
- Stage files by name. Never commit straight to `main`.
- No AI attribution trailers are required or expected.
