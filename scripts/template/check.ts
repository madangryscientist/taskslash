/**
 * What a product made by init must look like, as a list of failures (empty when it is right). The
 * template's tests and its CI's init matrix both run it, so a broken init fails the gate.
 */
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { listFiles } from "./files.ts";
import { type Options, PLACEHOLDER, licenceId, nameForms } from "./options.ts";
import { PART_PATHS } from "./product.ts";

type Check = { root: string; options: Options; fail: (message: string) => void };

function checkLeftovers({ root, fail }: Check): void {
  for (const path of listFiles(root)) {
    if (path.toLowerCase().includes(PLACEHOLDER.lower))
      fail(`${path}: the placeholder is in its path`);
    const text = readFileSync(join(root, path), "utf8");
    if (text.toLowerCase().includes(PLACEHOLDER.lower))
      fail(`${path}: the placeholder is left in it`);
    if (/template:(begin|end)/.test(text)) fail(`${path}: a template marker is left in it`);
  }
  for (const path of PART_PATHS["self-test"] ?? []) {
    if (existsSync(join(root, path))) fail(`${path} is still there`);
  }
  const ci = readFileSync(join(root, ".github/workflows/ci.yml"), "utf8");
  if (ci.includes("scripts/init.ts")) fail("ci.yml still has the template's self-test job");
}

function licenceFields(root: string): { path: string; value: string | undefined }[] {
  return listFiles(root)
    .filter((path) => path.endsWith("package.json") || path.endsWith("Cargo.toml"))
    .flatMap((path) => {
      const text = readFileSync(join(root, path), "utf8");
      if (path.endsWith("package.json")) {
        const value: unknown = JSON.parse(text).license;
        return [{ path, value: typeof value === "string" ? value : undefined }];
      }
      // The workspace root Cargo.toml has no [package] and no licence of its own.
      if (!text.includes("[package]")) return [];
      return [{ path, value: /^license = "([^"]*)"$/m.exec(text)?.[1] }];
    });
}

function checkLicence({ root, options, fail }: Check): void {
  const licence = readFileSync(join(root, "LICENSE"), "utf8");
  if (options.licence === "mit" && !licence.startsWith("MIT License")) {
    fail("LICENSE does not start with MIT License");
  }
  if (options.licence === "elastic" && !licence.includes("Elastic License 2.0")) {
    fail("LICENSE does not hold the Elastic License 2.0");
  }
  const id = licenceId(options.licence);
  for (const { path, value } of licenceFields(root)) {
    if (value !== id) fail(`${path}: license is ${value ?? "missing"}, not ${id}`);
  }
}

function expectPath(check: Check, path: string, present: boolean): void {
  if (existsSync(join(check.root, path)) !== present) {
    check.fail(`${path} should ${present ? "exist" : "not exist"}`);
  }
}

function checkEngines(check: Check): void {
  const { engines, web, name } = check.options;
  const lower = nameForms(name).lower;
  const rust = engines.has("rust");
  const ts = engines.has("ts");
  for (const path of ["Cargo.toml", "crates", `crates/${lower}`, `crates/${lower}-core`]) {
    expectPath(check, path, rust);
  }
  expectPath(check, "packages/engine", ts || web);
  expectPath(check, "packages/cli", ts && !rust);
  expectPath(check, ".github/workflows/release.yml", ts && !rust);
}

function checkWeb(check: Check): void {
  const { web } = check.options;
  expectPath(check, "apps/web", web);
  expectPath(check, ".github/workflows/ui-review.yml", web);
  if (!web) return;
  const engine = `@hackcove/${nameForms(check.options.name).lower}-engine`;
  const pkg = JSON.parse(readFileSync(join(check.root, "apps/web/package.json"), "utf8"));
  if (pkg.dependencies?.[engine] !== "workspace:*")
    check.fail(`apps/web does not depend on ${engine}`);
}

/** The moon projects a product should have, by id. */
export function expectedProjects(options: Options): string[] {
  const ts = options.engines.has("ts");
  const rust = options.engines.has("rust");
  const ids = ["scripts"];
  if (ts || options.web) ids.push("engine");
  if (ts && !rust) ids.push("cli");
  if (rust) ids.push("core", "bin");
  if (options.web) ids.push("web");
  return ids.toSorted();
}

function checkWorkspace(check: Check): void {
  const workspace: unknown = Bun.YAML.parse(
    readFileSync(join(check.root, ".moon/workspace.yml"), "utf8"),
  );
  const projects =
    typeof workspace === "object" && workspace !== null && "projects" in workspace
      ? Object.keys(workspace.projects ?? {})
      : [];
  const got = projects.toSorted().join(", ");
  const want = expectedProjects(check.options).join(", ");
  if (got !== want) check.fail(`.moon/workspace.yml lists ${got}, not ${want}`);
}

export function checkProduct(root: string, options: Options): string[] {
  const failures: string[] = [];
  const check: Check = { root, options, fail: (message) => failures.push(message) };
  for (const step of [checkLeftovers, checkLicence, checkEngines, checkWeb, checkWorkspace]) {
    step(check);
  }
  return failures;
}
