import { expect, test } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";

const PASSWORD = "e2e-password-1";
const LICENSE = fs.readFileSync(path.join(import.meta.dirname, "fixtures/valid.lic"), "utf8");

test("an admin installs a license, exports the audit log and meets the seat limit", async ({ page, request }) => {
  await request.post("/api/v1/auth/setup", { data: { username: "admin", password: PASSWORD } });
  await page.goto("/login");
  await page.getByLabel("Username").fill("admin");
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Log in" }).click();
  await expect(page).not.toHaveURL(/\/login/);

  await test.step("without a license there is no Export", async () => {
    await page.goto("/admin/audit");
    await expect(page.getByRole("table", { name: "Audit log" })).toBeVisible();
    await expect(page.getByRole("button", { name: "Export" })).toHaveCount(0);
  });

  await test.step("install the license", async () => {
    await page.getByRole("link", { name: "License" }).click();
    await page.getByLabel("License key").fill(LICENSE.trim());
    await page.getByRole("button", { name: "Install license" }).click();
    await expect(page.getByText("E2E Corp")).toBeVisible();
    await expect(page.getByText("1 of 2")).toBeVisible();
  });

  await test.step("export the audit log as CSV", async () => {
    await page.goto("/admin/audit");
    await page.getByRole("button", { name: "Export" }).click();
    const download = page.waitForEvent("download");
    await page.getByRole("menuitem", { name: "CSV" }).click();
    const csv = fs.readFileSync(await (await download).path(), "utf8");
    expect(csv.startsWith("id,ts,user_id,username,environment_id,action")).toBe(true);
    expect(csv).toContain("license.update");
  });

  await test.step("the seat limit stops the third user", async () => {
    await page.goto("/admin/users");
    for (const [name, ok] of [
      ["e2e-one", true],
      ["e2e-two", false],
    ] as const) {
      await page.getByRole("button", { name: "Add user" }).click();
      const dialog = page.getByRole("dialog");
      await dialog.getByLabel("Username").fill(name);
      await dialog.getByLabel("Password").fill(PASSWORD);
      await dialog.getByRole("button", { name: "Add" }).click();
      if (ok) await expect(dialog).toBeHidden();
      else await expect(page.getByText(/allows 2 users/)).toBeVisible();
    }
  });
});
