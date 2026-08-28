import { test, expect, type Page } from "@playwright/test";

/**
 * Regression cover for the PR/MR URL flow in the repository field.
 *
 * Both bugs these tests pin were shipped and hot-fixed:
 *  - analysis started in the same tick as setRepoPath, so every callback it
 *    triggered still closed over the URL instead of the resolved checkout
 *    ("Failed to load diff for …: Invalid repo path");
 *  - loadRepoInfo then auto-detected branches over the resolved refs, so the
 *    selectors silently reverted to the checkout's default branch.
 *
 * In demo mode `resolve_pr_url` returns MOCK_RESOLVED_PR, whose refs are
 * deliberately unlike MOCK_REPO_INFO's branches — so a regression in either
 * direction is visible in the branch selectors.
 */

const PR_URL = "https://github.com/rust-lang/rust/pull/12345";
const RESOLVED_PATH = "/demo/repo-pr";
const RESOLVED_BASE = "a1b2c3d";
const RESOLVED_HEAD = "pr-42";

async function waitForAnalysis(page: Page) {
  await expect(page.locator(".group-item").first()).toBeVisible({ timeout: 15000 });
}

async function submitRepoUrl(page: Page, url: string) {
  const input = page.locator(".repo-input");
  await input.fill(url);
  await input.press("Enter");
}

test.describe("PR/MR URL in the repository field", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("resolves the URL to a local checkout and analyzes it", async ({ page }) => {
    await submitRepoUrl(page, PR_URL);

    // The field is replaced by the resolved checkout, never left as the URL.
    await expect(page.locator(".repo-input")).toHaveValue(RESOLVED_PATH);
    await waitForAnalysis(page);
  });

  test("keeps the resolved base and head refs", async ({ page }) => {
    await submitRepoUrl(page, PR_URL);
    await expect(page.locator(".repo-input")).toHaveValue(RESOLVED_PATH);

    const head = page.locator('[data-testid="head-branch-dropdown"] .branch-name');
    const base = page.locator('[data-testid="base-branch-dropdown"] .branch-name');

    // loadRepoInfo lands asynchronously after the repoPath change, and the bug
    // was that it then auto-detected over these refs. Asserting straight away
    // would just race it and pass either way, so first wait for proof that it
    // has completed: the branch list is only populated from its result.
    await page.locator('[data-testid="base-branch-dropdown"] .branch-dropdown-trigger').click();
    await expect(
      page
        .locator('[data-testid="base-branch-dropdown"] .branch-option-name')
        .filter({ hasText: "develop" }),
    ).toBeVisible();

    // Now the assertion means something: the PR's refs outlived auto-detection.
    await expect(base).toHaveText(RESOLVED_BASE);
    await expect(head).toHaveText(RESOLVED_HEAD);
  });

  test("selecting a file after resolving loads its diff", async ({ page }) => {
    await submitRepoUrl(page, PR_URL);
    await waitForAnalysis(page);

    // This is the exact call that failed: get_file_diff received the URL
    // rather than the checkout and reported "Invalid repo path".
    await page.locator(".file-item").first().click();
    await expect(page.locator(".error-bar")).toHaveCount(0);
  });

  test("editing the field by hand restores branch auto-detection", async ({ page }) => {
    await submitRepoUrl(page, PR_URL);
    await expect(page.locator(".repo-input")).toHaveValue(RESOLVED_PATH);

    // Typing a plain path means the user is choosing a repo again, so the
    // PR refs must stop being pinned.
    await submitRepoUrl(page, "/demo/repo");
    await waitForAnalysis(page);
    await expect(
      page.locator('[data-testid="base-branch-dropdown"] .branch-name'),
    ).toHaveText("main");
  });

  test("a plain path is not treated as a URL", async ({ page }) => {
    await submitRepoUrl(page, "/demo/repo");
    await expect(page.locator(".repo-input")).toHaveValue("/demo/repo");
    await waitForAnalysis(page);
  });
});
