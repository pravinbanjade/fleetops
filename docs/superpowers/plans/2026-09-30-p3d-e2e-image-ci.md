# FleetOps P3d — End-to-End Tests, Container Image and CI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish sub-project 1's delivery pipeline:
- a **Playwright end-to-end test** of the spec's core flow, against a release build and a real Docker daemon (`make e2e`);
- a **container image**: a static musl binary for linux/amd64 and linux/arm64 on `distroless/static`, running as non-root (`make image`), with a smoke test (`make image-smoke`);
- a **GitHub Actions workflow** that runs every gate: fmt, clippy, cargo tests, the Docker integration tests, the web checks, the e2e test, and the image build.

**Architecture:**
- **No application code changes.** Every file this plan touches is build, test or CI tooling, plus `README.md`. The one change that reaches the shipped binary is `strip = true` in the release profile.
- **e2e:**
  - `web/e2e/core-flow.spec.ts` is one Playwright test, with a `test.step` per stage of the spec's flow. It runs in Chromium.
  - Playwright's `webServer` starts the release binary (`FLEETOPS_BIN`) on `127.0.0.1:9123`, with a data directory it empties first (`web/e2e-data`), so first-run setup is always open.
  - The test talks to Docker only through FleetOps, except in two places. It uses the docker CLI to stop the container from outside, which proves the list follows Docker events. It also uses the CLI to clean up after a failure.
- **Image:**
  - The multi-stage `Dockerfile` runs every build stage on the build machine's own platform (`--platform=$BUILDPLATFORM`).
  - A Node stage builds `web/dist`. A Rust stage then cross-compiles the server for the target platform with `cargo zigbuild --target <arch>-unknown-linux-musl`, using zig and cargo-zigbuild copied from the cargo-zigbuild project's image.
  - A multi-arch build needs no emulation and no host toolchain beyond Docker.
- **CI:** five independent jobs on `ubuntu-latest`: `rust`, `docker-it`, `web`, `e2e` and `image`. They call the same `make` targets a developer runs. The exception is the image job, which builds with `docker/build-push-action` (see the spec deltas below).

**Tech Stack:** P3c's stack, plus:
- `@playwright/test` 1.63 (Chromium);
- Docker BuildKit / buildx, with `rust:1.98-slim-trixie`, `node:24-trixie-slim`, `ghcr.io/rust-cross/cargo-zigbuild:0.23.4` (zig 0.16) and `gcr.io/distroless/static-debian13:nonroot`;
- GitHub Actions:
  - `actions/checkout@v7`, `actions/setup-node@v7`, `actions/upload-artifact@v7`;
  - `pnpm/action-setup@v6`;
  - `Swatinem/rust-cache@v2`;
  - `docker/setup-buildx-action@v4`, `docker/build-push-action@v7`.

**Spec:** `docs/superpowers/specs/2026-09-29-fleetops-core-docker-design.md`:
- §7: the container image is distroless/static and runs as non-root. Socket access is granted through group membership, and the docs explain the trade-off.
- §8, e2e row: "Playwright: setup → login → pull image → run container → live list update → logs search → exec → remove".
- §9:
  - `make image`: a multi-arch (linux/amd64, linux/arm64) static musl binary via `cargo-zigbuild`, packaged in a distroless/static image.
  - CI (GitHub Actions): `cargo fmt --check`, `clippy -D warnings`, `cargo test`, the `docker-it` tests, `pnpm typecheck lint test`, Playwright e2e and the image build.
  - The OpenAPI client drift check.

**Plan series for sub-project 1:**
- P1, P2a–P2c and P3a–P3c are done.
- **P3d (this plan)** completes sub-project 1.

## Global Constraints

- **Everything from P3a–P3c's Global Constraints still holds:**
  - Node ≥24 and pnpm 12.8.1; TypeScript `~6.0.3`.
  - `pnpm check:api` stays clean.
  - `src/test/setup.ts`'s 3 s `asyncUtilTimeout`.
- **Tools this plan needs on the machine:**
  - A running Docker daemon, reachable through the current docker context: `docker context inspect --format '{{.Endpoints.docker.Host}}'`.
  - buildx, and for `make image`'s two-platform `--load`, Docker's containerd image store. Check with `docker info --format '{{json .DriverStatus}}'`, which should show `io.containerd.snapshotter.v1`.
  - No zig or cargo-zigbuild on the host.
- **The daemon may run other people's containers:**
  - The e2e test creates exactly one container, named `fleetops-e2e-<base36 time>`, and removes it, even when the test fails (`afterAll`).
  - The smoke test's container is `fleetops-smoke-<pid>`, removed on exit.
  - Neither touches anything else. The e2e test leaves `busybox:1.37` pulled.
- **Ports:**
  - The e2e server listens on `127.0.0.1:9123` (override with `FLEETOPS_E2E_PORT`).
  - The smoke test publishes on a random loopback port.
- **Pinned versions** (bump deliberately, not incidentally):
  - `@playwright/test` `^1.63.0`;
  - `RUST_VERSION=1.98`, `NODE_VERSION=24` and `ZIGBUILD_VERSION=0.23.4` in the Dockerfile;
  - `busybox:1.37`;
  - the GitHub Action majors listed above.
- **Rust worktrees:** set `CARGO_TARGET_DIR` to the main checkout's `target/` so that builds are shared. `make e2e` finds the binary through `$(CARGO_TARGET_DIR)`, falling back to `target`.
- **Spec deltas (intentional):**
  - **The cross toolchain lives inside the image build.** cargo-zigbuild runs in the Dockerfile's build stage, not on the host, so `make image` needs only Docker. zig and cargo-zigbuild come from `ghcr.io/rust-cross/cargo-zigbuild`, and Rust comes from the official `rust` image. The cargo-zigbuild image ships Rust 1.93, and the official image gives this repo the current stable.
  - **`rust-toolchain.toml` is not in the image build context** (`.dockerignore`). It names `stable`, which would make rustup download a second toolchain inside the build stage, without the musl targets.
  - **`make image` loads into the local image store** as `fleetops:dev`. Nothing is pushed anywhere, and publishing to a registry is out of scope for sub-project 1.
  - **CI's image job uses `docker/build-push-action`, not `make image`.** The runner's Docker has no containerd image store, so a two-platform `--load` fails there. The job builds both platforms, keeping the result in the GHA cache, then loads the runner's platform and runs `make image-smoke` on it.
  - **The e2e flow logs out and back in after setup.** Setup already signs the admin in, and the spec's flow names "login" as its own step.
  - **The live list update is driven from outside FleetOps.** The test runs `docker stop` from the CLI, and the list must show `exited` without a reload. A change made through FleetOps would refresh the list without any event.
  - **`strip = true` in `[profile.release]`.** It takes the binary from about 29 MB to 15–17 MB, which is what makes the spec's "~20 MB image" true. It also applies to `make build`.
  - **The e2e test uses two stable test hooks that aren't roles:** xterm.js's `.xterm` (to focus the terminal) and `.xterm-rows` (its DOM renderer's text). Everything else is found by role and name.

## Review Focus

1. **The e2e test fails midway, or is interrupted.** The run must not leave a `fleetops-e2e-*` container on a daemon that also runs other people's containers. The next run must start clean, with an empty data directory and setup open. Pinned in:
   - Task 1: the `afterAll` that runs `docker rm -f`, and `rm -rf e2e-data` in the `webServer` command.
   - Task 1, Step 7: a deliberately broken run, then `docker ps -a --filter name=fleetops-e2e` must print nothing.
2. **A green e2e run that proves nothing.** For example, a stale server already on the port, or assertions that pass whatever the UI does. Pinned in:
   - Task 1: `reuseExistingServer: false`, so Playwright refuses to start when something already answers on 9123.
   - Task 1, Step 7: the sabotage run. The event-driven cache invalidation is switched off, and the test must fail at "the list follows changes made outside FleetOps" with `Received … running`.
3. **The socket group differs by platform** (Linux, Docker Desktop, Colima). The image must reach Docker with the documented `--group-add`, and must fail loudly without it. Pinned in:
   - Task 2: the smoke script reads the group from a container.
   - Task 2, Step 6: a run without `--group-add` must fail with "cannot reach Docker through the socket".
4. **An image without the web UI.** A context that leaves out `web/`, or a build that skips the Node stage, compiles fine with `build.rs`'s placeholder page. Pinned in:
   - Task 2: `image-smoke` requires `/` to reference `/assets/`, which the placeholder page does not.
5. **Cross-building from an amd64 host.** CI builds on amd64, and this plan was verified on an arm64 host. Pinned in:
   - Task 2, Step 5: both platforms build, the amd64 binary runs (`--version`), and both are static, stripped ELF files.
   - The first CI run of the `image` job is the proof for an amd64 build host. The Dockerfile has nothing architecture-specific beyond the `TARGETARCH` switch, and the `zig-*` wildcard matches both `zig-x86_64-linux-*` and `zig-aarch64-linux-*`.

---

## File Structure

```
.gitignore                         + web/e2e-data/, web/test-results/, web/playwright-report/
Makefile                           + e2e, image, image-smoke
Cargo.toml                         + [profile.release] strip = true
Dockerfile                         web → build (cargo-zigbuild) → distroless/static:nonroot
.dockerignore                      only Cargo.*, crates/ and web/ (no node_modules, dist, rust-toolchain.toml)
scripts/image-smoke.sh             runs an image against the local socket and checks UI, user and Docker access
.github/workflows/ci.yml           rust, docker-it, web, e2e and image jobs
README.md                          status, container image quick start, targets, e2e and CI notes, roadmap
web/
  package.json, pnpm-lock.yaml     + @playwright/test
  playwright.config.ts             one worker; webServer = the release binary with a fresh data dir
  e2e/core-flow.spec.ts            the spec's core flow
  vite.config.ts                   vitest excludes e2e/
  tsconfig.json                    typechecks e2e/ and playwright.config.ts
```

**Test commands.**
- `make e2e` builds, then runs the e2e test. It needs Docker.
- `make image`, then `make image-smoke`.
- The whole-repo gate is `make lint test test-web e2e image image-smoke`, plus `make test-docker` when the daemon may be used for integration tests.

---

### Task 1: The Playwright end-to-end test

**Files:**
- Create: `web/playwright.config.ts` and `web/e2e/core-flow.spec.ts`.
- Modify: `web/package.json` and `web/pnpm-lock.yaml` (via `pnpm add`), `web/vite.config.ts`, `web/tsconfig.json`, `Makefile` and `.gitignore`.
- Test: `web/e2e/core-flow.spec.ts`, run by `make e2e`.

**Interfaces:**
- Consumes (existing UI, found by role and name):
  - The setup page shows the text "Welcome to FleetOps", has Username, Password and Confirm password fields, and a "Create administrator" button.
  - The login page shows "Log in to FleetOps", with a "Log in" button.
  - The dashboard's `h1` is the environment name, `local`.
  - The user menu button is named after the username, and has a "Log out" menu item.
  - The `navigation` named "Environment" links to Images and Containers.
  - The "Pull image" dialog has an Image field and a "Pull" button. Its `status` reads `Done.` when the pull succeeds, and its footer has a "Close" button. Its corner × is also named "Close".
  - The "Create container" link leads to a form with Image, Name and Command fields and a "Create" button. It lands on `/envs/local/containers/<id>`, whose heading is the container's name.
  - The Logs tab has a `log` named "Container logs" and a "Search logs" field, whose summary reads `1 match`. Hits are wrapped in `<mark>`.
  - The Terminal tab has a "Connect" button, a status that says `Connected…`, and a "Disconnect" button.
  - The `table` named "Containers" has row checkboxes labelled `Select <id>`. The `group` named "Selected" has a "Remove" button, which opens the dialog `Remove <name>?`.
- Produces:
  - `make e2e`, which needs `DOCKER_HOST` or a docker context.
  - `web/playwright.config.ts`, which reads `FLEETOPS_BIN`, `FLEETOPS_E2E_PORT` and `CI`.
  - Playwright writes `web/test-results/` (traces kept on failure) and, on CI, `web/playwright-report/`.

- [ ] **Step 1: Show that the target is missing**

Run: `make e2e`
Expected: FAIL: `make: *** No rule to make target 'e2e'.  Stop.`

- [ ] **Step 2: Add Playwright and its browser**

Run:
```bash
pnpm -C web add -D @playwright/test@^1.63.0
pnpm -C web exec playwright install chromium
```
Expected:
- `web/package.json` gains `"@playwright/test": "^1.63.0"` in `devDependencies`, and `web/pnpm-lock.yaml` changes to match.
- Chromium (headless shell) downloads, or is already present.

- [ ] **Step 3: Write the Playwright config and the test**

`web/playwright.config.ts`:

```ts
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
```

`web/e2e/core-flow.spec.ts`:

```ts
import { execFileSync } from "node:child_process";
import { expect, test, type Page } from "@playwright/test";

// The spec's end-to-end flow (§8), against a real server and Docker daemon:
// setup → login → pull image → run container → live list update → logs
// search → exec → remove.

const IMAGE = "busybox:1.37";
const NAME = `fleetops-e2e-${Date.now().toString(36)}`;
const PASSWORD = "e2e-password-1";

/** Runs the docker CLI against the same daemon as the server (DOCKER_HOST). */
function docker(...args: string[]) {
  return execFileSync("docker", args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
}

test.afterAll(() => {
  try {
    docker("rm", "-f", NAME);
  } catch {
    // Already removed by the test.
  }
});

async function openContainers(page: Page) {
  await page.getByRole("navigation", { name: "Environment" }).getByRole("link", { name: "Containers" }).click();
  await expect(page.getByRole("heading", { name: "Containers" })).toBeVisible();
}

test("setup, log in, pull, run, watch, search logs, exec and remove", async ({ page }) => {
  await test.step("first-run setup creates the administrator", async () => {
    await page.goto("/");
    await expect(page.getByText("Welcome to FleetOps")).toBeVisible();
    await page.getByLabel("Username").fill("admin");
    await page.getByLabel("Password", { exact: true }).fill(PASSWORD);
    await page.getByLabel("Confirm password").fill(PASSWORD);
    await page.getByRole("button", { name: "Create administrator" }).click();
    await expect(page.getByRole("heading", { name: "local", level: 1 })).toBeVisible();
  });

  await test.step("log out and log back in", async () => {
    await page.getByRole("button", { name: "admin" }).click();
    await page.getByRole("menuitem", { name: "Log out" }).click();
    await expect(page.getByText("Log in to FleetOps")).toBeVisible();
    await page.getByLabel("Username").fill("admin");
    await page.getByLabel("Password").fill(PASSWORD);
    await page.getByRole("button", { name: "Log in" }).click();
    await expect(page.getByRole("heading", { name: "local", level: 1 })).toBeVisible();
  });

  await test.step("pull an image", async () => {
    await page.getByRole("navigation", { name: "Environment" }).getByRole("link", { name: "Images" }).click();
    await page.getByRole("button", { name: "Pull" }).click();
    const dialog = page.getByRole("dialog", { name: "Pull image" });
    await dialog.getByLabel("Image").fill(IMAGE);
    await dialog.getByRole("button", { name: "Pull" }).click();
    await expect(dialog.getByRole("status")).toHaveText("Done.", { timeout: 90_000 });
    // The footer's Close button; the dialog's corner × is also named "Close".
    await dialog.getByRole("button", { name: "Close" }).first().click();
    await expect(page.getByRole("table", { name: "Images" }).getByRole("row").filter({ hasText: IMAGE })).toBeVisible();
  });

  let id = "";
  await test.step("create and start a container", async () => {
    await openContainers(page);
    await page.getByRole("link", { name: "Create container" }).click();
    await page.getByLabel("Image").fill(IMAGE);
    await page.getByLabel("Name").fill(NAME);
    await page.getByLabel("Command").fill(`sh -c 'i=0; while true; do echo "tick $i"; i=$((i+1)); sleep 1; done'`);
    await page.getByRole("button", { name: "Create", exact: true }).click();
    await expect(page.getByRole("heading", { name: NAME })).toBeVisible();
    id = page.url().split("/").pop()!;
  });

  await test.step("search the live log", async () => {
    await page.getByRole("tab", { name: "Logs" }).click();
    const log = page.getByRole("log", { name: "Container logs" });
    await expect(log).toContainText("tick 2");
    await page.getByLabel("Search logs").fill("tick 2");
    await expect(page.getByText("1 match")).toBeVisible();
    await expect(log.locator("mark", { hasText: "tick 2" })).toBeVisible();
  });

  await test.step("run a command in the terminal", async () => {
    await page.getByRole("tab", { name: "Terminal" }).click();
    await page.getByRole("button", { name: "Connect", exact: true }).click();
    await expect(page.getByRole("status")).toHaveText(/Connected/);
    await page.locator(".xterm").click();
    await page.keyboard.type("echo hello-$((6*7))\n");
    await expect(page.locator(".xterm-rows")).toContainText("hello-42");
    await page.getByRole("button", { name: "Disconnect" }).click();
  });

  await test.step("the list follows changes made outside FleetOps", async () => {
    await openContainers(page);
    const row = page.getByRole("table", { name: "Containers" }).getByRole("row").filter({ hasText: NAME });
    await expect(row).toContainText("running");
    docker("stop", "--time", "1", NAME);
    await expect(row).toContainText("exited");
  });

  await test.step("remove the container", async () => {
    const table = page.getByRole("table", { name: "Containers" });
    await table.getByLabel(`Select ${id}`).check();
    await page.getByRole("group", { name: "Selected" }).getByRole("button", { name: "Remove" }).click();
    const dialog = page.getByRole("dialog", { name: `Remove ${NAME}?` });
    await dialog.getByRole("button", { name: "Remove" }).click();
    await expect(table.getByRole("row").filter({ hasText: NAME })).toHaveCount(0);
  });
});
```

- [ ] **Step 4: Watch vitest pick up the Playwright file**

Run: `pnpm -C web test`
Expected: FAIL:
```
 FAIL  e2e/core-flow.spec.ts [ e2e/core-flow.spec.ts ]
Error: Playwright Test did not expect test.afterAll() to be called here.
 Test Files  1 failed | 40 passed (41)
```
vitest's default `include` matches `*.spec.ts`.

- [ ] **Step 5: Keep vitest out of `e2e/`, typecheck the new files, add the target, and ignore the output**

`web/vite.config.ts`. Importing `configDefaults` from `vitest/config` also brings in vitest's config types, so the triple-slash reference goes; ESLint rejects both together.

```diff
@@ -1,8 +1,8 @@
-/// <reference types="vitest/config" />
 import path from "node:path";
 import react from "@vitejs/plugin-react";
 import tailwindcss from "@tailwindcss/vite";
 import { defineConfig } from "vite";
+import { configDefaults } from "vitest/config";
 
 // The dev server proxies the API (and its websocket) to `make dev-server`.
 // The Host header is left as the browser sent it, so the server's websocket
@@ -21,6 +21,8 @@ export default defineConfig({
     environment: "jsdom",
     globals: true,
     setupFiles: ["src/test/setup.ts"],
+    // e2e/ holds the Playwright tests (`make e2e`).
+    exclude: [...configDefaults.exclude, "e2e/**"],
     restoreMocks: true,
   },
 });
```

`web/tsconfig.json`:

```diff
@@ -16,5 +16,5 @@
     "types": ["vite/client", "vitest/globals", "@testing-library/jest-dom"],
     "paths": { "@/*": ["./src/*"] }
   },
-  "include": ["src", "vite.config.ts", "openapi-ts.config.ts"]
+  "include": ["src", "e2e", "vite.config.ts", "playwright.config.ts", "openapi-ts.config.ts"]
 }
```

`Makefile`:

```diff
@@ -1,4 +1,4 @@
-.PHONY: dev dev-server dev-web web test test-docker test-web lint fmt build openapi
+.PHONY: dev dev-server dev-web web test test-docker test-web e2e lint fmt build openapi
 
 # The API on :9000 and the Vite dev server on :5173, which proxies /api to it.
 dev:
@@ -29,6 +29,15 @@ test-docker:
 	DOCKER_HOST=$$(docker context inspect --format '{{.Endpoints.docker.Host}}') \
 		sh -c 'cargo test -p fleetops-docker --features docker-it --test docker_it && cargo test -p fleetops-server --features docker-it --test live'
 
+# The Playwright end-to-end test: a release build with the UI embedded,
+# driven in Chromium against the current docker context's daemon. It pulls
+# busybox and creates (then removes) one container named fleetops-e2e-*.
+e2e: build
+	pnpm -C web exec playwright install chromium
+	DOCKER_HOST=$${DOCKER_HOST:-$$(docker context inspect --format '{{.Endpoints.docker.Host}}')} \
+		FLEETOPS_BIN=$(abspath $(or $(CARGO_TARGET_DIR),target))/release/fleetops \
+		pnpm -C web exec playwright test
+
 lint:
 	cargo fmt --all --check
 	cargo clippy --workspace --all-targets -- -D warnings
```

`.gitignore`. The file had no trailing newline, so make sure `.impeccable/` stays on its own line:

```diff
@@ -7,4 +7,7 @@ web/dist/
 .dev-data/
 .playwright-mcp/
 .claude/
-.impeccable/
\ No newline at end of file
+.impeccable/
+web/e2e-data/
+web/test-results/
+web/playwright-report/
```

- [ ] **Step 6: Run the unit suite and the e2e test**

Run: `make test-web`
Expected: PASS: `Test Files  40 passed (40)` and `Tests  271 passed (271)`, and typecheck and lint are clean. typecheck now also covers `e2e/` and `playwright.config.ts`.

Run: `make e2e`
Expected: PASS. The release build runs first, then:
```
[WebServer] … registered local Docker environment url=…
[WebServer] … fleetops listening (http) addr=127.0.0.1:9123
Running 1 test using 1 worker
  ✓  1 [chromium] › e2e/core-flow.spec.ts:30:1 › setup, log in, pull, run, watch, search logs, exec and remove (…s)
  1 passed
```
It takes about 10–12 s once busybox is local. Then run `docker ps -a --filter name=fleetops-e2e -q`, which should print nothing.

Run it twice more (`make e2e` again, twice).
Expected: `1 passed` both times. Every run starts from an empty `web/e2e-data`.

- [ ] **Step 7: Prove the test catches a regression, and cleans up after one**

Switch off the event-driven cache invalidation in `web/src/live/events.ts`:
```bash
grep -n 'void this.qc.invalidateQueries({ queryKey });' web/src/live/events.ts   # one line, around 84
sed -i.bak 's|void this.qc.invalidateQueries({ queryKey });|void queryKey; // sabotage|' web/src/live/events.ts
make e2e
```
Expected: FAIL at the live-update step:
```
  1) … › the list follows changes made outside FleetOps
    Error: expect(locator).toContainText(expected) failed
    Expected substring: "exited"
    Received string:    "fleetops-e2e-…busybox:1.37running…"
```
Then restore the file and check that nothing is left behind:
```bash
mv web/src/live/events.ts.bak web/src/live/events.ts
git status --short web/src          # prints nothing
docker ps -a --filter name=fleetops-e2e -q   # prints nothing: afterAll removed it
make build                          # rebuild without the sabotage
```

- [ ] **Step 8: Commit**

```bash
git add .gitignore Makefile web/package.json web/pnpm-lock.yaml web/tsconfig.json web/vite.config.ts web/playwright.config.ts web/e2e
git commit -m "test(e2e): Playwright core flow against a release build and a real Docker daemon"
```

---

### Task 2: The container image and its smoke test

**Files:**
- Create: `Dockerfile`, `.dockerignore` and `scripts/image-smoke.sh` (executable).
- Modify: `Cargo.toml` and `Makefile`.
- Test: `make image` and `make image-smoke`.

**Interfaces:**
- Consumes:
  - `fleetops --version` / `--help` (clap), with `FLEETOPS_DATA` defaulting to `/data` and `FLEETOPS_BIND` to `0.0.0.0:9000`.
  - `GET /api/v1/health` → `{"status":"ok"}`.
  - `POST /api/v1/auth/setup` (JSON `{username, password}`, with an `Origin` header), which sets the session cookie.
  - `GET /api/v1/envs/local/info` → Docker's info, including `"ServerVersion"`.
  - `build.rs`'s placeholder `index.html`, which has no `/assets/` reference, when `web/dist` is missing.
- Produces:
  - `make image`: builds `$(IMAGE)` (default `fleetops:dev`) for `$(PLATFORMS)` (default `linux/amd64,linux/arm64`) and loads it.
  - `make image-smoke`: runs `scripts/image-smoke.sh $(IMAGE)`.
  - The image has `ENTRYPOINT ["/fleetops"]`, `USER nonroot` (65532), `VOLUME /data` owned by 65532, and `EXPOSE 9000`.
  - `scripts/image-smoke.sh [image]` exits 0 and prints `image-smoke: <image> ok (<url>)`. Otherwise it exits 1 with a reason and the container's logs.

- [ ] **Step 1: Show that the targets are missing**

Run: `make image`
Expected: FAIL: `make: *** No rule to make target 'image'.  Stop.`

- [ ] **Step 2: Write the Dockerfile and its ignore file**

`Dockerfile`:

```dockerfile
# syntax=docker/dockerfile:1
#
# The FleetOps image: one static musl binary, with the web UI embedded, on
# distroless/static, running as a non-root user. Build it with `make image`.
#
# Every build stage runs on the build machine's own platform. The binary is
# cross-compiled for each target platform with cargo-zigbuild, so a
# multi-arch build needs no emulation.

ARG RUST_VERSION=1.98
ARG NODE_VERSION=24
ARG ZIGBUILD_VERSION=0.23.4

# The web UI, which the server binary embeds at compile time.
FROM --platform=$BUILDPLATFORM node:${NODE_VERSION}-trixie-slim AS web
WORKDIR /src/web
RUN corepack enable
COPY web/package.json web/pnpm-lock.yaml ./
RUN --mount=type=cache,target=/root/.local/share/pnpm/store \
    pnpm install --frozen-lockfile
COPY web/ ./
RUN pnpm build

# zig and cargo-zigbuild, taken from the cargo-zigbuild project's image.
FROM --platform=$BUILDPLATFORM ghcr.io/rust-cross/cargo-zigbuild:${ZIGBUILD_VERSION} AS zigbuild

FROM --platform=$BUILDPLATFORM rust:${RUST_VERSION}-slim-trixie AS build
COPY --from=zigbuild /usr/local/cargo/bin/cargo-zigbuild /usr/local/cargo/bin/
COPY --from=zigbuild /usr/local/zig-* /usr/local/zig/
ENV PATH=/usr/local/zig:$PATH
RUN rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY --from=web /src/web/dist web/dist
ARG TARGETARCH
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target,id=fleetops-target-${TARGETARCH} \
    case "$TARGETARCH" in \
      amd64) target=x86_64-unknown-linux-musl ;; \
      arm64) target=aarch64-unknown-linux-musl ;; \
      *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac && \
    cargo zigbuild --locked --release -p fleetops-server --target "$target" && \
    cp "target/$target/release/fleetops" /fleetops
# The data directory, owned by distroless's nonroot user (65532).
RUN mkdir -p /data

FROM gcr.io/distroless/static-debian13:nonroot
COPY --from=build /fleetops /fleetops
COPY --from=build --chown=65532:65532 /data /data
VOLUME /data
EXPOSE 9000
USER nonroot
ENTRYPOINT ["/fleetops"]
```

`.dockerignore`:

```
# Only the sources the image build reads.
*
!Cargo.toml
!Cargo.lock
!crates/
!web/
web/node_modules/
web/dist/
web/test-results/
web/playwright-report/
# rust-toolchain.toml is left out on purpose: it names the `stable` channel,
# and inside the build stage that would make rustup download a second
# toolchain instead of using the image's pinned one.
```

- [ ] **Step 3: Strip release binaries**

`Cargo.toml`:

```diff
@@ -45,3 +45,7 @@ tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt"] }
 utoipa = "5.5"
 utoipa-axum = "0.2"
 uuid = { version = "1", features = ["v4"] }
+
+# Release binaries ship without debug symbols (the image is about 20 MB).
+[profile.release]
+strip = true
```

- [ ] **Step 4: Write the smoke test and the targets**

`scripts/image-smoke.sh` (then `chmod +x scripts/image-smoke.sh`):

```sh
#!/bin/sh
# Smoke-tests a FleetOps image against the local Docker daemon: it starts
# the image with the daemon's socket, then checks that it serves the web
# UI, that it runs as a non-root user, and that the admin it sets up can
# reach Docker through the socket.
#
# Usage: scripts/image-smoke.sh [image]    (default fleetops:dev)
set -eu

image=${1:-fleetops:dev}
name=fleetops-smoke-$$
jar=$(mktemp)
trap 'docker rm -f "$name" >/dev/null 2>&1 || true; rm -f "$jar"' EXIT

fail() {
  echo "image-smoke: $*" >&2
  docker logs "$name" >&2 2>&1 || true
  exit 1
}

# The group that owns the socket inside the daemon's machine. Read it from a
# container, because on Docker Desktop and Colima the host path differs.
gid=$(docker run --rm -v /var/run/docker.sock:/docker.sock busybox:1.37 stat -c %g /docker.sock)

docker run -d --name "$name" -p 127.0.0.1::9000 \
  -v /var/run/docker.sock:/var/run/docker.sock --group-add "$gid" "$image" >/dev/null
base="http://$(docker port "$name" 9000/tcp | head -n1)"

i=0
until curl -fsS "$base/api/v1/health" >/dev/null 2>&1; do
  i=$((i + 1))
  [ "$i" -lt 30 ] || fail "no answer from $base/api/v1/health"
  sleep 1
done

[ "$(docker inspect -f '{{.Config.User}}' "$name")" = nonroot ] || fail "the image does not run as nonroot"
curl -fsS "$base/" | grep -q '/assets/' || fail "/ does not serve the built web UI"

curl -fsS -c "$jar" -H 'content-type: application/json' -H "Origin: $base" \
  -d '{"username":"admin","password":"smoke-password-1"}' "$base/api/v1/auth/setup" >/dev/null ||
  fail "first-run setup failed"
curl -fsS -b "$jar" "$base/api/v1/envs/local/info" | grep -q '"ServerVersion"' ||
  fail "the local environment cannot reach Docker through the socket"

echo "image-smoke: $image ok ($base)"
```

`Makefile`:

```diff
@@ -1,4 +1,4 @@
-.PHONY: dev dev-server dev-web web test test-docker test-web e2e lint fmt build openapi
+.PHONY: dev dev-server dev-web web test test-docker test-web e2e lint fmt build image image-smoke openapi
 
 # The API on :9000 and the Vite dev server on :5173, which proxies /api to it.
 dev:
@@ -58,3 +58,19 @@ build: web
 openapi:
 	cargo run -q -p fleetops-server -- --openapi > crates/fleetops-api/openapi.json
 	pnpm -C web gen:api
+
+# The container image: a static musl binary on distroless/static, for
+# linux/amd64 and linux/arm64, built entirely inside BuildKit (no local zig
+# or cross toolchain). It is loaded into the local image store as $(IMAGE);
+# a multi-platform load needs Docker's containerd image store. Build one
+# platform with `make image PLATFORMS=linux/amd64`.
+IMAGE ?= fleetops:dev
+PLATFORMS ?= linux/amd64,linux/arm64
+
+image:
+	docker buildx build --platform $(PLATFORMS) --tag $(IMAGE) --load .
+
+# Runs $(IMAGE) against the local daemon's socket and checks that it
+# serves the UI and reaches Docker as its non-root user.
+image-smoke:
+	scripts/image-smoke.sh $(IMAGE)
```

Run: `docker run --rm -v "$PWD":/mnt koalaman/shellcheck:stable scripts/image-smoke.sh; echo $?`
Expected: `0`, with no findings.

- [ ] **Step 5: Build both platforms and check the binaries**

Run: `make image`
Expected: PASS. Both `cargo zigbuild` steps print `Finished \`release\` profile [optimized]`. The first build takes about 3 minutes on 4 CPUs, and later builds reuse the cache mounts. The export ends with `naming to docker.io/library/fleetops:dev done`.

Run: `docker image ls fleetops:dev --tree`
Expected: an index with a `linux/amd64` and a `linux/arm64` child, each about 8 MB of content.

Run:
```bash
docker run --rm --platform linux/amd64 fleetops:dev --version
docker run --rm --platform linux/arm64 fleetops:dev --version
for p in amd64 arm64; do c=$(docker create --platform linux/$p fleetops:dev); docker cp -q $c:/fleetops /tmp/fleetops-$p; docker rm $c >/dev/null; done
file /tmp/fleetops-amd64 /tmp/fleetops-arm64; ls -l /tmp/fleetops-amd64 /tmp/fleetops-arm64; rm /tmp/fleetops-amd64 /tmp/fleetops-arm64
```
Expected:
- `fleetops 0.1.0`, twice. The foreign platform runs under the daemon's emulation.
- Both files report `ELF 64-bit LSB executable, … statically linked, stripped`.
- Sizes are about 15–17 MB: 16.9 MB for amd64 and 15.3 MB for arm64 when this plan was written.

- [ ] **Step 6: Smoke-test the image, including the failure it guards against**

Run: `make image-smoke`
Expected: PASS: `image-smoke: fleetops:dev ok (http://127.0.0.1:<port>)`. Then `docker ps -a --filter name=fleetops-smoke -q` prints nothing.

Then run it without the socket group, to see the check fail:
```bash
sed 's/ --group-add "\$gid"//' scripts/image-smoke.sh > /tmp/smoke-nogroup.sh
sh /tmp/smoke-nogroup.sh fleetops:dev; echo "exit $?"; rm /tmp/smoke-nogroup.sh
```
Expected: FAIL, with `image-smoke: the local environment cannot reach Docker through the socket`. The container's log follows, including `no reachable Docker endpoint; add an environment manually … (Connect)`, and then `exit 1`.

- [ ] **Step 7: Commit**

```bash
git add Dockerfile .dockerignore scripts/image-smoke.sh Cargo.toml Makefile
git commit -m "build: multi-arch distroless image with cargo-zigbuild, and an image smoke test"
```

---

### Task 3: CI, and the README

**Files:**
- Create: `.github/workflows/ci.yml`.
- Modify: `README.md`.
- Test: actionlint on the workflow, and the whole-repo gate.

**Interfaces:**
- Consumes these `make` targets: `lint`, `test`, `test-docker`, `test-web` (which includes `check:api`), `e2e` and `image-smoke`, all from Tasks 1 and 2.
- Produces:
  - The workflow `CI`, with the jobs `rust`, `docker-it`, `web`, `e2e` and `image`.
  - It runs on pushes to `main` and on pull requests, with `permissions: contents: read`, and cancels superseded runs per ref.
  - On failure, the e2e job uploads the artifact `playwright-report` (the report and traces).

- [ ] **Step 1: Write the workflow**

`.github/workflows/ci.yml`:

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

permissions:
  contents: read

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always

jobs:
  rust:
    name: Rust (fmt, clippy, test)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      # rustup reads rust-toolchain.toml (stable, with rustfmt and clippy).
      - uses: Swatinem/rust-cache@v2
      - run: make lint
      - run: make test

  docker-it:
    name: Docker integration tests
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: Swatinem/rust-cache@v2
      - run: make test-docker

  web:
    name: Web (typecheck, lint, test, API client drift)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: pnpm/action-setup@v6
        with:
          package_json_file: web/package.json
      - uses: actions/setup-node@v7
        with:
          node-version: 24
          cache: pnpm
          cache-dependency-path: web/pnpm-lock.yaml
      - run: make test-web

  e2e:
    name: End-to-end (Playwright)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: Swatinem/rust-cache@v2
      - uses: pnpm/action-setup@v6
        with:
          package_json_file: web/package.json
      - uses: actions/setup-node@v7
        with:
          node-version: 24
          cache: pnpm
          cache-dependency-path: web/pnpm-lock.yaml
      - run: pnpm -C web install --frozen-lockfile
      - run: pnpm -C web exec playwright install --with-deps chromium
      - run: make e2e
      - if: failure()
        uses: actions/upload-artifact@v7
        with:
          name: playwright-report
          path: |
            web/playwright-report/
            web/test-results/
          retention-days: 7

  image:
    name: Container image (amd64 and arm64)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      - uses: docker/setup-buildx-action@v4
      # Both platforms, to prove they build. The result stays in the cache.
      - uses: docker/build-push-action@v7
        with:
          context: .
          platforms: linux/amd64,linux/arm64
          cache-from: type=gha
          cache-to: type=gha,mode=max
      # The runner's own platform again (a cache hit), loaded to smoke-test.
      - uses: docker/build-push-action@v7
        with:
          context: .
          platforms: linux/amd64
          load: true
          tags: fleetops:ci
          cache-from: type=gha
      - run: make image-smoke IMAGE=fleetops:ci
```

- [ ] **Step 2: Lint it**

Run: `docker run --rm -v "$PWD":/repo --workdir /repo rhysd/actionlint:latest -color; echo $?`
Expected: `0`, with no findings. actionlint also checks each action's inputs, for example `package_json_file` and `cache-dependency-path`.

- [ ] **Step 3: Update the README**

`README.md`:

````diff
@@ -9,7 +9,7 @@ What sets it apart:
 - **Live UI.** Lists and detail pages update from Docker events, with no polling or manual refresh.
 - **Built for Compose and GitOps.** Stack authoring and git sync are planned (sub-project 3).
 
-> **Status: early development.** Sub-project 1 (core and local Docker) is in progress. The backend and the web UI are complete: every resource page, the admin pages, the log viewer, the terminal and live stats. Still to come: end-to-end tests, the container image and CI (P3d).
+> **Status: early development.** Sub-project 1 (core and local Docker) is complete: the backend, the web UI, end-to-end tests, the container image and CI. Next is sub-project 2, the agent for managing more than one host.
 
 ## Contents
 
@@ -90,7 +90,22 @@ On first start, FleetOps registers a `local` environment when it can reach the s
 
 > Access to the Docker socket is equivalent to root access on the host. Don't expose FleetOps to untrusted networks without TLS, and create the admin account right away.
 
-A container image (`make image`, a multi-arch distroless image) is planned for P3d.
+### Run the container image
+
+`make image` builds `fleetops:dev` for linux/amd64 and linux/arm64: a static binary of about 17 MB on `distroless/static`, running as the non-root user `nonroot` (uid 65532). The build runs entirely inside Docker BuildKit, so it needs no Rust or Node toolchain on the host. Loading both platforms needs Docker's containerd image store; otherwise build one with `make image PLATFORMS=linux/amd64`.
+
+```sh
+make image
+docker run -d --name fleetops -p 9000:9000 \
+  -v fleetops-data:/data \
+  -v /var/run/docker.sock:/var/run/docker.sock \
+  --group-add "$(stat -c %g /var/run/docker.sock)" \
+  fleetops:dev
+```
+
+The container reaches the socket through the group that owns it, which `--group-add` grants. That group is root-equivalent on the host, like the socket itself. On Docker Desktop and Colima the socket is inside a VM, so read the group from a container instead: `docker run --rm -v /var/run/docker.sock:/s busybox stat -c %g /s`.
+
+`make image-smoke` starts the image against the local daemon and checks that it serves the UI and can reach Docker.
 
 ## Configuration
 
@@ -123,10 +138,13 @@ Open <http://localhost:5173>. `make dev-server` keeps its data in `./.dev-data`,
 | `make test` | `cargo test --workspace`, which uses mocks and needs no Docker |
 | `make test-docker` | Integration tests against the current docker context's daemon (`docker-it` feature) |
 | `make test-web` | Web typecheck, lint, vitest, and a check that the generated API client matches the OpenAPI document |
+| `make e2e` | `make build`, then the Playwright end-to-end test in Chromium against the current docker context's daemon |
 | `make lint` | `cargo fmt --check` and `clippy -D warnings` |
 | `make fmt` | `cargo fmt --all` |
 | `make web` | Builds the UI into `web/dist` |
 | `make build` | `make web`, then a release build of the `fleetops` binary |
+| `make image` | Builds the `fleetops:dev` container image for linux/amd64 and linux/arm64 (`IMAGE` and `PLATFORMS` override) |
+| `make image-smoke` | Runs `$(IMAGE)` against the local daemon and checks the UI, the non-root user and Docker access |
 | `make openapi` | Regenerates `crates/fleetops-api/openapi.json` and the TypeScript client after an API change |
 
 ### Notes
@@ -134,6 +152,8 @@ Open <http://localhost:5173>. `make dev-server` keeps its data in `./.dev-data`,
 - **Building without the UI.** `cargo build` works without `web/dist`. The server's `build.rs` writes a placeholder page, and the API still works under `/api/v1`.
 - **After changing the API,** run `make openapi` and commit both generated files. `cargo test` and `make test-web` fail until the committed document and client match the code.
 - **The integration tests** (`make test-docker`) create and remove their own labelled resources. Run them against a daemon where that's acceptable.
+- **The end-to-end test** (`make e2e`) starts the server on `127.0.0.1:9123` with an empty data directory (`web/e2e-data`). It pulls `busybox:1.37`, and creates and then removes one container named `fleetops-e2e-*`. It leaves the image in place. On failure, `pnpm -C web exec playwright show-trace` opens the trace saved under `web/test-results`.
+- **CI** (`.github/workflows/ci.yml`) runs on pushes to `main` and on pull requests: `make lint test`, `make test-docker`, `make test-web`, `make e2e`, and the image build for both platforms followed by `make image-smoke`.
 - **TDD.** New behaviour starts with a failing test.
 
 ### Design documents
@@ -141,7 +161,7 @@ Open <http://localhost:5173>. `make dev-server` keeps its data in `./.dev-data`,
 Design documents live in `docs/superpowers/`:
 
 - `specs/2026-09-29-fleetops-core-docker-design.md` is the sub-project 1 design and the authority for behaviour.
-- `plans/` holds the implementation plans, one per phase (P1, P2a–c, P3a–c so far).
+- `plans/` holds the implementation plans, one per phase (P1, P2a–c, P3a–d).
 
 ## Architecture
 
@@ -196,12 +216,13 @@ The full protocol is in section 5.2 of the spec.
 - **Headers.** Responses send a strict CSP, `frame-ancestors 'none'`, `X-Content-Type-Options` and `Referrer-Policy`. They also send HSTS when you enable it.
 - **Secret masking is display-only.** The API returns values unredacted, and anyone with access can reveal them.
 - **No telemetry.** FleetOps does not phone home.
+- **Container image.** It runs as a non-root user on `distroless/static`, with no shell. Docker access comes only from the socket group you grant with `--group-add`.
 
 ## Roadmap
 
 | # | Sub-project | Summary |
 |---|---|---|
-| **1** | **Core + local Docker** (in progress) | Server, embedded UI, auth, environments, full Docker resource management, real-time channel |
+| **1** | **Core + local Docker** (done) | Server, embedded UI, auth, environments, full Docker resource management, real-time channel |
 | 2 | Agent + multi-host | Rust agent that dials out to the server over a multiplexed websocket tunnel |
 | 3 | Stacks + GitOps | Compose authoring, diff before deploy, rollback, git-backed stacks with webhook or poll sync |
 | 4 | Kubernetes | Clusters through kubeconfig or an in-cluster agent: workloads, services, ingress, config and secrets, logs and exec, Helm |
@@ -217,5 +238,4 @@ Progress on sub-project 1:
   - P3a, web foundation
   - P3b, resource and admin pages
   - P3c, logs, terminal and stats
-- **Remaining:**
   - P3d, Playwright e2e, container image and CI
````

Check the README's `docker run` as written, with a throwaway name, port and volume:
```bash
G=$(docker run --rm -v /var/run/docker.sock:/s busybox stat -c %g /s)
docker run -d --name fleetops-readme -p 127.0.0.1:9125:9000 -v fleetops-readme-data:/data \
  -v /var/run/docker.sock:/var/run/docker.sock --group-add "$G" fleetops:dev
sleep 2; curl -s 127.0.0.1:9125/api/v1/auth/status; echo
docker run --rm -v fleetops-readme-data:/d busybox ls -ln /d
docker rm -f fleetops-readme; docker volume rm fleetops-readme-data
```
Expected:
- `{"setup_required":true,"setup_open":true}`.
- `fleetops.db`, `-shm` and `-wal`, owned by `65532 65532`, so a new named volume takes the image's ownership of `/data`.

If `busybox:latest` was not present before, remove it again: `docker rmi busybox:latest`.

- [ ] **Step 4: Run every gate**

Run: `make lint && make test`
Expected: PASS: fmt and clippy are clean, and `cargo test` reports 240 passed, 0 failed.

Run: `make test-web`
Expected: PASS: `Tests  271 passed (271)`.

Run: `make e2e && make image && make image-smoke`
Expected: `1 passed`, then `image-smoke: fleetops:dev ok (…)`.

When the daemon may be used for integration tests, also run `make test-docker`, which should PASS.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/ci.yml README.md
git commit -m "ci: GitHub Actions for lint, tests, docker-it, web, e2e and the image; README for P3d"
```

The workflow first runs for real when the branch is pushed. Pushing is the user's decision, not part of this plan. The first run is the check on Review Focus item 5.
