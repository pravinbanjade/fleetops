import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { expect, request as pwRequest, test } from "@playwright/test";
import { startInstance, type Instance } from "../../../web/e2e/instance";

// Scheduled backups to a local directory, then a one-click restore from
// the recent list. A server of its own: the restore replaces its data.

const PASSWORD = "e2e-password-1";
const PASSPHRASE = "correct horse battery";
const LICENSE = fs.readFileSync(path.join(import.meta.dirname, "fixtures/valid.lic"), "utf8");
let srv: Instance;
let out: string;

test.beforeAll(async () => {
  srv = await startInstance(9133);
  out = fs.mkdtempSync(path.join(os.tmpdir(), "fleetops-e2e-backups-"));
});

test.afterAll(() => {
  srv?.stop();
  fs.rmSync(out, { recursive: true, force: true });
});

test("an admin backs up to a directory and restores from the recent list", async ({ page }) => {
  const api = await pwRequest.newContext({ baseURL: srv.url });
  const setup = await api.post("/api/v1/auth/setup", { data: { username: "admin", password: PASSWORD } });
  const headers = { "x-csrf-token": (await setup.json()).csrf_token as string };
  expect((await api.put("/api/v1/ee/license", { headers: { ...headers, "content-type": "text/plain" }, data: LICENSE.trim() })).ok()).toBe(true);
  await api.dispose();

  await page.goto(`${srv.url}/login`);
  await page.getByLabel("Username").fill("admin");
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Log in" }).click();
  await expect(page).not.toHaveURL(/\/login/);
  await page.goto(`${srv.url}/admin/backup`);

  await test.step("set the passphrase and a local directory", async () => {
    await page.getByRole("button", { name: "Set passphrase" }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("Passphrase", { exact: true }).fill(PASSPHRASE);
    await dialog.getByLabel("Passphrase again").fill(PASSPHRASE);
    await dialog.getByRole("button", { name: "Save passphrase" }).click();
    await page.getByLabel("Directory").fill(out);
    await page.getByRole("group", { name: "Local directory" }).getByRole("button", { name: "Save" }).click();
    await expect(page.getByRole("group", { name: "Local directory" }).getByRole("button", { name: "Test" })).toBeVisible();
  });

  await test.step("run now; it shows under recent backups", async () => {
    await page.getByRole("button", { name: "Run now" }).click();
    await expect(page.getByText("success").first()).toBeVisible({ timeout: 30_000 });
    await page.reload();
    await expect(page.getByRole("heading", { name: "Recent backups" })).toBeVisible();
    expect(fs.readdirSync(out).some((n) => n.startsWith("fleetops-backup-"))).toBe(true);
  });

  await test.step("restore it and sign in again", async () => {
    const section = page.locator("section", { has: page.getByRole("heading", { name: "Recent backups" }) });
    await section.getByRole("button", { name: "Restore" }).first().click();
    await page.getByLabel("Type RESTORE to confirm").fill("RESTORE");
    await page.getByRole("button", { name: "Restore", exact: true }).click();
    await expect(page.getByText("Log in to FleetOps")).toBeVisible({ timeout: 60_000 });
    await page.getByLabel("Username").fill("admin");
    await page.getByLabel("Password").fill(PASSWORD);
    await page.getByRole("button", { name: "Log in" }).click();
    await expect(page).not.toHaveURL(/\/login/);
  });
});
