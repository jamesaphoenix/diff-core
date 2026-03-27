import { expect, test, type Page } from "@playwright/test";

async function waitForAnalysis(page: Page) {
  await expect(page.locator(".summary")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator(".group-item.selected .file-list")).toBeVisible({ timeout: 5_000 });
  await expect(page.getByRole("code").first()).toBeVisible({ timeout: 10_000 });
  await page.waitForTimeout(1200);
}

async function openSourceView(page: Page) {
  await page.getByRole("tab", { name: "Source" }).click();
  await expect(page.locator(".source-explorer")).toBeVisible();
}

test.describe("Source Explorer", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("01 — source view shows native symbol sections and Monaco editor", async ({ page }) => {
    await openSourceView(page);

    await expect(page.locator(".source-outline-section-header").filter({ hasText: "Operations" })).toBeVisible();
    await expect(page.locator(".source-outline-section-header").filter({ hasText: "Dependencies" })).toBeVisible();
    await expect(page.locator(".source-outline-item").filter({ hasText: "POST /api/users" })).toBeVisible();
    await expect(page.locator(".source-editor-surface .monaco-editor")).toBeVisible();
  });

  test("02 — clicking a file symbol updates the native editor context", async ({ page }) => {
    await page.locator(".file-item").filter({ hasText: "services/user-service.ts" }).click();
    await openSourceView(page);

    const symbol = page.locator(".source-outline-item").filter({ hasText: "UserService.create" });
    await symbol.click();

    await expect(page.locator(".source-editor-title")).toContainText("UserService.create");
    await expect(page.locator(".source-editor-subtitle")).toContainText("Fn");
    await expect(symbol).toHaveClass(/active/);
  });

  test("03 — interface-heavy files render interfaces and types natively", async ({ page }) => {
    await page.locator(".file-item").filter({ hasText: "models/user.ts" }).click();
    await openSourceView(page);

    const interfaceSection = page.locator(".source-outline-section").filter({ hasText: "Interfaces & Types" });
    await expect(interfaceSection).toBeVisible();
    await expect(page.locator(".source-outline-name").getByText(/^User$/)).toBeVisible();
    await expect(page.locator(".source-outline-name").getByText(/^CreateUserInput$/)).toBeVisible();
  });

  test("04 — dependency clicks navigate to linked files and symbols", async ({ page }) => {
    await page.locator(".file-item").filter({ hasText: "services/user-service.ts" }).click();
    await openSourceView(page);

    const dependency = page.locator(".source-outline-item").filter({ hasText: "UserRepository.insert" });
    await dependency.click();

    await expect(page.locator(".file-item.selected")).toContainText("repositories/user-repo.ts");
    await expect(page.locator(".source-explorer")).toBeVisible();
    await expect(page.locator(".source-editor-title")).toContainText("UserRepository.insert");
  });

  test("05 — source explorer surfaces stay on the dark theme while scrolling", async ({ page }) => {
    await page.locator(".file-item").filter({ hasText: "services/user-service.ts" }).click();
    await openSourceView(page);

    const surfaces = await page.locator(".source-outline, .source-editor-pane").evaluateAll((els) =>
      els.map((el) => getComputedStyle(el).backgroundColor),
    );
    for (const color of surfaces) {
      expect(color).not.toBe("rgb(255, 255, 255)");
    }

    await page.locator(".source-outline-sections").evaluate((el) => {
      el.scrollTop = el.scrollHeight;
    });

    await expect(page.locator(".source-outline-item").last()).toBeVisible();
  });

  test("06 — external editor launcher still exists alongside the native source view", async ({ page }) => {
    await openSourceView(page);

    await expect(page.locator(".open-with-btn")).toBeVisible();
    await page.locator(".open-with-arrow").click();
    await expect(page.locator(".open-with-dropdown")).toBeVisible();
    await expect(page.locator(".open-with-option")).toHaveCount(5);
  });
});
