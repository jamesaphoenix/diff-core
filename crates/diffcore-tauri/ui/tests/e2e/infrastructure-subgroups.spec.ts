/**
 * Infrastructure Sub-Groups — Playwright E2E tests.
 *
 * Phase 5.2: Infrastructure files are clickable (load diff, selection highlight).
 * Phase 5.3: Sub-groups render with collapsible headers.
 */
import { test, expect, type Page } from "@playwright/test";

// ── Helpers ──

async function waitForAnalysis(page: Page) {
  await expect(page.locator(".summary")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator(".group-item.selected .file-list")).toBeVisible({ timeout: 5_000 });
  await expect(page.getByRole("code").first()).toBeVisible({ timeout: 10_000 });
  await page.waitForTimeout(2000);
}

/** Generate analysis with infrastructure sub-groups. */
function analysisWithSubGroups() {
  return {
    version: "0.1.0",
    diff_source: {
      diff_type: "BranchComparison",
      base: "main",
      head: "feature/test",
      base_sha: "abc123",
      head_sha: "def456",
    },
    summary: {
      total_files_changed: 8,
      total_groups: 1,
      languages_detected: ["typescript"],
      frameworks_detected: ["express"],
    },
    groups: [
      {
        id: "group_1",
        name: "User API",
        entrypoint: {
          file: "src/routes/users.ts",
          symbol: "GET /users",
          entrypoint_type: "HttpRoute",
        },
        files: [
          {
            path: "src/routes/users.ts",
            flow_position: 0,
            role: "Entrypoint",
            changes: { additions: 10, deletions: 5 },
            symbols_changed: ["getUsers"],
          },
        ],
        edges: [],
        risk_score: 0.45,
        review_order: 1,
      },
    ],
    infrastructure_group: {
      files: [
        "Dockerfile",
        ".env.dev",
        "tsconfig.json",
        "src/schemas/user.ts",
        "src/schemas/billing.ts",
        "scripts/deploy.sh",
        "docs/README.md",
      ],
      sub_groups: [
        {
          name: "Infrastructure",
          category: "Infrastructure",
          files: ["Dockerfile", ".env.dev", "tsconfig.json"],
        },
        {
          name: "Schemas",
          category: "Schema",
          files: ["src/schemas/user.ts", "src/schemas/billing.ts"],
        },
        {
          name: "Scripts",
          category: "Script",
          files: ["scripts/deploy.sh"],
        },
        {
          name: "Documentation",
          category: "Documentation",
          files: ["docs/README.md"],
        },
      ],
      reason: "Not reachable from any detected entrypoint",
    },
    annotations: null,
  };
}

/** Generate analysis with infrastructure files but NO sub-groups (backward compat). */
function analysisWithoutSubGroups() {
  return {
    version: "0.1.0",
    diff_source: {
      diff_type: "BranchComparison",
      base: "main",
      head: "feature/test",
      base_sha: "abc123",
      head_sha: "def456",
    },
    summary: {
      total_files_changed: 5,
      total_groups: 1,
      languages_detected: ["typescript"],
      frameworks_detected: ["express"],
    },
    groups: [
      {
        id: "group_1",
        name: "User API",
        entrypoint: {
          file: "src/routes/users.ts",
          symbol: "GET /users",
          entrypoint_type: "HttpRoute",
        },
        files: [
          {
            path: "src/routes/users.ts",
            flow_position: 0,
            role: "Entrypoint",
            changes: { additions: 10, deletions: 5 },
            symbols_changed: ["getUsers"],
          },
        ],
        edges: [],
        risk_score: 0.45,
        review_order: 1,
      },
    ],
    infrastructure_group: {
      files: ["tsconfig.json", "package.json", ".eslintrc.json", "Dockerfile"],
      reason: "Not reachable from any detected entrypoint",
    },
    annotations: null,
  };
}

// ── Phase 5.2: Clickable Infrastructure Files ──

test.describe("Infrastructure Files — Clickable (Phase 5.2)", () => {
  test("01 — clicking an infrastructure file selects it", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((analysis) => {
      (window as any).__TEST_API__.setAnalysis(analysis);
    }, analysisWithSubGroups());
    await page.waitForTimeout(1000);

    // Expand infrastructure group
    await page.locator(".infra-group .group-header").click();
    await page.waitForTimeout(300);

    // Expand the first sub-group (Infrastructure)
    await page.locator(".infra-sub-group-header").first().click();
    await page.waitForTimeout(300);

    // Click the first file in the sub-group
    const firstFile = page.locator(".infra-sub-group .file-item").first();
    await firstFile.click();
    await page.waitForTimeout(500);

    // Verify the file gets the selected class
    await expect(firstFile).toHaveClass(/selected/);
  });

  test("02 — clicking an infrastructure file loads the selected file state", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((analysis) => {
      (window as any).__TEST_API__.setAnalysis(analysis);
    }, analysisWithSubGroups());
    await page.waitForTimeout(1000);

    // Expand infrastructure group
    await page.locator(".infra-group .group-header").click();
    await page.waitForTimeout(300);

    // Expand "Infrastructure" sub-group
    await page.locator(".infra-sub-group-header").first().click();
    await page.waitForTimeout(300);

    // Click a file
    await page.locator(".infra-sub-group .file-item").first().click();
    await page.waitForTimeout(500);

    // Verify selectedFile is set via test API
    const selectedFile = await page.evaluate(() => {
      return (window as any).__TEST_API__.getSelectedFile();
    });
    expect(selectedFile).toBeTruthy();
  });

  test("03 — selection highlight follows clicks between files", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((analysis) => {
      (window as any).__TEST_API__.setAnalysis(analysis);
    }, analysisWithSubGroups());
    await page.waitForTimeout(1000);

    // Expand infra group + first sub-group
    await page.locator(".infra-group .group-header").click();
    await page.waitForTimeout(300);
    await page.locator(".infra-sub-group-header").first().click();
    await page.waitForTimeout(300);

    const files = page.locator(".infra-sub-group .file-item");

    // Click first file
    await files.first().click();
    await page.waitForTimeout(300);
    await expect(files.first()).toHaveClass(/selected/);

    // If there are multiple files, click the second one
    const count = await files.count();
    if (count > 1) {
      await files.nth(1).click();
      await page.waitForTimeout(300);
      // First should no longer be selected
      await expect(files.first()).not.toHaveClass(/selected/);
      // Second should be selected
      await expect(files.nth(1)).toHaveClass(/selected/);
    }
  });

  test("04 — backward compat: flat list files are clickable when no sub_groups", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((analysis) => {
      (window as any).__TEST_API__.setAnalysis(analysis);
    }, analysisWithoutSubGroups());
    await page.waitForTimeout(1000);

    // Expand infrastructure group
    await page.locator(".infra-group .group-header").click();
    await page.waitForTimeout(300);

    // Click a file in the flat list
    const firstFile = page.locator(".infra-group .file-item").first();
    await firstFile.click();
    await page.waitForTimeout(500);

    // Verify the file gets the selected class
    await expect(firstFile).toHaveClass(/selected/);
  });
});

// ── Phase 5.3: Sub-Group Rendering ──

test.describe("Infrastructure Sub-Groups — Rendering (Phase 5.3)", () => {
  test("05 — sub-group headers are rendered when expanded", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((analysis) => {
      (window as any).__TEST_API__.setAnalysis(analysis);
    }, analysisWithSubGroups());
    await page.waitForTimeout(1000);

    // Expand infrastructure group
    await page.locator(".infra-group .group-header").click();
    await page.waitForTimeout(300);

    // Verify sub-group headers are shown
    const subGroupHeaders = page.locator(".infra-sub-group-header");
    expect(await subGroupHeaders.count()).toBe(4);

    // Verify sub-group names
    await expect(subGroupHeaders.nth(0)).toContainText("Infrastructure");
    await expect(subGroupHeaders.nth(1)).toContainText("Schemas");
    await expect(subGroupHeaders.nth(2)).toContainText("Scripts");
    await expect(subGroupHeaders.nth(3)).toContainText("Documentation");
  });

  test("06 — sub-group headers show file counts", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((analysis) => {
      (window as any).__TEST_API__.setAnalysis(analysis);
    }, analysisWithSubGroups());
    await page.waitForTimeout(1000);

    await page.locator(".infra-group .group-header").click();
    await page.waitForTimeout(300);

    // Check file counts
    const counts = page.locator(".infra-sub-group-count");
    await expect(counts.nth(0)).toContainText("3 files");  // Infrastructure
    await expect(counts.nth(1)).toContainText("2 files");  // Schemas
    await expect(counts.nth(2)).toContainText("1 file");   // Scripts
    await expect(counts.nth(3)).toContainText("1 file");   // Documentation
  });

  test("07 — sub-groups are collapsed by default", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((analysis) => {
      (window as any).__TEST_API__.setAnalysis(analysis);
    }, analysisWithSubGroups());
    await page.waitForTimeout(1000);

    // Expand infrastructure group
    await page.locator(".infra-group .group-header").click();
    await page.waitForTimeout(300);

    // Sub-group headers visible but no file-items visible inside sub-groups
    await expect(page.locator(".infra-sub-group-header").first()).toBeVisible();
    const subGroupFiles = page.locator(".infra-sub-group .file-item");
    expect(await subGroupFiles.count()).toBe(0);
  });

  test("08 — clicking sub-group header expands it to show files", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((analysis) => {
      (window as any).__TEST_API__.setAnalysis(analysis);
    }, analysisWithSubGroups());
    await page.waitForTimeout(1000);

    // Expand infrastructure group
    await page.locator(".infra-group .group-header").click();
    await page.waitForTimeout(300);

    // Click "Schemas" sub-group header (2nd one)
    await page.locator(".infra-sub-group-header").nth(1).click();
    await page.waitForTimeout(300);

    // Files should now be visible in the Schemas sub-group
    const schemasGroup = page.locator(".infra-sub-group").nth(1);
    const files = schemasGroup.locator(".file-item");
    expect(await files.count()).toBe(2);
  });

  test("09 — clicking sub-group header again collapses it", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((analysis) => {
      (window as any).__TEST_API__.setAnalysis(analysis);
    }, analysisWithSubGroups());
    await page.waitForTimeout(1000);

    // Expand infrastructure group
    await page.locator(".infra-group .group-header").click();
    await page.waitForTimeout(300);

    // Expand first sub-group
    await page.locator(".infra-sub-group-header").first().click();
    await page.waitForTimeout(300);

    // Verify files visible
    const firstSubGroup = page.locator(".infra-sub-group").first();
    expect(await firstSubGroup.locator(".file-item").count()).toBeGreaterThan(0);

    // Collapse it
    await page.locator(".infra-sub-group-header").first().click();
    await page.waitForTimeout(300);

    // Files hidden
    expect(await firstSubGroup.locator(".file-item").count()).toBe(0);
  });

  test("10 — infra group header shows 'Ungrouped' label and total file count", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((analysis) => {
      (window as any).__TEST_API__.setAnalysis(analysis);
    }, analysisWithSubGroups());
    await page.waitForTimeout(1000);

    // The infra group header should show "Ungrouped" and "7 files"
    const header = page.locator(".infra-group .group-header");
    await expect(header.locator(".group-name")).toContainText("Ungrouped");
    await expect(header.locator(".risk-badge")).toContainText("7 files");
  });

  test("11 — multiple sub-groups can be expanded simultaneously", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((analysis) => {
      (window as any).__TEST_API__.setAnalysis(analysis);
    }, analysisWithSubGroups());
    await page.waitForTimeout(1000);

    // Expand infrastructure group
    await page.locator(".infra-group .group-header").click();
    await page.waitForTimeout(300);

    // Expand first sub-group (Infrastructure - 3 files)
    await page.locator(".infra-sub-group-header").nth(0).click();
    await page.waitForTimeout(300);

    // Expand second sub-group (Schemas - 2 files)
    await page.locator(".infra-sub-group-header").nth(1).click();
    await page.waitForTimeout(300);

    // Both should show files
    const allFiles = page.locator(".infra-sub-group .file-item");
    expect(await allFiles.count()).toBe(5); // 3 + 2
  });
});
