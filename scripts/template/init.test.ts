import { execFileSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { describe, expect, it } from "vitest";
import { checkProduct } from "./check.ts";
import { listFiles } from "./files.ts";
import { type Engine, type Licence, UsageError, parseOptions } from "./options.ts";

const templateRoot = join(import.meta.dirname, "..", "..");

/** A copy of the template's tracked files, as `gh repo create --template` would give. */
function copyTemplate(): string {
  const dest = mkdtempSync(join(tmpdir(), "init-"));
  const tracked = execFileSync(
    "git",
    ["ls-files", "-z", "--cached", "--others", "--exclude-standard"],
    {
      cwd: templateRoot,
      encoding: "utf8",
    },
  );
  for (const path of tracked.split("\0").filter(Boolean)) {
    mkdirSync(join(dest, dirname(path)), { recursive: true });
    cpSync(join(templateRoot, path), join(dest, path));
  }
  return dest;
}

function init(dest: string, flags: string[]) {
  return Bun.spawnSync(["bun", "scripts/init.ts", ...flags], { cwd: dest });
}

// The CI init matrix runs all 12 choices with each product's gate. These three cover the
// different shapes (npm CLI, Rust CLI with web pulling the engine in, both engines) and both licences.
const combinations: { licence: Licence; engines: Engine[]; web: boolean }[] = [
  { licence: "mit", engines: ["ts"], web: false },
  { licence: "elastic", engines: ["rust"], web: true },
  { licence: "mit", engines: ["ts", "rust"], web: true },
];

/** Every file under `dir` with its contents, to show a run changed nothing. */
function snapshot(dir: string): Record<string, string> {
  return Object.fromEntries(
    listFiles(dir).map((path) => [path, readFileSync(join(dir, path), "utf8")]),
  );
}

describe("init", () => {
  it.each(combinations)("makes a right product for %o", ({ licence, engines, web }) => {
    const flags = [
      "--name",
      "TaskSlash",
      "--licence",
      licence,
      ...engines.flatMap((e) => ["--engine", e]),
    ];
    if (web) flags.push("--web");
    const dest = copyTemplate();
    const run = init(dest, flags);
    expect(run.stderr.toString()).toBe("");
    expect(run.exitCode).toBe(0);
    expect(checkProduct(dest, parseOptions(flags))).toEqual([]);
  });

  it("says when --web brings in the TypeScript engine package", () => {
    const dest = copyTemplate();
    const run = init(dest, [
      "--name",
      "TaskSlash",
      "--licence",
      "mit",
      "--engine",
      "rust",
      "--web",
    ]);
    expect(run.stdout.toString()).toContain("packages/engine was added as well");
  });

  it("writes the CLI shortcut for the CLI that is kept", () => {
    const rust = copyTemplate();
    init(rust, ["--name", "TaskSlash", "--licence", "mit", "--engine", "rust", "--engine", "ts"]);
    const scripts = JSON.parse(readFileSync(join(rust, "package.json"), "utf8")).scripts;
    expect(scripts.taskslash).toBe("cargo run --quiet --package taskslash --");
    expect(scripts["test:e2e"]).toBeUndefined();
  });

  it("refuses a usage error with exit 2 and changes nothing", () => {
    const dest = copyTemplate();
    const before = snapshot(dest);
    const run = init(dest, ["--name", "taskslash", "--licence", "mit", "--engine", "ts"]);
    expect(run.exitCode).toBe(2);
    expect(run.stderr.toString()).toContain("PascalCase");
    expect(snapshot(dest)).toEqual(before);
  });
});

describe("parseOptions", () => {
  it("needs an engine", () => {
    expect(() => parseOptions(["--name", "A", "--licence", "mit"])).toThrow(UsageError);
  });

  it("refuses an unknown licence", () => {
    expect(() => parseOptions(["--name", "A", "--licence", "gpl", "--engine", "ts"])).toThrow(
      "unknown licence: gpl",
    );
  });

  it("refuses the placeholder as a name", () => {
    expect(() =>
      parseOptions(["--name", "SlashProduct", "--licence", "mit", "--engine", "ts"]),
    ).toThrow("pick a name of your own");
  });
});
