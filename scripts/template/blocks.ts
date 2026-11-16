/**
 * Marker blocks. A line `# template:begin <part>` (or `//` or `<!-- ... -->`) opens a block that
 * runs to the matching `template:end <part>`. Blocks may nest. A block is kept when its part is
 * kept; the marker lines themselves always go. Removing a block can leave two blank lines in a
 * row or a blank line at the end, which the formatter refuses, so both are tidied.
 */
const MARKER = /^\s*(?:#|\/\/|<!--)\s*template:(begin|end)\s+([\w-]+)\s*(?:-->)?\s*$/;

export function hasMarkers(text: string): boolean {
  return text.split("\n").some((line) => MARKER.test(line));
}

export function stripBlocks(text: string, kept: ReadonlySet<string>, path: string): string {
  const open: string[] = [];
  const out: string[] = [];
  text.split("\n").forEach((line, index) => {
    const marker = MARKER.exec(line);
    if (!marker) {
      if (open.every((part) => kept.has(part))) out.push(line);
      return;
    }
    const [, kind, part = ""] = marker;
    if (kind === "begin") {
      open.push(part);
      return;
    }
    if (open.pop() !== part) {
      throw new Error(`${path}:${index + 1}: template:end ${part} does not close the open block`);
    }
  });
  if (open.length > 0)
    throw new Error(`${path}: template:begin ${open.join(", ")} is never closed`);
  return tidy(out.join("\n"));
}

function tidy(text: string): string {
  return text.replace(/\n{3,}/g, "\n\n").replace(/\n+$/, "\n");
}
