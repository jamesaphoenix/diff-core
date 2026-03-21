/**
 * Screenshot Capture — Playwright visual documentation.
 *
 * Captures screenshots of all key UI states for docs/screenshots/.
 * Run with: npx playwright test screenshots.spec.ts
 */
import { test, expect, type Page } from "@playwright/test";
import path from "path";
import { fileURLToPath } from "url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const SCREENSHOTS_DIR = path.resolve(__dirname, "../../../../../docs/screenshots");

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

// ── Screenshots ──

test.describe("Screenshots", () => {
  test("60 — full app with analysis loaded", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "60-analysis-loaded.png"),
      fullPage: false,
    });
  });

  test("61 — comment strip with comments", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Add two comments
    await addCommentViaUI(page, "This validation logic needs error boundaries");
    await addCommentViaUI(page, "Consider extracting this into a shared utility");
    await page.waitForTimeout(500);

    // Focus on center panel to show the strip
    await page.locator(".panel-center").screenshot({
      path: path.join(SCREENSHOTS_DIR, "61-comment-strip.png"),
    });
  });

  test("62 — comment strip close-up", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await addCommentViaUI(page, "Potential SQL injection here — use parameterized queries");
    await page.waitForTimeout(500);

    const strip = page.locator(".comment-strip");
    await expect(strip).toBeVisible();
    await strip.screenshot({
      path: path.join(SCREENSHOTS_DIR, "62-comment-strip-closeup.png"),
    });
  });

  test("63 — edges section collapsed (default)", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Scroll the right panel to show edges toggle
    const edgesToggle = page.locator(".annotation-section .flow-graph-toggle").filter({ hasText: "Edges" });
    if (await edgesToggle.isVisible()) {
      await edgesToggle.scrollIntoViewIfNeeded();
      await page.waitForTimeout(300);
      await page.locator(".panel-right").screenshot({
        path: path.join(SCREENSHOTS_DIR, "63-edges-collapsed.png"),
      });
    }
  });

  test("64 — edges section expanded", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    const edgesToggle = page.locator(".annotation-section .flow-graph-toggle").filter({ hasText: "Edges" });
    if (await edgesToggle.isVisible()) {
      await edgesToggle.click();
      await page.waitForTimeout(300);
      await edgesToggle.scrollIntoViewIfNeeded();
      await page.locator(".panel-right").screenshot({
        path: path.join(SCREENSHOTS_DIR, "64-edges-expanded.png"),
      });
    }
  });

  test("65 — flow graph (normal view)", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    const graph = page.locator("[data-testid='flow-graph']");
    if (await graph.isVisible()) {
      await graph.screenshot({
        path: path.join(SCREENSHOTS_DIR, "65-flow-graph.png"),
      });
    }
  });

  test("66 — flow graph fullscreen", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    const fsBtn = page.locator(".flow-fullscreen-btn");
    if (await fsBtn.isVisible()) {
      await fsBtn.click();
      await page.waitForTimeout(1000);

      await page.screenshot({
        path: path.join(SCREENSHOTS_DIR, "66-flow-graph-fullscreen.png"),
        fullPage: false,
      });

      await page.keyboard.press("Escape");
    }
  });

  test("67 — open-with dropdown with icons", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    const arrow = page.locator(".open-with-arrow");
    if (await arrow.isVisible()) {
      await arrow.click();
      await page.waitForTimeout(300);

      await page.locator(".editor-toolbar").screenshot({
        path: path.join(SCREENSHOTS_DIR, "67-open-with-dropdown.png"),
      });
    }
  });

  test("68 — keyboard hints bar", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    const hints = page.locator(".keyboard-hints");
    await expect(hints).toBeVisible();
    await hints.screenshot({
      path: path.join(SCREENSHOTS_DIR, "68-keyboard-hints.png"),
    });
  });

  test("69 — flow group panel (left sidebar)", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.locator(".panel-left").screenshot({
      path: path.join(SCREENSHOTS_DIR, "69-flow-groups-panel.png"),
    });
  });

  test("70 — second group selected", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.keyboard.press("J");
    await page.waitForTimeout(500);

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "70-second-group.png"),
      fullPage: false,
    });
  });

  test("71 — replay mode active", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.keyboard.press("r");
    await page.waitForTimeout(500);

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "71-replay-mode.png"),
      fullPage: false,
    });

    await page.keyboard.press("Escape");
  });

  test("72 — annotations panel", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.locator(".panel-right").screenshot({
      path: path.join(SCREENSHOTS_DIR, "72-annotations-panel.png"),
    });
  });
});
