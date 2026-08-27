/**
 * UI Improvements — Playwright E2E tests.
 *
 * Covers:
 * - Comments tab in the right panel (comments moved out of the old strip below the diff)
 * - Comment cards: text, type badge, delete, file grouping, persistence
 * - Edges annotation subtab (replaced the old collapsible edges section)
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

async function openCommentsTab(page: Page) {
  await page.getByTestId("comments-tab").click();
}

// ── Comments Tab Tests ──

test.describe("Comments Tab", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("01 — comments tab shows empty state when no comments exist", async ({ page }) => {
    await openCommentsTab(page);
    await expect(page.locator(".comments-tab-empty")).toBeVisible();
  });

  test("02 — comment appears in comments tab after adding a file-level comment", async ({ page }) => {
    await addCommentViaUI(page, "Needs refactoring");

    await openCommentsTab(page);
    await expect(page.locator(".comments-tab-card")).toHaveCount(1);
  });

  test("03 — comments tab shows multiple comments", async ({ page }) => {
    await addCommentViaUI(page, "First comment");
    await addCommentViaUI(page, "Second comment");

    await expect(page.getByTestId("comments-tab").locator(".panel-tab-count")).toHaveText("2");

    await openCommentsTab(page);
    await expect(page.locator(".comments-tab-card")).toHaveCount(2);
  });

  test("04 — comment card shows comment text", async ({ page }) => {
    await addCommentViaUI(page, "Check error handling here");

    await openCommentsTab(page);
    const card = page.locator(".comments-tab-card").first();
    await expect(card.locator(".comments-tab-card-text")).toContainText("Check error handling here");
  });

  test("05 — comment card shows type badge", async ({ page }) => {
    await addCommentViaUI(page, "File level note");

    await openCommentsTab(page);
    const badge = page.locator(".comments-tab-card .comment-strip-badge").first();
    await expect(badge).toBeVisible();
    // Should be "file" type since file is selected
    await expect(badge).toContainText("file");
  });

  test("06 — deleting comment from comments tab removes it", async ({ page }) => {
    await addCommentViaUI(page, "Delete me");

    await openCommentsTab(page);
    await expect(page.locator(".comments-tab-card")).toHaveCount(1);

    // Click delete button
    await page.locator(".comment-strip-delete").first().click();
    await page.waitForTimeout(300);

    // Card should be gone, empty state back
    await expect(page.locator(".comments-tab-empty")).toBeVisible();
  });

  test("07 — comments are grouped under their file path", async ({ page }) => {
    await addCommentViaUI(page, "Comment on first file");

    await openCommentsTab(page);
    const header = page.locator(".comments-tab-file-header").first();
    await expect(header).toBeVisible();
    const pathText = await header.locator(".comments-tab-file-path").textContent();
    expect(pathText!.length).toBeGreaterThan(0);
    await expect(header.locator(".comments-tab-file-count")).toHaveText("1");
  });

  test("08 — comments persist when navigating between files", async ({ page }) => {
    await addCommentViaUI(page, "Persistent comment");

    await openCommentsTab(page);
    await expect(page.locator(".comments-tab-card")).toHaveCount(1);

    // Navigate away and back
    await page.keyboard.press("j");
    await page.waitForTimeout(500);
    await page.keyboard.press("k");
    await page.waitForTimeout(500);

    await expect(page.locator(".comments-tab-card")).toHaveCount(1);
    await expect(page.locator(".comments-tab-card-text")).toContainText("Persistent comment");
  });
});

// ── Edges Subtab Tests ──

test.describe("Edges Subtab", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("09 — edge list is hidden by default (Info subtab active)", async ({ page }) => {
    const infoTab = page.locator(".annotation-subtab", { hasText: "Info" });
    await expect(infoTab).toHaveClass(/active/);
    await expect(page.locator(".annotation-subtab", { hasText: "Edges" })).toBeVisible();
    await expect(page.locator(".edges-section")).not.toBeVisible();
  });

  test("10 — clicking edges subtab shows the edge list", async ({ page }) => {
    const edgesTab = page.locator(".annotation-subtab", { hasText: "Edges" });
    await edgesTab.click();
    await page.waitForTimeout(300);

    await expect(page.locator(".edge-list")).toBeVisible();
    const items = page.locator(".edge-item");
    expect(await items.count()).toBeGreaterThan(0);
  });

  test("11 — switching back to Info subtab hides the edge list", async ({ page }) => {
    const edgesTab = page.locator(".annotation-subtab", { hasText: "Edges" });

    // Show edges
    await edgesTab.click();
    await page.waitForTimeout(300);
    await expect(page.locator(".edge-list")).toBeVisible();

    // Back to Info
    await page.locator(".annotation-subtab", { hasText: "Info" }).click();
    await page.waitForTimeout(300);
    await expect(page.locator(".edges-section")).not.toBeVisible();
  });

  test("12 — edges subtab shows edge count", async ({ page }) => {
    const count = page.locator(".annotation-subtab", { hasText: "Edges" }).locator(".annotation-subtab-count");
    await expect(count).toBeVisible();
    expect(await count.textContent()).toMatch(/^\d+$/);
  });
});

// ── Export Buttons Removed ──

test.describe("Export Buttons Removed", () => {
  test("13 — PNG/SVG export buttons are not present", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Mount the graph first so the absence assertions are non-vacuous
    await page.locator(".annotation-subtab", { hasText: "Graph" }).click();
    await expect(page.locator("[data-testid='flow-graph']")).toBeVisible();

    await expect(page.locator(".flow-export-buttons")).not.toBeVisible();
    await expect(page.locator(".flow-export-btn")).not.toBeVisible();
  });
});

// ── MiniMap ──

test.describe("MiniMap", () => {
  test("14 — MiniMap hidden for small graphs (< 15 nodes)", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Mount the graph first so the absence assertion is non-vacuous
    await page.locator(".annotation-subtab", { hasText: "Graph" }).click();
    await expect(page.locator("[data-testid='flow-graph']")).toBeVisible();

    // The demo data has ~4-6 nodes per group, well under 15
    await expect(page.locator(".react-flow__minimap")).not.toBeVisible();
  });
});
