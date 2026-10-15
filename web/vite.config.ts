import path from "node:path";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";
import { configDefaults } from "vitest/config";

// The dev server proxies the API (and its websocket) to `make dev-server`.
// The Host header is left as the browser sent it, so the server's websocket
// Origin check sees matching Origin and Host.
const api = process.env.FLEETOPS_API ?? "http://127.0.0.1:9000";

// FLEETOPS_EDITION=ee builds the Enterprise UI (ee/web, under ee/LICENSE);
// anything else builds Community, where `@ee` is an empty stub.
const ee = process.env.FLEETOPS_EDITION === "ee";
if (ee) await import("./scripts/link-ee.mjs");
const eeEntry = ee
  ? path.resolve(import.meta.dirname, "../ee/web/src/index.ts")
  : path.resolve(import.meta.dirname, "src/ee-stub.ts");

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@ee": eeEntry, "@": path.resolve(import.meta.dirname, "src") } },
  server: {
    port: 5173,
    fs: { allow: ee ? [".."] : ["."] },
    proxy: { "/api": { target: api, ws: true } },
  },
  build: { outDir: "dist", emptyOutDir: true },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["src/test/setup.ts"],
    // e2e/ holds the Playwright tests (`make e2e`).
    include: ["src/**/*.test.{ts,tsx}", ...(ee ? ["../ee/web/src/**/*.test.{ts,tsx}"] : [])],
    exclude: [...configDefaults.exclude, "e2e/**"],
    restoreMocks: true,
  },
});
