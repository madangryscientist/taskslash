import { greet } from "@hackcove/slashproduct-engine";
import pkg from "../package.json" with { type: "json" };

const USAGE = `usage:
  slashproduct greet <name>   greet someone
  slashproduct --version      print the version
  slashproduct --help         print this help
`;

type Streams = { out: (text: string) => void; err: (text: string) => void };

const processStreams: Streams = {
  out: (text) => void process.stdout.write(text),
  err: (text) => void process.stderr.write(text),
};

const FLAGS: Record<string, (io: Streams) => void> = {
  "--version": (io) => io.out(`slashproduct ${pkg.version}\n`),
  "-v": (io) => io.out(`slashproduct ${pkg.version}\n`),
  "--help": (io) => io.out(USAGE),
  "-h": (io) => io.out(USAGE),
};

/** Runs one command line and returns its exit code: 0 ok, 2 a usage error. */
export function run(argv: string[], io: Streams = processStreams): number {
  const [first = "", ...rest] = argv;
  const flag = FLAGS[first];
  if (flag && rest.length === 0) {
    flag(io);
    return 0;
  }
  const [name] = rest;
  if (first === "greet" && name !== undefined && rest.length === 1) {
    io.out(`${greet(name)}\n`);
    return 0;
  }
  io.err(USAGE);
  return 2;
}
