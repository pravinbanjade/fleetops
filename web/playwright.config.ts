import { defineConfig, devices } from "@playwright/test";

// The e2e tests drive a release build of the fleetops binary (with this UI
// embedded) against a real Docker daemon. `make e2e` builds the binary and
// sets FLEETOPS_BIN and DOCKER_HOST. Each run starts from an empty data
// directory, so first-run setup is always open.
const port = Number(process.env.FLEETOPS_E2E_PORT ?? 9123);
const bin = process.env.FLEETOPS_BIN ?? "../target/release/fleetops";
const data = "e2e-data";

export default defineConfig({
  testDir: "e2e",
  // One server, one admin account and one Docker daemon: the tests share
  // state and run in order.
  workers: 1,
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: 0,
  timeout: 120_000,
  expect: { timeout: 15_000 },
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    trace: "retain-on-failure",
  },
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
  webServer: {
    command: `rm -rf ${data} && exec ${bin} --data ${data} --bind 127.0.0.1:${port}`,
    url: `http://127.0.0.1:${port}/api/v1/health`,
    reuseExistingServer: false,
    stdout: "pipe",
    stderr: "pipe",
    timeout: 30_000,
  },
});
