/** Walking the tree: every file init may rewrite, outside what tools leave behind. */
import { readdirSync, rmdirSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const SKIPPED = new Set(["node_modules", ".git", "target", "dist", ".wrangler", ".tanstack"]);
const SKIPPED_PATHS = new Set([".moon/cache"]);

function skipped(root: string, path: string, entry: string): boolean {
  return SKIPPED.has(entry) || SKIPPED_PATHS.has(relative(root, path));
}

/** Every file under `root`, as paths relative to it, deepest last. */
export function listFiles(root: string, dir = root): string[] {
  const files: string[] = [];
  for (const entry of readdirSync(dir).toSorted()) {
    const path = join(dir, entry);
    if (skipped(root, path, entry)) continue;
    if (statSync(path).isDirectory()) files.push(...listFiles(root, path));
    else files.push(relative(root, path));
  }
  return files;
}

/** Every directory under `root` (relative), parents before children. */
export function listDirs(root: string, dir = root): string[] {
  const dirs: string[] = [];
  for (const entry of readdirSync(dir).toSorted()) {
    const path = join(dir, entry);
    if (skipped(root, path, entry) || !statSync(path).isDirectory()) continue;
    dirs.push(relative(root, path), ...listDirs(root, path));
  }
  return dirs;
}

/** Removes `dir` under `root` if nothing is left in it. */
export function removeIfEmpty(root: string, dir: string): void {
  const path = join(root, dir);
  try {
    if (readdirSync(path).length === 0) rmdirSync(path);
  } catch (error) {
    if (!(error instanceof Error && "code" in error && error.code === "ENOENT")) throw error;
  }
}
