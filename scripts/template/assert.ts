/**
 * The init matrix's assertion step: `bun scripts/template/assert.ts <product dir> <init flags>`.
 * Checks the product init made against its flags, and the moon projects moon itself lists. Prints
 * each failure and exits 1 on any.
 */
import { checkProduct, expectedProjects } from "./check.ts";
import { parseOptions } from "./options.ts";

const [root, ...flags] = process.argv.slice(2);
if (!root) {
  console.error("usage: bun scripts/template/assert.ts <product dir> <init flags>");
  process.exit(2);
}
const options = parseOptions(flags);
const failures = checkProduct(root, options);

const query = Bun.spawnSync(["bunx", "moon", "query", "projects"], { cwd: root });
const listed: unknown = JSON.parse(query.stdout.toString());
const ids =
  typeof listed === "object" &&
  listed !== null &&
  "projects" in listed &&
  Array.isArray(listed.projects)
    ? listed.projects.map((project: { id?: unknown }) => String(project.id)).toSorted()
    : [];
const want = expectedProjects(options);
if (ids.join(", ") !== want.join(", ")) {
  failures.push(`moon query projects lists ${ids.join(", ")}, not ${want.join(", ")}`);
}

for (const failure of failures) console.error(`FAIL ${failure}`);
console.log(`${failures.length} failure(s); moon lists ${ids.join(", ")}`);
process.exit(failures.length > 0 ? 1 : 0);
