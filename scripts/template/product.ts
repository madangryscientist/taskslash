/**
 * Turns a copy of the template into a product: keeps the parts chosen, removes the rest with their
 * moon projects and workflow jobs, writes the licence, puts the product's name everywhere, and
 * removes itself. Runs on Bun with no installed dependency, so it works on a fresh clone.
 */
import { existsSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { listDirs, listFiles, removeIfEmpty } from "./files.ts";
import { type Options, ALL_PARTS, PLACEHOLDER, keptParts, licenceId } from "./options.ts";
import { rename, rewrite } from "./rewrite.ts";
import { hasMarkers, stripBlocks } from "./blocks.ts";

/** The paths each part owns, removed with it. */
export const PART_PATHS: Record<string, string[]> = {
  rust: ["Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "crates", ".moon/tasks/rust.yml"],
  cli: ["packages/cli", ".github/workflows/release.yml"],
  engine: ["packages/engine"],
  web: ["apps/web", ".github/workflows/ui-review.yml"],
  "self-test": ["scripts/init.ts", "scripts/template", "licences"],
};

const LICENCE_FILE = { mit: "MIT.txt", elastic: "Elastic-2.0.txt" };

function writeLicence(root: string, options: Options, kept: ReadonlySet<string>): void {
  const year = String(new Date().getFullYear());
  const licence = readFileSync(join(root, "licences", LICENCE_FILE[options.licence]), "utf8");
  writeFileSync(join(root, "LICENSE"), licence.replaceAll("{{year}}", year));
  const notice = readFileSync(join(root, "licences", "NOTICE"), "utf8");
  const text = hasMarkers(notice) ? stripBlocks(notice, kept, "licences/NOTICE") : notice;
  writeFileSync(join(root, "NOTICE"), text.replaceAll("{{year}}", year));
}

type Scripts = Record<string, string>;

function isScripts(value: unknown): value is Scripts {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The root scripts: the CLI shortcut runs whichever CLI is kept, and dropped parts lose theirs. */
function editRootScripts(root: string, kept: ReadonlySet<string>): void {
  const path = join(root, "package.json");
  const pkg: Record<string, unknown> = JSON.parse(readFileSync(path, "utf8"));
  const scripts = pkg["scripts"];
  if (!isScripts(scripts)) throw new Error("package.json has no scripts");
  if (!kept.has("cli")) {
    scripts[PLACEHOLDER.lower] = `cargo run --quiet --package ${PLACEHOLDER.lower} --`;
  }
  if (!kept.has("rust")) scripts["format:check"] = "oxfmt --check";
  if (!kept.has("web")) delete scripts["test:e2e"];
  writeFileSync(path, `${JSON.stringify(pkg, null, 2)}\n`);
}

function removeParts(root: string, kept: ReadonlySet<string>): void {
  for (const part of ALL_PARTS) {
    if (kept.has(part)) continue;
    for (const path of PART_PATHS[part] ?? [])
      rmSync(join(root, path), { recursive: true, force: true });
  }
  for (const dir of ["packages", "apps", "crates"]) removeIfEmpty(root, dir);
}

function rewriteFiles(root: string, options: Options, kept: ReadonlySet<string>): void {
  const id = licenceId(options.licence);
  for (const path of listFiles(root)) {
    const file = join(root, path);
    const bytes = readFileSync(file);
    if (bytes.includes(0)) continue;
    const text = bytes.toString("utf8");
    const next = rewrite({ path, text, kept, name: options.name, id });
    if (next !== text) writeFileSync(file, next);
  }
}

/** Renames every file and directory whose name holds the placeholder, deepest first. */
function renamePaths(root: string, name: string): void {
  const paths = [...listDirs(root), ...listFiles(root)]
    .filter((path) => path.split("/").at(-1)?.toLowerCase().includes(PLACEHOLDER.lower))
    .toSorted((a, b) => b.split("/").length - a.split("/").length);
  for (const path of paths) {
    const parts = path.split("/");
    const last = parts.pop() ?? "";
    renameSync(join(root, path), join(root, ...parts, rename(last, name)));
  }
}

/** Creates the product in `root` (a copy of the template) and returns what it did, line by line. */
export function createProduct(root: string, options: Options): string[] {
  if (!existsSync(join(root, "licences"))) throw new Error(`${root} is not a copy of the template`);
  const kept = keptParts(options);
  editRootScripts(root, kept);
  writeLicence(root, options, kept);
  removeParts(root, kept);
  rewriteFiles(root, options, kept);
  renamePaths(root, options.name);
  return describe(options, kept);
}

function describe(options: Options, kept: ReadonlySet<string>): string[] {
  const lines = [
    `${options.name}, under the ${options.licence === "mit" ? "MIT License" : "Elastic License 2.0"}.`,
  ];
  if (kept.has("rust"))
    lines.push("rust: a cargo workspace in crates/, and the CLI is the Rust binary.");
  if (kept.has("cli"))
    lines.push("ts: packages/engine and the npm CLI in packages/cli, with release.yml.");
  if (kept.has("engine") && !kept.has("cli"))
    lines.push("ts: packages/engine, the TypeScript library.");
  if (options.web && !options.engines.has("ts")) {
    lines.push(
      "--web imports the TypeScript engine package, so packages/engine was added as well.",
    );
  }
  if (kept.has("web")) lines.push("web: apps/web, with the UI review workflow.");
  lines.push("Next: bun install, then the gate (AGENTS.md), then commit.");
  return lines;
}
