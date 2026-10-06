import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { expect, test } from "@playwright/test";

// Sub-project 2 end to end: enroll a real fleetops-agent against the same
// daemon, browse a container's logs through it, then stop the agent. Runs
// after core-flow (files run in name order), which creates the admin.

const PASSWORD = "e2e-password-1";
const NAME = `fleetops-e2e-remote-${Date.now().toString(36)}`;

function docker(...args: string[]) {
  return execFileSync("docker", args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
}

test.beforeAll(() => {
  docker("run", "-d", "--name", NAME, "--label", "fleetops.e2e=1", "busybox:1.37", "sh", "-c", "echo remote-hello; sleep 300");
});

test.afterAll(() => {
  try {
    docker("rm", "-f", NAME);
  } catch {
    // Already gone.
  }
});

test("an agent environment comes online, serves logs, and goes offline", async ({ page, baseURL }) => {
  let agent: ChildProcess | undefined;
  await test.step("log in", async () => {
    await page.goto("/login");
    await page.getByLabel("Username").fill("admin");
    await page.getByLabel("Password").fill(PASSWORD);
    await page.getByRole("button", { name: "Log in" }).click();
    await expect(page.getByRole("heading", { name: "local", level: 1 })).toBeVisible();
  });

  let token = "";
  await test.step("add an agent environment and read its command", async () => {
    await page.goto("/admin/envs");
    await page.getByRole("button", { name: "Add environment" }).click();
    const form = page.getByRole("dialog", { name: "Add environment" });
    // The radio itself is visually hidden; click its label, as a user does.
    await form.getByText("Agent on a remote host", { exact: true }).click();
    await expect(form.getByRole("radio", { name: /agent on a remote host/i })).toBeChecked();
    await form.getByLabel("Name").fill("e2e-remote");
    await form.getByRole("button", { name: "Add" }).click();
    const shown = page.getByRole("dialog", { name: "Run this on the remote host" });
    token = /--join-token (fj_[0-9a-f]+)/.exec(await shown.locator("pre").innerText())![1];
    await shown.getByRole("button", { name: "Done" }).click();
  });

  try {
    await test.step("the agent connects and the environment comes up", async () => {
      agent = spawn(
        process.env.FLEETOPS_AGENT_BIN ?? "../target/release/fleetops-agent",
        ["--server", baseURL!, "--join-token", token, "--data", mkdtempSync(join(tmpdir(), "fleetops-agent-")),
          "--docker", process.env.DOCKER_HOST ?? "unix:///var/run/docker.sock", "--log", "warn"],
        { stdio: "inherit" },
      );
      const row = page.getByRole("table", { name: "Environments" }).getByRole("row").filter({ hasText: "e2e-remote" });
      await expect(row.getByText("up", { exact: true })).toBeAttached({ timeout: 15_000 });
    });

    await test.step("a container's logs come through the agent", async () => {
      await page.goto("/envs/e2e-remote/containers");
      await page.getByRole("table", { name: "Containers" }).getByRole("link", { name: NAME }).click();
      await page.getByRole("tab", { name: "Logs" }).click();
      await expect(page.getByRole("log", { name: "Container logs" })).toContainText("remote-hello");
    });
  } finally {
    agent?.kill("SIGTERM");
  }

  await test.step("stopping the agent takes the environment down", async () => {
    await page.goto("/admin/envs");
    const row = page.getByRole("table", { name: "Environments" }).getByRole("row").filter({ hasText: "e2e-remote" });
    await expect(row.getByText(/^: agent offline/)).toBeAttached({ timeout: 60_000 });
  });
});
