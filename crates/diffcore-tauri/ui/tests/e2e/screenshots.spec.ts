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

  // Tests 61 and 63-66 were deleted: the comment strip is dead code, the edges
  // toggle moved to the Edges subtab (covered in ui-improvements.spec.ts), and
  // the graph captures are owned by visual-polish (11-flow-graph.png) and
  // hardening (37-graph-fullscreen.png).

  test("62 — comments tab close-up", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await addCommentViaUI(page, "Potential SQL injection here — use parameterized queries");
    await page.waitForTimeout(500);

    await page.getByTestId("comments-tab").click();

    const tab = page.locator(".comments-tab");
    await expect(tab.locator(".comments-tab-card")).toHaveCount(1);
    await tab.screenshot({
      path: path.join(SCREENSHOTS_DIR, "62-comments-tab-closeup.png"),
    });
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
