/**
 * Bugfixes — Playwright E2E tests.
 *
 * Covers fixes for:
 * - Keyboard shortcuts work when Monaco editor has focus
 * - "Cannot edit in read-only editor" tooltip suppressed
 * - Flow graph fullscreen node click exits fullscreen and navigates
 * - Flow graph re-centers when entering fullscreen
 * - Open With dropdown shows editor icons
 * - c key captures Monaco selection for code-level comments
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

/** Click into the Monaco editor to give it focus. */
async function focusMonacoEditor(page: Page) {
  await page.evaluate(() => {
    const editors = (window as any).monaco?.editor?.getEditors?.() ?? [];
    for (const editor of editors) {
      const target = typeof editor.getModifiedEditor === "function"
        ? editor.getModifiedEditor()
        : editor;
      if (target?.focus) {
        target.focus();
        return;
      }
    }
  });
  await page.waitForTimeout(300);
}

// ── Tests ──

test.describe("Bugfix — Keyboard shortcuts with Monaco focus", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("01 — j/k navigation works when Monaco editor is focused", async ({ page }) => {
    // Record the initially selected file
    const initialFile = await page.locator(".file-item.selected").textContent();

    // Focus Monaco editor
    await focusMonacoEditor(page);

    // Press j to navigate to the next file
    await page.keyboard.press("j");
    await page.waitForTimeout(500);

    // The selected file should have changed
    const nextFile = await page.locator(".file-item.selected").textContent();
    expect(nextFile).not.toBe(initialFile);

    // Press k to go back
    await page.keyboard.press("k");
    await page.waitForTimeout(500);

    const backFile = await page.locator(".file-item.selected").textContent();
    expect(backFile).toBe(initialFile);
  });

  test("02 — J/K group navigation works when Monaco editor is focused", async ({ page }) => {
    const initialGroup = await page.locator(".group-item.selected .group-name").textContent();

    await focusMonacoEditor(page);

    // Press J to navigate to the next group
    await page.keyboard.press("J");
    await page.waitForTimeout(500);

    const nextGroup = await page.locator(".group-item.selected .group-name").textContent();
    expect(nextGroup).not.toBe(initialGroup);

    // Press K to go back
    await page.keyboard.press("K");
    await page.waitForTimeout(500);

    const backGroup = await page.locator(".group-item.selected .group-name").textContent();
    expect(backGroup).toBe(initialGroup);
  });

  test("03 — 'Cannot edit in read-only editor' tooltip not shown", async ({ page }) => {
    await focusMonacoEditor(page);

    // Press j (which would trigger the tooltip before the fix)
    await page.keyboard.press("j");
    await page.waitForTimeout(500);

    // The read-only message widget should not be visible
    const readOnlyMessage = page.locator(".monaco-editor-overlaymessage");
    await expect(readOnlyMessage).not.toBeVisible();
  });

  test("04 — x toggles reviewed state from Monaco focus", async ({ page }) => {
    await focusMonacoEditor(page);

    // Press x to toggle reviewed
    await page.keyboard.press("x");
    await page.waitForTimeout(300);

    // The group should now show a reviewed indicator
    const reviewedGroup = page.locator(".group-item.selected .group-review-check");
    await expect(reviewedGroup).toHaveClass(/checked/);

    // Toggle it back
    await page.keyboard.press("x");
    await page.waitForTimeout(300);
    await expect(reviewedGroup).not.toHaveClass(/checked/);
  });

  test("05 — r enters replay mode from Monaco focus", async ({ page }) => {
    await focusMonacoEditor(page);

    await page.keyboard.press("r");
    await page.waitForTimeout(500);

    // Replay bar should appear
    await expect(page.locator(".replay-bar")).toBeVisible();

    // Exit replay
    await page.keyboard.press("Escape");
    await page.waitForTimeout(300);
    await expect(page.locator(".replay-bar")).not.toBeVisible();
  });

  test("06 — arrow keys still work for scrolling inside Monaco", async ({ page }) => {
    await focusMonacoEditor(page);

    // Arrow keys should NOT trigger navigation — the selected file should stay the same
    const initialFile = await page.locator(".file-item.selected").textContent();

    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowUp");
    await page.waitForTimeout(300);

    const afterArrows = await page.locator(".file-item.selected").textContent();
    expect(afterArrows).toBe(initialFile);
  });
});

test.describe("Bugfix — Flow graph fullscreen", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("07 — fullscreen re-centers the graph (not stuck in top-left)", async ({ page }) => {
    // Enter fullscreen
    await page.locator(".flow-fullscreen-btn").click();
    await page.waitForTimeout(1000);

    await expect(page.locator(".flow-graph-fullscreen")).toBeVisible();

    // The React Flow viewport transform should center the graph.
    // Check that the first node is not stuck at the very top-left corner of the viewport.
    const node = page.locator(".flow-node").first();
    const box = await node.boundingBox();
    expect(box).not.toBeNull();

    // In a centered graph in a 1440x900 viewport, the first node should not be
    // within the top-left 50x50 corner (the bug placed it there).
    // With fitView padding=0.3, even in a small graph it should be offset.
    const viewportWidth = 1440;
    const viewportHeight = 900;
    // Node should be somewhat centered — at least not crammed into the extreme top-left
    const centerThreshold = 0.1; // node shouldn't be in the extreme edge
    expect(box!.x).toBeGreaterThan(viewportWidth * centerThreshold);
    expect(box!.y).toBeGreaterThan(viewportHeight * centerThreshold);

    // Exit fullscreen
    await page.keyboard.press("Escape");
    await page.waitForTimeout(500);
  });

  test("08 — clicking a node in fullscreen exits fullscreen and navigates to file", async ({ page }) => {
    // Enter fullscreen
    await page.locator(".flow-fullscreen-btn").click();
    await page.waitForTimeout(1000);
    await expect(page.locator(".flow-graph-fullscreen")).toBeVisible();

    // Click on a flow node
    const firstNode = page.locator(".flow-node").first();
    const nodeName = await firstNode.locator(".flow-node-label").textContent();
    await firstNode.click();
    await page.waitForTimeout(800);

    // Fullscreen should have exited
    await expect(page.locator(".flow-graph-fullscreen")).not.toBeVisible();

    // The file should now be selected in the file list
    if (nodeName) {
      const selectedFile = page.locator(".file-item.selected");
      await expect(selectedFile).toContainText(nodeName);
    }
  });
});

test.describe("Bugfix — Open With dropdown", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("09 — Open With dropdown shows editor icons", async ({ page }) => {
    // Click the dropdown arrow
    const arrow = page.locator(".open-with-arrow");
    await arrow.click();
    await page.waitForTimeout(300);

    // Dropdown should be visible
    const dropdown = page.locator(".open-with-dropdown");
    await expect(dropdown).toBeVisible();

    // Each option should contain an SVG icon
    const options = dropdown.locator(".open-with-option");
    const count = await options.count();
    expect(count).toBeGreaterThan(0);

    for (let i = 0; i < count; i++) {
      const icon = options.nth(i).locator(".editor-icon svg");
      await expect(icon).toBeVisible();
    }
  });

  test("10 — Open With dropdown has correct editor labels", async ({ page }) => {
    await page.locator(".open-with-arrow").click();
    await page.waitForTimeout(300);

    const dropdown = page.locator(".open-with-dropdown");
    // In demo mode, all editors shown (no Tauri detection)
    const allLabels = ["VS Code", "Cursor", "Zed", "Vim", "Terminal"];

    for (const label of allLabels) {
      await expect(dropdown.getByText(label)).toBeVisible();
    }
  });
});

test.describe("Bugfix — Comment with Monaco selection", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await waitForAnalysis(page);
  });

  test("11 — c key opens comment input (basic, no selection)", async ({ page }) => {
    await page.keyboard.press("c");
    await page.waitForTimeout(300);

    const overlay = page.locator(".comment-overlay");
    await expect(overlay).toBeVisible();

    // Should be a file-level comment (file is auto-selected)
    const scope = page.locator(".comment-input-scope");
    await expect(scope).toBeVisible();

    // Cancel
    await page.keyboard.press("Escape");
    await page.waitForTimeout(200);
    await expect(overlay).not.toBeVisible();
  });

  test("12 — c key from Monaco focus opens comment input", async ({ page }) => {
    // Focus Monaco first
    await focusMonacoEditor(page);

    // Press c — should open comment input, not be swallowed
    await page.keyboard.press("c");
    await page.waitForTimeout(300);

    const overlay = page.locator(".comment-overlay");
    await expect(overlay).toBeVisible();

    await page.keyboard.press("Escape");
  });

  test("13 — single-line text selection in Monaco creates a code-level comment, not file-level", async ({ page }) => {
    // Set up: select a partial range of text on a single line in the modified editor.
    // The user's reported bug: with text highlighted on one line, pressing 'c'
    // currently falls through to a file-level comment instead of a code-level one
    // bound to the selected text.
    await page.waitForFunction(() => {
      const eds = (window as any).monaco?.editor?.getEditors?.() ?? [];
      return eds.some((e: any) => (e.getModel?.()?.getLineCount?.() ?? 0) >= 1
        && (e.getModel?.()?.getLineContent?.(1)?.trim().length ?? 0) > 0);
    }, { timeout: 15_000 });

    const sel = await page.evaluate(() => {
      const monacoEditors = (window as any).monaco?.editor?.getEditors?.() ?? [];
      // Pick the editor with the most populated content (modified side of diff).
      let best: any = null;
      let bestLines = 0;
      for (const ed of monacoEditors) {
        const target = typeof ed.getModifiedEditor === "function"
          ? ed.getModifiedEditor()
          : ed;
        const model = target?.getModel?.();
        if (!model) continue;
        const lineCount = model.getLineCount?.() ?? 0;
        if (lineCount > bestLines) {
          bestLines = lineCount;
          best = target;
        }
      }
      if (!best) return null;
      const model = best.getModel();
      const lineCount = model.getLineCount();
      for (let line = 1; line <= lineCount; line++) {
        const content: string = model.getLineContent(line);
        if (content && content.trim().length >= 5) {
          const startCol = 1;
          const endCol = Math.min(content.length + 1, startCol + 5);
          best.focus?.();
          best.setSelection({
            startLineNumber: line,
            startColumn: startCol,
            endLineNumber: line,
            endColumn: endCol,
          });
          return {
            line,
            startCol,
            endCol,
            expectedText: content.slice(startCol - 1, endCol - 1),
          };
        }
      }
      return null;
    });

    expect(sel, "could not find a Monaco line with content to select").not.toBeNull();
    await page.waitForTimeout(150);

    // Press 'c' to open a comment with the selection captured.
    await page.keyboard.press("c");
    await page.waitForTimeout(300);

    const overlay = page.locator(".comment-overlay");
    await expect(overlay).toBeVisible();

    // The comment input should be code-scoped, anchored to the selected line,
    // with selected_code matching the text the user highlighted.
    const input = await page.evaluate(() => (window as any).__TEST_API__.getCommentInput());
    expect(input, "comment input should exist after pressing c").not.toBeNull();
    expect(input.type).toBe("code");
    expect(input.start_line).toBe(sel!.line);
    expect(input.end_line).toBe(sel!.line);
    expect(input.selected_code).toBe(sel!.expectedText);
    expect(input.file_path).toBeTruthy();
  });

  test("14 — multi-line text selection in Monaco creates a code-level comment with full range", async ({ page }) => {
    // Control case: multi-line selections should also create code-level comments.
    // (This path already works — it guards against regressions while we fix the
    // single-line case.)
    await page.waitForFunction(() => {
      const eds = (window as any).monaco?.editor?.getEditors?.() ?? [];
      return eds.some((e: any) => (e.getModel?.()?.getLineCount?.() ?? 0) >= 3);
    }, { timeout: 15_000 });

    const sel = await page.evaluate(() => {
      const monacoEditors = (window as any).monaco?.editor?.getEditors?.() ?? [];
      let best: any = null;
      let bestLines = 0;
      for (const ed of monacoEditors) {
        const target = typeof ed.getModifiedEditor === "function"
          ? ed.getModifiedEditor()
          : ed;
        const model = target?.getModel?.();
        if (!model) continue;
        const lineCount = model.getLineCount?.() ?? 0;
        if (lineCount > bestLines) {
          bestLines = lineCount;
          best = target;
        }
      }
      if (!best || bestLines < 3) return null;
      const model = best.getModel();
      const startLine = 1;
      const endLine = Math.min(3, bestLines);
      best.focus?.();
      best.setSelection({
        startLineNumber: startLine,
        startColumn: 1,
        endLineNumber: endLine,
        endColumn: 1,
      });
      const lines: string[] = [];
      for (let i = startLine; i <= endLine; i++) lines.push(model.getLineContent(i));
      return { startLine, endLine, expectedText: lines.join("\n") };
    });

    expect(sel, "could not find a Monaco editor with >=3 lines").not.toBeNull();
    await page.waitForTimeout(150);

    await page.keyboard.press("c");
    await page.waitForTimeout(300);

    await expect(page.locator(".comment-overlay")).toBeVisible();

    const input = await page.evaluate(() => (window as any).__TEST_API__.getCommentInput());
    expect(input.type).toBe("code");
    expect(input.start_line).toBe(sel!.startLine);
    expect(input.end_line).toBe(sel!.endLine);
    expect(input.selected_code).toBe(sel!.expectedText);
  });
});
