/**
 * Comments v2 — Playwright E2E tests.
 *
 * Covers:
 * - File comment icon in left panel (clickable, with count)
 * - Clicking the file comment icon opens the right-panel comments tab
 * - Active comment highlighting on comment cards
 * - File path shown on comment file-group headers
 * - Scrollability of the comments list with many comments
 */
import { test, expect, type Page } from "@playwright/test";

// ── Helpers ──

async function waitForAnalysis(page: Page) {
  await expect(page.locator(".summary")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator(".group-item.selected .file-list")).toBeVisible({ timeout: 5_000 });
  await expect(page.getByRole("code").first()).toBeVisible({ timeout: 10_000 });
  await page.waitForTimeout(2000);
}

async function addCommentViaUI(page: Page, text: string) {
  await page.keyboard.press("c");
  await page.waitForTimeout(300);
  const textarea = page.locator(".comment-textarea");
  await textarea.fill(text);
  await page.keyboard.press("Enter");
  await page.waitForTimeout(300);
}

async function openCommentsTab(page: Page) {
  await page.getByTestId("comments-tab").click();
  await page.waitForTimeout(300);
}

// ── File Comment Icon ──

test.describe("File Comment Icon", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("01 — comment icon appears on file with comments", async ({ page }) => {
    // No icon initially
    await expect(page.locator(".file-comment-btn")).not.toBeVisible();

    // Add a comment
    await addCommentViaUI(page, "Test comment");

    // Icon should appear on the selected file
    const btn = page.locator(".file-comment-btn").first();
    await expect(btn).toBeVisible();
  });

  test("02 — comment icon shows count", async ({ page }) => {
    await addCommentViaUI(page, "First");
    await addCommentViaUI(page, "Second");

    const count = page.locator(".file-comment-count").first();
    await expect(count).toContainText("2");
  });

  test("03 — clicking comment icon opens the comments tab", async ({ page }) => {
    await addCommentViaUI(page, "A comment");

    // Comments tab is not active by default (annotations is)
    await expect(page.getByTestId("comments-tab")).toHaveAttribute("aria-selected", "false");

    // Click the file comment icon
    await page.locator(".file-comment-btn").first().click();
    await page.waitForTimeout(400);

    // Right panel should switch to the comments tab and show the comment
    await expect(page.getByTestId("comments-tab")).toHaveAttribute("aria-selected", "true");
    await expect(page.locator(".comments-tab-card")).toHaveCount(1);
  });
});

// ── Active Comment Highlighting ──

test.describe("Active Comment Highlight", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
    await addCommentViaUI(page, "First comment");
    await addCommentViaUI(page, "Second comment");
    await openCommentsTab(page);
  });

  test("04 — clicking a comment card highlights it as active", async ({ page }) => {
    const firstCard = page.locator(".comments-tab-card").first();
    await firstCard.click();
    await page.waitForTimeout(300);

    await expect(firstCard).toHaveClass(/comments-tab-card-active/);
  });

  test("05 — clicking a different card moves the active highlight", async ({ page }) => {
    const firstCard = page.locator(".comments-tab-card").first();
    const secondCard = page.locator(".comments-tab-card").nth(1);

    await firstCard.click();
    await page.waitForTimeout(300);
    await expect(firstCard).toHaveClass(/comments-tab-card-active/);

    await secondCard.click();
    await page.waitForTimeout(300);
    await expect(secondCard).toHaveClass(/comments-tab-card-active/);
    await expect(firstCard).not.toHaveClass(/comments-tab-card-active/);
  });
});

// ── File Path on Comments ──

test.describe("File Path on Comments", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("06 — comment file group shows file path", async ({ page }) => {
    await addCommentViaUI(page, "Comment with path");
    await openCommentsTab(page);

    const filepath = page.locator(".comments-tab-file-path").first();
    await expect(filepath).toBeVisible();
    // Should contain a file name (not empty)
    const text = await filepath.textContent();
    expect(text!.length).toBeGreaterThan(0);
  });
});

// ── Scrollability ──

test.describe("Comments List Scrollability", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("07 — comments list is scrollable with many comments", async ({ page }) => {
    // Add many comments to overflow the panel
    for (let i = 0; i < 10; i++) {
      await addCommentViaUI(page, `Comment number ${i + 1} — this is a longer comment to take up vertical space in the panel`);
    }
    await openCommentsTab(page);

    const list = page.locator(".comments-tab-list");
    await expect(list).toBeVisible();

    // Check that overflow-y is set to auto
    await expect(list).toHaveCSS("overflow-y", "auto");

    // The list should have scroll height larger than client height
    const hasScroll = await list.evaluate((el) => el.scrollHeight > el.clientHeight);
    expect(hasScroll).toBe(true);
  });
});
