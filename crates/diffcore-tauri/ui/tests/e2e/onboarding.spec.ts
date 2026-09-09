import { test, expect, type Page } from "@playwright/test";

async function waitForDemoApp(page: Page) {
  await page.goto("/");
  await expect(page.locator(".top-bar")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator(".summary")).toBeVisible({ timeout: 10_000 });
  await expect(page.locator(".group-item.selected")).toBeVisible({ timeout: 10_000 });
}

async function setLlmSettings(page: Page, settings: Record<string, unknown>) {
  await page.evaluate((value) => {
    (window as unknown as { __TEST_API__: { setLlmSettings: (settings: Record<string, unknown>) => void } }).__TEST_API__.setLlmSettings(value);
  }, settings);
}

function baseMissingSettings(overrides: Record<string, unknown> = {}) {
  return {
    annotations_enabled: false,
    refinement_enabled: false,
    metadata_enabled: false,
    provider: "openai",
    model: "gpt-5.4",
    api_key_source: "none",
    has_api_key: false,
    refinement_provider: "openai",
    refinement_model: "gpt-5.4",
    global_config_path: "~/.config/diffcore/config.toml",
    api_key_in_config: false,
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
    await expect(page.locator(".btn-analyze-flow")).toBeEnabled();
    await expect(page.locator(".llm-provider-badge")).toContainText("Codex CLI/default");
  });

  test("can reopen onboarding and choose Codex CLI explicitly", async ({ page }) => {
    await setLlmSettings(page, baseMissingSettings({
      codex_available: true,
      codex_authenticated: true,
    }));

    await expect(page.locator(".btn-analyze-flow")).toBeEnabled();
    await expect(page.locator(".llm-provider-badge")).toContainText("Codex CLI/default");

    await page.evaluate(() => {
      (window as unknown as { __TEST_API__: { openAiSetup: (step?: "recommended" | "api") => void } }).__TEST_API__.openAiSetup("recommended");
    });

    const onboarding = page.getByTestId("ai-onboarding");
    await expect(onboarding).toBeVisible();
    await expect(page.getByTestId("ai-card-codex")).toContainText("Ready");
    await page.getByTestId("ai-card-codex").getByRole("button", { name: "Use Codex CLI" }).click();

    await expect(onboarding).not.toBeVisible();
    await expect(page.locator(".btn-analyze-flow")).toBeEnabled();
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
    await expect(page.locator(".btn-analyze-flow")).toBeEnabled();
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

// ═══════════════════════════════════════════════════════════════════
// API key removal
//
// Regression guard: the Clear button used to be gated on
// `api_key_source === "~/.diffcore/config.toml"`, a string comparison against
// a display path. Moving the config to XDG changed that path and silently
// removed the only way to delete a stored plaintext key from the UI.
// ═══════════════════════════════════════════════════════════════════

test.describe("Stored API key removal", () => {
  test.beforeEach(async ({ page }) => {
    await waitForDemoApp(page);
  });

  test("Clear button shows when a key is stored in the config file", async ({ page }) => {
    await setLlmSettings(
      page,
      baseMissingSettings({
        provider: "anthropic",
        has_api_key: true,
        api_key_in_config: true,
        api_key_source: "~/.config/diffcore/config.toml",
      }),
    );

    await page.locator(".btn-settings").click();
    await expect(page.locator(".btn-clear-key")).toBeVisible();
  });

  test("Clear button stays hidden when the key comes from somewhere else", async ({ page }) => {
    await setLlmSettings(
      page,
      baseMissingSettings({
        provider: "anthropic",
        has_api_key: true,
        api_key_in_config: false,
        api_key_source: "ANTHROPIC_API_KEY",
      }),
    );

    await page.locator(".btn-settings").click();
    await expect(page.locator(".btn-clear-key")).toHaveCount(0);
  });

  test("Clear button does not depend on the config file path", async ({ page }) => {
    // The whole point: an unrecognised path must not hide the control.
    await setLlmSettings(
      page,
      baseMissingSettings({
        provider: "anthropic",
        has_api_key: true,
        api_key_in_config: true,
        api_key_source: "/some/entirely/unexpected/location/config.toml",
        global_config_path: "/some/entirely/unexpected/location/config.toml",
      }),
    );

    await page.locator(".btn-settings").click();
    await expect(page.locator(".btn-clear-key")).toBeVisible();
  });
});
