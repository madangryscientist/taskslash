/** What init does to the text of each file: blocks, the product's name, the licence fields. */
import { PLACEHOLDER, nameForms } from "./options.ts";
import { hasMarkers, stripBlocks } from "./blocks.ts";

/** Swaps the placeholder name for the product's, in each of its three spellings. */
export function rename(text: string, name: string): string {
  const forms = nameForms(name);
  return text
    .replaceAll(PLACEHOLDER.name, forms.name)
    .replaceAll(PLACEHOLDER.upper, forms.upper)
    .replaceAll(PLACEHOLDER.lower, forms.lower);
}

/** Sets the `license` field of a package.json or a Cargo.toml, wherever one is written. */
export function setLicenceField(path: string, text: string, id: string): string {
  if (path.endsWith("package.json")) {
    return text.replace(/^(\s*"license":\s*)"[^"]*"/m, `$1"${id}"`);
  }
  if (path.endsWith("Cargo.toml")) return text.replace(/^license = "[^"]*"$/m, `license = "${id}"`);
  return text;
}

type Rewrite = { path: string; text: string; kept: ReadonlySet<string>; name: string; id: string };

export function rewrite({ path, text, kept, name, id }: Rewrite): string {
  const stripped = hasMarkers(text) ? stripBlocks(text, kept, path) : text;
  return rename(setLicenceField(path, stripped, id), name);
}
