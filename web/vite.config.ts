import { readFileSync } from "node:fs";
import path from "node:path";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";
import { configDefaults } from "vitest/config";

// The dev server proxies the API (and its websocket) to `make dev-server`.
// The Host header is left as the browser sent it, so the server's websocket
// Origin check sees matching Origin and Host.
const api = process.env.FLEETOPS_API ?? "http://127.0.0.1:9000";

// The Enterprise UI module (ee/web, under ee/LICENSE) is part of every build;
// without a license the app runs as the Community plan.
await import("./scripts/link-ee.mjs");
const eeEntry = path.resolve(import.meta.dirname, "../ee/web/src/index.ts");

// The release, from the workspace Cargo.toml, so the UI and server agree.
const version = /^version = "([^"]+)"/m.exec(readFileSync(path.resolve(import.meta.dirname, "../Cargo.toml"), "utf8"))?.[1] ?? "dev";

export default defineConfig({
  define: { "import.meta.env.FLEETOPS_VERSION": JSON.stringify(version) },
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@ee": eeEntry, "@": path.resolve(import.meta.dirname, "src") } },
  server: {
    port: 5173,
    fs: { allow: [".."] },
    proxy: { "/api": { target: api, ws: true } },
  },
  build: { outDir: "dist", emptyOutDir: true },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["src/test/setup.ts"],
    // e2e/ holds the Playwright tests (`make e2e`).
    include: ["src/**/*.test.{ts,tsx}", "../ee/web/src/**/*.test.{ts,tsx}"],
    exclude: [...configDefaults.exclude, "e2e/**"],
    restoreMocks: true,
  },
});
