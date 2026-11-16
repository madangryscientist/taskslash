import { defineConfig, devices } from "@playwright/test";

const PORT = 4320;

// Happy paths only, against a local production build served by `vite preview`.
export default defineConfig({
  testDir: "./e2e",
  globalSetup: "../../scripts/test-tmp-scope.ts",
  forbidOnly: !!process.env["CI"],
  reporter: "list",
  use: { baseURL: `http://localhost:${PORT}`, trace: "on-first-retry" },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command: [
      "rm -rf .wrangler/e2e",
      "mkdir -p .wrangler/e2e/tmp",
      "bun run build",
      `vite preview --port ${PORT} --strictPort`,
    ].join(" && "),
    env: {
      // The web server starts before `globalSetup` scopes TMPDIR, so it gets its own under
      // .wrangler/e2e, which the next run clears: miniflare and wrangler leave nothing in /tmp.
      TMPDIR: ".wrangler/e2e/tmp",
    },
    url: `http://localhost:${PORT}`,
    reuseExistingServer: false,
    timeout: 300_000,
  },
});
