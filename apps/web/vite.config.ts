import { cloudflare } from "@cloudflare/vite-plugin";
import tailwindcss from "@tailwindcss/vite";
import { tanstackStart } from "@tanstack/react-start/plugin/vite";
import viteReact from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  resolve: { tsconfigPaths: true },
  plugins: [
    cloudflare({
      viteEnvironment: { name: "ssr" },
      // The plugin probes for a free debugger port starting at 9229, and two runs probing at once
      // both get it (EADDRINUSE). Nothing here attaches a debugger.
      inspectorPort: false,
    }),
    tailwindcss(),
    tanstackStart(),
    viteReact(),
  ],
});
