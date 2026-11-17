/**
 * Records the machine's pressure and memory once a second, for a proof run:
 * `bun scripts/record-machine.ts <out.jsonl>`. It reads `/proc/pressure/*` and `/proc/meminfo`
 * itself, not the daemon, so a run without TaskSlash is recorded the same way as a run with it.
 *
 * Each second it appends one JSON line to the file: `t` (ISO time), `cpu_some`, `io_some`,
 * `io_full` and `mem_some` (each `avg10`, in percent), `mem_available_mib`, `mem_total_mib` and
 * `swap_used_mib`. On SIGINT or SIGTERM it prints one summary line and exits 0: the max of each
 * pressure, the min of `MemAvailable` as a percentage of `MemTotal`, and swap growth (last minus
 * first sample) in MiB. Any argument count but one is exit 2.
 */
import { appendFileSync, readFileSync } from "node:fs";

const INTERVAL_MS = 1_000;

/** The one argument, the output path; any other argument count is exit 2. */
function outPath(): string {
  const [out, ...extra] = process.argv.slice(2);
  if (out === undefined || extra.length > 0) {
    process.stderr.write("usage: bun scripts/record-machine.ts <out.jsonl>\n");
    process.exit(2);
  }
  return out;
}
const out = outPath();

type Sample = {
  t: string;
  cpu_some: number;
  io_some: number;
  io_full: number;
  mem_some: number;
  mem_available_mib: number;
  mem_total_mib: number;
  swap_used_mib: number;
};

/** The `avg10` of the `some` or `full` line of a `/proc/pressure` file. */
function avg10(resource: string, line: "some" | "full"): number {
  const text = readFileSync(`/proc/pressure/${resource}`, "utf8");
  const match = new RegExp(`^${line} avg10=([0-9.]+)`, "m").exec(text);
  if (!match?.[1]) throw new Error(`/proc/pressure/${resource} has no ${line} avg10`);
  return Number(match[1]);
}

/** The `/proc/meminfo` fields, in MiB. */
function meminfo(): Map<string, number> {
  const fields = new Map<string, number>();
  for (const line of readFileSync("/proc/meminfo", "utf8").split("\n")) {
    const match = /^(\w+):\s+(\d+) kB$/.exec(line);
    if (match?.[1] && match[2]) fields.set(match[1], Number(match[2]) / 1024);
  }
  return fields;
}

function field(fields: Map<string, number>, name: string): number {
  const value = fields.get(name);
  if (value === undefined) throw new Error(`/proc/meminfo has no ${name}`);
  return value;
}

function sample(): Sample {
  const mem = meminfo();
  return {
    t: new Date().toISOString(),
    cpu_some: avg10("cpu", "some"),
    io_some: avg10("io", "some"),
    io_full: avg10("io", "full"),
    mem_some: avg10("memory", "some"),
    mem_available_mib: Math.round(field(mem, "MemAvailable")),
    mem_total_mib: Math.round(field(mem, "MemTotal")),
    swap_used_mib: Math.round(field(mem, "SwapTotal") - field(mem, "SwapFree")),
  };
}

const samples: Sample[] = [];

function record(): void {
  const s = sample();
  samples.push(s);
  appendFileSync(out, `${JSON.stringify(s)}\n`);
}

function summary(): string {
  const first = samples[0];
  const last = samples.at(-1);
  if (!first || !last) return "record-machine: no samples";
  const max = (key: "cpu_some" | "io_some" | "io_full" | "mem_some") =>
    Math.max(...samples.map((s) => s[key])).toFixed(2);
  const minAvailablePct = Math.min(
    ...samples.map((s) => (100 * s.mem_available_mib) / s.mem_total_mib),
  ).toFixed(1);
  return (
    `record-machine: ${samples.length} samples, ${first.t} to ${last.t}; ` +
    `max cpu some ${max("cpu_some")}%, io some ${max("io_some")}%, ` +
    `io full ${max("io_full")}%, memory some ${max("mem_some")}%; ` +
    `min MemAvailable ${minAvailablePct}% of MemTotal; ` +
    `swap growth ${last.swap_used_mib - first.swap_used_mib} MiB`
  );
}

record();
const timer = setInterval(record, INTERVAL_MS);

function stop(): void {
  clearInterval(timer);
  process.stdout.write(`${summary()}\n`);
  process.exit(0);
}
process.on("SIGINT", stop);
process.on("SIGTERM", stop);
