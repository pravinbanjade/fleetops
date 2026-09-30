/// <reference types="vitest/config" />
import path from "node:path";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vite";

// The dev server proxies the API (and its websocket) to `make dev-server`.
// The Host header is left as the browser sent it, so the server's websocket
// Origin check sees matching Origin and Host.
const api = process.env.FLEETOPS_API ?? "http://127.0.0.1:9000";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": path.resolve(import.meta.dirname, "src") } },
  server: {
    port: 5173,
    proxy: { "/api": { target: api, ws: true } },
  },
  // One bundle until the resource pages land (P3b splits them by route).
  build: { outDir: "dist", emptyOutDir: true, chunkSizeWarningLimit: 1024 },
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["src/test/setup.ts"],
    restoreMocks: true,
  },
});
