#!/usr/bin/env bun
/**
 * Creates a product from this template, in place: `bun scripts/init.ts --name <Name> --licence
 * mit|elastic --engine ts|rust [--engine ...] [--web]`. It removes itself when it is done.
 * Exit codes: 0 ok, 1 it failed, 2 a usage error.
 */
import { join } from "node:path";
import { USAGE, UsageError, parseOptions } from "./template/options.ts";
import { createProduct } from "./template/product.ts";

const root = join(import.meta.dir, "..");

try {
  const options = parseOptions(process.argv.slice(2));
  for (const line of createProduct(root, options)) console.log(line);
} catch (error) {
  if (error instanceof UsageError) {
    console.error(`${error.message}\n\n${USAGE}`);
    process.exit(2);
  }
  throw error;
}
