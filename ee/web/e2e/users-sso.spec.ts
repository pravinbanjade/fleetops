import { expect, test, type Page } from "@playwright/test";

// Single sign-on end to end, against mock-idp.mjs. Runs last (files run in
// name order): the license's two seats are taken by "admin" and "e2e-one",
// and team "ops" (Operator on local) exists from teams.spec.

const PASSWORD = "e2e-password-1";
const IDP = `http://127.0.0.1:${process.env.MOCK_IDP_PORT ?? 9124}`;

async function logIn(page: Page, username: string) {
  await page.goto("/login");
  const link = page.getByRole("button", { name: "Sign in with password" });
  if (await link.isVisible()) await link.click();
  await page.getByLabel("Username").fill(username);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Log in" }).click();
  await expect(page).not.toHaveURL(/\/login/);
}

test("an admin connects a provider and a new user signs in into team ops", async ({ page, browser }) => {
  await logIn(page, "admin");

  await test.step("add an OpenID Connect connection and map ops", async () => {
    await page.goto("/admin/sso");
    await page.getByRole("button", { name: "New connection" }).click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("Name").fill("Mock IdP");
    await dialog.getByLabel("Issuer URL").fill(IDP);
    await dialog.getByLabel("Client ID").fill("fleetops-e2e");
    await dialog.getByLabel("Client secret").fill("e2e-secret");
    await dialog.getByRole("button", { name: "Add" }).click();
    await expect(dialog).toBeHidden();
    const row = page.getByRole("row", { name: /Mock IdP/ });
    await row.getByRole("button", { name: "Mappings" }).click();
    await dialog.getByLabel("Group").fill("ops");
    await dialog.getByLabel("Gives").selectOption({ label: "Team ops" });
    await dialog.getByRole("button", { name: "Add" }).click();
    await expect(dialog.getByText("team ops")).toBeVisible();
    await page.keyboard.press("Escape");
    await row.getByRole("button", { name: "Test" }).click();
    await dialog.getByRole("button", { name: "Run test" }).click();
    await expect(dialog.getByRole("status")).toContainText("OK");
    await page.keyboard.press("Escape");
    await row.getByRole("button", { name: "Enable" }).click();
    await expect(row).toContainText("Enabled");
  });

  const signIn = async () => {
    const ctx = await browser.newContext();
    const p = await ctx.newPage();
    await p.goto("/login");
    await p.getByRole("link", { name: "Sign in with Mock IdP" }).click();
    return { ctx, p };
  };

  await test.step("the seat limit stops a new user", async () => {
    const { ctx, p } = await signIn();
    await expect(p).toHaveURL(/sso_error=seat_limit/);
    await expect(p.getByRole("alert")).toHaveText("FleetOps has reached its licensed user limit. Ask an admin.");
    await ctx.close();
  });

  await test.step("with a free seat they land with team ops's access", async () => {
    await page.goto("/admin/users");
    await page.getByRole("row", { name: /e2e-one/ }).getByRole("button", { name: "Delete" }).click();
    await page.getByRole("dialog").getByRole("button", { name: "Delete" }).click();
    await expect(page.getByRole("row", { name: /e2e-one/ })).toHaveCount(0);
    const { ctx, p } = await signIn();
    await expect(p.getByRole("heading", { name: "local", level: 1 })).toBeVisible();
    await ctx.close();
    await page.reload();
    await expect(page.getByRole("row", { name: /sso-jane/ })).toContainText("Signed in with Mock IdP");
  });

  await test.step("password login can be limited to local admins", async () => {
    await page.goto("/admin/sso");
    await page.getByRole("radio", { name: /Local admins only/ }).check();
    await expect(page.getByRole("radio", { name: /Local admins only/ })).toBeChecked();
    const ctx = await browser.newContext();
    const p = await ctx.newPage();
    await logIn(p, "admin");
    await ctx.close();
  });
});
