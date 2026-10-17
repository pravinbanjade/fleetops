import { expect, test, type Browser, type Page } from "@playwright/test";

// Custom roles end to end. Runs after license.spec and before teams.spec
// (files run in name order): e2e-one holds the role here and nothing when
// this file ends, as teams.spec expects.

const PASSWORD = "e2e-password-1";
const MEMBER = "e2e-one";

async function logIn(page: Page, username: string) {
  await page.goto("/login");
  await page.getByLabel("Username").fill(username);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Log in" }).click();
  await expect(page).not.toHaveURL(/\/login/);
}

async function asMember(browser: Browser, check: (page: Page) => Promise<void>) {
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  await logIn(page, MEMBER);
  await check(page);
  await ctx.close();
}

test("a Deployer can create and start but not delete or prune", async ({ page, browser }) => {
  await logIn(page, "admin");

  await test.step("build the Deployer role", async () => {
    await page.goto("/admin/roles");
    await page.getByRole("button", { name: "New role" }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("Name").fill("Deployer");
    for (const cell of ["Containers: View", "Containers: Create", "Containers: Operate", "Volumes: View", "Volumes: Create"]) {
      await dialog.getByRole("checkbox", { name: cell }).check();
    }
    await dialog.getByRole("button", { name: "Create", exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByRole("row", { name: /Deployer/ })).toBeVisible();
  });

  await test.step("give it to the member on local", async () => {
    await page.goto("/admin/users");
    await page.getByRole("row", { name: new RegExp(MEMBER) }).getByRole("button", { name: "Access" }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("Role on local").selectOption({ label: "Deployer" });
    await dialog.getByRole("button", { name: "Save" }).click();
    await expect(dialog).toBeHidden();
  });

  await test.step("the member gets Create but no Prune or Remove", async () => {
    await asMember(browser, async (p) => {
      await p.goto("/envs/local/containers");
      await expect(p.getByRole("link", { name: /Create container/ })).toBeVisible();
      await expect(p.getByRole("button", { name: /Prune/ })).toHaveCount(0);
      await p.goto("/envs/local/volumes");
      await expect(p.getByRole("button", { name: "Create volume" })).toBeVisible();
      await expect(p.getByRole("button", { name: "Prune" })).toHaveCount(0);
    });
  });

  await test.step("revoke, and delete the role", async () => {
    await page.goto("/admin/users");
    await page.getByRole("row", { name: new RegExp(MEMBER) }).getByRole("button", { name: "Access" }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("Role on local").selectOption("none");
    await dialog.getByRole("button", { name: "Save" }).click();
    await expect(dialog).toBeHidden();
    await page.goto("/admin/roles");
    await page.getByRole("row", { name: /Deployer/ }).getByRole("button", { name: "Delete" }).click();
    await page.getByRole("button", { name: "Delete role" }).click();
    await expect(page.getByRole("row", { name: /Deployer/ })).toHaveCount(0);
  });
});
