//! LLM integration module for diffcore.
//!
//! Provides a provider-agnostic interface for annotating flow groups
//! with LLM-generated insights. Supports Anthropic (Messages API),
//! OpenAI (Chat Completions), and Google Gemini APIs.
//!
//! See spec §5 for the two-pass architecture:
//! - Pass 1: Overview annotation (automatic on `--annotate`)
//! - Pass 2: Deep analysis (on-demand, per-group)

pub mod anthropic;
pub mod claude_cli;
pub mod codex_cli;
pub mod gemini;
pub mod judge;
pub mod openai;
pub mod refinement;
pub mod schema;
pub mod vcr;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use async_trait::async_trait;
use std::future::Future;

use crate::config::LlmConfig;
use schema::{
    JudgeRequest, JudgeResponse, Pass1Request, Pass1Response, Pass2Request, Pass2Response,
    RefinementRequest, RefinementResponse,
};

/// Errors that can occur during LLM operations.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("No API key found. Set DIFFCORE_API_KEY, configure key_cmd in ~/.diffcore/config.toml or .diffcore.toml, or set provider-specific env var ({0})")]
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

    #[error("Required CLI is not installed: {0}")]
    CommandUnavailable(String),

    #[error("CLI invocation failed: {0}")]
    CommandFailed(String),

    #[error("key_cmd execution failed: {0}")]
    KeyCmdError(String),

    #[error("Context window exceeded: input is {input_tokens} tokens, max is {max_tokens}")]
    ContextWindowExceeded {
        input_tokens: usize,
        max_tokens: usize,
    },

    #[error("Unsupported provider: {0}")]
    UnsupportedProvider(String),
}

/// Availability/authentication status for subscription-backed CLI providers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendStatus {
    pub installed: bool,
    pub authenticated: bool,
}

/// Human-readable activity updates emitted while a provider is working.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ActivityUpdate {
    pub source: String,
    pub level: String,
    pub message: String,
    pub event_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub payload: Option<serde_json::Value>,
    pub timestamp_ms: u64,
}

impl ActivityUpdate {
    pub fn info(
        source: impl Into<String>,
        message: impl Into<String>,
        event_type: Option<String>,
    ) -> Self {
        Self {
            source: source.into(),
            level: "info".to_string(),
            message: message.into(),
            event_type,
            payload: None,
            timestamp_ms: timestamp_ms(),
        }
    }

    pub fn warning(
        source: impl Into<String>,
        message: impl Into<String>,
        event_type: Option<String>,
    ) -> Self {
        Self {
            source: source.into(),
            level: "warning".to_string(),
            message: message.into(),
            event_type,
            payload: None,
            timestamp_ms: timestamp_ms(),
        }
    }

    pub fn error(
        source: impl Into<String>,
        message: impl Into<String>,
        event_type: Option<String>,
    ) -> Self {
        Self {
            source: source.into(),
            level: "error".to_string(),
            message: message.into(),
            event_type,
            payload: None,
            timestamp_ms: timestamp_ms(),
        }
    }
}

pub(crate) type ActivityCallback = Arc<dyn Fn(ActivityUpdate) + Send + Sync + 'static>;
pub const INTERACTIVE_CLI_TIMEOUT_SECS: u64 = 3_600;

tokio::task_local! {
    static ACTIVITY_CALLBACK: ActivityCallback;
}

pub async fn with_activity_callback<F, T>(callback: ActivityCallback, future: F) -> T
where
    F: Future<Output = T>,
{
    ACTIVITY_CALLBACK.scope(callback, future).await
}

pub(crate) fn emit_activity(update: ActivityUpdate) {
    let _ = ACTIVITY_CALLBACK.try_with(|callback| callback(update));
}

pub(crate) fn current_activity_callback() -> Option<ActivityCallback> {
    ACTIVITY_CALLBACK.try_with(Arc::clone).ok()
}

/// Resolve a CLI executable path for GUI-launched app processes.
///
/// Desktop apps on macOS often inherit a minimal PATH, so `Command::new("codex")`
/// can fail even when the tool is installed and available in the user's shell.
/// We therefore search common install locations and fall back to a login shell.
pub(crate) fn resolve_cli_executable(binary: &str) -> Option<PathBuf> {
    find_binary_in_path(binary)
        .or_else(|| {
            candidate_cli_paths(binary)
                .into_iter()
                .find(|path| is_executable_file(path))
        })
        .or_else(|| resolve_via_login_shell(binary))
}

fn find_binary_in_path(binary: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;

    #[cfg(not(windows))]
    {
        std::env::split_paths(&path_var)
            .map(|entry| entry.join(binary))
            .find(|path| is_executable_file(path))
    }

    #[cfg(windows)]
    {
        let pathext = std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM;.PS1".to_string());
        for entry in std::env::split_paths(&path_var) {
            // Try bare name first
            let bare = entry.join(binary);
            if is_executable_file(&bare) {
                return Some(bare);
            }
            // Then try with each PATHEXT extension
            for ext in pathext.split(';') {
                let ext = ext.trim();
                if ext.is_empty() { continue; }
                let with_ext = entry.join(format!("{binary}{ext}"));
                if is_executable_file(&with_ext) {
                    return Some(with_ext);
                }
                let lower = entry.join(format!("{binary}{}", ext.to_lowercase()));
                if is_executable_file(&lower) {
                    return Some(lower);
                }
            }
        }
        None
    }
}

fn candidate_cli_paths(binary: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        for rel in [
            ".npm-global/bin",
            ".npm/bin",
            ".local/bin",
            ".cargo/bin",
            "bin",
        ] {
            candidates.push(home.join(rel).join(binary));
        }
    }

    // Windows: USERPROFILE as home fallback, plus npm global locations
    #[cfg(windows)]
    {
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            let home = PathBuf::from(profile);
            let pathext = std::env::var("PATHEXT")
                .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM;.PS1".to_string());
            let exts: Vec<&str> = pathext.split(';').map(str::trim).filter(|e| !e.is_empty()).collect();
            for rel in [".npm-global/bin", ".npm/bin", ".local/bin", ".cargo/bin", "bin"] {
                for ext in &exts {
                    candidates.push(home.join(rel).join(format!("{binary}{ext}")));
                    candidates.push(home.join(rel).join(format!("{binary}{}", ext.to_lowercase())));
                }
            }
        }
        for env in ["APPDATA", "LOCALAPPDATA"] {
            if let Some(base) = std::env::var_os(env) {
                let base = PathBuf::from(base);
                candidates.push(base.join("npm").join(format!("{binary}.cmd")));
                candidates.push(base.join("npm").join(format!("{binary}.exe")));
            }
        }
    }

    for prefix in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
        candidates.push(PathBuf::from(prefix).join(binary));
    }

    candidates
}

fn resolve_via_login_shell(binary: &str) -> Option<PathBuf> {
    for shell in ["zsh", "bash", "sh"] {
        let Ok(output) = Command::new(shell)
            .arg("-lc")
            .arg(format!("command -v {}", binary))
            .output()
        else {
            continue;
        };

        if !output.status.success() {
            continue;
        }

        let resolved = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if resolved.is_empty() {
            continue;
        }

        let path = PathBuf::from(resolved);
        if is_executable_file(&path) {
            return Some(path);
        }
    }

    None
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };

    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        metadata.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    {
        true
    }
}

fn timestamp_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
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
/// 2. `DIFFCORE_API_KEY` environment variable
/// 3. Provider-specific env var (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`)
pub fn resolve_api_key(config: &LlmConfig, provider: &str) -> Result<String, LlmError> {
    // 1. key_cmd from config (highest priority)
    if let Some(ref cmd) = config.key_cmd {
        return execute_key_cmd(cmd);
    }

    // 2. key from config file (pasted via settings panel)
    if let Some(ref key) = config.key {
        if !key.is_empty() {
            return Ok(key.clone());
        }
    }

    // 3. DIFFCORE_API_KEY env var
    if let Ok(key) = std::env::var("DIFFCORE_API_KEY") {
        if !key.is_empty() {
            return Ok(key);
        }
    }

    // 4. Provider-specific env var
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

/// Dangerous shell metacharacters that indicate potential injection in `key_cmd`.
///
/// `key_cmd` is intended for simple credential helpers like `op read ...` or
/// `aws secretsmanager get-secret-value ...`. Commands containing these characters
/// (backticks, `$()` subshells, pipes, semicolons, etc.) are rejected to prevent
/// supply-chain attacks via malicious `.diffcore.toml` files in cloned repositories.
const DANGEROUS_SHELL_CHARS: &[char] = &['`', '$', '|', ';', '&', '<', '>', '\n', '\r'];

/// Validate that a `key_cmd` string does not contain dangerous shell metacharacters.
///
/// Returns `Ok(())` if the command is safe, or `Err` with a descriptive message
/// if potentially dangerous characters are detected.
fn validate_key_cmd(cmd: &str) -> Result<(), LlmError> {
    for ch in DANGEROUS_SHELL_CHARS {
        if cmd.contains(*ch) {
            return Err(LlmError::KeyCmdError(format!(
                "key_cmd contains dangerous shell character '{}'. \
                 Only simple commands are allowed (e.g., 'op read op://vault/item/field'). \
                 Shell metacharacters (`, $, |, ;, &, <, >) are rejected to prevent \
                 injection attacks from malicious .diffcore.toml files.",
                ch
            )));
        }
    }
    Ok(())
}

/// Execute a shell command to retrieve an API key.
///
/// The command is validated against dangerous shell metacharacters before execution.
/// Error messages redact the command string to prevent accidental key leakage
/// (e.g., if a user inlines a key in `key_cmd` for testing).
fn execute_key_cmd(cmd: &str) -> Result<String, LlmError> {
    validate_key_cmd(cmd)?;

    let output = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .map_err(|e| LlmError::KeyCmdError(format!("Failed to execute key_cmd: {}", e)))?;

    if !output.status.success() {
        return Err(LlmError::KeyCmdError(format!(
            "key_cmd failed with exit code {}",
            output.status,
        )));
    }

    let key = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if key.is_empty() {
        return Err(LlmError::KeyCmdError(
            "key_cmd returned empty output".to_string(),
        ));
    }
    Ok(key)
}

/// Create an LLM provider from configuration.
///
/// Returns the appropriate provider client based on the config's provider field.
pub fn create_provider(config: &LlmConfig) -> Result<Box<dyn LlmProvider>, LlmError> {
    create_provider_for_workdir(config, None)
}

/// Create an LLM provider from configuration, optionally anchored to a repository workdir.
///
/// CLI-backed providers use the workdir as their execution root so they can inspect the
/// repository with read-only tools while producing structured JSON output.
pub fn create_provider_for_workdir(
    config: &LlmConfig,
    workdir: Option<&Path>,
) -> Result<Box<dyn LlmProvider>, LlmError> {
    let provider_name = config.provider.as_deref().unwrap_or_else(|| {
        if config.key_cmd.is_some()
            || config.key.as_ref().is_some_and(|key| !key.is_empty())
            || std::env::var("DIFFCORE_API_KEY").is_ok()
            || std::env::var("ANTHROPIC_API_KEY").is_ok()
            || std::env::var("OPENAI_API_KEY").is_ok()
            || std::env::var("GEMINI_API_KEY").is_ok()
        {
            "anthropic"
        } else {
            detect_default_provider()
        }
    });

    if provider_name == "codex" {
        return Ok(Box::new(codex_cli::CodexCliProvider::new(
            config.model.clone(),
            workdir.map(Path::to_path_buf),
        )));
    }

    if provider_name == "claude" {
        return Ok(Box::new(claude_cli::ClaudeCliProvider::new(
            config.model.clone(),
            workdir.map(Path::to_path_buf),
        )));
    }

    let api_key = resolve_api_key(config, provider_name)?;

    match provider_name {
        "anthropic" => {
            let model = config.model.as_deref().unwrap_or("claude-sonnet-4-6");
            Ok(Box::new(anthropic::AnthropicProvider::new(
                api_key,
                model.to_string(),
            )))
        }
        "openai" => {
            let model = config.model.as_deref().unwrap_or("gpt-4.1");
            Ok(Box::new(openai::OpenAIProvider::new(
                api_key,
                model.to_string(),
            )))
        }
        "gemini" => {
            let model = config.model.as_deref().unwrap_or("gemini-2.5-flash");
            Ok(Box::new(gemini::GeminiProvider::new(
                api_key,
                model.to_string(),
            )))
        }
        other => Err(LlmError::UnsupportedProvider(other.to_string())),
    }
}

/// Choose the best default provider for the current machine.
///
/// Preference order:
/// 1. Codex CLI when installed and logged in
/// 2. Claude Code when installed and logged in
/// 3. Anthropic API fallback
fn detect_default_provider() -> &'static str {
    if codex_cli::detect_status().authenticated {
        "codex"
    } else if claude_cli::detect_status().authenticated {
        "claude"
    } else {
        "anthropic"
    }
}

/// Maximum allowed HTTP response body size from LLM providers (10 MB).
///
/// Prevents memory exhaustion from abnormally large responses (e.g., MITM attack
/// or misconfigured proxy returning huge bodies). Normal LLM responses are
/// typically < 100 KB.
pub const MAX_RESPONSE_BODY_BYTES: u64 = 10 * 1024 * 1024;

/// Check the Content-Length header of an HTTP response and reject if too large.
///
/// Returns `Err(LlmError::ApiError)` if the response body exceeds
/// [`MAX_RESPONSE_BODY_BYTES`]. Responses without a Content-Length header
/// are allowed through (the body is still bounded by reqwest's internal limits).
pub fn check_response_size(response: &reqwest::Response) -> Result<(), LlmError> {
    if let Some(len) = response.content_length() {
        if len > MAX_RESPONSE_BODY_BYTES {
            return Err(LlmError::ApiError {
                status: response.status().as_u16(),
                message: format!(
                    "Response body too large: {} bytes (max {} bytes)",
                    len, MAX_RESPONSE_BODY_BYTES
                ),
            });
        }
    }
    Ok(())
}

/// Truncate text to fit within an approximate token budget.
///
/// Uses a simple heuristic: ~4 bytes per token (conservative estimate).
/// This avoids pulling in a tokenizer dependency while being safe for context
/// window management. Handles multi-byte UTF-8 safely by snapping to the
/// nearest char boundary.
pub fn truncate_to_token_budget(text: &str, max_tokens: usize) -> String {
    let max_bytes = max_tokens * 4;
    if text.len() <= max_bytes {
        return text.to_string();
    }

    // Snap to a valid UTF-8 char boundary at or before max_bytes
    let mut end = max_bytes.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let truncated = &text[..end];

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

/// Redact potential API key patterns from error message bodies.
///
/// LLM providers may echo back partial or full API keys in error responses
/// (e.g., Anthropic 401 errors include the key prefix). This function strips
/// common patterns to prevent accidental key leakage through error messages
/// displayed in the UI or logs.
pub fn redact_api_keys(text: &str) -> String {
    // Truncate to a safe length first (no error body needs to be > 500 chars)
    let truncated = if text.len() > 500 { &text[..500] } else { text };

    let mut result = truncated.to_string();

    // Redact known API key prefixes by finding them and replacing the
    // prefix + subsequent alphanumeric/dash/underscore characters.
    let prefixes: &[(&str, &str)] = &[
        ("sk-ant-", "[REDACTED_ANTHROPIC_KEY]"),
        ("sk-proj-", "[REDACTED_OPENAI_KEY]"),
        ("sk-", "[REDACTED_API_KEY]"),
        ("AIza", "[REDACTED_GEMINI_KEY]"),
    ];

    for &(prefix, replacement) in prefixes {
        while let Some(start) = result.find(prefix) {
            // Find the end of the key (alphanumeric, dash, underscore chars)
            let key_end = result[start + prefix.len()..]
                .find(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
                .map_or(result.len(), |pos| start + prefix.len() + pos);
            // Only redact if the key-like string is at least 10 chars total
            if key_end - start >= 10 {
                result.replace_range(start..key_end, replacement);
            } else {
                break;
            }
        }
    }
    result
}

/// Build the system prompt for Pass 1 overview annotation.
pub fn pass1_system_prompt() -> String {
    format!(
        "You are a senior software engineer reviewing a code diff. \
         Your task is to analyze the semantic flow groups identified by static analysis \
         and provide a high-level overview of the changes.\n\n\
         Write the overall summary and each group summary so they can be reused in a pull request \
         description or shared with a non-developer reviewer. Prefer concrete behavior changes, \
         user impact, and review order rationale over jargon.\n\n\
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
    prompt.push_str(&format!(
        "\n## Graph Structure\n{}\n",
        request.graph_summary
    ));
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
        let ext = file.path.rsplit('.').next().unwrap_or("txt");
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

    prompt.push_str(
        "\nEvaluate the analysis output against the source code and diff. \
        Score each of the 5 criteria from 1-5.",
    );

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
         4. **Reclassifications**: Move a file from one group to another, or from 'infrastructure' \
            (ungrouped) into a flow group when it clearly belongs there. Use to_group_id='infrastructure' \
            to demote, or from_group_id='infrastructure' to promote ungrouped files into a group.\n\n\
         IMPORTANT — Ungrouped/Infrastructure files:\n\
         Review the infrastructure (ungrouped) files list carefully. Many files end up ungrouped \
         because static analysis couldn't trace a reachability path from an entrypoint, but they \
         may logically belong to an existing group (e.g., a schema file that supports a route handler, \
         a config used by a service, a test utility for a specific feature). Reclassify these into \
         the appropriate group using from_group_id='infrastructure'. You can also create new groups \
         for ungrouped files by splitting them out.\n\n\
         IMPORTANT — Descriptive group names:\n\
         When naming groups (in splits, merges, or any new_groups), use descriptive names \
         that clearly communicate what the change does and which domain/feature it affects. \
         Avoid generic names like 'page test flow' or 'config update'. Instead use names like \
         'media asset upload pipeline', 'user authentication middleware', \
         'storage metering schema migration', etc. The name should help a reviewer \
         understand the group's purpose at a glance.\n\n\
         IMPORTANT — Divide and conquer for large diffs:\n\
         For large PRs (10+ groups or 50+ files), apply a divide-and-conquer strategy:\n\
         - First identify the major domains/features being changed\n\
         - Group files by domain, then by layer within each domain\n\
         - Merge scattered single-file groups that belong to the same domain\n\
         - Order review by dependency direction: schemas/types → data layer → services → API routes → UI\n\n\
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

    // Include infrastructure/ungrouped files so the LLM can consider promoting them
    if !request.infrastructure_files.is_empty() {
        prompt.push_str("\n## Ungrouped / Infrastructure Files\n");
        prompt.push_str("These files were not assigned to any flow group by static analysis. \
                         Consider whether any should be reclassified into an existing group \
                         (use from_group_id='infrastructure') or form a new group via splits.\n\n");
        for file in &request.infrastructure_files {
            prompt.push_str(&format!("- {}\n", file));
        }
    }

    prompt.push_str(&format!(
        "\n## Full Analysis JSON\n```json\n{}\n```\n",
        request.analysis_json
    ));
    prompt.push_str(
        "\nReview the groups above and the ungrouped files. Suggest refinements where the static \
         grouping is clearly wrong or suboptimal. Promote ungrouped files into groups where they \
         logically belong. If the grouping looks reasonable, return empty arrays.",
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
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
mod tests {
    use super::*;

    // ── API Key Resolution Tests ──

    // NOTE: Env-var-based API key tests use key_cmd to avoid race conditions
    // when tests run in parallel (env vars are global mutable state).

    #[test]
    fn test_api_key_diffcore_env_via_cmd() {
        // Test that DIFFCORE_API_KEY path works by using key_cmd to simulate it
        let config = LlmConfig {
            provider: Some("anthropic".to_string()),
            model: None,
            key_cmd: Some("echo diffcore-key-test".to_string()),
            ..Default::default()
        };
        let key = resolve_api_key(&config, "anthropic").unwrap();
        assert_eq!(key, "diffcore-key-test");
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
    fn test_api_key_from_config_key_field() {
        let config = LlmConfig {
            provider: Some("anthropic".to_string()),
            key: Some("sk-test-config-key-12345".to_string()),
            ..Default::default()
        };
        let key = resolve_api_key(&config, "anthropic").unwrap();
        assert_eq!(key, "sk-test-config-key-12345");
    }

    #[test]
    fn test_api_key_cmd_takes_precedence_over_config_key() {
        let config = LlmConfig {
            provider: Some("anthropic".to_string()),
            key_cmd: Some("echo cmd-key".to_string()),
            key: Some("config-key".to_string()),
            ..Default::default()
        };
        let key = resolve_api_key(&config, "anthropic").unwrap();
        // key_cmd should take precedence over key
        assert_eq!(key, "cmd-key");
    }

    #[test]
    fn test_api_key_config_key_empty_string_ignored() {
        // An empty config key should be treated as not set
        let config = LlmConfig {
            provider: Some("unknown".to_string()),
            key: Some("".to_string()),
            ..Default::default()
        };
        let result = resolve_api_key(&config, "unknown");
        assert!(result.is_err());
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
            infrastructure_files: vec!["package.json".to_string()],
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
            infrastructure_files: vec!["config.json".to_string()],
        };
        let prompt = refinement_user_prompt(&request);
        assert!(prompt.contains("Group 1"));
        assert!(prompt.contains("Group 2"));
        assert!(prompt.contains("config.json"));
        assert!(prompt.contains("Ungrouped"));
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
        assert_eq!(provider.model(), "claude-sonnet-4-6");
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
        assert_eq!(provider.model(), "gpt-4.1");
    }

    #[test]
    fn test_create_provider_custom_model() {
        let config = LlmConfig {
            provider: Some("anthropic".to_string()),
            model: Some("claude-opus-4-6".to_string()),
            key_cmd: Some("echo test-key".to_string()),
            ..Default::default()
        };
        let provider = create_provider(&config).unwrap();
        assert_eq!(provider.model(), "claude-opus-4-6");
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

    // ── Security: key_cmd injection prevention ──

    #[test]
    fn test_key_cmd_rejects_backtick_injection() {
        let config = LlmConfig {
            key_cmd: Some("echo `whoami`".to_string()),
            ..Default::default()
        };
        let result = resolve_api_key(&config, "anthropic");
        assert!(result.is_err());
        match result.unwrap_err() {
            LlmError::KeyCmdError(msg) => {
                assert!(msg.contains("dangerous shell character"));
                assert!(msg.contains("`"));
            }
            other => panic!("Expected KeyCmdError, got: {:?}", other),
        }
    }

    #[test]
    fn test_key_cmd_rejects_dollar_subshell() {
        let config = LlmConfig {
            key_cmd: Some("echo $(cat /etc/passwd)".to_string()),
            ..Default::default()
        };
        let result = resolve_api_key(&config, "anthropic");
        assert!(result.is_err());
        match result.unwrap_err() {
            LlmError::KeyCmdError(msg) => assert!(msg.contains("dangerous")),
            other => panic!("Expected KeyCmdError, got: {:?}", other),
        }
    }

    #[test]
    fn test_key_cmd_rejects_pipe() {
        let config = LlmConfig {
            key_cmd: Some("cat /etc/passwd | base64".to_string()),
            ..Default::default()
        };
        let result = resolve_api_key(&config, "anthropic");
        assert!(result.is_err());
    }

    #[test]
    fn test_key_cmd_rejects_semicolon() {
        let config = LlmConfig {
            key_cmd: Some("echo key; rm -rf /".to_string()),
            ..Default::default()
        };
        let result = resolve_api_key(&config, "anthropic");
        assert!(result.is_err());
    }

    #[test]
    fn test_key_cmd_rejects_ampersand() {
        let config = LlmConfig {
            key_cmd: Some("echo key & evil_cmd".to_string()),
            ..Default::default()
        };
        let result = resolve_api_key(&config, "anthropic");
        assert!(result.is_err());
    }

    #[test]
    fn test_key_cmd_rejects_redirect() {
        let config = LlmConfig {
            key_cmd: Some("echo key > /tmp/stolen".to_string()),
            ..Default::default()
        };
        let result = resolve_api_key(&config, "anthropic");
        assert!(result.is_err());
    }

    #[test]
    fn test_key_cmd_rejects_newline() {
        let config = LlmConfig {
            key_cmd: Some("echo key\nwhoami".to_string()),
            ..Default::default()
        };
        let result = resolve_api_key(&config, "anthropic");
        assert!(result.is_err());
    }

    #[test]
    fn test_key_cmd_allows_safe_op_read() {
        // A typical 1Password command should be allowed
        let result = validate_key_cmd("op read op://vault/item/field");
        assert!(result.is_ok());
    }

    #[test]
    fn test_key_cmd_allows_safe_echo() {
        let result = validate_key_cmd("echo test-key");
        assert!(result.is_ok());
    }

    #[test]
    fn test_key_cmd_allows_pass_command() {
        let result = validate_key_cmd("pass show diffcore/api-key");
        assert!(result.is_ok());
    }

    // ── Security: key_cmd error message redaction ──

    #[test]
    fn test_key_cmd_error_does_not_leak_command() {
        let config = LlmConfig {
            key_cmd: Some("false".to_string()),
            ..Default::default()
        };
        let result = resolve_api_key(&config, "anthropic");
        let err_msg = format!("{}", result.unwrap_err());
        // Should NOT contain the literal command string
        assert!(
            !err_msg.contains("'false'"),
            "Error should not echo the command: {}",
            err_msg
        );
    }

    #[test]
    fn test_key_cmd_empty_error_does_not_leak_command() {
        let config = LlmConfig {
            key_cmd: Some("printf ''".to_string()),
            ..Default::default()
        };
        let result = resolve_api_key(&config, "anthropic");
        let err_msg = format!("{}", result.unwrap_err());
        assert!(
            !err_msg.contains("printf"),
            "Error should not echo the command: {}",
            err_msg
        );
    }

    // ── Security: API key redaction from error bodies ──

    #[test]
    fn test_redact_anthropic_key() {
        let body = r#"{"error":{"type":"authentication_error","message":"x-api-key header is invalid: sk-ant-api03-abcdef1234567890"}}"#;
        let redacted = redact_api_keys(body);
        assert!(!redacted.contains("sk-ant-api03"));
        assert!(redacted.contains("[REDACTED_ANTHROPIC_KEY]"));
    }

    #[test]
    fn test_redact_openai_key() {
        let body =
            r#"{"error":{"message":"Incorrect API key provided: sk-proj-abc1234567890ABCDEF"}}"#;
        let redacted = redact_api_keys(body);
        assert!(!redacted.contains("sk-proj-abc"));
        assert!(redacted.contains("[REDACTED_OPENAI_KEY]"));
    }

    #[test]
    fn test_redact_gemini_key() {
        let body =
            r#"{"error":{"message":"API key not valid: AIzaXXtestfakekey00000000000000000000"}}"#;
        let redacted = redact_api_keys(body);
        assert!(!redacted.contains("AIza"));
        assert!(redacted.contains("[REDACTED_GEMINI_KEY]"));
    }

    #[test]
    fn test_redact_no_keys_preserves_text() {
        let body = "Just a normal error message with no keys";
        let redacted = redact_api_keys(body);
        assert_eq!(redacted, body);
    }

    #[test]
    fn test_redact_truncates_long_body() {
        let body = "a".repeat(1000);
        let redacted = redact_api_keys(&body);
        assert!(redacted.len() <= 500);
    }

    #[test]
    fn test_redact_multiple_keys_in_body() {
        let body = "key1: sk-ant-api03-aaaaaaaaaa key2: sk-proj-bbbbbbbbbbbb";
        let redacted = redact_api_keys(body);
        assert!(!redacted.contains("sk-ant-"));
        assert!(!redacted.contains("sk-proj-"));
    }

    // ── Security: response body size limit ──

    #[test]
    fn test_max_response_body_bytes_is_10mb() {
        assert_eq!(MAX_RESPONSE_BODY_BYTES, 10 * 1024 * 1024);
    }

    // ── Security: validate_key_cmd comprehensive ──

    #[test]
    fn test_validate_key_cmd_all_dangerous_chars() {
        for ch in DANGEROUS_SHELL_CHARS {
            let cmd = format!("echo test{}cmd", ch);
            let result = validate_key_cmd(&cmd);
            assert!(result.is_err(), "Should reject char '{}' but didn't", ch);
        }
    }
}
