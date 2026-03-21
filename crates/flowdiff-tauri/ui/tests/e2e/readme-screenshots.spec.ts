/**
 * README Screenshots — High-quality captures for open-source README.
 *
 * Uses 1440x900 viewport, captures key app states with demo data.
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
  await page.waitForTimeout(2500);
}

async function addCodeComment(page: Page, text: string) {
  await page.evaluate(() => {
    const api = (window as any).__TEST_API__;
    const file = api.getSelectedFile();
    const group = api.getSelectedGroup();
    if (!file || !group) return;
    api.openCommentInput({
      type: "code",
      group_id: group.id,
      file_path: file,
      start_line: 7,
      end_line: 10,
      selected_code: "const user = await createUser(email, name, password);\nres.status(201).json({ id: user.id, email: user.email });",
    });
  });
  await page.waitForTimeout(200);
  await page.evaluate((t) => (window as any).__TEST_API__.setCommentText(t), text);
  await page.waitForTimeout(100);
  await page.evaluate(() => (window as any).__TEST_API__.submitComment());
  await page.waitForTimeout(500);
}

test.describe("README Screenshots", () => {
  test("01 — hero: full app with analysis", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "hero-analysis.png"),
      fullPage: false,
    });
  });

  test("02 — flow graph", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    const graph = page.locator("[data-testid='flow-graph']");
    if (await graph.isVisible()) {
      await graph.screenshot({
        path: path.join(SCREENSHOTS_DIR, "flow-graph.png"),
      });
    }
  });

  test("03 — flow graph fullscreen", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    const fsBtn = page.locator(".flow-fullscreen-btn");
    if (await fsBtn.isVisible()) {
      await fsBtn.click();
      await page.waitForTimeout(1000);
      await page.screenshot({
        path: path.join(SCREENSHOTS_DIR, "flow-graph-fullscreen.png"),
        fullPage: false,
      });
      await page.keyboard.press("Escape");
    }
  });

  test("04 — comments with gutter icons", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await addCodeComment(page, "Validate email format before creating user");
    await addCodeComment(page, "Add rate limiting to prevent abuse");
    await page.waitForTimeout(1000);

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "comments-gutter.png"),
      fullPage: false,
    });
  });

  test("05 — comment strip closeup", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await addCodeComment(page, "Consider extracting this into a shared validation utility");
    await page.waitForTimeout(500);

    const strip = page.locator(".comment-strip");
    if (await strip.isVisible()) {
      await strip.screenshot({
        path: path.join(SCREENSHOTS_DIR, "comment-strip.png"),
      });
    }
  });

  test("06 — replay mode", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.keyboard.press("r");
    await page.waitForTimeout(800);

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "replay-mode.png"),
      fullPage: false,
    });

    await page.keyboard.press("Escape");
  });

  test("07 — second group selected", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.keyboard.press("J");
    await page.waitForTimeout(500);

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "second-group.png"),
      fullPage: false,
    });
  });

  test("08 — keyboard hints", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.locator(".keyboard-hints").screenshot({
      path: path.join(SCREENSHOTS_DIR, "keyboard-hints.png"),
    });
  });

  test("09 — open with dropdown", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    const arrow = page.locator(".open-with-arrow");
    if (await arrow.isVisible()) {
      await arrow.click();
      await page.waitForTimeout(300);
      await page.locator(".editor-toolbar").screenshot({
        path: path.join(SCREENSHOTS_DIR, "open-with.png"),
      });
    }
  });

  test("10 — annotations panel", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.locator(".panel-right").screenshot({
      path: path.join(SCREENSHOTS_DIR, "annotations-panel.png"),
    });
  });
});
