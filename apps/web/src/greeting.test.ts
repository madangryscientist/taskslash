import { greet } from "@hackcove/slashproduct-engine";
import { describe, expect, it } from "vitest";

// The web app imports the engine as a workspace package; this test fails if that link breaks.
describe("the engine, as the web app sees it", () => {
  it("greets", () => {
    expect(greet("world")).toBe("Hello, world, from SlashProduct.");
  });
});
