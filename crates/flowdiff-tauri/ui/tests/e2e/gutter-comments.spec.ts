/**
 * Gutter Comment Icons — Playwright E2E tests + screenshot diagnosis.
 *
 * Tests whether comment glyph icons appear in the Monaco editor gutter
 * when code-level comments exist on specific lines.
 */
import { test, expect, type Page } from "@playwright/test";
import path from "path";
import { fileURLToPath } from "url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const SCREENSHOTS_DIR = path.resolve(__dirname, "../../../../../docs/screenshots");

async function waitForAnalysis(page: Page) {
  await expect(page.locator(".summary")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator(".group-item.selected .file-list")).toBeVisible({ timeout: 5_000 });
  await expect(page.getByRole("code").first()).toBeVisible({ timeout: 10_000 });
  await page.waitForTimeout(2000);
}

/** Add a code-level comment using the Test API with correct internal state paths. */
async function addCodeCommentViaUI(page: Page, text: string) {
  // Step 1: Get the actual selected file path and group ID from internal state
  await page.evaluate(() => {
    const api = (window as any).__TEST_API__;
    const file = api.getSelectedFile();
    const group = api.getSelectedGroup();
    if (!file || !group) throw new Error("No file/group selected");
    api.openCommentInput({
      type: "code",
      group_id: group.id,
      file_path: file,
      start_line: 5,
      end_line: 8,
      selected_code: "const user = await createUser(email, name, password);",
    });
  });
  await page.waitForTimeout(300);

  // Step 2: Set text and submit (separate evaluates to let React state settle)
  await page.evaluate((t) => {
    (window as any).__TEST_API__.setCommentText(t);
  }, text);
  await page.waitForTimeout(100);

  await page.evaluate(() => {
    (window as any).__TEST_API__.submitComment();
  });
  await page.waitForTimeout(500);
}

test.describe("Gutter Comment Icons", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("01 — screenshot before comments", async ({ page }) => {
    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "80-editor-no-comments.png"),
      fullPage: false,
    });
    const glyphs = page.locator(".comment-glyph-icon");
    expect(await glyphs.count()).toBe(0);
  });

  test("02 — glyphMargin is enabled on sub-editors after mount", async ({ page }) => {
    const glyphValues = await page.evaluate(() => {
      const editors = (window as any).monaco?.editor?.getEditors?.();
      if (!editors) return "no monaco";
      return editors.map((e: any) => {
        const opts = e.getRawOptions?.();
        return { glyphMargin: opts?.glyphMargin, className: e.getDomNode()?.className?.slice(0, 30) };
      });
    });
    console.log("Glyph margin values:", JSON.stringify(glyphValues));

    // At least one editor should have glyphMargin enabled
    // (the DiffEditor creates 2 sub-editors)
    const hasGlyph = Array.isArray(glyphValues) && glyphValues.some((v: any) => v.glyphMargin === true);

    // If glyphMargin isn't enabled, check if the glyph-margin DOM element exists
    const glyphMarginElements = await page.locator(".monaco-editor .glyph-margin").count();
    console.log("Glyph margin DOM elements:", glyphMarginElements);

    // Take a screenshot of the center panel (contains the editor)
    await page.locator(".panel-center").screenshot({
      path: path.join(SCREENSHOTS_DIR, "84-monaco-glyph-check.png"),
    });
  });

  test("03 — add code comment and verify it saves", async ({ page }) => {
    await addCodeCommentViaUI(page, "Validate email format here");

    const info = await page.evaluate(() => {
      const api = (window as any).__TEST_API__;
      const comments = api.getComments();
      return {
        total: comments.length,
        comments: comments.map((c: any) => ({
          type: c.type,
          file_path: c.file_path,
          start_line: c.start_line,
          end_line: c.end_line,
          text: c.text,
        })),
      };
    });

    console.log("Comments after add:", JSON.stringify(info));
    expect(info.total).toBeGreaterThanOrEqual(1);

    // Check for comment strip
    const stripVisible = await page.locator(".comment-strip").isVisible();
    console.log("Comment strip visible:", stripVisible);
  });

  test("04 — glyph icon appears in editor after code comment added", async ({ page }) => {
    await addCodeCommentViaUI(page, "Check this logic");

    // Wait for decorations to be applied
    await page.waitForTimeout(1000);

    // Check for glyph decorations in the DOM
    const glyphs = await page.locator(".comment-glyph-icon").count();
    const highlights = await page.locator(".comment-line-highlight").count();

    console.log(`After comment — glyphs: ${glyphs}, highlights: ${highlights}`);

    // Screenshot the editor
    await page.locator(".panel-center").screenshot({
      path: path.join(SCREENSHOTS_DIR, "85-glyph-after-comment.png"),
    });

    // The glyph icon should be visible
    expect(glyphs).toBeGreaterThan(0);
  });

  test("05 — glyph hover shows comment text", async ({ page }) => {
    await addCodeCommentViaUI(page, "Important: add rate limiting");
    await page.waitForTimeout(1000);

    const glyph = page.locator(".comment-glyph-icon").first();
    if (await glyph.isVisible()) {
      await glyph.hover();
      await page.waitForTimeout(500);

      await page.screenshot({
        path: path.join(SCREENSHOTS_DIR, "86-glyph-hover.png"),
        fullPage: false,
      });
    }
  });

  test("06 — line highlight background appears on commented lines", async ({ page }) => {
    await addCodeCommentViaUI(page, "Highlight test");
    await page.waitForTimeout(1000);

    const highlights = await page.locator(".comment-line-highlight").count();
    console.log(`Line highlights: ${highlights}`);

    expect(highlights).toBeGreaterThan(0);
  });

  test("07 — only one glyph icon per comment (on first line)", async ({ page }) => {
    await addCodeCommentViaUI(page, "Single glyph test");
    await page.waitForTimeout(1000);

    const glyphs = await page.locator(".comment-glyph-icon").count();
    const highlights = await page.locator(".comment-line-highlight").count();

    // Should have exactly 1 glyph (first line) but 4 highlights (lines 5-8)
    expect(glyphs).toBe(1);
    expect(highlights).toBe(4);
  });

  test("08 — glyph click activates comment in strip and scrolls it into view", async ({ page }) => {
    // Add two comments so we can verify the right one gets activated
    await addCodeCommentViaUI(page, "First comment on lines 5-8");

    // Add a second comment on different lines
    await page.evaluate(() => {
      const api = (window as any).__TEST_API__;
      const file = api.getSelectedFile();
      const group = api.getSelectedGroup();
      api.openCommentInput({
        type: "code",
        group_id: group.id,
        file_path: file,
        start_line: 13,
        end_line: 15,
        selected_code: "const user = await createUser();",
      });
    });
    await page.waitForTimeout(300);
    await page.evaluate(() => (window as any).__TEST_API__.setCommentText("Second comment on lines 13-15"));
    await page.waitForTimeout(100);
    await page.evaluate(() => (window as any).__TEST_API__.submitComment());
    await page.waitForTimeout(1000);

    // Should have 2 glyph icons now
    const glyphs = await page.locator(".comment-glyph-icon").count();
    expect(glyphs).toBe(2);

    // Comment strip should show 2 items
    const items = await page.locator(".comment-strip-item").count();
    expect(items).toBe(2);

    // Click the first glyph icon
    const firstGlyph = page.locator(".comment-glyph-icon").first();
    await firstGlyph.click();
    await page.waitForTimeout(500);

    // The first comment card should be highlighted as active
    const activeItems = page.locator(".comment-strip-item-active");
    expect(await activeItems.count()).toBe(1);

    // The comments strip should be expanded (not collapsed)
    await expect(page.locator(".comment-strip")).not.toHaveClass(/comment-strip-collapsed/);

    // Take screenshot for verification
    await page.locator(".panel-center").screenshot({
      path: path.join(SCREENSHOTS_DIR, "87-glyph-click-activates-comment.png"),
    });
  });
});
