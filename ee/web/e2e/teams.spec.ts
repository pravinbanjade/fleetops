import { expect, test, type Browser, type Page } from "@playwright/test";

// Teams end to end. Runs after license.spec (files run in name order),
// which installs the test license and fills its two seats with "admin" and
// "e2e-one", so e2e-one is the member here.

const PASSWORD = "e2e-password-1";
const MEMBER = "e2e-one";
const NO_ACCESS = "You don't have access to any environments yet. Ask an admin.";

async function logIn(page: Page, username: string) {
  await page.goto("/login");
  await page.getByLabel("Username").fill(username);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Log in" }).click();
  await expect(page).not.toHaveURL(/\/login/);
}

/** What the member sees on a fresh login. */
async function asMember(browser: Browser, check: (page: Page) => Promise<void>) {
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  await logIn(page, MEMBER);
  await check(page);
  await ctx.close();
}

test("an admin gives a team access and a member sees the environment", async ({ page, browser }) => {
  await logIn(page, "admin");

  await test.step("the member starts with no environments", async () => {
    await asMember(browser, (p) => expect(p.getByText(NO_ACCESS)).toBeVisible());
  });

  await test.step("create team ops with Operator on local", async () => {
    await page.goto("/admin/teams");
    await page.getByRole("button", { name: "New team" }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("Name").fill("ops");
    await dialog.getByRole("button", { name: "Create" }).click();
    await expect(dialog).toBeHidden();
    const row = page.getByRole("row", { name: /ops/ });
    await row.getByRole("button", { name: "Access" }).click();
    await dialog.getByLabel("Role on local").selectOption("operator");
    await dialog.getByRole("button", { name: "Save" }).click();
    await expect(dialog).toBeHidden();
  });

  await test.step("add the member", async () => {
    await page.getByRole("row", { name: /ops/ }).getByRole("button", { name: "Members" }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("User to add").selectOption(MEMBER);
    await dialog.getByRole("button", { name: "Add" }).click();
    await expect(dialog.getByRole("button", { name: `Remove ${MEMBER}` })).toBeVisible();
    await page.keyboard.press("Escape");
  });

  await test.step("the member sees local, and the admin sees where it comes from", async () => {
    await asMember(browser, (p) => expect(p.getByRole("heading", { name: "local", level: 1 })).toBeVisible());
    await page.goto("/admin/users");
    await page.getByRole("row", { name: new RegExp(MEMBER) }).getByRole("button", { name: "Access" }).click();
    await expect(page.getByRole("region", { name: "Through teams" })).toContainText("ops");
    await page.keyboard.press("Escape");
  });

  await test.step("removing the member takes the access away", async () => {
    await page.goto("/admin/teams");
    await page.getByRole("row", { name: /ops/ }).getByRole("button", { name: "Members" }).click();
    await page.getByRole("dialog").getByRole("button", { name: `Remove ${MEMBER}` }).click();
    await expect(page.getByRole("dialog").getByText("No members yet.")).toBeVisible();
    await asMember(browser, (p) => expect(p.getByText(NO_ACCESS)).toBeVisible());
  });
});
