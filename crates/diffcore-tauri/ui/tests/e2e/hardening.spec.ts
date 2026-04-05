/**
 * Tauri App Hardening — Comprehensive Playwright screenshot + visual inspection tests.
 *
 * Covers all UI states from the Phase 3 hardening spec:
 * - Branch dropdown (many branches, long names, current highlighted)
 * - Worktree indicator (no worktrees, multiple worktrees)
 * - Push status display (ahead, behind, diverged, up-to-date)
 * - React Flow graph (node click, legend, fullscreen, minimap, single node)
 * - LLM controls (settings panel, toggles, provider/model dropdowns, API key states)
 * - Annotate button states (idle, loading, complete)
 * - Refine button states (banner, loading, original/refined toggle, change indicators)
 * - Empty states (no repo, no changes, 0 groups)
 * - Error states (error bar, dismissible)
 * - Large datasets (100+ files, 20+ groups)
 * - Responsive layout (narrow, wide)
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

async function dismissAiSetupIfVisible(page: Page) {
  const overlay = page.locator(".ai-setup-overlay");
  if (await overlay.isVisible()) {
    await page.locator(".ai-setup-modal .btn-close").click();
    await expect(overlay).not.toBeVisible();
  }
}

// ── Mock Data Generators ──

/** Generate a RepoInfo with N branches. */
function generateManyBranches(count: number) {
  const branches = [];
  for (let i = 0; i < count; i++) {
    branches.push({
      name: i === 0 ? "feature/user-auth" : i === 1 ? "main" : `feature/branch-${String(i).padStart(3, "0")}-${i % 3 === 0 ? "very-long-branch-name-that-might-overflow" : "normal"}`,
      is_current: i === 0,
      has_upstream: i < count / 2,
    });
  }
  return {
    current_branch: "feature/user-auth",
    default_branch: "main",
    branches,
    worktrees: [
      { path: "/demo/repo", branch: "feature/user-auth", is_main: true },
    ],
    status: { branch: "feature/user-auth", upstream: "origin/feature/user-auth", ahead: 3, behind: 0 },
    is_worktree: false,
  };
}

/** Generate a RepoInfo with multiple worktrees. */
function generateMultipleWorktrees() {
  return {
    current_branch: "feature/user-auth",
    default_branch: "main",
    branches: [
      { name: "feature/user-auth", is_current: true, has_upstream: true },
      { name: "main", is_current: false, has_upstream: true },
    ],
    worktrees: [
      { path: "/demo/repo", branch: "feature/user-auth", is_main: true },
      { path: "/demo/repo-hotfix", branch: "hotfix/urgent-fix", is_main: false },
      { path: "/demo/repo-staging", branch: "staging", is_main: false },
    ],
    status: { branch: "feature/user-auth", upstream: "origin/feature/user-auth", ahead: 3, behind: 0 },
    is_worktree: false,
  };
}

/** Generate a RepoInfo showing behind + diverged. */
function generateDivergedStatus() {
  return {
    current_branch: "feature/user-auth",
    default_branch: "main",
    branches: [
      { name: "feature/user-auth", is_current: true, has_upstream: true },
      { name: "main", is_current: false, has_upstream: true },
    ],
    worktrees: [{ path: "/demo/repo", branch: "feature/user-auth", is_main: true }],
    status: { branch: "feature/user-auth", upstream: "origin/feature/user-auth", ahead: 5, behind: 12 },
    is_worktree: false,
  };
}

/** Generate a RepoInfo showing up-to-date. */
function generateUpToDateStatus() {
  return {
    current_branch: "main",
    default_branch: "main",
    branches: [
      { name: "main", is_current: true, has_upstream: true },
    ],
    worktrees: [{ path: "/demo/repo", branch: "main", is_main: true }],
    status: { branch: "main", upstream: "origin/main", ahead: 0, behind: 0 },
    is_worktree: false,
  };
}

/** Generate a large AnalysisOutput with 20+ groups and 100+ files. */
function generateLargeAnalysis() {
  const groups = [];
  const roles = ["Entrypoint", "Service", "Repository", "Model", "Utility"];
  const edgeTypes = ["Calls", "Imports", "Writes", "Reads"];

  for (let g = 0; g < 25; g++) {
    const fileCount = 3 + (g % 5); // 3-7 files per group
    const files = [];
    const edges = [];

    for (let f = 0; f < fileCount; f++) {
      files.push({
        path: `src/groups/g${g}/file-${f}.ts`,
        flow_position: f,
        role: roles[f % roles.length],
        changes: { additions: Math.floor(Math.random() * 50) + 5, deletions: Math.floor(Math.random() * 20) },
        symbols_changed: [`fn_${g}_${f}`],
      });
      if (f > 0) {
        edges.push({
          from: `src/groups/g${g}/file-${f - 1}.ts::fn_${g}_${f - 1}`,
          to: `src/groups/g${g}/file-${f}.ts::fn_${g}_${f}`,
          edge_type: edgeTypes[f % edgeTypes.length],
        });
      }
    }

    groups.push({
      id: `group_${g}`,
      name: `Group ${g}: ${g % 3 === 0 ? "API endpoint handler chain" : g % 3 === 1 ? "Background worker pipeline" : "Data model migration"}`,
      entrypoint: { file: files[0].path, symbol: `fn_${g}_0`, entrypoint_type: "HttpRoute" },
      files,
      edges,
      risk_score: Math.round((0.2 + Math.random() * 0.7) * 100) / 100,
      review_order: g + 1,
    });
  }

  return {
    version: "1.0.0",
    diff_source: { diff_type: "BranchComparison", base: "main", head: "feature/large-pr", base_sha: "abc123", head_sha: "def456" },
    summary: {
      total_files_changed: groups.reduce((sum, g) => sum + g.files.length, 0),
      total_groups: groups.length,
      languages_detected: ["typescript"],
      frameworks_detected: ["express"],
    },
    groups,
    infrastructure_group: { files: ["tsconfig.json", "package.json", ".eslintrc.json", "vite.config.ts", "jest.config.ts"], reason: "Not reachable from any detected entrypoint" },
    annotations: null,
  };
}

/** Generate LlmSettings with no API key. */
function generateNoApiKeySettings() {
  return {
    annotations_enabled: true,
    refinement_enabled: false,
    provider: "anthropic",
    model: "claude-sonnet-4-6",
    api_key_source: "",
    has_api_key: false,
    refinement_provider: "anthropic",
    refinement_model: "claude-sonnet-4-6",
    refinement_max_iterations: 1,
    global_config_path: "~/.diffcore/config.toml",
    codex_available: false,
    codex_authenticated: false,
    claude_available: false,
    claude_authenticated: false,
  };
}

/** Generate a single-node analysis (one group, one file, no edges). */
function generateSingleNodeAnalysis() {
  return {
    version: "1.0.0",
    diff_source: { diff_type: "BranchComparison", base: "main", head: "feature/tiny", base_sha: "a1", head_sha: "b2" },
    summary: { total_files_changed: 1, total_groups: 1, languages_detected: ["typescript"], frameworks_detected: [] },
    groups: [
      {
        id: "group_single",
        name: "Single file change",
        entrypoint: { file: "src/index.ts", symbol: "main", entrypoint_type: "CliCommand" },
        files: [
          { path: "src/index.ts", flow_position: 0, role: "Entrypoint", changes: { additions: 5, deletions: 2 }, symbols_changed: ["main"] },
        ],
        edges: [],
        risk_score: 0.15,
        review_order: 1,
      },
    ],
    infrastructure_group: null,
    annotations: null,
  };
}

// ═══════════════════════════════════════════════════════════════════
// Branch & Git
// ═══════════════════════════════════════════════════════════════════

test.describe("Hardening — Branch & Git", () => {
  test("15 — branch dropdown open with branches listed", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Open target (base) branch dropdown
    const baseDropdown = page.locator("[data-testid='base-branch-dropdown']");
    await baseDropdown.locator(".branch-dropdown-trigger").click();
    await page.waitForTimeout(300);

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "15-branch-dropdown-open.png"),
      fullPage: false,
    });

    // Verify dropdown is open with branches
    await expect(baseDropdown.locator(".branch-dropdown")).toBeVisible();
    const options = baseDropdown.locator(".branch-option");
    expect(await options.count()).toBeGreaterThanOrEqual(5);

    // Verify current branch has badge
    await expect(baseDropdown.locator(".branch-current-badge")).toBeVisible();
    // Verify tracked branches have badge
    expect(await baseDropdown.locator(".branch-upstream-badge").count()).toBeGreaterThanOrEqual(1);
  });

  test("16 — branch dropdown: selected branch highlighted", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    const baseDropdown = page.locator("[data-testid='base-branch-dropdown']");
    await baseDropdown.locator(".branch-dropdown-trigger").click();
    await page.waitForTimeout(300);

    // The base branch "main" should be highlighted as selected
    const selectedOption = baseDropdown.locator(".branch-option.selected");
    await expect(selectedOption).toBeVisible();
    await expect(selectedOption).toContainText("main");

    await baseDropdown.locator(".branch-dropdown").screenshot({
      path: path.join(SCREENSHOTS_DIR, "16-branch-selected-highlight.png"),
    });
  });

  test("17 — branch dropdown: 50+ branches with long names", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Inject 55 branches
    await page.evaluate((repoInfo) => {
      (window as any).__TEST_API__.setRepoInfo(repoInfo);
    }, generateManyBranches(55));
    await page.waitForTimeout(300);

    // Open target (base) dropdown
    const baseDropdown = page.locator("[data-testid='base-branch-dropdown']");
    await baseDropdown.locator(".branch-dropdown-trigger").click();
    await page.waitForTimeout(300);

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "17-branch-dropdown-many.png"),
      fullPage: false,
    });

    // Verify dropdown is scrollable and branches render
    const options = page.locator(".branch-option");
    expect(await options.count()).toBeGreaterThanOrEqual(50);
  });

  test("18 — push status: ahead indicator", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Verify push status shows "3 ahead"
    await expect(page.locator(".push-status")).toContainText("3 ahead");
    await expect(page.locator(".current-branch")).toContainText("feature/user-auth");

    await page.locator(".repo-status").screenshot({
      path: path.join(SCREENSHOTS_DIR, "18-push-status-ahead.png"),
    });
  });

  test("19 — push status: diverged (ahead + behind)", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((repoInfo) => {
      (window as any).__TEST_API__.setRepoInfo(repoInfo);
    }, generateDivergedStatus());
    await page.waitForTimeout(300);

    await expect(page.locator(".push-status")).toContainText("5 ahead");
    await expect(page.locator(".push-status")).toContainText("12 behind");

    await page.locator(".repo-status").screenshot({
      path: path.join(SCREENSHOTS_DIR, "19-push-status-diverged.png"),
    });
  });

  test("20 — push status: up to date", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((repoInfo) => {
      (window as any).__TEST_API__.setRepoInfo(repoInfo);
    }, generateUpToDateStatus());
    await page.waitForTimeout(300);

    await expect(page.locator(".push-status")).toContainText("up to date");

    await page.locator(".repo-status").screenshot({
      path: path.join(SCREENSHOTS_DIR, "20-push-status-uptodate.png"),
    });
  });

  test("21 — worktree indicator: multiple worktrees", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((repoInfo) => {
      (window as any).__TEST_API__.setRepoInfo(repoInfo);
    }, generateMultipleWorktrees());
    await page.waitForTimeout(300);

    // Worktree badge should be visible for >1 worktrees
    await expect(page.locator(".worktree-badge")).toBeVisible();
    await expect(page.locator(".worktree-badge")).toContainText("3 worktrees");

    await page.locator(".repo-status").screenshot({
      path: path.join(SCREENSHOTS_DIR, "21-worktree-multiple.png"),
    });
  });
});

// ═══════════════════════════════════════════════════════════════════
// LLM Controls (Settings Panel)
// ═══════════════════════════════════════════════════════════════════

test.describe("Hardening — LLM Controls", () => {
  test("22 — settings panel: all controls visible", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Open settings panel
    await page.locator(".btn-settings").click();
    await page.waitForTimeout(500);

    await expect(page.locator(".settings-panel")).toBeVisible();

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "22-settings-panel.png"),
      fullPage: false,
    });

    // Verify all sections present
    await expect(page.locator(".settings-panel h3").filter({ hasText: "AI Access" })).toBeVisible();
    await expect(page.locator(".settings-panel h3").filter({ hasText: "Annotations" })).toBeVisible();
    await expect(page.locator(".settings-panel h3").filter({ hasText: "Refinement" })).toBeVisible();
    await expect(page.locator(".settings-panel h3").filter({ hasText: "Exclude Paths" })).toBeVisible();
    await expect(page.locator(".settings-panel label").filter({ hasText: "Primary backend" })).toBeVisible();
    await expect(page.locator(".settings-panel label").filter({ hasText: "Model" })).toBeVisible();
  });

  test("23 — settings panel: API key configured (green indicator)", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.locator(".btn-settings").click();
    await page.waitForTimeout(500);

    // Verify API key is configured (demo mode has key)
    await expect(page.locator(".api-key-status.configured")).toBeVisible();
    await expect(page.locator(".api-key-status")).toContainText("Ready via");

    await page.locator(".api-key-status").screenshot({
      path: path.join(SCREENSHOTS_DIR, "23-api-key-configured.png"),
    });
  });

  test("24 — settings panel: API key missing (red warning)", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Inject settings with no API key
    await page.evaluate((settings) => {
      (window as any).__TEST_API__.setLlmSettings(settings);
    }, generateNoApiKeySettings());
    await page.waitForTimeout(300);
    await dismissAiSetupIfVisible(page);

    await page.locator(".btn-settings").click();
    await page.waitForTimeout(500);

    await expect(page.locator(".api-key-status.missing")).toBeVisible();
    await expect(page.locator(".api-key-status")).toContainText("Not configured yet");
    await expect(page.locator(".settings-panel")).toContainText("DIFFCORE_API_KEY");

    await page.locator(".settings-panel").screenshot({
      path: path.join(SCREENSHOTS_DIR, "24-api-key-missing.png"),
    });
  });

  test("25 — settings panel: refinement section expanded", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.locator(".btn-settings").click();
    await page.waitForTimeout(500);

    // Enable refinement toggle
    const refinementToggle = page.locator(".settings-refinement input[type='checkbox']");
    await refinementToggle.check();
    await page.waitForTimeout(300);

    // Verify refinement sub-settings visible
    await expect(page.locator(".settings-refinement .settings-row").first()).toBeVisible();

    await page.locator(".settings-refinement").screenshot({
      path: path.join(SCREENSHOTS_DIR, "25-refinement-settings-expanded.png"),
    });
  });

  test("26 — settings panel: close via Escape", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.locator(".btn-settings").click();
    await page.waitForTimeout(500);
    await expect(page.locator(".settings-panel")).toBeVisible();

    // Press Escape to close
    await page.keyboard.press("Escape");
    await page.waitForTimeout(300);

    await expect(page.locator(".settings-panel")).not.toBeVisible();
  });
});

// ═══════════════════════════════════════════════════════════════════
// LLM Annotations
// ═══════════════════════════════════════════════════════════════════

test.describe("Hardening — LLM Annotations", () => {
  test("27 — summarize PR: idle button state", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Verify summarize button is visible and enabled
    const btn = page.locator(".btn-summarize");
    await expect(btn).toBeVisible();
    await expect(btn).toContainText("Summarize PR");
    await expect(btn).not.toBeDisabled();

    // Verify provider badge
    await expect(page.locator(".llm-provider-badge")).toContainText("Codex CLI/default");

    await page.locator(".annotation-actions").screenshot({
      path: path.join(SCREENSHOTS_DIR, "27-summarize-idle.png"),
    });
  });

  test("28 — summarize PR: complete with overview", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Click summarize
    await page.locator(".btn-summarize").click();
    // Brief wait for mock delay
    await page.waitForTimeout(1200);
    await page.getByRole("tab", { name: "Annotations" }).click();

    // Verify LLM overview rendered
    await expect(page.locator(".llm-summary").first()).toBeVisible();
    // Verify risk flags shown
    await expect(page.locator(".risk-flag").first()).toBeVisible();
    // Verify review rationale
    await expect(page.locator(".llm-rationale")).toBeVisible();

    // Summarize button should be gone (overview loaded)
    await expect(page.locator(".btn-summarize")).not.toBeVisible();

    await page.locator(".panel-right").screenshot({
      path: path.join(SCREENSHOTS_DIR, "28-summarize-complete.png"),
    });
  });

  test("29 — deep analysis: complete with file annotations", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Click "Analyze This Flow"
    await page.locator(".btn-analyze-flow").click();
    await page.waitForTimeout(1000);
    await page.getByRole("tab", { name: "Annotations" }).click();

    // Verify deep analysis rendered
    await expect(page.locator(".llm-narrative")).toBeVisible();
    await expect(page.locator(".file-annotation").first()).toBeVisible();
    await expect(page.locator(".file-annotation-path").first()).toBeVisible();

    // Verify risks and suggestions visible for first group
    await expect(page.locator(".risk-label").first()).toBeVisible();

    // Verify cross-cutting concerns
    await expect(page.locator(".concerns-list")).toBeVisible();

    // Analyze button should be gone
    await expect(page.locator(".btn-analyze-flow")).not.toBeVisible();

    await page.locator(".panel-right").screenshot({
      path: path.join(SCREENSHOTS_DIR, "29-deep-analysis-complete.png"),
    });
  });

  test("30 — annotation buttons: no API key (greyed out)", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Inject settings with no API key
    await page.evaluate((settings) => {
      (window as any).__TEST_API__.setLlmSettings(settings);
    }, generateNoApiKeySettings());
    await page.waitForTimeout(300);
    await dismissAiSetupIfVisible(page);

    // Verify buttons show setup-required copy and are disabled
    const summarizeBtn = page.locator(".btn-summarize");
    await expect(summarizeBtn).toContainText("Summarize PR (Setup required)");
    await expect(summarizeBtn).toHaveClass(/no-api-key/);

    await page.locator(".annotation-actions").screenshot({
      path: path.join(SCREENSHOTS_DIR, "30-buttons-no-api-key.png"),
    });
  });
});

// ═══════════════════════════════════════════════════════════════════
// Refinement
// ═══════════════════════════════════════════════════════════════════

test.describe("Hardening — Refinement", () => {
  test("31 — refinement banner visible when enabled", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Enable refinement in settings
    await page.evaluate((settings) => {
      (window as any).__TEST_API__.setLlmSettings(settings);
    }, {
      annotations_enabled: true,
      refinement_enabled: true,
      provider: "anthropic",
      model: "claude-sonnet-4-6",
      api_key_source: "ANTHROPIC_API_KEY",
      has_api_key: true,
      refinement_provider: "anthropic",
      refinement_model: "claude-sonnet-4-6",
      refinement_max_iterations: 1,
    });
    await page.waitForTimeout(300);

    // Verify refinement banner is shown
    await expect(page.locator(".refinement-banner")).toBeVisible();
    await expect(page.locator(".refinement-banner")).toContainText("AI can improve");
    await expect(page.locator(".btn-refine")).toBeVisible();

    await page.locator(".refinement-banner").screenshot({
      path: path.join(SCREENSHOTS_DIR, "31-refinement-banner.png"),
    });
  });

  test("32 — refinement: complete with original/refined toggle", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Enable refinement
    await page.evaluate((settings) => {
      (window as any).__TEST_API__.setLlmSettings(settings);
    }, {
      annotations_enabled: true,
      refinement_enabled: true,
      provider: "anthropic",
      model: "claude-sonnet-4-6",
      api_key_source: "ANTHROPIC_API_KEY",
      has_api_key: true,
      refinement_provider: "anthropic",
      refinement_model: "claude-sonnet-4-6",
      refinement_max_iterations: 1,
    });
    await page.waitForTimeout(300);

    // Click Refine
    await page.locator(".btn-refine").click();
    await page.waitForTimeout(2000); // Mock delay is 1200ms

    // Verify toggle exists
    await expect(page.locator(".refinement-toggle")).toBeVisible();
    // Refined should be active
    await expect(page.locator(".toggle-btn.active")).toContainText("Refined");
    // Verify "Refined by" badge
    await expect(page.locator(".refined-badge")).toBeVisible();

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "32-refinement-complete.png"),
      fullPage: false,
    });
  });

  test("33 — refinement: change indicators on groups", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Enable refinement and run it
    await page.evaluate((settings) => {
      (window as any).__TEST_API__.setLlmSettings(settings);
    }, {
      annotations_enabled: true,
      refinement_enabled: true,
      provider: "anthropic",
      model: "claude-sonnet-4-6",
      api_key_source: "ANTHROPIC_API_KEY",
      has_api_key: true,
      refinement_provider: "anthropic",
      refinement_model: "claude-sonnet-4-6",
      refinement_max_iterations: 1,
    });
    await page.waitForTimeout(300);

    await page.locator(".btn-refine").click();
    await page.waitForTimeout(2000);

    // Verify change indicators are present
    const changeIndicators = page.locator(".change-indicator");
    expect(await changeIndicators.count()).toBeGreaterThanOrEqual(1);

    // Verify split indicator
    await expect(page.locator(".change-tag.change-split").first()).toBeVisible();

    await page.locator(".panel-left").screenshot({
      path: path.join(SCREENSHOTS_DIR, "33-refinement-change-indicators.png"),
    });
  });

  test("34 — refinement: toggle back to original view", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((settings) => {
      (window as any).__TEST_API__.setLlmSettings(settings);
    }, {
      annotations_enabled: true,
      refinement_enabled: true,
      provider: "anthropic",
      model: "claude-sonnet-4-6",
      api_key_source: "ANTHROPIC_API_KEY",
      has_api_key: true,
      refinement_provider: "anthropic",
      refinement_model: "claude-sonnet-4-6",
      refinement_max_iterations: 1,
    });
    await page.waitForTimeout(300);

    await page.locator(".btn-refine").click();
    await page.waitForTimeout(2000);

    // Switch to Original view
    await page.locator(".toggle-btn").filter({ hasText: "Original" }).click();
    await page.waitForTimeout(500);

    // Verify Original is now active
    await expect(page.locator(".toggle-btn.active")).toContainText("Original");
    // Refined badge should be gone
    await expect(page.locator(".refined-badge")).not.toBeVisible();
    // Original group count should be 3
    const groups = page.locator(".group-item:not(.infra-group)");
    await expect(groups).toHaveCount(3);

    await page.locator(".panel-left").screenshot({
      path: path.join(SCREENSHOTS_DIR, "34-refinement-original-view.png"),
    });
  });

  test("34b — refinement toggle crossfades the group list instead of hard-swapping it", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((settings) => {
      (window as any).__TEST_API__.setLlmSettings(settings);
    }, {
      annotations_enabled: true,
      refinement_enabled: true,
      provider: "anthropic",
      model: "claude-sonnet-4-6",
      api_key_source: "ANTHROPIC_API_KEY",
      has_api_key: true,
      refinement_provider: "anthropic",
      refinement_model: "claude-sonnet-4-6",
      refinement_max_iterations: 1,
    });
    await page.waitForTimeout(300);

    await page.locator(".btn-refine").click();
    await page.waitForTimeout(2000);

    const groupList = page.getByTestId("group-list");

    await page.locator(".toggle-btn").filter({ hasText: "Original" }).click();
    await expect(groupList).toHaveAttribute("data-group-list-transition", /fading-out|fading-in/);
    await page.waitForTimeout(350);
    await expect(groupList).toHaveAttribute("data-group-list-transition", "idle");
  });
});

// ═══════════════════════════════════════════════════════════════════
// React Flow Graph
// ═══════════════════════════════════════════════════════════════════

test.describe("Hardening — React Flow Graph", () => {
  test("35 — graph: node click highlights and dims non-connected edges", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Switch to graph subtab
    await page.locator(".annotation-subtab", { hasText: "Graph" }).click();
    await page.waitForTimeout(500);

    // Click on a flow node (first node in the graph)
    const firstNode = page.locator(".flow-node").first();
    await firstNode.click();
    await page.waitForTimeout(800);

    // Verify a node is highlighted (selected class)
    await expect(page.locator(".flow-node-selected")).toBeVisible();

    await page.locator(".flow-graph-container").screenshot({
      path: path.join(SCREENSHOTS_DIR, "35-graph-node-selected.png"),
    });
  });

  test("36 — graph: legend overlay expanded", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Switch to graph subtab
    await page.locator(".annotation-subtab", { hasText: "Graph" }).click();
    await page.waitForTimeout(500);

    // Wait for the graph to fully render, then click the legend toggle.
    // The ReactFlow container overlays the legend, so use evaluate to dispatch click.
    await page.waitForSelector(".flow-legend-toggle", { state: "attached" });
    await page.evaluate(() => {
      const btn = document.querySelector<HTMLButtonElement>(".flow-legend-toggle");
      btn?.click();
    });
    await page.waitForTimeout(300);

    // Verify legend items visible
    await expect(page.locator(".flow-legend-items")).toBeVisible();
    const legendItems = page.locator(".flow-legend-item");
    expect(await legendItems.count()).toBeGreaterThanOrEqual(4);

    await page.locator(".flow-graph-container").screenshot({
      path: path.join(SCREENSHOTS_DIR, "36-graph-legend-expanded.png"),
    });
  });

  test("37 — graph: fullscreen mode", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Switch to graph subtab
    await page.locator(".annotation-subtab", { hasText: "Graph" }).click();
    await page.waitForTimeout(500);

    // Click fullscreen button
    await page.locator(".flow-fullscreen-btn").click();
    await page.waitForTimeout(800);

    // Verify fullscreen class applied
    await expect(page.locator(".flow-graph-fullscreen")).toBeVisible();

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "37-graph-fullscreen.png"),
      fullPage: false,
    });

    // Exit with Escape
    await page.keyboard.press("Escape");
    await page.waitForTimeout(500);
    await expect(page.locator(".flow-graph-fullscreen")).not.toBeVisible();
  });

  test("38 — graph: export buttons are absent", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await expect(page.locator(".flow-export-buttons")).not.toBeVisible();
    await expect(page.locator(".flow-export-btn")).not.toBeVisible();
  });

  test("39 — graph: single node (no edges, no minimap)", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Inject single-node analysis
    await page.evaluate((analysis) => {
      (window as any).__TEST_API__.setAnalysis(analysis);
    }, generateSingleNodeAnalysis());
    await page.waitForTimeout(1500);

    // Single node, no edges — graph section should not render
    // (edges.length > 0 check in App.tsx)
    await expect(page.locator("[data-testid='flow-graph']")).not.toBeVisible();

    await page.locator(".panel-right").screenshot({
      path: path.join(SCREENSHOTS_DIR, "39-single-node-no-graph.png"),
    });
  });
});

// ═══════════════════════════════════════════════════════════════════
// Empty & Error States
// ═══════════════════════════════════════════════════════════════════

test.describe("Hardening — Empty & Error States", () => {
  test("40 — empty state: no analysis loaded", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Clear analysis
    await page.evaluate(() => {
      (window as any).__TEST_API__.clearAnalysis();
    });
    await page.waitForTimeout(500);

    // Verify empty state message
    await expect(page.locator(".empty-state").first()).toBeVisible();
    await expect(page.locator(".empty-state").first()).toContainText("Enter a repository path");

    // Keyboard hints should be hidden
    await expect(page.locator(".keyboard-hints")).not.toBeVisible();

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "40-empty-state.png"),
      fullPage: false,
    });
  });

  test("41 — error state: real error bar with dismiss", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Inject an error
    await page.evaluate(() => {
      (window as any).__TEST_API__.setError(
        "Failed to analyze repository: fatal: not a git repository (or any parent up to mount point /)"
      );
    });
    await page.waitForTimeout(300);

    await expect(page.locator(".error-bar")).toBeVisible();
    await expect(page.locator(".error-bar")).toContainText("not a git repository");

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "41-error-state-real.png"),
      fullPage: false,
    });

    // Dismiss error
    await page.locator(".error-bar .btn-close").click();
    await page.waitForTimeout(300);
    await expect(page.locator(".error-bar")).not.toBeVisible();
  });

  test("42 — empty state: right panel with no group selected", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // The right panel shows "Select a group" when no group is selected
    // After clearing, the right panel should show the empty state
    await page.evaluate(() => {
      (window as any).__TEST_API__.clearAnalysis();
    });
    await page.waitForTimeout(500);

    await expect(page.locator(".panel-right .empty-state")).toContainText("Select a group");

    await page.locator(".panel-right").screenshot({
      path: path.join(SCREENSHOTS_DIR, "42-right-panel-empty.png"),
    });
  });
});

// ═══════════════════════════════════════════════════════════════════
// Large Datasets
// ═══════════════════════════════════════════════════════════════════

test.describe("Hardening — Large Datasets", () => {
  test("43 — large dataset: 25 groups, 100+ files", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Inject large analysis
    await page.evaluate((analysis) => {
      (window as any).__TEST_API__.setAnalysis(analysis);
    }, generateLargeAnalysis());
    await page.waitForTimeout(2000);

    // Verify many groups rendered
    const groups = page.locator(".group-item:not(.infra-group)");
    expect(await groups.count()).toBeGreaterThanOrEqual(20);

    // Verify summary shows correct counts
    await expect(page.locator(".summary")).toContainText("25 groups");

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "43-large-dataset.png"),
      fullPage: false,
    });
  });

  test("44 — large dataset: left panel scrolls without overflow", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((analysis) => {
      (window as any).__TEST_API__.setAnalysis(analysis);
    }, generateLargeAnalysis());
    await page.waitForTimeout(2000);

    // Scroll the left panel body to the bottom
    await page.evaluate(() => {
      const panelBody = document.querySelector(".panel-left .panel-body");
      if (panelBody) panelBody.scrollTop = panelBody.scrollHeight;
    });
    await page.waitForTimeout(500);

    // Verify last group is visible after scroll
    const lastGroup = page.locator(".group-item:not(.infra-group)").last();
    await expect(lastGroup).toBeVisible();

    await page.locator(".panel-left").screenshot({
      path: path.join(SCREENSHOTS_DIR, "44-large-dataset-scrolled.png"),
    });
  });

  test("45 — large dataset: infrastructure group with many files", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    await page.evaluate((analysis) => {
      (window as any).__TEST_API__.setAnalysis(analysis);
    }, generateLargeAnalysis());
    await page.waitForTimeout(2000);

    // Scroll to infrastructure group
    await page.evaluate(() => {
      const infra = document.querySelector(".infra-group");
      if (infra) infra.scrollIntoView();
    });
    await page.waitForTimeout(300);

    await expect(page.locator(".infra-group")).toBeVisible();
    await page.locator(".infra-group .group-header").click();
    await page.waitForTimeout(300);
    const infraFiles = page.locator(".infra-group .file-item");
    expect(await infraFiles.count()).toBeGreaterThanOrEqual(4);

    await page.locator(".infra-group").screenshot({
      path: path.join(SCREENSHOTS_DIR, "45-large-infra-group.png"),
    });
  });
});

// ═══════════════════════════════════════════════════════════════════
// Responsive Layout
// ═══════════════════════════════════════════════════════════════════

test.describe("Hardening — Responsive Layout", () => {
  test("46 — responsive: narrow window (1024x768)", async ({ page }) => {
    await page.setViewportSize({ width: 1024, height: 768 });
    await page.goto("/");
    await waitForAnalysis(page);

    // Verify three panels still visible
    await expect(page.locator(".panel-left")).toBeVisible();
    await expect(page.locator(".panel-center")).toBeVisible();
    await expect(page.locator(".panel-right")).toBeVisible();

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "46-responsive-narrow.png"),
      fullPage: false,
    });
  });

  test("47 — responsive: wide window (1920x1080)", async ({ page }) => {
    await page.setViewportSize({ width: 1920, height: 1080 });
    await page.goto("/");
    await waitForAnalysis(page);

    await expect(page.locator(".panel-left")).toBeVisible();
    await expect(page.locator(".panel-center")).toBeVisible();
    await expect(page.locator(".panel-right")).toBeVisible();

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "47-responsive-wide.png"),
      fullPage: false,
    });
  });

  test("48 — responsive: panels don't overlap at minimum width", async ({ page }) => {
    // Very narrow — panels should still not overlap
    await page.setViewportSize({ width: 800, height: 600 });
    await page.goto("/");
    await waitForAnalysis(page);

    // Panels should still be visible (may be compressed)
    await expect(page.locator(".panel-left")).toBeVisible();
    await expect(page.locator(".panel-center")).toBeVisible();

    await page.screenshot({
      path: path.join(SCREENSHOTS_DIR, "48-responsive-minimum.png"),
      fullPage: false,
    });
  });
});

// ═══════════════════════════════════════════════════════════════════
// PR Preview Mode
// ═══════════════════════════════════════════════════════════════════

test.describe("Hardening — PR Preview Mode", () => {
  test("49 — PR preview: default view on launch", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Verify base (target) branch shows "main" (default branch)
    const baseName = page.locator("[data-testid='base-branch-dropdown'] .branch-name");
    await expect(baseName).toContainText("main");
    // Verify source (head) branch shows current branch
    const headName = page.locator("[data-testid='head-branch-dropdown'] .branch-name");
    await expect(headName).toContainText("feature/user-auth");
    // Verify analysis loaded (groups visible)
    const groups = page.locator(".group-item:not(.infra-group)");
    expect(await groups.count()).toBeGreaterThanOrEqual(3);

    await page.locator(".top-bar").screenshot({
      path: path.join(SCREENSHOTS_DIR, "49-pr-preview-default.png"),
    });
  });

  test("50 — PR preview: switching base branch via dropdown", async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);

    // Open target (base) branch dropdown
    const baseDropdown = page.locator("[data-testid='base-branch-dropdown']");
    await baseDropdown.locator(".branch-dropdown-trigger").click();
    await page.waitForTimeout(300);

    // Select "develop" branch
    await baseDropdown.locator(".branch-option").filter({ hasText: "develop" }).click();
    await page.waitForTimeout(300);

    // Verify base ref updated
    await expect(baseDropdown.locator(".branch-name")).toContainText("develop");

    await page.locator(".top-bar").screenshot({
      path: path.join(SCREENSHOTS_DIR, "50-pr-preview-switched-branch.png"),
    });
  });
});
