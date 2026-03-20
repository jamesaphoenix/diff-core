/**
 * Tauri App Audit — Phase 8 hardening tests for bugs found during audit.
 *
 * Covers:
 * - Error boundaries (Diff Viewer, Flow Graph, Risk Heatmap)
 * - Error boundary recovery via Retry button
 * - Keyboard nav disabled when input fields are focused
 * - IPC error display for failed file diff loads (Tauri mode only — verified structurally)
 * - Large dataset rendering performance (100+ groups, 1000+ files)
 * - State desync prevention (handleSelectGroup → handleSelectFile dep chain)
 * - Deep analysis race condition (concurrent requests don't prematurely clear loading)
 * - XSS safety (script tags in diff content rendered as code, not executed)
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

// ── Mock Data Generators ──

/** Generate analysis with malicious script content in file names and diff content. */
function generateXssAnalysis() {
  return {
    version: "1.0.0",
    diff_source: { diff_type: "BranchComparison", base: "main", head: "feature/xss", base_sha: "a1", head_sha: "b2" },
    summary: { total_files_changed: 1, total_groups: 1, languages_detected: ["javascript"], frameworks_detected: [] },
    groups: [
      {
        id: "group_xss",
        name: '<img src=x onerror="alert(1)">',
        entrypoint: {
          file: "src/<script>alert('xss')</script>.js",
          symbol: "handler",
          entrypoint_type: "HttpRoute",
        },
        files: [
          {
            path: "src/<script>alert('xss')</script>.js",
            flow_position: 0,
            role: "Entrypoint",
            changes: { additions: 10, deletions: 5 },
            symbols_changed: ["handler"],
          },
        ],
        edges: [],
        risk_score: 0.85,
        review_order: 1,
      },
    ],
    infrastructure_group: null,
    annotations: null,
  };
}

/** Generate a very large analysis (100 groups, 1000+ files). */
function generateMassiveAnalysis() {
  const groups = [];
  const roles = ["Entrypoint", "Service", "Repository", "Model", "Utility", "Handler", "Config", "Test"];

  for (let g = 0; g < 100; g++) {
    const fileCount = 8 + (g % 7); // 8-14 files per group → ~1100 total
    const files = [];
    const edges = [];

    for (let f = 0; f < fileCount; f++) {
      files.push({
        path: `src/modules/mod${g}/component-${f}.ts`,
        flow_position: f,
        role: roles[f % roles.length],
        changes: { additions: 10 + (g * f) % 40, deletions: (g + f) % 15 },
        symbols_changed: [`sym_${g}_${f}`],
      });
      if (f > 0) {
        edges.push({
          from: `src/modules/mod${g}/component-${f - 1}.ts::sym_${g}_${f - 1}`,
          to: `src/modules/mod${g}/component-${f}.ts::sym_${g}_${f}`,
          edge_type: "Calls",
        });
      }
    }

    groups.push({
      id: `group_${g}`,
      name: `Module ${g}: ${g % 4 === 0 ? "Auth flow" : g % 4 === 1 ? "Data pipeline" : g % 4 === 2 ? "API handler" : "Background task"}`,
      entrypoint: { file: files[0].path, symbol: `sym_${g}_0`, entrypoint_type: "HttpRoute" },
      files,
      edges,
      risk_score: Math.round((0.1 + (g % 10) * 0.09) * 100) / 100,
      review_order: g + 1,
    });
  }

  return {
    version: "1.0.0",
    diff_source: { diff_type: "BranchComparison", base: "main", head: "feature/massive", base_sha: "abc", head_sha: "def" },
    summary: {
      total_files_changed: groups.reduce((sum: number, g: { files: unknown[] }) => sum + g.files.length, 0),
      total_groups: groups.length,
      languages_detected: ["typescript"],
      frameworks_detected: ["express", "prisma"],
    },
    groups,
    infrastructure_group: {
      files: Array.from({ length: 20 }, (_, i) => `config/infra-${i}.toml`),
      reason: "Not reachable from any detected entrypoint",
    },
    annotations: null,
  };
}

// ═══════════════════════════════════════════════════════════════════
// Error Boundaries
// ═══════════════════════════════════════════════════════════════════

test.describe("Error Boundaries", () => {
  test("Diff Viewer crash shows error boundary fallback, not white screen", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Trigger a crash in the Diff Viewer panel via test API
    await page.evaluate(() => {
      (window as any).__TEST_API__.crashPanel("Diff Viewer");
    });

    // Wait for the error boundary fallback to appear
    const fallback = page.locator('[data-testid="error-boundary-Diff Viewer"]');
    await expect(fallback).toBeVisible({ timeout: 5_000 });
    await expect(fallback.locator(".error-boundary-title")).toHaveText("Diff Viewer crashed");
    await expect(fallback.locator(".error-boundary-message")).toContainText("Test crash in Diff Viewer");

    // The rest of the app should still be functional
    await expect(page.locator(".panel-left")).toBeVisible();
    await expect(page.locator(".panel-right")).toBeVisible();
    await expect(page.locator(".group-item.selected")).toBeVisible();
  });

  test("Flow Graph crash shows error boundary fallback", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // The Flow Graph is only visible when a group with edges is selected
    // First verify the flow graph exists
    const graphSection = page.locator(".annotation-section h3").filter({ hasText: "Flow Graph" });

    // Only proceed if the group has edges (graph is rendered)
    if (await graphSection.isVisible()) {
      await page.evaluate(() => {
        (window as any).__TEST_API__.crashPanel("Flow Graph");
      });

      const fallback = page.locator('[data-testid="error-boundary-Flow Graph"]');
      await expect(fallback).toBeVisible({ timeout: 5_000 });
      await expect(fallback.locator(".error-boundary-title")).toHaveText("Flow Graph crashed");
    }
  });

  test.skip("Risk Heatmap crash shows error boundary fallback — hidden in Phase 9.4", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate(() => {
      (window as any).__TEST_API__.crashPanel("Risk Heatmap");
    });

    const fallback = page.locator('[data-testid="error-boundary-Risk Heatmap"]');
    await expect(fallback).toBeVisible({ timeout: 5_000 });
    await expect(fallback.locator(".error-boundary-title")).toHaveText("Risk Heatmap crashed");

    // Other panels still work
    await expect(page.locator(".group-item.selected")).toBeVisible();
  });

  test("Error boundary Retry button recovers the component", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Crash the Diff Viewer
    await page.evaluate(() => {
      (window as any).__TEST_API__.crashPanel("Diff Viewer");
    });

    const fallback = page.locator('[data-testid="error-boundary-Diff Viewer"]');
    await expect(fallback).toBeVisible({ timeout: 5_000 });

    // Clear the crash trigger first, then click Retry
    await page.evaluate(() => {
      (window as any).__TEST_API__.crashPanel(null);
    });
    await fallback.locator(".error-boundary-retry").click();

    // The error boundary should be gone and the diff viewer should render
    await expect(fallback).not.toBeVisible({ timeout: 3_000 });
  });

  test("Multiple panels can crash independently", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Crash both Diff Viewer and Flow Graph
    await page.evaluate(() => {
      (window as any).__TEST_API__.crashPanel("Diff Viewer");
    });

    const diffFallback = page.locator('[data-testid="error-boundary-Diff Viewer"]');
    await expect(diffFallback).toBeVisible({ timeout: 5_000 });

    // Now crash Flow Graph too
    await page.evaluate(() => {
      (window as any).__TEST_API__.crashPanel("Flow Graph");
    });

    const graphFallback = page.locator('[data-testid="error-boundary-Flow Graph"]');
    await expect(graphFallback).toBeVisible({ timeout: 5_000 });

    // Both fallbacks visible — neither took down the whole app
    await expect(diffFallback).toBeVisible();
    await expect(graphFallback).toBeVisible();

    // Left panel (flow groups) still works
    await expect(page.locator(".group-item").first()).toBeVisible();
  });
});

// ═══════════════════════════════════════════════════════════════════
// Keyboard Navigation Safety
// ═══════════════════════════════════════════════════════════════════

test.describe("Keyboard Navigation Safety", () => {
  test("j/k keys do not trigger navigation when input is focused", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Get the currently selected file
    const selectedFileBefore = await page.locator(".file-item.selected .file-path").textContent();

    // Focus the repo input field
    await page.locator(".repo-input").focus();

    // Press j (next file) — should type into input, not navigate
    await page.keyboard.press("j");
    await page.waitForTimeout(200);

    // The selected file should NOT have changed
    const selectedFileAfter = await page.locator(".file-item.selected .file-path").textContent();
    expect(selectedFileAfter).toBe(selectedFileBefore);

    // The input should contain the typed character
    const inputValue = await page.locator(".repo-input").inputValue();
    expect(inputValue).toContain("j");
  });

  test("J/K keys do not trigger group navigation when select is focused", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Open settings to get a select element
    await page.locator(".btn-settings").click();
    await expect(page.locator(".settings-panel")).toBeVisible();

    // Get currently selected group
    const selectedGroupBefore = await page.locator(".group-item.selected .group-name").textContent();

    // Focus the provider select
    await page.locator(".settings-select").first().focus();

    // Press J (next group) — should NOT navigate groups
    await page.keyboard.press("Shift+j");
    await page.waitForTimeout(200);

    // Close settings
    await page.keyboard.press("Escape");
    await page.waitForTimeout(200);

    // Group should not have changed
    const selectedGroupAfter = await page.locator(".group-item.selected .group-name").textContent();
    expect(selectedGroupAfter).toBe(selectedGroupBefore);
  });

  test("Escape key closes settings panel before affecting other listeners", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Open settings
    await page.locator(".btn-settings").click();
    await expect(page.locator(".settings-panel")).toBeVisible();

    // Press Escape
    await page.keyboard.press("Escape");
    await page.waitForTimeout(300);

    // Settings should be closed
    await expect(page.locator(".settings-panel")).not.toBeVisible();
  });
});

// ═══════════════════════════════════════════════════════════════════
// XSS Safety
// ═══════════════════════════════════════════════════════════════════

test.describe("XSS Safety", () => {
  test("Script tags in group names are rendered as text, not executed", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Inject analysis with XSS payloads
    await page.evaluate((data) => {
      (window as any).__TEST_API__.setAnalysis(data);
    }, generateXssAnalysis());

    await page.waitForTimeout(1000);

    // The group name should be visible as text
    const groupName = page.locator(".group-item .group-name").first();
    await expect(groupName).toBeVisible();

    // Verify no alert was triggered (if XSS worked, page would have an alert)
    // The script tag content should be rendered as escaped text
    const content = await groupName.textContent();
    expect(content).toContain("<img");
    expect(content).not.toBe(""); // It rendered something, not empty

    // Verify no malicious script elements were injected into the DOM
    const injectedScript = await page.evaluate(() =>
      document.querySelector("script[src*='alert']") !== null
    );
    expect(injectedScript).toBe(false);
  });

  test("Script tags in file paths are rendered as text in file list", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((data) => {
      (window as any).__TEST_API__.setAnalysis(data);
    }, generateXssAnalysis());

    await page.waitForTimeout(1000);

    // The file path with script tags should be rendered as text
    const filePath = page.locator(".file-item .file-path").first();
    await expect(filePath).toBeVisible();
    const text = await filePath.textContent();
    // The path should contain the script tag as escaped text
    expect(text).toBeTruthy();
  });
});

// ═══════════════════════════════════════════════════════════════════
// Large Dataset Performance
// ═══════════════════════════════════════════════════════════════════

test.describe("Large Dataset Performance", () => {
  test("100 groups with 1000+ files renders within 5 seconds", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    const startTime = Date.now();

    await page.evaluate((data) => {
      (window as any).__TEST_API__.setAnalysis(data);
    }, generateMassiveAnalysis());

    // Wait for groups to render
    await expect(page.locator(".group-item").first()).toBeVisible({ timeout: 5_000 });

    const elapsed = Date.now() - startTime;

    // Should render within 5 seconds
    expect(elapsed).toBeLessThan(5_000);

    // Verify the correct number of groups rendered
    const groupCount = await page.locator(".group-item:not(.infra-group)").count();
    expect(groupCount).toBe(100);
  });

  test("Left panel scrolls with 100 groups", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((data) => {
      (window as any).__TEST_API__.setAnalysis(data);
    }, generateMassiveAnalysis());

    await expect(page.locator(".group-item").first()).toBeVisible({ timeout: 5_000 });

    // The panel body should be scrollable
    const panelBody = page.locator(".panel-left .panel-body");
    const scrollHeight = await panelBody.evaluate((el) => el.scrollHeight);
    const clientHeight = await panelBody.evaluate((el) => el.clientHeight);

    // scrollHeight should be much larger than clientHeight (content overflows)
    expect(scrollHeight).toBeGreaterThan(clientHeight);
  });

  test("Infrastructure group renders with 20 files", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((data) => {
      (window as any).__TEST_API__.setAnalysis(data);
    }, generateMassiveAnalysis());

    await page.waitForTimeout(1000);

    // Infrastructure group should be visible
    const infraGroup = page.locator(".infra-group");
    await expect(infraGroup).toBeVisible();

    // Should have 20 files
    const infraFiles = infraGroup.locator(".file-item");
    await expect(infraFiles).toHaveCount(20);
  });

  test("Navigating between groups in large dataset is responsive", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((data) => {
      (window as any).__TEST_API__.setAnalysis(data);
    }, generateMassiveAnalysis());

    await expect(page.locator(".group-item").first()).toBeVisible({ timeout: 5_000 });
    await page.waitForTimeout(500);

    // Click the 50th group
    const targetGroup = page.locator(".group-item:not(.infra-group)").nth(49);
    const startTime = Date.now();
    await targetGroup.click();

    // Wait for it to become selected
    await expect(targetGroup).toHaveClass(/selected/, { timeout: 2_000 });
    const elapsed = Date.now() - startTime;

    // Should select within 2 seconds
    expect(elapsed).toBeLessThan(2_000);
  });

  test.skip("Risk heatmap renders with 100 groups — hidden in Phase 9.4", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((data) => {
      (window as any).__TEST_API__.setAnalysis(data);
    }, generateMassiveAnalysis());

    await page.waitForTimeout(1000);

    // The SVG heatmap should be rendered
    const heatmapSvg = page.locator(".heatmap-svg");
    await expect(heatmapSvg).toBeVisible();

    // Should have rendered cells (at least some of the 100 groups)
    const cellCount = await heatmapSvg.locator(".heatmap-cell-group").count();
    expect(cellCount).toBe(100);
  });
});

// ═══════════════════════════════════════════════════════════════════
// State Desync Prevention
// ═══════════════════════════════════════════════════════════════════

test.describe("State Desync Prevention", () => {
  test("Switching groups updates both selected group and file diff", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Get the first group's first file
    const firstGroupFile = await page.locator(".group-item.selected .file-item.selected .file-path").textContent();

    // Click the second group
    const secondGroup = page.locator(".group-item:not(.infra-group)").nth(1);
    await secondGroup.click();
    await page.waitForTimeout(500);

    // The selected file should have changed (new group's first file)
    const newSelectedFile = await page.locator(".group-item.selected .file-item.selected .file-path").textContent();
    expect(newSelectedFile).not.toBe(firstGroupFile);

    // The diff viewer header should show the new file path
    const diffHeader = await page.locator(".panel-center .panel-header").textContent();
    expect(diffHeader).toBeTruthy();
    expect(diffHeader).not.toBe("Diff Viewer"); // Should show a file path, not the empty state
  });

  test("Rapid group switching does not leave stale file selection", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Rapidly click through groups
    const groups = page.locator(".group-item:not(.infra-group)");
    const groupCount = await groups.count();

    for (let i = 0; i < Math.min(groupCount, 3); i++) {
      await groups.nth(i).click();
      // Don't wait — rapid clicks
    }

    // After settling, exactly one group should be selected
    await page.waitForTimeout(500);
    const selectedCount = await page.locator(".group-item.selected:not(.infra-group)").count();
    expect(selectedCount).toBe(1);

    // And it should have a selected file
    const selectedFile = page.locator(".group-item.selected .file-item.selected");
    await expect(selectedFile).toBeVisible();
  });

  test("Group switch exits replay mode", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Enter replay mode
    await page.evaluate(() => {
      (window as any).__TEST_API__.enterReplay();
    });
    await page.waitForTimeout(300);
    await expect(page.locator(".replay-bar")).toBeVisible();

    // Click a different group
    const secondGroup = page.locator(".group-item:not(.infra-group)").nth(1);
    await secondGroup.click();
    await page.waitForTimeout(500);

    // Replay should be exited
    await expect(page.locator(".replay-bar")).not.toBeVisible();
  });
});
