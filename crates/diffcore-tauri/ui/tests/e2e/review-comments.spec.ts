/**
 * Review Comments — Playwright E2E tests.
 *
 * Tests the review comment system across three scopes:
 * - Group-level comments (c key with no file selected)
 * - File-level comments (c key with file selected)
 * - Comment display in right panel and left panel badges
 * - Copy all comments (C key)
 * - Comment deletion
 * - Context menu "Add Comment" option
 * - Keyboard hints
 */
import { test, expect, type Page } from "@playwright/test";

// ── Helpers ──

/** Wait for the demo data to load and the first group to be auto-selected. */
async function waitForAnalysis(page: Page) {
  await expect(page.locator(".summary")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator(".group-item.selected .file-list")).toBeVisible({ timeout: 5_000 });
  await expect(page.getByRole("code").first()).toBeVisible({ timeout: 10_000 });
  await page.waitForTimeout(2000);
}

/** Add a comment via the UI (c key → type → Enter). */
async function addCommentViaUI(page: Page, text: string) {
  await page.keyboard.press("c");
  await page.waitForTimeout(300);
  const textarea = page.locator(".comment-textarea");
  await textarea.fill(text);
  await page.keyboard.press("Enter");
  await page.waitForTimeout(300);
}

// ── Tests ──

test.describe("Review Comments", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("01 — c key opens comment input overlay (file-level when file selected)", async ({ page }) => {
    // A file is auto-selected, so c should open file-level comment
    await page.keyboard.press("c");
    await page.waitForTimeout(300);

    // Comment overlay should be visible
    const overlay = page.locator(".comment-overlay");
    await expect(overlay).toBeVisible();

    // Should show the comment input panel
    const panel = page.locator(".comment-input-panel");
    await expect(panel).toBeVisible();

    // Textarea should be focused
    const textarea = page.locator(".comment-textarea");
    await expect(textarea).toBeVisible();
    await expect(textarea).toBeFocused();

    // Scope label should show file path
    const scope = page.locator(".comment-input-scope");
    await expect(scope).toBeVisible();
  });

  test("02 — Escape cancels comment input", async ({ page }) => {
    await page.keyboard.press("c");
    await page.waitForTimeout(300);
    await expect(page.locator(".comment-overlay")).toBeVisible();

    // Press Escape to cancel
    await page.keyboard.press("Escape");
    await page.waitForTimeout(200);

    // Overlay should be gone
    await expect(page.locator(".comment-overlay")).not.toBeVisible();
  });

  test("03 — typing and submitting a comment via Enter", async ({ page }) => {
    await addCommentViaUI(page, "This needs more validation");

    // Overlay should close
    await expect(page.locator(".comment-overlay")).not.toBeVisible();

    // Toast should show "Comment saved"
    await expect(page.locator(".toast")).toContainText("Comment saved");

    const commentStripItem = page.locator(".comment-strip-item").first();
    await expect(commentStripItem).toBeVisible();
    await expect(commentStripItem.locator(".comment-strip-text")).toContainText("This needs more validation");
  });

  test("04 — comment count badge shows on group with comments", async ({ page }) => {
    await addCommentViaUI(page, "Test comment for badge");

    // Comment count badge should show "1" on the group
    const badge = page.locator(".group-item.selected .comment-count-badge");
    await expect(badge).toBeVisible();
    await expect(badge).toHaveText("1");
  });

  test("05 — file comment icon shows on files with comments", async ({ page }) => {
    await addCommentViaUI(page, "File-level comment");

    // File should show a comment icon
    const commentIcon = page.locator(".file-item .file-comment-icon");
    await expect(commentIcon.first()).toBeVisible();
  });

  test("06 — multiple comments displayed in right panel", async ({ page }) => {
    // Add first comment (file-level, since file is selected)
    await addCommentViaUI(page, "First comment");

    // Add second comment (still file-level)
    await addCommentViaUI(page, "Second comment");

    const commentStripItems = page.locator(".comment-strip-item");
    await expect(commentStripItems).toHaveCount(2);

    const firstItem = commentStripItems.nth(0);
    await expect(firstItem.locator(".comment-strip-badge")).toHaveText("file");
    await expect(firstItem.locator(".comment-strip-text")).toContainText("First comment");

    const secondItem = commentStripItems.nth(1);
    await expect(secondItem.locator(".comment-strip-text")).toContainText("Second comment");
  });

  test("07 — delete comment via X button", async ({ page }) => {
    await addCommentViaUI(page, "To be deleted");
    await expect(page.locator(".comment-strip-item")).toHaveCount(1);

    // Click delete button
    await page.locator(".comment-strip-delete").click();
    await page.waitForTimeout(300);

    // Comment should be gone
    await expect(page.locator(".comment-strip-item")).toHaveCount(0);
  });

  test("08 — copy comments button copies all comments to clipboard", async ({ page, context }) => {
    // Grant clipboard permissions upfront
    await context.grantPermissions(["clipboard-read", "clipboard-write"]);

    await addCommentViaUI(page, "Copy me");

    // Wait for the "Comment saved" toast to auto-dismiss (2s timer)
    await page.waitForTimeout(2500);

    // Click the copy comments button in the left panel header
    const btn = page.locator(".btn-copy-comments");
    await expect(btn).toBeVisible();
    await btn.click();

    // Toast should confirm copy
    await expect(page.locator(".toast")).toContainText("copied to clipboard", { timeout: 5_000 });
  });

  test("09 — copy comments button shows in left panel header when comments exist", async ({ page }) => {
    // No comments yet — button should not be visible
    await expect(page.locator(".btn-copy-comments")).not.toBeVisible();

    // Add a comment
    await addCommentViaUI(page, "Test");

    // Now button should be visible
    const btn = page.locator(".btn-copy-comments");
    await expect(btn).toBeVisible();
  });

  test("10 — right-click context menu has Add Comment option", async ({ page }) => {
    // Right-click on a file item
    const fileItem = page.locator(".file-item").first();
    await fileItem.click({ button: "right" });
    await page.waitForTimeout(200);

    // Context menu should show Add Comment
    const contextMenu = page.locator(".context-menu");
    await expect(contextMenu).toBeVisible();
    const addCommentBtn = contextMenu.locator(".context-menu-item", { hasText: "Add Comment" });
    await expect(addCommentBtn).toBeVisible();

    // Click Add Comment
    await addCommentBtn.click();
    await page.waitForTimeout(300);

    // Comment input overlay should open
    await expect(page.locator(".comment-overlay")).toBeVisible();
  });

  test("11 — keyboard hints show c and C shortcuts", async ({ page }) => {
    const hints = page.locator(".keyboard-hints");
    await expect(hints).toContainText("c");
    await expect(hints).toContainText("C");
    await expect(hints).toContainText("comment");
    await expect(hints).toContainText("copy comments");
  });

  test("12 — Save button disabled when comment text is empty", async ({ page }) => {
    await page.keyboard.press("c");
    await page.waitForTimeout(300);

    const saveBtn = page.locator(".btn-comment-save");
    await expect(saveBtn).toBeDisabled();

    // Type something
    const textarea = page.locator(".comment-textarea");
    await textarea.fill("Not empty");
    await expect(saveBtn).not.toBeDisabled();

    // Clear it
    await textarea.fill("");
    await expect(saveBtn).toBeDisabled();
  });

  test("13 — clicking comment overlay backdrop cancels", async ({ page }) => {
    await page.keyboard.press("c");
    await page.waitForTimeout(300);
    await expect(page.locator(".comment-overlay")).toBeVisible();

    // Click the overlay backdrop (not the panel)
    await page.locator(".comment-overlay").click({ position: { x: 10, y: 10 } });
    await page.waitForTimeout(200);

    await expect(page.locator(".comment-overlay")).not.toBeVisible();
  });

  test("14 — comments survive group switching", async ({ page }) => {
    // Remember the first group's name
    const firstGroupName = await page.locator(".group-detail-name").textContent();

    // Add a comment to the first group
    await addCommentViaUI(page, "Persistent comment");

    // Verify comment is there
    await expect(page.locator(".comment-strip-item")).toHaveCount(1);

    // Switch to next group (J = next group)
    await page.keyboard.press("J");
    await page.waitForTimeout(500);

    // Verify we actually switched groups (right panel shows different group name)
    const secondGroupName = await page.locator(".group-detail-name").textContent();
    expect(secondGroupName).not.toBe(firstGroupName);

    // Switch back to first group (K = previous group)
    await page.keyboard.press("K");
    await page.waitForTimeout(500);

    // Verify we're back on the first group
    await expect(page.locator(".group-detail-name")).toHaveText(firstGroupName!);

    // Comment should still be there
    await expect(page.locator(".comment-strip-item")).toHaveCount(1);
    await expect(page.locator(".comment-strip-text")).toContainText("Persistent comment");
  });
});
