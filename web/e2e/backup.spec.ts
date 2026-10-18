import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { expect, request as pwRequest, test } from "@playwright/test";
import { startInstance, type Instance } from "./instance";

// Moving a server: back up instance A, restore it on a fresh instance B
// from the first-run screen. Both are servers of their own, so the suite's
// shared server is untouched.

const PASSWORD = "e2e-password-1";
const PASSPHRASE = "correct horse battery";
let a: Instance;
let b: Instance;

test.beforeAll(async () => {
  [a, b] = await Promise.all([startInstance(9131), startInstance(9132)]);
});

test.afterAll(() => {
  a?.stop();
  b?.stop();
});

test("back up a server and restore it on a new one", async ({ page }) => {
  const file = path.join(os.tmpdir(), `fleetops-e2e-${Date.now()}.fob`);

  await test.step("instance A: an admin, a user and an environment, then a backup", async () => {
    const api = await pwRequest.newContext({ baseURL: a.url });
    const setup = await api.post("/api/v1/auth/setup", { data: { username: "admin", password: PASSWORD } });
    expect(setup.status()).toBe(201);
    const csrf = (await setup.json()).csrf_token as string;
    const headers = { "x-csrf-token": csrf };
    expect((await api.post("/api/v1/users", { headers, data: { username: "bob", password: "bob-password-1", role: "user" } })).status()).toBe(201);
    const res = await api.post("/api/v1/backup", { headers, data: { passphrase: PASSPHRASE } });
    expect(res.status()).toBe(200);
    fs.writeFileSync(file, await res.body());
    await api.dispose();
  });

  await test.step("instance B: restore from the first-run screen", async () => {
    await page.goto(`${b.url}/`);
    await expect(page.getByText("Welcome to FleetOps")).toBeVisible();
    await page.getByRole("button", { name: "Restore from backup" }).click();
    await page.getByLabel("Backup file").setInputFiles(file);
    await page.getByLabel("Passphrase").fill(PASSPHRASE);
    await page.getByRole("button", { name: "Open backup" }).click();
    await expect(page.getByText("Users")).toBeVisible();
    await page.getByRole("button", { name: "Restore", exact: true }).click();
    await expect(page.getByText("Log in to FleetOps")).toBeVisible({ timeout: 60_000 });
  });

  await test.step("A's admin signs in on B and finds A's users", async () => {
    await page.getByLabel("Username").fill("admin");
    await page.getByLabel("Password").fill(PASSWORD);
    await page.getByRole("button", { name: "Log in" }).click();
    await expect(page).not.toHaveURL(/\/login/);
    await page.goto(`${b.url}/admin/users`);
    await expect(page.getByRole("table", { name: "Users" }).getByText("bob")).toBeVisible();
  });

  fs.rmSync(file, { force: true });
});
