/**
 * Group review metadata — Playwright E2E tests.
 *
 * Covers the desktop surface from specs/group-metadata.md §6.1:
 * - group_type / risk chips in the left scan list, and no description there
 * - the fixed-layout summary block in the right panel
 * - absent fields omitted entirely, with the block holding its height
 * - review_focus truncating instead of wrapping
 */
import { test, expect, type Page, type Locator } from "@playwright/test";

async function waitForAnalysis(page: Page) {
  await expect(page.locator(".summary")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator(".group-item.selected .file-list")).toBeVisible({ timeout: 5_000 });
}

/** The group with every metadata field populated. */
function fullGroup(page: Page): Locator {
  return page.locator(".group-item", { hasText: "POST /api/users creation flow" });
}

/** The group with only the heuristic floor: risk, group_type, impact. */
function floorGroup(page: Page): Locator {
  return page.locator(".group-item", { hasText: "Email notification worker" });
}

test.describe("Group metadata — left list", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("01 — group header shows type and risk chips", async ({ page }) => {
    const chips = fullGroup(page).locator(".group-meta-row .group-meta-chip");
    await expect(chips).toHaveText(["FEAT", "HIGH"]);
  });

  test("02 — floor-only group still shows both chips", async ({ page }) => {
    const chips = floorGroup(page).locator(".group-meta-row .group-meta-chip");
    await expect(chips).toHaveText(["CHORE", "LOW"]);
  });

  test("03 — risk chip carries the level for colouring", async ({ page }) => {
    await expect(
      fullGroup(page).locator(".group-meta-chip-risk"),
    ).toHaveAttribute("data-risk", "high");
    await expect(
      page.locator(".group-item", { hasText: "auth/refresh" }).locator(".group-meta-chip-risk"),
    ).toHaveAttribute("data-risk", "critical");
  });

  test("04 — description never appears in the scan list", async ({ page }) => {
    await expect(page.locator(".panel-left")).not.toContainText(
      "Add user creation with validation",
    );
  });
});

test.describe("Group metadata — right panel", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("05 — verdict row reads type, risk, impact, complexity", async ({ page }) => {
    const verdict = page.getByTestId("group-review-meta").locator(".review-meta-verdict");
    await expect(verdict.locator(".review-meta-item")).toHaveText([
      "FEAT",
      "HIGH RISK",
      "CROSS-CUTTING",
      "COMPLEX",
    ]);
  });

  test("06 — focus, description and invariant render as plain text", async ({ page }) => {
    const meta = page.getByTestId("group-review-meta");
    await expect(meta.locator(".review-meta-focus")).toHaveText(
      "Focus: correctness, security, data-integrity",
    );
    await expect(meta.locator(".review-meta-description")).toHaveText(
      "Add user creation with validation and a persisted audit trail.",
    );
    await expect(meta.locator(".review-meta-invariant")).toHaveText(
      "Invariant: A user row and its audit entry must be written in the same transaction.",
    );
  });

  test("07 — absent fields are omitted, no placeholder and no Invariant label", async ({ page }) => {
    await floorGroup(page).locator(".group-name").click();
    const meta = page.getByTestId("group-review-meta");

    await expect(meta.locator(".review-meta-item")).toHaveText(["CHORE", "LOW RISK", "LOCAL"]);
    await expect(meta.locator(".review-meta-focus")).toHaveText("");
    await expect(meta.locator(".review-meta-description")).toHaveText("");
    await expect(meta.locator(".review-meta-invariant")).toHaveText("");
    await expect(meta).not.toContainText("Invariant:");
    await expect(meta).not.toContainText("Focus:");
    await expect(meta).not.toContainText("—");
  });

  test("08 — block keeps the same height across groups with different fields", async ({ page }) => {
    const meta = page.getByTestId("group-review-meta");
    const withEverything = await meta.boundingBox();

    await floorGroup(page).locator(".group-name").click();
    await expect(meta.locator(".review-meta-item").first()).toHaveText("CHORE");
    const withFloorOnly = await meta.boundingBox();

    expect(withEverything).not.toBeNull();
    expect(withFloorOnly).not.toBeNull();
    expect(withFloorOnly!.height).toBe(withEverything!.height);
    expect(withFloorOnly!.y).toBe(withEverything!.y);
  });

  test("09 — review focus truncates rather than wrapping", async ({ page }) => {
    const focus = page.getByTestId("group-review-meta").locator(".review-meta-focus");

    await expect(focus).toHaveCSS("white-space", "nowrap");
    await expect(focus).toHaveCSS("text-overflow", "ellipsis");

    const threeEntries = (await focus.boundingBox())!.height;

    await page.locator(".group-item", { hasText: "auth/refresh" }).locator(".group-name").click();
    await expect(focus).toHaveText("Focus: security, concurrency");
    const twoEntries = (await focus.boundingBox())!.height;

    expect(threeEntries).toBe(twoEntries);
  });

  test("10 — existing group detail still renders below the block", async ({ page }) => {
    const panel = page.locator(".panel-right");
    await expect(panel.getByTestId("annotations-panel")).toContainText("POST /api/users");

    const metaY = (await panel.getByTestId("group-review-meta").boundingBox())!.y;
    const detailY = (await panel.getByTestId("annotations-panel").boundingBox())!.y;
    expect(metaY).toBeLessThan(detailY);
  });

  test("11 — a multi-point summary renders as bullets under the fixed block", async ({ page }) => {
    const summary = page.getByTestId("group-summary");

    await expect(summary.locator(".group-summary-list li")).toHaveCount(3);
    await expect(summary.locator(".group-summary-text")).toHaveCount(0);

    const metaY = (await page.getByTestId("group-review-meta").boundingBox())!.y;
    const summaryY = (await summary.boundingBox())!.y;
    expect(metaY).toBeLessThan(summaryY);
  });

  test("12 — a single-point summary renders as prose, not a one-item list", async ({ page }) => {
    await page.locator(".group-item", { hasText: "auth/refresh" }).locator(".group-name").click();

    const summary = page.getByTestId("group-summary");
    await expect(summary.locator(".group-summary-text")).toBeVisible();
    await expect(summary.locator(".group-summary-list")).toHaveCount(0);
  });

  test("13 — a group with no summary renders no summary section", async ({ page }) => {
    await floorGroup(page).locator(".group-name").click();
    await expect(page.getByTestId("group-summary")).toHaveCount(0);
  });

  test("13b — descriptions survive a refinement instead of vanishing with the old groups", async ({ page }) => {
    await expect(page.getByTestId("group-summary")).toBeVisible();
    const before = await page.locator(".review-meta-description").textContent();

    await page.getByTestId("refine-btn").click();
    await page.waitForTimeout(2000);
    await page.getByTestId("annotations-tab").click();

    // Refinement rebuilds groups from scratch; the review metadata has to come
    // back with them rather than leaving the panel bare until another click.
    await expect(page.getByTestId("group-review-meta")).toBeVisible();
    await expect(page.locator(".review-meta-description")).not.toBeEmpty();
    await expect(page.getByTestId("group-summary")).toBeVisible();
    expect(before).not.toBeNull();
  });

  test("13c — the refinement rationale stays collapsed instead of burying the group", async ({ page }) => {
    await page.getByTestId("refine-btn").click();
    await page.waitForTimeout(2000);
    await page.getByTestId("annotations-tab").click();

    const verdict = page.getByTestId("refinement-verdict");
    await expect(verdict).toBeVisible();

    // The one-line verdict is glanceable; the prose behind it is not, so it
    // must not be competing with the group's own summary for attention.
    const reasoning = verdict.locator(".refinement-verdict-reasoning");
    await expect(reasoning).toBeHidden();

    const metaY = (await page.getByTestId("group-review-meta").boundingBox())!.y;
    const verdictY = (await verdict.boundingBox())!.y;
    expect(metaY).toBeLessThan(verdictY);

    await verdict.locator("summary").click();
    await expect(reasoning).toBeVisible();
  });

  test("14 — the PR-level overall summary stays out of the group panel", async ({ page }) => {
    await expect(page.getByTestId("group-review-meta")).toBeVisible();
    await expect(page.getByTestId("pr-overview")).toHaveCount(0);
  });
});
