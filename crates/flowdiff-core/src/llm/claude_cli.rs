use std::path::PathBuf;
use std::process::{Command as StdCommand, Stdio};

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::time::{sleep, Duration};

use super::schema;
use super::{
    redact_api_keys, truncate_to_token_budget, BackendStatus, JudgeRequest, JudgeResponse,
    LlmError, LlmProvider, Pass1Request, Pass1Response, Pass2Request, Pass2Response,
    RefinementRequest, RefinementResponse,
};

const CLI_TIMEOUT_SECS: u64 = 180;
const AGENT_ADDENDUM: &str =
    "You are running inside the target repository root. Use your built-in \
read/search tools or safe inspection commands when needed to inspect files, plans, and git state. \
Do not modify files. Return only the final JSON object that matches the provided schema.";

#[derive(Debug, Clone)]
pub struct ClaudeCliProvider {
    model: Option<String>,
    workdir: Option<PathBuf>,
}

impl ClaudeCliProvider {
    pub fn new(model: Option<String>, workdir: Option<PathBuf>) -> Self {
        Self { model, workdir }
    }

    fn selected_model(&self) -> Option<&str> {
        self.model
            .as_deref()
            .filter(|model| !model.is_empty() && *model != "default")
    }

    fn workdir(&self) -> PathBuf {
        self.workdir
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    async fn run_structured_prompt<T: DeserializeOwned>(
        &self,
        system_prompt: String,
        user_prompt: String,
        json_schema: serde_json::Value,
    ) -> Result<T, LlmError> {
        let Some(claude_path) = super::resolve_cli_executable("claude") else {
            return Err(LlmError::CommandUnavailable(
                "claude CLI is not installed".to_string(),
            ));
        };

        let status = detect_status();
        if !status.authenticated {
            return Err(LlmError::AuthError(
                "Claude Code is installed but not logged in. Run `claude auth login`.".to_string(),
            ));
        }

        let schema_text = serde_json::to_string(&schema::flatten_json_schema(json_schema))
            .map_err(|e| LlmError::ParseResponse(format!("Failed to serialize schema: {}", e)))?;

        let mut command = Command::new(&claude_path);
        command
            .current_dir(self.workdir())
            .arg("--print")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            .arg("--json-schema")
            .arg(schema_text)
            .arg("--system-prompt")
            .arg(format!("{}\n\n{}", system_prompt, AGENT_ADDENDUM))
            .arg(user_prompt);

        if let Some(model) = self.selected_model() {
            command.arg("--model").arg(model);
        }

        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        super::emit_activity(super::ActivityUpdate::info(
            "claude",
            "Launching Claude Code",
            Some("claude.launch".to_string()),
        ));

        let mut child = command
            .spawn()
            .map_err(|e| LlmError::CommandFailed(format!("Failed to launch claude: {}", e)))?;
        let stdout = child.stdout.take().ok_or_else(|| {
            LlmError::CommandFailed("Failed to capture claude stdout".to_string())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            LlmError::CommandFailed("Failed to capture claude stderr".to_string())
        })?;
        let activity_callback = super::current_activity_callback();

        let stdout_activity_callback = activity_callback.clone();
        let stdout_task = tokio::spawn(async move {
            collect_claude_stream(stdout, "stdout", true, stdout_activity_callback).await
        });
        let stderr_task = tokio::spawn(async move {
            collect_claude_stream(stderr, "stderr", false, activity_callback).await
        });

        let timeout_sleep = sleep(Duration::from_secs(CLI_TIMEOUT_SECS));
        tokio::pin!(timeout_sleep);

        let status = tokio::select! {
            result = child.wait() => result.map_err(|e| LlmError::CommandFailed(format!("Failed to wait for claude: {}", e)))?,
            _ = &mut timeout_sleep => {
                let _ = child.kill().await;
                return Err(LlmError::Timeout(CLI_TIMEOUT_SECS));
            }
        };

        let stdout = stdout_task
            .await
            .map_err(|e| {
                LlmError::CommandFailed(format!("Failed to join claude stdout task: {}", e))
            })?
            .map_err(|e| LlmError::CommandFailed(format!("Failed to read claude stdout: {}", e)))?;
        let stderr = stderr_task
            .await
            .map_err(|e| {
                LlmError::CommandFailed(format!("Failed to join claude stderr task: {}", e))
            })?
            .map_err(|e| LlmError::CommandFailed(format!("Failed to read claude stderr: {}", e)))?;

        if !status.success() {
            let combined = format!("{}\n{}", stderr, stdout);
            let message = redact_api_keys(&truncate_to_token_budget(&combined, 400));
            return Err(LlmError::CommandFailed(format!(
                "claude --print exited with {}: {}",
                status, message
            )));
        }

        let structured = parse_claude_structured_output(&stdout).ok_or_else(|| {
            LlmError::ParseResponse(format!(
                "Claude response did not include structured_output: {}",
                redact_api_keys(&truncate_to_token_budget(&stdout, 400))
            ))
        })?;

        serde_json::from_value(structured).map_err(|e| {
            LlmError::ParseResponse(format!(
                "Failed to parse Claude structured output: {} — response: {}",
                e,
                redact_api_keys(&truncate_to_token_budget(&stdout, 400))
            ))
        })
    }
}

#[async_trait]
impl LlmProvider for ClaudeCliProvider {
    fn name(&self) -> &str {
        "claude"
    }

    fn model(&self) -> &str {
        self.model.as_deref().unwrap_or("default")
    }

    fn max_context_tokens(&self) -> usize {
        1_000_000
    }

    async fn annotate_overview(&self, request: &Pass1Request) -> Result<Pass1Response, LlmError> {
        self.run_structured_prompt(
            super::pass1_system_prompt(),
            super::pass1_user_prompt(request),
            schema::pass1_json_schema(),
        )
        .await
    }

    async fn annotate_group(&self, request: &Pass2Request) -> Result<Pass2Response, LlmError> {
        self.run_structured_prompt(
            super::pass2_system_prompt(),
            super::pass2_user_prompt(request),
            schema::pass2_json_schema(),
        )
        .await
    }

    async fn evaluate_quality(&self, request: &JudgeRequest) -> Result<JudgeResponse, LlmError> {
        self.run_structured_prompt(
            super::judge_system_prompt(),
            super::judge_user_prompt(request),
            schema::judge_json_schema(),
        )
        .await
    }

    async fn refine_groups(
        &self,
        request: &RefinementRequest,
    ) -> Result<RefinementResponse, LlmError> {
        self.run_structured_prompt(
            super::refinement_system_prompt(),
            super::refinement_user_prompt(request),
            schema::refinement_json_schema(),
        )
        .await
    }
}

pub fn detect_status() -> BackendStatus {
    let Some(claude_path) = super::resolve_cli_executable("claude") else {
        return BackendStatus {
            installed: false,
            authenticated: false,
        };
    };

    let output = StdCommand::new(claude_path)
        .arg("auth")
        .arg("status")
        .output();

    match output {
        Ok(output) => {
            let authenticated = if output.status.success() {
                serde_json::from_slice::<serde_json::Value>(&output.stdout)
                    .ok()
                    .and_then(|json| json.get("loggedIn").and_then(serde_json::Value::as_bool))
                    .unwrap_or(false)
            } else {
                false
            };

            BackendStatus {
                installed: true,
                authenticated,
            }
        }
        Err(_) => BackendStatus {
            installed: false,
            authenticated: false,
        },
    }
}

async fn collect_claude_stream<R>(
    reader: R,
    stream_name: &str,
    parse_json: bool,
    activity_callback: Option<super::ActivityCallback>,
) -> Result<String, std::io::Error>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut output = String::new();
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await? {
        if !line.is_empty() {
            output.push_str(&line);
            output.push('\n');
        }
        if let Some(update) = summarize_claude_line(&line, stream_name, parse_json) {
            if let Some(callback) = &activity_callback {
                callback(update);
            } else {
                super::emit_activity(update);
            }
        }
    }
    Ok(output)
}

fn summarize_claude_line(
    line: &str,
    stream_name: &str,
    parse_json: bool,
) -> Option<super::ActivityUpdate> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    if !parse_json {
        return Some(super::ActivityUpdate::warning(
            "claude",
            truncate_for_activity(trimmed),
            Some(format!("{}.stderr", stream_name)),
        ));
    }

    let parsed = serde_json::from_str::<serde_json::Value>(trimmed).ok()?;
    let event_type = parsed
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);

    match event_type.as_deref() {
        Some("system") => match parsed
            .get("subtype")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
        {
            "init" => Some(super::ActivityUpdate::info(
                "claude",
                "Claude session initialized",
                Some("system.init".to_string()),
            )),
            "api_retry" => {
                let attempt = parsed
                    .get("attempt")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                let max_retries = parsed
                    .get("max_retries")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                Some(super::ActivityUpdate::warning(
                    "claude",
                    format!("Claude retrying request ({}/{})", attempt, max_retries),
                    Some("system.api_retry".to_string()),
                ))
            }
            other => Some(super::ActivityUpdate::info(
                "claude",
                format!("Claude system event: {}", other),
                event_type,
            )),
        },
        Some("assistant") => summarize_claude_assistant(&parsed),
        Some("result") => {
            let subtype = parsed
                .get("subtype")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("result");
            let is_error = parsed
                .get("is_error")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if is_error {
                Some(super::ActivityUpdate::error(
                    "claude",
                    format!("Claude finished with {}", subtype),
                    event_type,
                ))
            } else {
                Some(super::ActivityUpdate::info(
                    "claude",
                    "Claude completed the response",
                    event_type,
                ))
            }
        }
        Some("rate_limit_event") => Some(super::ActivityUpdate::warning(
            "claude",
            "Claude reported a rate limit event",
            event_type,
        )),
        _ => None,
    }
}

fn summarize_claude_assistant(parsed: &serde_json::Value) -> Option<super::ActivityUpdate> {
    let content = parsed
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(serde_json::Value::as_array)?;

    let first_item = content.first()?;
    let content_type = first_item
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("assistant");

    match content_type {
        "thinking" => Some(super::ActivityUpdate::info(
            "claude",
            "Claude is reasoning",
            Some("assistant.thinking".to_string()),
        )),
        "text" => Some(super::ActivityUpdate::info(
            "claude",
            truncate_for_activity(
                first_item
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("Claude sent text"),
            ),
            Some("assistant.text".to_string()),
        )),
        "tool_use" => {
            let tool_name = first_item
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("tool");
            let description = first_item
                .get("input")
                .and_then(|input| input.get("description"))
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    first_item
                        .get("input")
                        .and_then(|input| input.get("prompt"))
                        .and_then(serde_json::Value::as_str)
                })
                .map(truncate_for_activity);
            let message = match description {
                Some(description) => format!("Claude used {}: {}", tool_name, description),
                None => format!("Claude used {}", tool_name),
            };
            Some(super::ActivityUpdate::info(
                "claude",
                message,
                Some(format!("assistant.tool_use.{}", tool_name)),
            ))
        }
        other => Some(super::ActivityUpdate::info(
            "claude",
            format!("Claude emitted {}", other),
            Some(format!("assistant.{}", other)),
        )),
    }
}

fn parse_claude_structured_output(output: &str) -> Option<serde_json::Value> {
    for line in output.lines().rev() {
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if parsed.get("type").and_then(serde_json::Value::as_str) == Some("result") {
            if let Some(structured) = parsed.get("structured_output") {
                return Some(structured.clone());
            }
        }
    }
    None
}

fn truncate_for_activity(text: &str) -> String {
    const MAX_LEN: usize = 180;
    if text.chars().count() <= MAX_LEN {
        text.to_string()
    } else {
        let truncated: String = text.chars().take(MAX_LEN).collect();
        format!("{}...", truncated)
    }
}
