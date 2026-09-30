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
