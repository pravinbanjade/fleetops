import { expect, test, type Page } from "@playwright/test";

// Sub-project 3c end to end: an admin grants a new user Viewer on the local
// environment from the Access dialog; that user sees it read-only. Runs
// after core-flow (files run in name order), which creates the admin.

const PASSWORD = "e2e-password-1";
const VIEWER = `viewer-${Date.now().toString(36)}`;

async function logIn(page: Page, username: string) {
  await page.goto("/login");
  await page.getByLabel("Username").fill(username);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Log in" }).click();
  await expect(page).not.toHaveURL(/\/login/);
}

test("a viewer sees only what they were granted, read-only", async ({ page, browser }) => {
  await test.step("an admin adds a user", async () => {
    await logIn(page, "admin");
    await page.goto("/admin/users");
    await page.getByRole("button", { name: "Add user" }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("Username").fill(VIEWER);
    await dialog.getByLabel("Password").fill(PASSWORD);
    await dialog.getByRole("button", { name: "Add" }).click();
    await expect(dialog).toBeHidden();
  });

  await test.step("a new user sees no environments", async () => {
    const ctx = await browser.newContext();
    const other = await ctx.newPage();
    await logIn(other, VIEWER);
    await expect(other.getByText("You don't have access to any environments yet. Ask an admin.")).toBeVisible();
    await ctx.close();
  });

  await test.step("the admin grants Viewer on local", async () => {
    const row = page.getByRole("row", { name: new RegExp(VIEWER) });
    await row.getByRole("button", { name: "Access" }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("Role on local").selectOption("viewer");
    await dialog.getByRole("button", { name: "Save" }).click();
    await expect(dialog).toBeHidden();
  });

  await test.step("the viewer browses local without write controls", async () => {
    const ctx = await browser.newContext();
    const other = await ctx.newPage();
    await logIn(other, VIEWER);
    await expect(other.getByRole("heading", { name: "local", level: 1 })).toBeVisible();
    await other.goto("/envs/local/containers");
    await expect(other.getByRole("heading", { name: "Containers" })).toBeVisible();
    await expect(other.getByRole("link", { name: /Create container/ })).toHaveCount(0);
    await expect(other.getByRole("button", { name: "Prune" })).toHaveCount(0);
    await ctx.close();
  });
});
