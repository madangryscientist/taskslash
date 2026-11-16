/**
 * Installs or upgrades TaskSlash as a systemd user service on this machine, in one command:
 * `bun scripts/install-service.ts` from the repo root. It takes no arguments.
 *
 * The steps run in order, and the first one that fails stops the script with exit 1 and the
 * failing command's output: cargo install the binary, copy the unit into the user's systemd
 * directory, daemon-reload, then restart the unit when it is active (an upgrade) or enable it
 * now otherwise (a first install). Last, it waits for the daemon to answer `status`, so the
 * run only reports success when the daemon is up.
 */
import { copyFileSync, mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const UNIT = "taskslash";
const REPO_ROOT = join(import.meta.dir, "..");
const BINARY = join(homedir(), ".cargo", "bin", "taskslash");
const UNIT_SOURCE = join(REPO_ROOT, "crates", "taskslash", "systemd", `${UNIT}.service`);
const UNIT_DIR = join(process.env.XDG_CONFIG_HOME || join(homedir(), ".config"), "systemd", "user");
const READY_TIMEOUT_MS = 10_000;
const READY_POLL_MS = 250;
const READY_PROBE_MS = 1_000;

if (process.argv.length > 2) {
  process.stderr.write(`usage: bun scripts/install-service.ts (takes no arguments)\n`);
  process.exit(2);
}

type Captured = { exitCode: number; output: string };

/** Runs argv with no shell and returns its exit code and combined output. */
function capture(argv: string[], timeout?: number): Captured {
  const result = Bun.spawnSync(argv, {
    cwd: REPO_ROOT,
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
    ...(timeout === undefined ? {} : { timeout }),
  });
  const output = `${result.stdout.toString()}${result.stderr.toString()}`;
  return { exitCode: result.exitCode, output };
}

/** Stops the script with exit 1, naming the step and printing the output that explains it. */
function fail(label: string, output: string): never {
  process.stderr.write(
    `install-service: ${label} failed\n${output.endsWith("\n") ? output : `${output}\n`}`,
  );
  process.exit(1);
}

/** Runs one step; a non-zero exit (or a command that cannot start) fails the script. */
function step(name: string, argv: string[]): string {
  let result: Captured;
  try {
    result = capture(argv);
  } catch (error) {
    fail(name, error instanceof Error ? error.message : String(error));
  }
  if (result.exitCode !== 0) fail(name, result.output);
  return result.output;
}

/** Like `step`, but the command's output goes straight to the terminal, for a long build. */
function liveStep(name: string, argv: string[]): void {
  let exitCode: number;
  try {
    exitCode = Bun.spawnSync(argv, {
      cwd: REPO_ROOT,
      stdin: "ignore",
      stdout: "inherit",
      stderr: "inherit",
    }).exitCode;
  } catch (error) {
    fail(name, error instanceof Error ? error.message : String(error));
  }
  if (exitCode !== 0) fail(name, "the output above is the failing command's output");
}

/** One `status` probe, cut off at `timeout` ms so a silent daemon cannot hold the wait past its limit. */
function daemonAnswers(timeout: number): boolean {
  try {
    return capture([BINARY, "status", "--json"], timeout).exitCode === 0;
  } catch {
    return false;
  }
}

// 1. The binary.
liveStep("cargo install", [
  "cargo",
  "install",
  "--locked",
  "--path",
  join(REPO_ROOT, "crates", "taskslash"),
]);

// 2. The unit file.
try {
  mkdirSync(UNIT_DIR, { recursive: true });
  copyFileSync(UNIT_SOURCE, join(UNIT_DIR, `${UNIT}.service`));
} catch (error) {
  fail("copy unit", error instanceof Error ? error.message : String(error));
}

// 3. systemd reads the unit.
step("systemctl daemon-reload", ["systemctl", "--user", "daemon-reload"]);

// 4. Upgrade a running daemon, or start a new one.
const active = capture(["systemctl", "--user", "is-active", UNIT]).output.trim() === "active";
if (active) {
  step("systemctl restart", ["systemctl", "--user", "restart", UNIT]);
} else {
  step("systemctl enable --now", ["systemctl", "--user", "enable", "--now", UNIT]);
}

// 5. Wait for the daemon to answer.
const deadline = Date.now() + READY_TIMEOUT_MS;
for (;;) {
  const remaining = deadline - Date.now();
  if (remaining <= 0) {
    const journal = capture(["journalctl", "--user", "-u", UNIT, "--no-pager", "-n", "20"]).output;
    fail(`${UNIT} did not answer \`status\` within ${READY_TIMEOUT_MS / 1000} s`, journal);
  }
  if (daemonAnswers(Math.min(READY_PROBE_MS, remaining))) break;
  Bun.sleepSync(Math.min(READY_POLL_MS, Math.max(0, deadline - Date.now())));
}

// `taskslash --version` prints "taskslash <version>"; the version is its last word.
const version =
  step("taskslash --version", [BINARY, "--version"]).trim().split(/\s+/).at(-1) ?? "unknown";
process.stdout.write(`taskslash ${version} running\n`);
