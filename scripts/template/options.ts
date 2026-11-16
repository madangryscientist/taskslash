/**
 * The choices `bun scripts/init.ts` takes, and the parts of the template each choice keeps.
 * A part is a name used twice: in `template:begin <part>` / `template:end <part>` marker lines
 * (blocks.ts), and in the list of paths that belong to it (product.ts).
 */
import { parseArgs } from "node:util";

export type Licence = "mit" | "elastic";
export type Engine = "ts" | "rust";

export type Options = {
  /** The product's name as it is written, e.g. `TaskSlash`. */
  name: string;
  licence: Licence;
  engines: ReadonlySet<Engine>;
  web: boolean;
};

export const PLACEHOLDER = { name: "SlashProduct", lower: "slashproduct", upper: "SLASHPRODUCT" };

export const USAGE = `usage: bun scripts/init.ts --name <Name> --licence mit|elastic --engine ts|rust [--engine ...] [--web]

  --name      the product's name in PascalCase, e.g. TaskSlash
  --licence   mit (MIT License) or elastic (Elastic License 2.0)
  --engine    ts (a TypeScript engine and npm CLI) or rust (a cargo workspace with a Rust CLI);
              give it twice for both, side by side
  --web       add the web app (TanStack Start on Cloudflare); it imports the TypeScript engine
              package, so --web with only --engine rust adds packages/engine as well
`;

export class UsageError extends Error {}

const NAME = /^[A-Z][a-z0-9]*(?:[A-Z][a-z0-9]*)*$/;

function parseEngines(values: string[] | undefined): Set<Engine> {
  if (!values || values.length === 0) throw new UsageError("give at least one --engine");
  const engines = new Set<Engine>();
  for (const value of values) {
    if (value !== "ts" && value !== "rust") throw new UsageError(`unknown engine: ${value}`);
    engines.add(value);
  }
  return engines;
}

function parseLicence(value: string | undefined): Licence {
  if (value === "mit" || value === "elastic") return value;
  throw new UsageError(value ? `unknown licence: ${value}` : "give --licence mit or elastic");
}

function parseName(value: string | undefined): string {
  if (!value) throw new UsageError("give --name");
  if (!NAME.test(value))
    throw new UsageError(`--name must be PascalCase letters and digits: ${value}`);
  if (value.toLowerCase() === PLACEHOLDER.lower) throw new UsageError("pick a name of your own");
  return value;
}

/** Reads the command line; throws a UsageError on anything it cannot use. */
export function parseOptions(argv: string[]): Options {
  let values;
  try {
    ({ values } = parseArgs({
      args: argv,
      strict: true,
      options: {
        name: { type: "string" },
        licence: { type: "string" },
        engine: { type: "string", multiple: true },
        web: { type: "boolean", default: false },
      },
    }));
  } catch (error) {
    throw new UsageError(error instanceof Error ? error.message : String(error));
  }
  return {
    name: parseName(values.name),
    licence: parseLicence(values.licence),
    engines: parseEngines(values.engine),
    web: values.web,
  };
}

/** The template parts this product keeps. `self-test` (the template's own tests) is never kept. */
export function keptParts(options: Options): Set<string> {
  const ts = options.engines.has("ts");
  const rust = options.engines.has("rust");
  const parts = new Set<string>([options.licence]);
  if (rust) parts.add("rust");
  // With a Rust engine the CLI is the Rust binary, so the npm CLI is left out.
  if (ts && !rust) parts.add("cli");
  if (ts || options.web) parts.add("engine");
  if (options.web) parts.add("web");
  return parts;
}

export const ALL_PARTS = ["mit", "elastic", "rust", "cli", "engine", "web", "self-test"];

export function licenceId(licence: Licence): string {
  return licence === "mit" ? "MIT" : "Elastic-2.0";
}

export function nameForms(name: string) {
  return { name, lower: name.toLowerCase(), upper: name.toUpperCase() };
}
