import { describe, expect, it } from "vitest";
import pkg from "../package.json" with { type: "json" };
import { run } from "../src/cli.ts";

function capture(argv: string[]) {
  let out = "";
  let err = "";
  const code = run(argv, { out: (t) => (out += t), err: (t) => (err += t) });
  return { code, out, err };
}

describe("slashproduct", () => {
  it("prints its version", () => {
    expect(capture(["--version"])).toEqual({
      code: 0,
      out: `slashproduct ${pkg.version}\n`,
      err: "",
    });
  });

  it("greets by name", () => {
    expect(capture(["greet", "Ada"]).out).toBe("Hello, Ada, from SlashProduct.\n");
  });

  it("exits 2 on a usage error", () => {
    const result = capture(["nope"]);
    expect(result.code).toBe(2);
    expect(result.err).toContain("usage:");
  });
});
