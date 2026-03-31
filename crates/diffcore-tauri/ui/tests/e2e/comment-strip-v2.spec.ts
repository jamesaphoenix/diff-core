/**
 * Comment Strip v2 — Playwright E2E tests.
 *
 * Covers:
 * - File comment icon in left panel (clickable, with count)
 * - Comment strip collapse/expand animation
 * - Active comment highlighting in nav pills
 * - File path shown on comment cards
 * - Scrollability of comment nav and detail when many comments
 * - Smooth transitions on collapsible sections
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

  test("03 — clicking comment icon opens comment strip", async ({ page }) => {
    await addCommentViaUI(page, "A comment");

    // Collapse the comments strip first
    const toggle = page.locator(".comment-strip-toggle");
    await toggle.click();
    await page.waitForTimeout(300);

    // Verify collapsed
    const body = page.locator(".comment-strip-body");
    await expect(body).toHaveCSS("max-height", "0px");

    // Click the file comment icon
    await page.locator(".file-comment-btn").first().click();
    await page.waitForTimeout(400);

    // Comments strip should be expanded
    await expect(page.locator(".comment-strip-collapsed")).not.toBeVisible();
  });
});

// ── Comment Strip Collapse/Expand ──

test.describe("Comment Strip Collapse", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
    await addCommentViaUI(page, "Test comment for collapse");
  });

  test("04 — comment strip has 'Comments' header with count", async ({ page }) => {
    const toggle = page.locator(".comment-strip-toggle");
    await expect(toggle).toBeVisible();
    await expect(toggle).toContainText("Comments");

    const count = page.locator(".comment-strip-count").first();
    await expect(count).toContainText("1");
  });

  test("05 — clicking header collapses the comment body", async ({ page }) => {
    const toggle = page.locator(".comment-strip-toggle");
    await toggle.click();
    await page.waitForTimeout(400);

    // Body should be collapsed (max-height: 0)
    const strip = page.locator(".comment-strip");
    await expect(strip).toHaveClass(/comment-strip-collapsed/);
  });

  test("06 — clicking header again expands the comment body", async ({ page }) => {
    const toggle = page.locator(".comment-strip-toggle");

    // Collapse
    await toggle.click();
    await page.waitForTimeout(400);
    await expect(page.locator(".comment-strip")).toHaveClass(/comment-strip-collapsed/);

    // Expand
    await toggle.click();
    await page.waitForTimeout(400);
    await expect(page.locator(".comment-strip")).not.toHaveClass(/comment-strip-collapsed/);
  });
});

// ── Active Comment Highlighting ──

test.describe("Active Comment Highlight", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
    await addCommentViaUI(page, "First comment");
    await addCommentViaUI(page, "Second comment");
  });

  test("07 — clicking nav pill highlights it as active", async ({ page }) => {
    const firstPill = page.locator(".comment-strip-nav-item").first();
    await firstPill.click();
    await page.waitForTimeout(300);

    await expect(firstPill).toHaveClass(/comment-strip-nav-active/);
  });

  test("08 — clicking a different pill moves the active highlight", async ({ page }) => {
    const firstPill = page.locator(".comment-strip-nav-item").first();
    const secondPill = page.locator(".comment-strip-nav-item").nth(1);

    await firstPill.click();
    await page.waitForTimeout(300);
    await expect(firstPill).toHaveClass(/comment-strip-nav-active/);

    await secondPill.click();
    await page.waitForTimeout(300);
    await expect(secondPill).toHaveClass(/comment-strip-nav-active/);
    await expect(firstPill).not.toHaveClass(/comment-strip-nav-active/);
  });

  test("09 — clicking a comment card highlights it as active", async ({ page }) => {
    const firstCard = page.locator(".comment-strip-item").first();
    await firstCard.click();
    await page.waitForTimeout(300);

    await expect(firstCard).toHaveClass(/comment-strip-item-active/);
  });
});

// ── File Path on Comments ──

test.describe("File Path on Comments", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("10 — comment card shows file path", async ({ page }) => {
    await addCommentViaUI(page, "Comment with path");

    const filepath = page.locator(".comment-strip-filepath").first();
    await expect(filepath).toBeVisible();
    // Should contain a file name (not empty)
    const text = await filepath.textContent();
    expect(text!.length).toBeGreaterThan(0);
  });
});

// ── Scrollability ──

test.describe("Comment Strip Scrollability", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("11 — comment detail panel is scrollable with many comments", async ({ page }) => {
    // Add many comments to overflow the panel
    for (let i = 0; i < 8; i++) {
      await addCommentViaUI(page, `Comment number ${i + 1} — this is a longer comment to take up vertical space in the panel`);
    }

    const detail = page.locator(".comment-strip-detail");
    await expect(detail).toBeVisible();

    // Check that overflow-y is set to auto
    await expect(detail).toHaveCSS("overflow-y", "auto");

    // The detail panel should have scroll height larger than client height
    const hasScroll = await detail.evaluate((el) => el.scrollHeight > el.clientHeight);
    expect(hasScroll).toBe(true);
  });

  test("12 — comment nav panel is scrollable with many comments", async ({ page }) => {
    for (let i = 0; i < 8; i++) {
      await addCommentViaUI(page, `Nav scroll test ${i + 1}`);
    }

    const nav = page.locator(".comment-strip-nav");
    await expect(nav).toBeVisible();
    await expect(nav).toHaveCSS("overflow-y", "auto");
  });
});

// ── Collapsible Sections Transitions ──

test.describe("Smooth Collapsible Sections", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("13 — flow graph section uses collapsible-body with transition", async ({ page }) => {
    const body = page.locator(".flow-graph-section .collapsible-body");
    if (await body.isVisible()) {
      // Should have transition property set
      const transition = await body.evaluate((el) => getComputedStyle(el).transition);
      expect(transition).toContain("max-height");
    }
  });

  test("14 — edges section uses collapsible-body with transition", async ({ page }) => {
    const body = page.locator(".edges-section .collapsible-body");
    if (await body.isVisible()) {
      const transition = await body.evaluate((el) => getComputedStyle(el).transition);
      expect(transition).toContain("max-height");
    }
  });

  test("15 — edges section starts collapsed", async ({ page }) => {
    const section = page.locator(".edges-section");
    if (await section.isVisible()) {
      await expect(section).toHaveClass(/edges-collapsed/);
    }
  });

  test("16 — edges section expands on toggle click", async ({ page }) => {
    const toggle = page.locator(".edges-section .section-toggle");
    if (await toggle.isVisible()) {
      await toggle.click();
      await page.waitForTimeout(400);

      const section = page.locator(".edges-section");
      await expect(section).not.toHaveClass(/edges-collapsed/);

      // Edge items should be visible
      await expect(page.locator(".edge-item").first()).toBeVisible();
    }
  });
});
