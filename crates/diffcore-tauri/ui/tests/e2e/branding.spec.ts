import { expect, test } from "@playwright/test";

test("shows Diffcore branding in the top bar", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator(".top-bar")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator(".logo")).toHaveText("Diffcore");
});
