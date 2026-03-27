import { test, expect, type Page } from "@playwright/test";

async function waitForDemoApp(page: Page) {
  await page.goto("/");
  await expect(page.locator(".top-bar")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator(".summary")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator(".group-item.selected")).toBeVisible({ timeout: 10_000 });
}

async function setLlmSettings(page: Page, settings: Record<string, unknown>) {
  await page.evaluate((value) => {
    (window as { __TEST_API__: { setLlmSettings: (settings: Record<string, unknown>) => void } }).__TEST_API__.setLlmSettings(value);
  }, settings);
}

function baseMissingSettings(overrides: Record<string, unknown> = {}) {
  return {
    annotations_enabled: false,
    refinement_enabled: false,
    provider: "openai",
    model: "gpt-5.4",
    api_key_source: "none",
    has_api_key: false,
    refinement_provider: "openai",
    refinement_model: "gpt-5.4",
    refinement_max_iterations: 1,
    global_config_path: "~/.flowdiff/config.toml",
    codex_available: false,
    codex_authenticated: false,
    claude_available: false,
    claude_authenticated: false,
    ...overrides,
  };
}

test.describe("AI onboarding", () => {
  test.beforeEach(async ({ page }) => {
    await waitForDemoApp(page);
  });

  test("skips onboarding and prefers Codex immediately when local auth already exists", async ({ page }) => {
    await setLlmSettings(page, baseMissingSettings({
      codex_available: true,
      codex_authenticated: true,
      claude_available: true,
      claude_authenticated: false,
    }));

    await expect(page.getByTestId("ai-onboarding")).toHaveCount(0);
    await expect(page.locator(".btn-summarize")).toBeEnabled();
    await expect(page.locator(".llm-provider-badge")).toContainText("Codex CLI/default");
  });

  test("can reopen onboarding and choose Codex CLI explicitly", async ({ page }) => {
    await setLlmSettings(page, baseMissingSettings({
      codex_available: true,
      codex_authenticated: true,
    }));

    await page.evaluate(() => {
      (window as { __TEST_API__: { openAiSetup: (step?: "recommended" | "api") => void } }).__TEST_API__.openAiSetup("recommended");
    });

    const onboarding = page.getByTestId("ai-onboarding");
    await expect(onboarding).toBeVisible();
    await expect(page.getByTestId("ai-card-codex")).toContainText("Ready");
    await page.getByTestId("ai-card-codex").getByRole("button", { name: "Use Codex CLI" }).click();

    await expect(onboarding).not.toBeVisible();
    await expect(page.locator(".btn-summarize")).toBeEnabled();
    await expect(page.locator(".llm-provider-badge")).toContainText("Codex CLI/default");
  });

  test("supports direct API key fallback in the onboarding flow", async ({ page }) => {
    await setLlmSettings(page, baseMissingSettings());

    const onboarding = page.getByTestId("ai-onboarding");
    await expect(onboarding).toBeVisible();
    await page.getByRole("button", { name: "Use API key instead" }).click();
    await page.getByTestId("api-provider-select").selectOption("openai");
    await page.getByTestId("api-key-input").fill("sk-test-onboarding");
    await page.getByTestId("api-key-save").click();

    await expect(onboarding).not.toBeVisible();
    await expect(page.locator(".btn-summarize")).toBeEnabled();
    await expect(page.locator(".llm-provider-badge")).toContainText("OpenAI API/gpt-5.4");
  });

  test("can be dismissed and reopened from the top bar", async ({ page }) => {
    await setLlmSettings(page, baseMissingSettings());

    const onboarding = page.getByTestId("ai-onboarding");
    await expect(onboarding).toBeVisible();
    await onboarding.getByRole("button", { name: "Continue without AI" }).click();

    await expect(onboarding).not.toBeVisible();
    await page.locator(".top-bar-right .btn-ai-setup").click();
    await expect(onboarding).toBeVisible();
  });
});
