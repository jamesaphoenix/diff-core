import { test, expect, type Page } from "@playwright/test";

async function waitForDemoApp(page: Page) {
  await expect(page.locator(".summary")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator(".group-item.selected")).toBeVisible({ timeout: 10_000 });
}

test("repository field and branch selection survive a page reload", async ({ page }) => {
  await page.goto("/");
  await waitForDemoApp(page);

  await page.locator(".repo-input").fill("/work/other-repo");
  await expect(page.locator("[data-testid='base-branch-dropdown'] .branch-name")).toHaveText("main");

  await page.locator("[data-testid='base-branch-dropdown'] .branch-dropdown-trigger").click();
  await page.locator("[data-testid='base-branch-dropdown'] .branch-option-name", { hasText: "develop" }).click();
  await expect(page.locator("[data-testid='base-branch-dropdown'] .branch-name")).toHaveText("develop");
  await page.locator("[data-testid='head-branch-dropdown'] .branch-dropdown-trigger").click();
  await page.locator("[data-testid='head-branch-dropdown'] .branch-option-name", { hasText: "fix/login-bug" }).click();
  await expect(page.locator("[data-testid='head-branch-dropdown'] .branch-name")).toHaveText("fix/login-bug");

  await page.reload();
  await waitForDemoApp(page);

  await expect(page.locator(".repo-input")).toHaveValue("/work/other-repo");
  await expect(page.locator("[data-testid='base-branch-dropdown'] .branch-name")).toHaveText("develop");
  await expect(page.locator("[data-testid='head-branch-dropdown'] .branch-name")).toHaveText("fix/login-bug");

  await page.locator(".repo-input").fill("/work/third-repo");
  await expect(page.locator("[data-testid='base-branch-dropdown'] .branch-name")).toHaveText("main");
  await expect(page.locator("[data-testid='head-branch-dropdown'] .branch-name")).toHaveText("feature/user-auth");
});

test("auto-detected branches are re-detected after a reload", async ({ page }) => {
  await page.goto("/");
  await waitForDemoApp(page);
  await expect(page.locator("[data-testid='head-branch-dropdown'] .branch-name")).toHaveText("feature/user-auth");

  await page.evaluate(() => {
    (window as { __TEST_API__: { setHeadRef: (ref: string) => void } }).__TEST_API__.setHeadRef("release/v2.0");
  });
  await expect(page.locator("[data-testid='head-branch-dropdown'] .branch-name")).toHaveText("release/v2.0");

  await page.reload();
  await waitForDemoApp(page);
  await expect(page.locator("[data-testid='head-branch-dropdown'] .branch-name")).toHaveText("feature/user-auth");
});
