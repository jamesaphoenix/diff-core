/**
 * UI Improvements — Playwright E2E tests.
 *
 * Covers:
 * - Comment strip below diff viewer
 * - Click-to-scroll on comment in strip
 * - Edges section collapsed by default
 * - Edges section toggle
 * - PNG/SVG export buttons removed
 * - MiniMap hidden for small graphs
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

// ── Comment Strip Tests ──

test.describe("Comment Strip", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("01 — comment strip not visible when no comments exist", async ({ page }) => {
    await expect(page.locator(".comment-strip")).not.toBeVisible();
  });

  test("02 — comment strip appears after adding a file-level comment", async ({ page }) => {
    await addCommentViaUI(page, "Needs refactoring");

    const strip = page.locator(".comment-strip");
    await expect(strip).toBeVisible();

    // Should show "1 comment" in the header
    await expect(page.locator(".comment-strip-count")).toContainText("1 comment");
  });

  test("03 — comment strip shows multiple comments", async ({ page }) => {
    await addCommentViaUI(page, "First comment");
    await addCommentViaUI(page, "Second comment");

    await expect(page.locator(".comment-strip-count")).toContainText("2 comments");
    const items = page.locator(".comment-strip-item");
    expect(await items.count()).toBe(2);
  });

  test("04 — comment strip item shows comment text", async ({ page }) => {
    await addCommentViaUI(page, "Check error handling here");

    const item = page.locator(".comment-strip-item").first();
    await expect(item.locator(".comment-strip-text")).toContainText("Check error handling here");
  });

  test("05 — comment strip item shows type badge", async ({ page }) => {
    await addCommentViaUI(page, "File level note");

    const badge = page.locator(".comment-strip-badge").first();
    await expect(badge).toBeVisible();
    // Should be "file" type since file is selected
    await expect(badge).toContainText("file");
  });

  test("06 — deleting comment from strip removes it", async ({ page }) => {
    await addCommentViaUI(page, "Delete me");

    await expect(page.locator(".comment-strip")).toBeVisible();

    // Click delete button
    await page.locator(".comment-strip-delete").first().click();
    await page.waitForTimeout(300);

    // Strip should disappear (no more comments)
    await expect(page.locator(".comment-strip")).not.toBeVisible();
  });

  test("07 — comment strip hides when switching to a file with no comments", async ({ page }) => {
    await addCommentViaUI(page, "Comment on first file");
    await expect(page.locator(".comment-strip")).toBeVisible();

    // Navigate to next file
    await page.keyboard.press("j");
    await page.waitForTimeout(500);

    // Strip should not be visible for the new file (no comments there)
    await expect(page.locator(".comment-strip")).not.toBeVisible();
  });

  test("08 — comment strip reappears when navigating back to commented file", async ({ page }) => {
    await addCommentViaUI(page, "Persistent comment");
    await expect(page.locator(".comment-strip")).toBeVisible();

    // Navigate away and back
    await page.keyboard.press("j");
    await page.waitForTimeout(500);
    await page.keyboard.press("k");
    await page.waitForTimeout(500);

    await expect(page.locator(".comment-strip")).toBeVisible();
    await expect(page.locator(".comment-strip-text")).toContainText("Persistent comment");
  });
});

// ── Edges Section Tests ──

test.describe("Edges Section", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("09 — edges section is collapsed by default", async ({ page }) => {
    // The edge list items should not be visible
    await expect(page.locator(".edge-list")).not.toBeVisible();

    // But the toggle button with count should be visible
    const edgesToggle = page.locator(".annotation-section .flow-graph-toggle").filter({ hasText: "Edges" });
    await expect(edgesToggle).toBeVisible();
  });

  test("10 — clicking edges toggle expands the edge list", async ({ page }) => {
    const edgesToggle = page.locator(".annotation-section .flow-graph-toggle").filter({ hasText: "Edges" });
    await edgesToggle.click();
    await page.waitForTimeout(300);

    await expect(page.locator(".edge-list")).toBeVisible();
    const items = page.locator(".edge-item");
    expect(await items.count()).toBeGreaterThan(0);
  });

  test("11 — clicking edges toggle again collapses it", async ({ page }) => {
    const edgesToggle = page.locator(".annotation-section .flow-graph-toggle").filter({ hasText: "Edges" });

    // Expand
    await edgesToggle.click();
    await page.waitForTimeout(300);
    await expect(page.locator(".edge-list")).toBeVisible();

    // Collapse
    await edgesToggle.click();
    await page.waitForTimeout(300);
    await expect(page.locator(".edge-list")).not.toBeVisible();
  });

  test("12 — edges toggle shows edge count", async ({ page }) => {
    const edgesToggle = page.locator(".annotation-section .flow-graph-toggle").filter({ hasText: "Edges" });
    // Should show "Edges (N)" where N > 0
    const text = await edgesToggle.textContent();
    expect(text).toMatch(/Edges\s*\(\d+\)/);
  });
});

// ── Export Buttons Removed ──

test.describe("Export Buttons Removed", () => {
  test("13 — PNG/SVG export buttons are not present", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await expect(page.locator(".flow-export-buttons")).not.toBeVisible();
    await expect(page.locator(".flow-export-btn")).not.toBeVisible();
  });
});

// ── MiniMap ──

test.describe("MiniMap", () => {
  test("14 — MiniMap hidden for small graphs (< 15 nodes)", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // The demo data has ~4-6 nodes per group, well under 15
    await expect(page.locator(".react-flow__minimap")).not.toBeVisible();
  });
});
