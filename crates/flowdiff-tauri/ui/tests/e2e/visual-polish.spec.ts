import { test, expect, type Page } from "@playwright/test";
import path from "path";
import { fileURLToPath } from "url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const SCREENSHOTS_DIR = path.resolve(__dirname, "../../../../../docs/screenshots");

/** Wait for the demo data to load and the first group to be auto-selected. */
async function waitForAnalysis(page: Page) {
  // Wait for the summary text to appear (indicates analysis is loaded)
  await expect(page.locator(".summary")).toBeVisible({ timeout: 10_000 });
  // Wait for a group to be selected (selected group's file list visible)
  await expect(page.locator(".group-item.selected .file-list")).toBeVisible({ timeout: 5_000 });
  // Wait for Monaco diff editor to initialize (DiffEditor creates code role elements)
  await expect(page.getByRole("code").first()).toBeVisible({ timeout: 10_000 });
  // Brief extra wait for Mermaid rendering and Monaco content
  await page.waitForTimeout(2000);
}

test.describe("Visual Polish — Screenshot Baseline", () => {
  test("01 — loaded analysis with first group selected", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "01-loaded-analysis.png"),
      fullPage: false,
    });

    // Verify three-panel structure
    await expect(page.locator(".panel-left")).toBeVisible();
    await expect(page.locator(".panel-center")).toBeVisible();
    await expect(page.locator(".panel-right")).toBeVisible();

    // Verify flow groups rendered
    const groups = page.locator(".group-item");
    await expect(groups).toHaveCount(4); // 3 groups + 1 infrastructure

    // Verify keyboard hints footer
    await expect(page.locator(".keyboard-hints")).toBeVisible();
  });

  test("02 — left panel: flow groups with risk badges", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.locator(".panel-left").screenshot({
      path: path.join(SCREENSHOTS_DIR, "02-flow-groups-panel.png"),
    });

    // Verify risk badges
    const highRisk = page.locator('.risk-badge[data-risk="high"]');
    await expect(highRisk).toHaveCount(2); // 0.82 and 0.74

    const lowRisk = page.locator('.risk-badge[data-risk="low"]');
    await expect(lowRisk).toHaveCount(1); // 0.35

    // Verify infrastructure group
    await expect(page.locator(".infra-group")).toBeVisible();
  });

  test("03 — center panel: Monaco diff viewer", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.locator(".panel-center").screenshot({
      path: path.join(SCREENSHOTS_DIR, "03-diff-viewer.png"),
    });

    // Verify Monaco loaded
    await expect(page.getByRole("code").first()).toBeVisible();

    // Verify panel header shows file path
    const header = page.locator(".panel-center .panel-header");
    await expect(header).toContainText("src/routes/users.ts");
  });

  test("04 — right panel: annotations with Mermaid graph", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.locator(".panel-right").screenshot({
      path: path.join(SCREENSHOTS_DIR, "04-annotations-panel.png"),
    });

    // Verify group details shown
    await expect(page.locator(".group-detail-name")).toContainText("POST /api/users");
    await expect(page.locator(".entrypoint-info")).toBeVisible();

    // Verify Mermaid SVG rendered
    await expect(page.locator(".mermaid-container svg")).toBeVisible();

    // Verify edges list
    await expect(page.locator(".edge-list")).toBeVisible();
    const edges = page.locator(".edge-item");
    expect(await edges.count()).toBeGreaterThanOrEqual(3);
  });

  test("05 — second group selected: auth flow", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Click on the second group (auth)
    const secondGroup = page.locator(".group-item").nth(1);
    await secondGroup.click();
    await page.waitForTimeout(1500); // Wait for Mermaid re-render

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "05-second-group-selected.png"),
      fullPage: false,
    });

    // Verify second group is now selected
    await expect(page.locator(".group-detail-name")).toContainText("auth/refresh");
  });

  test("06 — third group selected: email worker (low risk)", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Click on the third group
    const thirdGroup = page.locator(".group-item").nth(2);
    await thirdGroup.click();
    await page.waitForTimeout(1500);

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "06-third-group-low-risk.png"),
      fullPage: false,
    });

    await expect(page.locator(".group-detail-name")).toContainText("Email notification");
  });

  test("07 — file navigation within group", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Click on the second file in the first group
    const secondFile = page.locator(".file-item").nth(1);
    await secondFile.click();
    await page.waitForTimeout(1500);

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "07-second-file-selected.png"),
      fullPage: false,
    });

    // Verify second file is selected
    await expect(page.locator(".file-item.selected .file-path")).toContainText("user-service");
    // Verify diff viewer header updated
    await expect(page.locator(".panel-center .panel-header")).toContainText("user-service");
  });

  test("08 — keyboard navigation: j/k for files", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // First file should be selected initially
    await expect(page.locator(".file-item.selected .file-path")).toContainText("users.ts");

    // Click on left panel header to ensure focus is not on Monaco or inputs
    await page.locator(".panel-left .panel-header").click();
    await page.waitForTimeout(300);

    // Press 'j' to go to next file
    await page.keyboard.press("j");
    await page.waitForTimeout(1500);

    // Should now show second file selected
    await expect(page.locator(".file-item.selected .file-path")).toContainText("user-service");

    // Press 'k' to go back
    await page.keyboard.press("k");
    await page.waitForTimeout(1500);

    await expect(page.locator(".file-item.selected .file-path")).toContainText("users.ts");

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "08-keyboard-navigation.png"),
      fullPage: false,
    });
  });

  test("09 — keyboard navigation: J/K for groups", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Should start at first group
    await expect(page.locator(".group-detail-name")).toContainText("POST /api/users");

    // Click on left panel header to ensure focus is not on Monaco or inputs
    await page.locator(".panel-left .panel-header").click();
    await page.waitForTimeout(300);

    // Press 'J' to go to next group
    await page.evaluate(() => {
      document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "J", shiftKey: true, bubbles: true }));
    });
    await page.waitForTimeout(2000);

    // Should now show second group
    await expect(page.locator(".group-detail-name")).toContainText("auth/refresh");

    // Press 'K' to go back
    await page.evaluate(() => {
      document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "K", shiftKey: true, bubbles: true }));
    });
    await page.waitForTimeout(2000);

    await expect(page.locator(".group-detail-name")).toContainText("POST /api/users");

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "09-group-keyboard-navigation.png"),
      fullPage: false,
    });
  });

  test("10 — top bar: summary stats and controls", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.locator(".top-bar").screenshot({
      path: path.join(SCREENSHOTS_DIR, "10-top-bar.png"),
    });

    // Verify summary
    await expect(page.locator(".summary")).toContainText("12 files");
    await expect(page.locator(".summary")).toContainText("3 groups");

    // Verify inputs populated
    await expect(page.locator(".repo-input")).toHaveValue("/demo/repo");
    await expect(page.locator(".base-input")).toHaveValue("main");
  });

  test("11 — Mermaid graph close-up", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.locator(".mermaid-container").screenshot({
      path: path.join(SCREENSHOTS_DIR, "11-mermaid-graph.png"),
    });

    // Verify SVG has nodes
    const svgNodes = page.locator(".mermaid-container svg .node");
    expect(await svgNodes.count()).toBeGreaterThanOrEqual(3);
  });

  test("12 — error state", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Inject an error via page.evaluate
    await page.evaluate(() => {
      const errorBar = document.createElement("div");
      errorBar.className = "error-bar";
      errorBar.innerHTML = `
        <span>Error: Failed to connect to repository at /invalid/path — not a git repository</span>
        <button class="btn-close">&times;</button>
      `;
      document.querySelector(".app")?.insertBefore(
        errorBar,
        document.querySelector(".panels"),
      );
    });

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "12-error-state.png"),
      fullPage: false,
    });

    await expect(page.locator(".error-bar")).toBeVisible();
  });

  test("13 — infrastructure group expanded", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Screenshot just the infrastructure group section
    await page.locator(".infra-group").screenshot({
      path: path.join(SCREENSHOTS_DIR, "13-infrastructure-group.png"),
    });

    // Verify infrastructure files
    const infraFiles = page.locator(".infra-group .file-item");
    await expect(infraFiles).toHaveCount(3); // tsconfig, package.json, .eslintrc
  });

  test("14 — keyboard hints footer", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.locator(".keyboard-hints").screenshot({
      path: path.join(SCREENSHOTS_DIR, "14-keyboard-hints.png"),
    });

    await expect(page.locator(".keyboard-hints")).toContainText("j");
    await expect(page.locator(".keyboard-hints")).toContainText("next file");
    await expect(page.locator(".keyboard-hints")).toContainText("next group");
  });
});
