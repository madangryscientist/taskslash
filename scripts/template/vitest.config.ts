import { defineConfig } from "vitest/config";

// The template's own tests. Run under the Bun runtime (`bun --bun vitest`): init uses Bun APIs.
export default defineConfig({
  test: {
    environment: "node",
    include: ["template/**/*.test.ts"],
    globalSetup: ["./test-tmp-scope.ts"],
    testTimeout: 30_000,
  },
});
