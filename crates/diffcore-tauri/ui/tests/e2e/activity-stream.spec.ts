import { expect, test, type Page } from "@playwright/test";

async function waitForDemoApp(page: Page) {
  await page.goto("/");
  await expect(page.locator(".top-bar")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator(".summary")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator(".group-item.selected")).toBeVisible({ timeout: 10_000 });
}

async function setLlmSettings(page: Page, settings: Record<string, unknown>) {
  await page.evaluate((value) => {
    (window as { __TEST_API__: { setLlmSettings: (next: Record<string, unknown>) => void } }).__TEST_API__.setLlmSettings(value);
  }, settings);
}

async function setActivityEntries(page: Page, entries: Array<Record<string, unknown>>) {
  await page.evaluate((value) => {
    (
      window as {
        __TEST_API__: { setActivityEntries: (next: Array<Record<string, unknown>>) => void };
      }
    ).__TEST_API__.setActivityEntries(value);
  }, entries);
}

test.describe("AI activity stream", () => {
  test.beforeEach(async ({ page }) => {
    await waitForDemoApp(page);
  });

  test("shows a rich overview timeline and lets the user switch back to annotations", async ({ page }) => {
    await page.getByRole("button", { name: "Summarize PR" }).click();

    const panel = page.getByTestId("activity-panel");
    const log = page.getByTestId("activity-log");

    await expect(panel).toBeVisible();
    await expect(panel).toContainText("Summarizing PR");
    await expect(panel).toContainText("Codex CLI/default");
    await expect(page.getByTestId("activity-stats")).toContainText("events");
    await expect(log).toContainText("Preparing overview request");
    await expect(log).toContainText("Searching the repo");
    await expect(log).toContainText("Reading files");
    await expect(log).toContainText("Writing PR-ready summary");
    await expect(panel.locator(".activity-live-badge")).toHaveText("Saved");

    await page.getByTestId("annotations-tab").click();
    await expect(page.getByTestId("annotations-panel")).toBeVisible();
    await expect(page.getByTestId("annotations-panel")).toContainText("POST /api/users creation flow");
    await expect(page.getByRole("button", { name: "Copy PR Description" })).toBeVisible();
  });

  test("prefers Codex CLI over direct API settings for refinement when a local backend is ready", async ({ page }) => {
    await setLlmSettings(page, {
      annotations_enabled: true,
      refinement_enabled: true,
      provider: "openai",
      model: "gpt-5.4",
      api_key_source: "~/.diffcore/config.toml",
      has_api_key: true,
      refinement_provider: "openai",
      refinement_model: "gpt-5.4",
      refinement_max_iterations: 1,
      global_config_path: "~/.diffcore/config.toml",
      codex_available: true,
      codex_authenticated: true,
      claude_available: true,
      claude_authenticated: true,
    });

    await page.getByRole("button", { name: "Refine" }).click();

    const panel = page.getByTestId("activity-panel");
    const log = page.getByTestId("activity-log");

    await expect(panel).toBeVisible();
    await expect(panel).toContainText("Refining groups");
    await expect(panel).toContainText("Codex CLI/default");
    await expect(panel).toContainText("Live repo activity enabled");
    await expect(log).toContainText("Preparing refinement request");
    await expect(log).toContainText("Searching the repo");
    await expect(log).toContainText("Reading files");
    await expect(log).toContainText("Refinement rationale");
    await expect(panel).toContainText("Applied structural changes");
    await expect(panel).not.toContainText("Refinement finished without structural changes");
    await expect(page.getByRole("button", { name: "Refined" })).toBeVisible();
    await expect(panel.locator(".activity-live-badge")).toHaveText("Saved");
    await page.getByTestId("annotations-tab").click();
    await expect(page.locator(".llm-provider-badge")).toContainText("Codex CLI/default");
  });

  test("explains when a direct API provider cannot show file reads or grep activity", async ({ page }) => {
    await setLlmSettings(page, {
      annotations_enabled: true,
      refinement_enabled: true,
      provider: "openai",
      model: "gpt-5.4",
      api_key_source: "~/.diffcore/config.toml",
      has_api_key: true,
      refinement_provider: "openai",
      refinement_model: "gpt-5.4",
      refinement_max_iterations: 1,
      global_config_path: "~/.diffcore/config.toml",
      codex_available: false,
      codex_authenticated: false,
      claude_available: false,
      claude_authenticated: false,
    });

    await page.getByRole("button", { name: "Summarize PR" }).click();

    await expect(page.getByTestId("activity-direct-api-note")).toBeVisible();
    await expect(page.getByTestId("activity-direct-api-note")).toContainText("Direct API mode");
    await expect(page.getByTestId("activity-log")).toContainText("Submitting structured request to the API provider");
  });

  test("keeps the stream to the latest ten events and exposes the full history in All events", async ({ page }) => {
    const base = Date.now();
    await setActivityEntries(
      page,
      Array.from({ length: 12 }, (_, index) => ({
        source: index % 2 === 0 ? "codex" : "claude",
        level: "info",
        message: index % 3 === 0
          ? `Inspecting a file: crates/diffcore-tauri/ui/src/file-${index}.tsx`
          : `Codex is running sed -n '1,120p' crates/diffcore-tauri/ui/src/file-${index}.tsx`,
        event_type: index % 3 === 0 ? "stdout.read_file" : "stdout.command_execution",
        timestamp_ms: base + index,
      })),
    );

    const log = page.getByTestId("activity-log");
    await expect(log).toHaveAttribute("data-activity-view", "stream");
    await expect(log.getByTestId("activity-entry")).toHaveCount(10);
    await expect(log).toContainText("file-11.tsx");
    await expect(log).not.toContainText("file-0.tsx");

    await page.getByTestId("activity-view-all-tab").click();
    await expect(log).toHaveAttribute("data-activity-view", "all");
    await expect(log.getByTestId("activity-entry")).toHaveCount(12);
    await expect(log).toContainText("file-0.tsx");
  });

  test("shows event payloads without horizontal overflow in the right panel", async ({ page }) => {
    const base = Date.now();
    await setActivityEntries(page, [
      {
        source: "diffcore",
        level: "info",
        message: "Preparing refinement request",
        event_type: "diffcore.prepare",
        payload: { operation: "refinement", stage: "prepare" },
        timestamp_ms: base,
      },
      {
        source: "codex",
        level: "info",
        message: "Codex is running sed -n '1,120p' crates/diffcore-tauri/ui/src/App.tsx",
        event_type: "stdout.command_execution",
        payload: {
          command: "sed -n '1,120p' crates/diffcore-tauri/ui/src/App.tsx",
          path: "crates/diffcore-tauri/ui/src/App.tsx",
          cwd: "/demo/repo",
        },
        timestamp_ms: base + 1,
      },
    ]);

    const panel = page.locator(".panel-right");
    const logPanel = page.getByTestId("activity-log-panel");
    const latestCard = page.getByTestId("activity-entry").last();
    const inspector = page.getByTestId("activity-inspector");

    await latestCard.click();
    await expect(inspector).toContainText("stdout.command_execution");
    await expect(inspector).toContainText("\"path\": \"crates/diffcore-tauri/ui/src/App.tsx\"");
    await expect(page.getByTestId("activity-panel")).toContainText("Latest stream captured from Codex CLI");

    for (const locator of [panel, logPanel, page.locator(".activity-view-switch")]) {
      const hasOverflow = await locator.evaluate((element) => element.scrollWidth > element.clientWidth + 1);
      expect(hasOverflow).toBeFalsy();
    }
  });

  test("keeps stream cards readable instead of vertically squashing them", async ({ page }) => {
    await page.setViewportSize({ width: 1180, height: 780 });

    const base = Date.now();
    await setActivityEntries(page, [
      {
        source: "diffcore",
        level: "info",
        message: "Preparing refinement request",
        event_type: "diffcore.prepare",
        timestamp_ms: base,
      },
      {
        source: "diffcore",
        level: "info",
        message: "Using Codex CLI/default",
        event_type: "diffcore.model",
        timestamp_ms: base + 1,
      },
      {
        source: "codex",
        level: "info",
        message: "Codex is running rg --files /demo/repo/src | head -n 40",
        event_type: "stdout.command_execution",
        timestamp_ms: base + 2,
      },
      {
        source: "codex",
        level: "info",
        message: "Inspecting a file: /demo/repo/src/routes/users.ts",
        event_type: "stdout.read_file",
        timestamp_ms: base + 3,
      },
      {
        source: "codex",
        level: "info",
        message: "Refinement rationale: keep the current grouping because the changed files already form coherent review flows.",
        event_type: "refinement.reasoning",
        timestamp_ms: base + 4,
      },
    ]);

    await page.getByTestId("activity-entry").last().click();
    const cards = page.getByTestId("activity-entry");
    await expect(cards).toHaveCount(5);
    await expect(cards.first()).toContainText("Preparing refinement request");
    await expect(cards.last()).toContainText("Refinement rationale");

    const heights = await cards.evaluateAll((elements) =>
      elements.map((element) => Math.round(element.getBoundingClientRect().height)),
    );
    expect(heights.every((height) => height >= 78)).toBeTruthy();
  });

  test("surfaces the effective Codex backend in Settings when local auth is available", async ({ page }) => {
    await setLlmSettings(page, {
      annotations_enabled: true,
      refinement_enabled: true,
      provider: "openai",
      model: "gpt-5.4",
      api_key_source: "~/.diffcore/config.toml",
      has_api_key: true,
      refinement_provider: "openai",
      refinement_model: "gpt-5.4",
      refinement_max_iterations: 1,
      global_config_path: "~/.diffcore/config.toml",
      codex_available: true,
      codex_authenticated: true,
      claude_available: true,
      claude_authenticated: true,
    });

    await page.getByTitle("Settings").click();

    const panel = page.locator(".settings-panel");
    await expect(panel).toContainText("Effective summary backend on this machine: Codex CLI/default");
    await expect(panel).toContainText("Effective refinement backend on this machine: Codex CLI/default");
    await expect(panel).toContainText("Diffcore will prefer Codex CLI for live jobs on this machine");
  });
});
