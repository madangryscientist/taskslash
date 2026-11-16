#!/usr/bin/env bun
import { run } from "./cli.ts";

process.exit(run(process.argv.slice(2)));
