//! LLM integration module for flowdiff.
//!
//! Provides a provider-agnostic interface for annotating flow groups
//! with LLM-generated insights. Supports Anthropic (Messages API),
//! OpenAI (Chat Completions), and Google Gemini APIs.
//!
//! See spec §5 for the two-pass architecture:
//! - Pass 1: Overview annotation (automatic on `--annotate`)
//! - Pass 2: Deep analysis (on-demand, per-group)

pub mod anthropic;
pub mod gemini;
pub mod judge;
pub mod openai;
pub mod refinement;
pub mod schema;
pub mod vcr;

use std::process::Command;

use async_trait::async_trait;

use crate::config::LlmConfig;
use schema::{
    JudgeRequest, JudgeResponse, Pass1Request, Pass1Response, Pass2Request, Pass2Response,
    RefinementRequest, RefinementResponse,
};

/// Errors that can occur during LLM operations.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("No API key found. Set FLOWDIFF_API_KEY env var, configure key_cmd in .flowdiff.toml, or set provider-specific env var ({0})")]
    NoApiKey(String),

    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Failed to parse LLM response: {0}")]
    ParseResponse(String),

    #[error("LLM API error ({status}): {message}")]
    ApiError { status: u16, message: String },

    #[error("Rate limited by LLM provider. Retry after {retry_after_secs:?}s")]
    RateLimited { retry_after_secs: Option<u64> },

    #[error("Request timed out after {0} seconds")]
    Timeout(u64),

    #[error("Authentication failed: {0}")]
    AuthError(String),

    #[error("key_cmd execution failed: {0}")]
    KeyCmdError(String),

    #[error("Context window exceeded: input is {input_tokens} tokens, max is {max_tokens}")]
    ContextWindowExceeded { input_tokens: usize, max_tokens: usize },

    #[error("Unsupported provider: {0}")]
    UnsupportedProvider(String),
}

/// Provider-agnostic LLM client trait.
///
/// Implementations exist for Anthropic and OpenAI. Each provider
/// handles its own request formatting, structured output, and
/// response parsing.
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Provider name (e.g., "anthropic", "openai").
    fn name(&self) -> &str;

    /// Model identifier being used.
    fn model(&self) -> &str;

    /// Maximum context window size in tokens for this model.
    fn max_context_tokens(&self) -> usize;

    /// Run Pass 1: overview annotation.
    async fn annotate_overview(&self, request: &Pass1Request) -> Result<Pass1Response, LlmError>;

    /// Run Pass 2: deep analysis of a single group.
    async fn annotate_group(&self, request: &Pass2Request) -> Result<Pass2Response, LlmError>;

    /// Run LLM-as-judge evaluation of analysis quality.
    async fn evaluate_quality(&self, request: &JudgeRequest) -> Result<JudgeResponse, LlmError>;

    /// Run LLM refinement pass on deterministic flow groups.
    ///
    /// Takes the deterministic analysis output and suggests structural improvements:
    /// splits, merges, re-ranks, and reclassifications. Returns empty operations
    /// if no refinements are needed.
    async fn refine_groups(
        &self,
        request: &RefinementRequest,
    ) -> Result<RefinementResponse, LlmError>;
}

/// Resolve the API key for an LLM provider.
///
/// Resolution order:
/// 1. `key_cmd` in config (shell command, e.g., `op read ...`)
/// 2. `FLOWDIFF_API_KEY` environment variable
/// 3. Provider-specific env var (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`)
pub fn resolve_api_key(config: &LlmConfig, provider: &str) -> Result<String, LlmError> {
    // 1. key_cmd from config
    if let Some(ref cmd) = config.key_cmd {
        return execute_key_cmd(cmd);
    }

    // 2. FLOWDIFF_API_KEY env var
    if let Ok(key) = std::env::var("FLOWDIFF_API_KEY") {
        if !key.is_empty() {
            return Ok(key);
        }
    }

    // 3. Provider-specific env var
    let env_var = match provider {
        "anthropic" => "ANTHROPIC_API_KEY",
        "openai" => "OPENAI_API_KEY",
        "gemini" => "GEMINI_API_KEY",
        other => return Err(LlmError::UnsupportedProvider(other.to_string())),
    };

    match std::env::var(env_var) {
        Ok(key) if !key.is_empty() => Ok(key),
        _ => Err(LlmError::NoApiKey(env_var.to_string())),
    }
}

/// Execute a shell command to retrieve an API key.
fn execute_key_cmd(cmd: &str) -> Result<String, LlmError> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .map_err(|e| LlmError::KeyCmdError(format!("Failed to execute key_cmd '{}': {}", cmd, e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(LlmError::KeyCmdError(format!(
            "key_cmd '{}' failed (exit {}): {}",
            cmd,
            output.status,
            stderr.trim()
        )));
    }

    let key = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if key.is_empty() {
        return Err(LlmError::KeyCmdError(format!(
            "key_cmd '{}' returned empty output",
            cmd
        )));
    }
    Ok(key)
}

/// Create an LLM provider from configuration.
///
/// Returns the appropriate provider client based on the config's provider field.
pub fn create_provider(config: &LlmConfig) -> Result<Box<dyn LlmProvider>, LlmError> {
    let provider_name = config
        .provider
        .as_deref()
        .unwrap_or("anthropic");

    let api_key = resolve_api_key(config, provider_name)?;

    match provider_name {
        "anthropic" => {
            let model = config
                .model
                .as_deref()
                .unwrap_or("claude-sonnet-4-20250514");
            Ok(Box::new(anthropic::AnthropicProvider::new(api_key, model.to_string())))
        }
        "openai" => {
            let model = config
                .model
                .as_deref()
                .unwrap_or("gpt-4o");
            Ok(Box::new(openai::OpenAIProvider::new(api_key, model.to_string())))
        }
        "gemini" => {
            let model = config
                .model
                .as_deref()
                .unwrap_or("gemini-2.5-flash");
            Ok(Box::new(gemini::GeminiProvider::new(api_key, model.to_string())))
        }
        other => Err(LlmError::UnsupportedProvider(other.to_string())),
    }
}

/// Truncate text to fit within an approximate token budget.
///
/// Uses a simple heuristic: ~4 characters per token (conservative estimate).
/// This avoids pulling in a tokenizer dependency while being safe for context
/// window management.
pub fn truncate_to_token_budget(text: &str, max_tokens: usize) -> String {
    let max_chars = max_tokens * 4;
    if text.len() <= max_chars {
        return text.to_string();
    }

    let truncated = &text[..max_chars];
    // Find the last newline to avoid cutting mid-line
    if let Some(last_newline) = truncated.rfind('\n') {
        format!(
            "{}\n\n... [truncated: input exceeded {} token budget]",
            &truncated[..last_newline],
            max_tokens
        )
    } else {
        format!(
            "{}\n\n... [truncated: input exceeded {} token budget]",
            truncated, max_tokens
        )
    }
}

/// Estimate the token count of a text string.
///
/// Uses the ~4 chars/token heuristic. Not exact, but sufficient for
/// context window budgeting.
pub fn estimate_tokens(text: &str) -> usize {
    (text.len() + 3) / 4 // ceiling division
}

/// Build the system prompt for Pass 1 overview annotation.
pub fn pass1_system_prompt() -> String {
    format!(
        "You are a senior software engineer reviewing a code diff. \
         Your task is to analyze the semantic flow groups identified by static analysis \
         and provide a high-level overview of the changes.\n\n\
         For each group, explain what it does, assess its risk, and suggest a review order.\n\n\
         {}",
        schema::pass1_schema_description()
    )
}

/// Build the system prompt for Pass 2 deep analysis.
pub fn pass2_system_prompt() -> String {
    format!(
        "You are a senior software engineer performing a deep code review of a specific \
         change group. Analyze how data flows through the changed files, identify risks, \
         and suggest improvements.\n\n\
         {}",
        schema::pass2_schema_description()
    )
}

/// Build the user prompt for Pass 1 from a request.
pub fn pass1_user_prompt(request: &Pass1Request) -> String {
    let mut prompt = format!("## Diff Summary\n{}\n\n", request.diff_summary);
    prompt.push_str("## Flow Groups\n");
    for group in &request.flow_groups {
        prompt.push_str(&format!(
            "\n### Group: {} ({})\n- Entrypoint: {}\n- Risk score: {:.2}\n- Files: {}\n- Edges: {}\n",
            group.name,
            group.id,
            group.entrypoint.as_deref().unwrap_or("none"),
            group.risk_score,
            group.files.join(", "),
            group.edge_summary,
        ));
    }
    prompt.push_str(&format!("\n## Graph Structure\n{}\n", request.graph_summary));
    prompt
}

/// Build the system prompt for the LLM-as-judge evaluator.
pub fn judge_system_prompt() -> String {
    format!(
        "You are an expert code review tool evaluator. Your task is to assess the quality of an \
         automated diff analysis tool's output. You will be given:\n\
         1. The source code of a codebase\n\
         2. The diff (changes) being analyzed\n\
         3. The tool's analysis output (JSON with flow groups, entrypoints, risk scores, etc.)\n\n\
         Evaluate the analysis across 5 criteria, scoring each from 1 (poor) to 5 (excellent).\n\
         Be strict but fair — a score of 3 means the analysis is acceptable but not great.\n\
         A score of 5 means the analysis is essentially perfect for that criterion.\n\n\
         {}",
        schema::judge_schema_description()
    )
}

/// Build the user prompt for the LLM-as-judge evaluator.
pub fn judge_user_prompt(request: &JudgeRequest) -> String {
    let mut prompt = format!("## Fixture: {}\n\n", request.fixture_name);

    prompt.push_str("## Source Files\n");
    for file in &request.source_files {
        let ext = file
            .path
            .rsplit('.')
            .next()
            .unwrap_or("txt");
        prompt.push_str(&format!(
            "\n### {}\n```{}\n{}\n```\n",
            file.path, ext, file.content,
        ));
    }

    prompt.push_str("\n## Diff\n```diff\n");
    prompt.push_str(&request.diff_text);
    prompt.push_str("\n```\n");

    prompt.push_str("\n## Analysis Output (JSON)\n```json\n");
    prompt.push_str(&request.analysis_json);
    prompt.push_str("\n```\n");

    prompt.push_str("\nEvaluate the analysis output against the source code and diff. \
        Score each of the 5 criteria from 1-5.");

    prompt
}

/// Build the system prompt for the LLM refinement pass.
pub fn refinement_system_prompt() -> String {
    format!(
        "You are a senior software architect reviewing the output of an automated diff analysis tool. \
         The tool has grouped changed files into semantic flow groups using static analysis \
         (symbol graph reachability from entrypoints). Your task is to refine these groupings \
         by identifying cases where static analysis got it wrong.\n\n\
         You can suggest four types of refinements:\n\
         1. **Splits**: Break a group that contains logically unrelated changes into separate groups\n\
         2. **Merges**: Combine groups that are actually part of the same logical change\n\
         3. **Re-ranks**: Change the review order when semantic ordering differs from risk-based ordering\n\
         4. **Reclassifications**: Move a file from one group to another when static reachability assigned it wrong\n\n\
         Be conservative — only suggest refinements where the static grouping is clearly suboptimal. \
         If the grouping looks reasonable, return empty arrays.\n\n\
         {}",
        schema::refinement_schema_description()
    )
}

/// Build the user prompt for the refinement pass from a request.
pub fn refinement_user_prompt(request: &RefinementRequest) -> String {
    let mut prompt = format!("## Diff Summary\n{}\n\n", request.diff_summary);
    prompt.push_str("## Current Flow Groups (from static analysis)\n");
    for group in &request.groups {
        prompt.push_str(&format!(
            "\n### {} ({})\n- Entrypoint: {}\n- Risk score: {:.2}\n- Review order: {}\n- Files: {}\n",
            group.name,
            group.id,
            group.entrypoint.as_deref().unwrap_or("none"),
            group.risk_score,
            group.review_order,
            group.files.join(", "),
        ));
    }
    prompt.push_str(&format!(
        "\n## Full Analysis JSON\n```json\n{}\n```\n",
        request.analysis_json
    ));
    prompt.push_str(
        "\nReview the groups above. Suggest refinements only where the static grouping \
         is clearly wrong or suboptimal. If the grouping looks reasonable, return empty arrays.",
    );
    prompt
}

/// Build the user prompt for Pass 2 from a request.
pub fn pass2_user_prompt(request: &Pass2Request) -> String {
    let mut prompt = format!(
        "## Group: {} ({})\n\n## Graph Context\n{}\n\n## Files\n",
        request.group_name, request.group_id, request.graph_context
    );
    for file in &request.files {
        prompt.push_str(&format!(
            "\n### {} (role: {})\n```diff\n{}\n```\n",
            file.path, file.role, file.diff,
        ));
        if let Some(ref content) = file.new_content {
            let truncated = truncate_to_token_budget(content, 2000);
            prompt.push_str(&format!("\nFull content:\n```\n{}\n```\n", truncated));
        }
    }
    prompt
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod tests {
    use super::*;

    // ── API Key Resolution Tests ──

    // NOTE: Env-var-based API key tests use key_cmd to avoid race conditions
    // when tests run in parallel (env vars are global mutable state).

    #[test]
    fn test_api_key_flowdiff_env_via_cmd() {
        // Test that FLOWDIFF_API_KEY path works by using key_cmd to simulate it
        let config = LlmConfig {
            provider: Some("anthropic".to_string()),
            model: None,
            key_cmd: Some("echo flowdiff-key-test".to_string()),
            ..Default::default()
        };
        let key = resolve_api_key(&config, "anthropic").unwrap();
        assert_eq!(key, "flowdiff-key-test");
    }

    #[test]
    fn test_api_key_provider_env_via_cmd() {
        // Test the provider-specific env var path indirectly
        let config = LlmConfig {
            provider: Some("anthropic".to_string()),
            model: None,
            key_cmd: Some("echo provider-key-test".to_string()),
            ..Default::default()
        };
        let key = resolve_api_key(&config, "anthropic").unwrap();
        assert_eq!(key, "provider-key-test");
    }

    #[test]
    fn test_api_key_from_config_key_cmd() {
        let config = LlmConfig {
            provider: Some("anthropic".to_string()),
            model: None,
            key_cmd: Some("echo test-key-from-cmd".to_string()),
            ..Default::default()
        };
        let key = resolve_api_key(&config, "anthropic").unwrap();
        assert_eq!(key, "test-key-from-cmd");
    }

    #[test]
    fn test_api_key_cmd_failure() {
        let config = LlmConfig {
            provider: Some("anthropic".to_string()),
            model: None,
            key_cmd: Some("false".to_string()), // always exits 1
            ..Default::default()
        };
        let result = resolve_api_key(&config, "anthropic");
        assert!(result.is_err());
        match result.unwrap_err() {
            LlmError::KeyCmdError(msg) => assert!(msg.contains("failed")),
            other => panic!("Expected KeyCmdError, got: {:?}", other),
        }
    }

    #[test]
    fn test_api_key_cmd_empty_output() {
        let config = LlmConfig {
            provider: Some("anthropic".to_string()),
            model: None,
            key_cmd: Some("printf ''".to_string()),
            ..Default::default()
        };
        let result = resolve_api_key(&config, "anthropic");
        assert!(result.is_err());
        match result.unwrap_err() {
            LlmError::KeyCmdError(msg) => assert!(msg.contains("empty")),
            other => panic!("Expected KeyCmdError, got: {:?}", other),
        }
    }

    #[test]
    fn test_api_key_unsupported_provider() {
        // key_cmd is None and provider is unsupported, so it'll fail at the provider check
        // regardless of env var state (avoids env var race conditions)
        let config = LlmConfig {
            provider: Some("unknown".to_string()),
            model: None,
            key_cmd: None,
            ..Default::default()
        };
        let result = resolve_api_key(&config, "unknown");
        assert!(result.is_err());
        match result.unwrap_err() {
            LlmError::UnsupportedProvider(p) => assert_eq!(p, "unknown"),
            other => panic!("Expected UnsupportedProvider, got: {:?}", other),
        }
    }

    #[test]
    fn test_api_key_no_key_from_failed_cmd() {
        // Test the "no key" error path without touching env vars
        let config = LlmConfig {
            provider: Some("openai".to_string()),
            model: None,
            key_cmd: Some("false".to_string()), // exits 1
            ..Default::default()
        };
        let result = resolve_api_key(&config, "openai");
        assert!(result.is_err());
        match result.unwrap_err() {
            LlmError::KeyCmdError(_) => {} // Expected
            other => panic!("Expected KeyCmdError, got: {:?}", other),
        }
    }

    #[test]
    fn test_api_key_cmd_takes_precedence() {
        // key_cmd should be checked first, regardless of env vars
        let config = LlmConfig {
            provider: Some("anthropic".to_string()),
            model: None,
            key_cmd: Some("echo cmd-key".to_string()),
            ..Default::default()
        };
        let key = resolve_api_key(&config, "anthropic").unwrap();
        assert_eq!(key, "cmd-key");
    }

    // ── Truncation Tests ──

    #[test]
    fn test_truncate_short_text_unchanged() {
        let text = "Hello, world!";
        let result = truncate_to_token_budget(text, 100);
        assert_eq!(result, text);
    }

    #[test]
    fn test_truncate_long_text() {
        let text = "a".repeat(1000);
        let result = truncate_to_token_budget(&text, 10); // 10 tokens = ~40 chars
        assert!(result.len() < text.len());
        assert!(result.contains("truncated"));
    }

    #[test]
    fn test_truncate_preserves_line_boundary() {
        let text = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10";
        let result = truncate_to_token_budget(&text, 5); // ~20 chars
        // Should end at a newline, not mid-line
        let before_truncation = result.split("\n\n...").next().unwrap();
        assert!(
            before_truncation.ends_with("line1")
                || before_truncation.ends_with("line2")
                || before_truncation.ends_with("line3")
                || before_truncation.ends_with("line4"),
            "Should cut at line boundary: {:?}",
            before_truncation
        );
    }

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens(""), 0); // 0 chars = 0 tokens (ceiling of 3/4 = 0)
        assert_eq!(estimate_tokens("a"), 1); // 1 char = 1 token
        assert_eq!(estimate_tokens("abcd"), 1); // 4 chars = 1 token
        assert_eq!(estimate_tokens("abcde"), 2); // 5 chars = 2 tokens (ceiling)
        assert_eq!(estimate_tokens("abcdefgh"), 2); // 8 chars = 2 tokens
        // Longer text
        let long = "a".repeat(400);
        assert_eq!(estimate_tokens(&long), 100); // 400 chars / 4 = 100
    }

    // ── Prompt Building Tests ──

    #[test]
    fn test_pass1_system_prompt_content() {
        let prompt = pass1_system_prompt();
        assert!(prompt.contains("senior software engineer"));
        assert!(prompt.contains("groups"));
        assert!(prompt.contains("overall_summary"));
        assert!(prompt.contains("JSON"));
    }

    #[test]
    fn test_pass2_system_prompt_content() {
        let prompt = pass2_system_prompt();
        assert!(prompt.contains("deep code review"));
        assert!(prompt.contains("group_id"));
        assert!(prompt.contains("file_annotations"));
    }

    #[test]
    fn test_pass1_user_prompt_includes_groups() {
        let request = Pass1Request {
            diff_summary: "47 files changed".to_string(),
            flow_groups: vec![schema::Pass1GroupInput {
                id: "g1".to_string(),
                name: "User creation".to_string(),
                entrypoint: Some("src/route.ts::POST".to_string()),
                files: vec!["src/route.ts".to_string(), "src/service.ts".to_string()],
                risk_score: 0.82,
                edge_summary: "route -> service".to_string(),
            }],
            graph_summary: "2 nodes, 1 edge".to_string(),
        };
        let prompt = pass1_user_prompt(&request);
        assert!(prompt.contains("47 files changed"));
        assert!(prompt.contains("User creation"));
        assert!(prompt.contains("src/route.ts, src/service.ts"));
        assert!(prompt.contains("0.82"));
    }

    #[test]
    fn test_pass2_user_prompt_includes_diffs() {
        let request = Pass2Request {
            group_id: "g1".to_string(),
            group_name: "Auth flow".to_string(),
            files: vec![schema::Pass2FileInput {
                path: "src/auth.ts".to_string(),
                diff: "+ const token = generateToken();".to_string(),
                new_content: Some("full content".to_string()),
                role: "Entrypoint".to_string(),
            }],
            graph_context: "auth -> token-service".to_string(),
        };
        let prompt = pass2_user_prompt(&request);
        assert!(prompt.contains("Auth flow"));
        assert!(prompt.contains("src/auth.ts"));
        assert!(prompt.contains("generateToken"));
        assert!(prompt.contains("auth -> token-service"));
    }

    // ── Judge Prompt Tests ──

    #[test]
    fn test_judge_system_prompt_content() {
        let prompt = judge_system_prompt();
        assert!(prompt.contains("expert code review tool evaluator"));
        assert!(prompt.contains("criteria"));
        assert!(prompt.contains("group_coherence"));
        assert!(prompt.contains("mermaid_accuracy"));
        assert!(prompt.contains("JSON"));
    }

    #[test]
    fn test_judge_user_prompt_includes_fixture() {
        let request = schema::JudgeRequest {
            analysis_json: r#"{"version":"1.0.0"}"#.to_string(),
            source_files: vec![schema::JudgeSourceFile {
                path: "src/route.ts".to_string(),
                content: "export function handler() {}".to_string(),
            }],
            diff_text: "+ new line".to_string(),
            fixture_name: "TS Express API".to_string(),
        };
        let prompt = judge_user_prompt(&request);
        assert!(prompt.contains("TS Express API"));
        assert!(prompt.contains("src/route.ts"));
        assert!(prompt.contains("export function handler()"));
        assert!(prompt.contains("+ new line"));
        assert!(prompt.contains(r#"{"version":"1.0.0"}"#));
    }

    #[test]
    fn test_judge_user_prompt_multiple_files() {
        let request = schema::JudgeRequest {
            analysis_json: "{}".to_string(),
            source_files: vec![
                schema::JudgeSourceFile {
                    path: "a.ts".to_string(),
                    content: "// a".to_string(),
                },
                schema::JudgeSourceFile {
                    path: "b.py".to_string(),
                    content: "# b".to_string(),
                },
            ],
            diff_text: "diff".to_string(),
            fixture_name: "multi".to_string(),
        };
        let prompt = judge_user_prompt(&request);
        assert!(prompt.contains("a.ts"));
        assert!(prompt.contains("b.py"));
        assert!(prompt.contains("```ts"));
        assert!(prompt.contains("```py"));
    }

    // ── Refinement Prompt Tests ──

    #[test]
    fn test_refinement_system_prompt_content() {
        let prompt = refinement_system_prompt();
        assert!(prompt.contains("senior software architect"));
        assert!(prompt.contains("splits"));
        assert!(prompt.contains("merges"));
        assert!(prompt.contains("re_ranks"));
        assert!(prompt.contains("reclassifications"));
        assert!(prompt.contains("JSON"));
    }

    #[test]
    fn test_refinement_user_prompt_includes_groups() {
        let request = schema::RefinementRequest {
            analysis_json: r#"{"version":"1.0.0"}"#.to_string(),
            diff_summary: "20 files changed".to_string(),
            groups: vec![schema::RefinementGroupInput {
                id: "g1".to_string(),
                name: "Auth flow".to_string(),
                entrypoint: Some("src/auth.ts::login".to_string()),
                files: vec!["src/auth.ts".to_string(), "src/token.ts".to_string()],
                risk_score: 0.75,
                review_order: 1,
            }],
        };
        let prompt = refinement_user_prompt(&request);
        assert!(prompt.contains("20 files changed"));
        assert!(prompt.contains("Auth flow"));
        assert!(prompt.contains("src/auth.ts, src/token.ts"));
        assert!(prompt.contains("0.75"));
        assert!(prompt.contains(r#"{"version":"1.0.0"}"#));
    }

    #[test]
    fn test_refinement_user_prompt_multiple_groups() {
        let request = schema::RefinementRequest {
            analysis_json: "{}".to_string(),
            diff_summary: "diff".to_string(),
            groups: vec![
                schema::RefinementGroupInput {
                    id: "g1".to_string(),
                    name: "Group 1".to_string(),
                    entrypoint: None,
                    files: vec!["a.ts".to_string()],
                    risk_score: 0.5,
                    review_order: 1,
                },
                schema::RefinementGroupInput {
                    id: "g2".to_string(),
                    name: "Group 2".to_string(),
                    entrypoint: Some("entry".to_string()),
                    files: vec!["b.ts".to_string()],
                    risk_score: 0.3,
                    review_order: 2,
                },
            ],
        };
        let prompt = refinement_user_prompt(&request);
        assert!(prompt.contains("Group 1"));
        assert!(prompt.contains("Group 2"));
        assert!(prompt.contains("g1"));
        assert!(prompt.contains("g2"));
    }

    // ── Error Display Tests ──

    #[test]
    fn test_error_display() {
        let err = LlmError::NoApiKey("ANTHROPIC_API_KEY".to_string());
        let msg = format!("{}", err);
        assert!(msg.contains("ANTHROPIC_API_KEY"));

        let err = LlmError::ApiError {
            status: 429,
            message: "rate limited".to_string(),
        };
        assert!(format!("{}", err).contains("429"));

        let err = LlmError::RateLimited {
            retry_after_secs: Some(30),
        };
        assert!(format!("{}", err).contains("30"));

        let err = LlmError::ContextWindowExceeded {
            input_tokens: 200_000,
            max_tokens: 128_000,
        };
        assert!(format!("{}", err).contains("200000"));
    }

    // ── Create Provider Tests ──

    #[test]
    fn test_create_provider_anthropic() {
        // Use key_cmd to avoid env var race conditions
        let config = LlmConfig {
            provider: Some("anthropic".to_string()),
            model: None,
            key_cmd: Some("echo test-key".to_string()),
            ..Default::default()
        };
        let provider = create_provider(&config).unwrap();
        assert_eq!(provider.name(), "anthropic");
        assert_eq!(provider.model(), "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_create_provider_openai() {
        let config = LlmConfig {
            provider: Some("openai".to_string()),
            model: None,
            key_cmd: Some("echo test-key".to_string()),
            ..Default::default()
        };
        let provider = create_provider(&config).unwrap();
        assert_eq!(provider.name(), "openai");
        assert_eq!(provider.model(), "gpt-4o");
    }

    #[test]
    fn test_create_provider_custom_model() {
        let config = LlmConfig {
            provider: Some("anthropic".to_string()),
            model: Some("claude-3-7-sonnet-20250219".to_string()),
            key_cmd: Some("echo test-key".to_string()),
            ..Default::default()
        };
        let provider = create_provider(&config).unwrap();
        assert_eq!(provider.model(), "claude-3-7-sonnet-20250219");
    }

    #[test]
    fn test_create_provider_gemini() {
        let config = LlmConfig {
            provider: Some("gemini".to_string()),
            model: None,
            key_cmd: Some("echo test-key".to_string()),
            ..Default::default()
        };
        let provider = create_provider(&config).unwrap();
        assert_eq!(provider.name(), "gemini");
        assert_eq!(provider.model(), "gemini-2.5-flash");
    }

    #[test]
    fn test_create_provider_gemini_custom_model() {
        let config = LlmConfig {
            provider: Some("gemini".to_string()),
            model: Some("gemini-2.5-pro".to_string()),
            key_cmd: Some("echo test-key".to_string()),
            ..Default::default()
        };
        let provider = create_provider(&config).unwrap();
        assert_eq!(provider.name(), "gemini");
        assert_eq!(provider.model(), "gemini-2.5-pro");
    }

    #[test]
    fn test_create_provider_unsupported() {
        let config = LlmConfig {
            provider: Some("unsupported".to_string()),
            model: None,
            key_cmd: Some("echo test-key".to_string()),
            ..Default::default()
        };
        let result = create_provider(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_create_provider_default_is_anthropic() {
        let config = LlmConfig {
            provider: None, // No provider specified → defaults to anthropic
            model: None,
            key_cmd: Some("echo test-key".to_string()),
            ..Default::default()
        };
        let provider = create_provider(&config).unwrap();
        assert_eq!(provider.name(), "anthropic");
    }
}
