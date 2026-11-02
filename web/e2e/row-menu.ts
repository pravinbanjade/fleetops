import type { Page } from "@playwright/test";

/** Opens the ⋮ menu of the row labelled `row` and picks `item` (DataTable row menus). */
export async function pickRowMenu(page: Page, row: string, item: string) {
  await page.getByRole("button", { name: `Actions for ${row}`, exact: true }).click();
  await page.getByRole("menu").getByRole("menuitem", { name: item }).click();
}
