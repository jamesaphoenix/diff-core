/**
 * Flow Replay Mode — Playwright E2E tests.
 *
 * Tests the guided step-by-step walkthrough of a flow group's files in data flow order:
 * - Enter/exit replay via keyboard (r / Escape) and buttons
 * - Step navigation (n/Space/Arrow keys for next, p/Left for prev)
 * - Replay bar visibility with progress dots, step counter, file role
 * - Visited file checkmarks in left panel
 * - Keyboard hints update when replay is active
 * - Graph node highlight during replay (via replayNodeId prop)
 */
import { test, expect, type Page } from "@playwright/test";
import path from "path";
import { fileURLToPath } from "url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const SCREENSHOTS_DIR = path.resolve(__dirname, "../../../../../docs/screenshots");

// ── Helpers ──

/** Wait for the demo data to load and the first group to be auto-selected. */
async function waitForAnalysis(page: Page) {
  await expect(page.locator(".summary")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator(".group-item.selected .file-list")).toBeVisible({ timeout: 5_000 });
  await expect(page.getByRole("code").first()).toBeVisible({ timeout: 10_000 });
  await page.waitForTimeout(2000);
}

// ── Tests ──

test.describe("Flow Replay Mode", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("01 — r key enters replay mode, shows replay bar", async ({ page }) => {
    // Press 'r' to enter replay
    await page.keyboard.press("r");
    await page.waitForTimeout(300);

    // Replay bar should be visible
    const replayBar = page.locator(".replay-bar");
    await expect(replayBar).toBeVisible();

    // Badge shows "REPLAY"
    await expect(replayBar.locator(".replay-badge")).toHaveText("REPLAY");

    // Step counter shows "Step 1 of N"
    const stepLabel = replayBar.locator(".replay-step-label");
    await expect(stepLabel).toContainText("Step 1 of");

    // File role badge is shown
    await expect(replayBar.locator(".replay-file-role")).toBeVisible();

    // Progress dots are visible
    const dots = replayBar.locator(".replay-dot");
    await expect(dots.first()).toBeVisible();

    await page.screenshot({ path: path.join(SCREENSHOTS_DIR, "51-replay-active.png"), fullPage: true });
  });

  test("02 — Escape exits replay mode", async ({ page }) => {
    await page.keyboard.press("r");
    await page.waitForTimeout(300);
    await expect(page.locator(".replay-bar")).toBeVisible();

    // Press Escape to exit
    await page.keyboard.press("Escape");
    await page.waitForTimeout(300);

    // Replay bar should be gone
    await expect(page.locator(".replay-bar")).not.toBeVisible();
  });

  test("03 — n key advances to next step", async ({ page }) => {
    await page.keyboard.press("r");
    await page.waitForTimeout(300);

    const stepLabel = page.locator(".replay-step-label");
    await expect(stepLabel).toContainText("Step 1 of");

    // Press n to advance
    await page.keyboard.press("n");
    await page.waitForTimeout(300);

    await expect(stepLabel).toContainText("Step 2 of");

    await page.screenshot({ path: path.join(SCREENSHOTS_DIR, "52-replay-step-2.png"), fullPage: true });
  });

  test("04 — p key goes to previous step", async ({ page }) => {
    await page.keyboard.press("r");
    await page.waitForTimeout(300);
    await page.keyboard.press("n");
    await page.waitForTimeout(300);

    const stepLabel = page.locator(".replay-step-label");
    await expect(stepLabel).toContainText("Step 2 of");

    // Press p to go back
    await page.keyboard.press("p");
    await page.waitForTimeout(300);

    await expect(stepLabel).toContainText("Step 1 of");
  });

  test("05 — Space and ArrowRight also advance step", async ({ page }) => {
    await page.keyboard.press("r");
    await page.waitForTimeout(300);

    const stepLabel = page.locator(".replay-step-label");
    await expect(stepLabel).toContainText("Step 1 of");

    // Space advances
    await page.keyboard.press("Space");
    await page.waitForTimeout(300);
    await expect(stepLabel).toContainText("Step 2 of");

    // ArrowRight advances
    await page.keyboard.press("ArrowRight");
    await page.waitForTimeout(300);
    await expect(stepLabel).toContainText("Step 3 of");
  });

  test("06 — ArrowLeft goes to previous step", async ({ page }) => {
    await page.keyboard.press("r");
    await page.waitForTimeout(300);
    await page.keyboard.press("n");
    await page.waitForTimeout(300);

    const stepLabel = page.locator(".replay-step-label");
    await expect(stepLabel).toContainText("Step 2 of");

    await page.keyboard.press("ArrowLeft");
    await page.waitForTimeout(300);
    await expect(stepLabel).toContainText("Step 1 of");
  });

  test("07 — visited files show checkmark in left panel", async ({ page }) => {
    await page.keyboard.press("r");
    await page.waitForTimeout(300);

    // First file should have visited checkmark
    const firstCheck = page.locator(".file-list .replay-visited-check").first();
    await expect(firstCheck).toBeVisible();

    // Advance to step 2
    await page.keyboard.press("n");
    await page.waitForTimeout(300);

    // Now two files should have checkmarks
    const checks = page.locator(".file-list .replay-visited-check");
    await expect(checks).toHaveCount(2);

    await page.screenshot({ path: path.join(SCREENSHOTS_DIR, "53-replay-visited-checks.png"), fullPage: true });
  });

  test("08 — keyboard hints update during replay", async ({ page }) => {
    const hints = page.locator(".keyboard-hints");

    // Normal mode shows j/k/J/K/r
    await expect(hints).toContainText("replay flow");

    // Enter replay
    await page.keyboard.press("r");
    await page.waitForTimeout(300);

    // Replay mode shows n/p/Esc
    await expect(hints).toContainText("next step");
    await expect(hints).toContainText("prev step");
    await expect(hints).toContainText("exit replay");

    // Exit replay
    await page.keyboard.press("Escape");
    await page.waitForTimeout(300);

    // Back to normal hints
    await expect(hints).toContainText("replay flow");
  });

  test("09 — Replay Flow button in right panel enters replay", async ({ page }) => {
    // The "Replay Flow" button should be visible in annotations
    const replayBtn = page.locator(".btn-replay");
    await expect(replayBtn).toBeVisible();
    await expect(replayBtn).toContainText("Replay Flow");

    // Click it
    await replayBtn.click();
    await page.waitForTimeout(300);

    // Replay bar should be visible
    await expect(page.locator(".replay-bar")).toBeVisible();

    // Button should switch to "Exit Replay"
    const exitBtn = page.locator(".btn-replay-exit");
    await expect(exitBtn).toBeVisible();
    await expect(exitBtn).toContainText("Exit Replay");
  });

  test("10 — Exit Replay button exits replay", async ({ page }) => {
    await page.keyboard.press("r");
    await page.waitForTimeout(300);
    await expect(page.locator(".replay-bar")).toBeVisible();

    // Click Exit Replay button
    const exitBtn = page.locator(".btn-replay-exit");
    await exitBtn.click();
    await page.waitForTimeout(300);

    await expect(page.locator(".replay-bar")).not.toBeVisible();
  });

  test("11 — clicking progress dot jumps to that step", async ({ page }) => {
    await page.keyboard.press("r");
    await page.waitForTimeout(300);

    const stepLabel = page.locator(".replay-step-label");
    await expect(stepLabel).toContainText("Step 1 of");

    // Click the third dot (index 2)
    const dots = page.locator(".replay-dot");
    const dotCount = await dots.count();
    if (dotCount >= 3) {
      await dots.nth(2).click();
      await page.waitForTimeout(300);
      await expect(stepLabel).toContainText("Step 3 of");
    }
  });

  test("12 — replay exit button in replay bar works", async ({ page }) => {
    await page.keyboard.press("r");
    await page.waitForTimeout(300);
    await expect(page.locator(".replay-bar")).toBeVisible();

    // Click the X button in the replay bar
    await page.locator(".replay-bar .replay-exit").click();
    await page.waitForTimeout(300);

    await expect(page.locator(".replay-bar")).not.toBeVisible();
  });

  test("13 — j/k/J/K blocked during replay", async ({ page }) => {
    await page.keyboard.press("r");
    await page.waitForTimeout(300);

    const stepLabel = page.locator(".replay-step-label");
    await expect(stepLabel).toContainText("Step 1 of");

    // Press j — should not change step or exit replay
    await page.keyboard.press("j");
    await page.waitForTimeout(200);
    await expect(page.locator(".replay-bar")).toBeVisible();
    await expect(stepLabel).toContainText("Step 1 of");

    // Press J — should not switch groups or exit replay
    await page.keyboard.press("J");
    await page.waitForTimeout(200);
    await expect(page.locator(".replay-bar")).toBeVisible();
  });

  test("14 — switching groups exits replay", async ({ page }) => {
    await page.keyboard.press("r");
    await page.waitForTimeout(300);
    await expect(page.locator(".replay-bar")).toBeVisible();

    // Click a different group
    const groups = page.locator(".group-item");
    const groupCount = await groups.count();
    if (groupCount >= 2) {
      await groups.nth(1).click();
      await page.waitForTimeout(500);

      // Replay should be exited
      await expect(page.locator(".replay-bar")).not.toBeVisible();
    }
  });

  test("15 — prev button disabled on first step, next disabled on last", async ({ page }) => {
    await page.keyboard.press("r");
    await page.waitForTimeout(300);

    // On step 1, prev button should be disabled
    const prevBtn = page.locator(".replay-bar .replay-btn").first();
    await expect(prevBtn).toBeDisabled();

    // Navigate to last step
    const stepLabel = page.locator(".replay-step-label");
    const text = await stepLabel.textContent();
    const match = text?.match(/of (\d+)/);
    const totalSteps = match ? parseInt(match[1]) : 0;

    for (let i = 1; i < totalSteps; i++) {
      await page.keyboard.press("n");
      await page.waitForTimeout(100);
    }

    // On last step, next button should be disabled
    const nextBtn = page.locator(".replay-bar .replay-btn").nth(1);
    await expect(nextBtn).toBeDisabled();

    await page.screenshot({ path: path.join(SCREENSHOTS_DIR, "54-replay-last-step.png"), fullPage: true });
  });
});
