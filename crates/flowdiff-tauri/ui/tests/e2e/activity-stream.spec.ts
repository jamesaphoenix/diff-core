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
    await expect(page.getByRole("button", { name: "Copy PR Description" })).toBeVisible();
    await expect(panel.locator(".activity-live-badge")).toHaveText("Saved");

    await page.getByTestId("annotations-tab").click();
    await expect(page.getByTestId("annotations-panel")).toBeVisible();
    await expect(page.getByTestId("annotations-panel")).toContainText("POST /api/users creation flow");
  });

  test("prefers Codex CLI over direct API settings for refinement when a local backend is ready", async ({ page }) => {
    await setLlmSettings(page, {
      annotations_enabled: true,
      refinement_enabled: true,
      provider: "openai",
      model: "gpt-5.4",
      api_key_source: "~/.flowdiff/config.toml",
      has_api_key: true,
      refinement_provider: "openai",
      refinement_model: "gpt-5.4",
      refinement_max_iterations: 1,
      global_config_path: "~/.flowdiff/config.toml",
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
    await expect(page.locator(".llm-provider-badge")).toContainText("Codex CLI/default");
  });

  test("explains when a direct API provider cannot show file reads or grep activity", async ({ page }) => {
    await setLlmSettings(page, {
      annotations_enabled: true,
      refinement_enabled: true,
      provider: "openai",
      model: "gpt-5.4",
      api_key_source: "~/.flowdiff/config.toml",
      has_api_key: true,
      refinement_provider: "openai",
      refinement_model: "gpt-5.4",
      refinement_max_iterations: 1,
      global_config_path: "~/.flowdiff/config.toml",
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

  test("surfaces the effective Codex backend in Settings when local auth is available", async ({ page }) => {
    await setLlmSettings(page, {
      annotations_enabled: true,
      refinement_enabled: true,
      provider: "openai",
      model: "gpt-5.4",
      api_key_source: "~/.flowdiff/config.toml",
      has_api_key: true,
      refinement_provider: "openai",
      refinement_model: "gpt-5.4",
      refinement_max_iterations: 1,
      global_config_path: "~/.flowdiff/config.toml",
      codex_available: true,
      codex_authenticated: true,
      claude_available: true,
      claude_authenticated: true,
    });

    await page.getByTitle("Settings").click();

    const panel = page.locator(".settings-panel");
    await expect(panel).toContainText("Effective summary backend on this machine: Codex CLI/default");
    await expect(panel).toContainText("Effective refinement backend on this machine: Codex CLI/default");
    await expect(panel).toContainText("Flowdiff will prefer Codex CLI for live jobs on this machine");
  });
});
