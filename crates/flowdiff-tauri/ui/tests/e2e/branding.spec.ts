import { expect, test } from "@playwright/test";

test("shows Flowdiff branding in the top bar", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator(".top-bar")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator(".logo")).toHaveText("Flowdiff");
});
