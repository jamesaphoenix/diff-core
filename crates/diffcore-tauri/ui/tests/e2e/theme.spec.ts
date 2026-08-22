import { expect, test, type Page } from "@playwright/test";

async function openSettings(page: Page) {
  await page.locator(".btn-settings").click();
  await expect(page.locator(".settings-panel")).toBeVisible();
}

function bgPrimary(page: Page) {
  return page.evaluate(() =>
    getComputedStyle(document.documentElement).getPropertyValue("--bg-primary").trim(),
  );
}

test.describe("Appearance settings", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await expect(page.locator(".summary")).toBeVisible({ timeout: 10_000 });
  });

  test("01 — default theme stays Catppuccin Mocha", async ({ page }) => {
    await expect.poll(() => bgPrimary(page)).toBe("#1e1e2e");
  });

  test("02 — selecting a dark theme applies it and persists across reload", async ({ page }) => {
    await openSettings(page);
    const appearance = page.locator(".settings-section").filter({ hasText: "Appearance" });
    await appearance.locator("select").first().selectOption("dracula");
    await expect.poll(() => bgPrimary(page)).toBe("#282a36");

    await page.reload();
    await expect(page.locator(".summary")).toBeVisible({ timeout: 10_000 });
    await expect.poll(() => bgPrimary(page)).toBe("#282a36");
  });

  test("03 — light mode switches to the selected light theme", async ({ page }) => {
    await openSettings(page);
    await page.locator(".theme-mode-toggle button").filter({ hasText: "light" }).click();
    await expect.poll(() => bgPrimary(page)).toBe("#eff1f5");

    await page.locator(".theme-mode-toggle button").filter({ hasText: "dark" }).click();
    await expect.poll(() => bgPrimary(page)).toBe("#1e1e2e");
  });
});
