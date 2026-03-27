use std::path::PathBuf;
use std::process::{Command as StdCommand, Stdio};

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use tempfile::NamedTempFile;
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
read/search tools or read-only shell commands when needed to inspect files, plans, and git state. \
Do not modify files. Return only the final JSON object that matches the provided schema.";

#[derive(Debug, Clone)]
pub struct CodexCliProvider {
    model: Option<String>,
    workdir: Option<PathBuf>,
}

impl CodexCliProvider {
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
        let Some(codex_path) = super::resolve_cli_executable("codex") else {
            return Err(LlmError::CommandUnavailable(
                "codex CLI is not installed".to_string(),
            ));
        };

        let status = detect_status();
        if !status.authenticated {
            return Err(LlmError::AuthError(
                "Codex CLI is installed but not logged in. Run `codex login`.".to_string(),
            ));
        }

        let flattened_schema = schema::flatten_json_schema(json_schema);
        let schema_file = NamedTempFile::new()
            .map_err(|e| LlmError::CommandFailed(format!("Failed to create schema file: {}", e)))?;
        let schema_text = serde_json::to_string_pretty(&flattened_schema)
            .map_err(|e| LlmError::ParseResponse(format!("Failed to serialize schema: {}", e)))?;
        std::fs::write(schema_file.path(), schema_text).map_err(|e| {
            LlmError::CommandFailed(format!("Failed to write schema file for codex: {}", e))
        })?;

        let output_file = NamedTempFile::new()
            .map_err(|e| LlmError::CommandFailed(format!("Failed to create output file: {}", e)))?;

        let prompt = format!("{}\n\n{}\n\n{}", system_prompt, AGENT_ADDENDUM, user_prompt);

        let mut command = Command::new(&codex_path);
        command
            .arg("exec")
            .arg("-C")
            .arg(self.workdir())
            .arg("--sandbox")
            .arg("read-only")
            .arg("--skip-git-repo-check")
            .arg("--output-schema")
            .arg(schema_file.path())
            .arg("--json")
            .arg("-o")
            .arg(output_file.path())
            .arg(prompt);

        if let Some(model) = self.selected_model() {
            command.arg("--model").arg(model);
        }

        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        super::emit_activity(super::ActivityUpdate::info(
            "codex",
            "Launching Codex CLI",
            Some("codex.launch".to_string()),
        ));

        let mut child = command
            .spawn()
            .map_err(|e| LlmError::CommandFailed(format!("Failed to launch codex: {}", e)))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| LlmError::CommandFailed("Failed to capture codex stdout".to_string()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| LlmError::CommandFailed("Failed to capture codex stderr".to_string()))?;
        let activity_callback = super::current_activity_callback();

        let stdout_activity_callback = activity_callback.clone();
        let stdout_task = tokio::spawn(async move {
            collect_codex_stream(stdout, "stdout", stdout_activity_callback).await
        });
        let stderr_task =
            tokio::spawn(
                async move { collect_codex_stream(stderr, "stderr", activity_callback).await },
            );

        let timeout_sleep = sleep(Duration::from_secs(CLI_TIMEOUT_SECS));
        tokio::pin!(timeout_sleep);

        let status = tokio::select! {
            result = child.wait() => result.map_err(|e| LlmError::CommandFailed(format!("Failed to wait for codex: {}", e)))?,
            _ = &mut timeout_sleep => {
                let _ = child.kill().await;
                return Err(LlmError::Timeout(CLI_TIMEOUT_SECS));
            }
        };

        let stdout = stdout_task
            .await
            .map_err(|e| {
                LlmError::CommandFailed(format!("Failed to join codex stdout task: {}", e))
            })?
            .map_err(|e| LlmError::CommandFailed(format!("Failed to read codex stdout: {}", e)))?;
        let stderr = stderr_task
            .await
            .map_err(|e| {
                LlmError::CommandFailed(format!("Failed to join codex stderr task: {}", e))
            })?
            .map_err(|e| LlmError::CommandFailed(format!("Failed to read codex stderr: {}", e)))?;

        if !status.success() {
            let combined = format!("{}\n{}", stderr, stdout);
            let message = redact_api_keys(&truncate_to_token_budget(&combined, 400));
            return Err(LlmError::CommandFailed(format!(
                "codex exec exited with {}: {}",
                status, message
            )));
        }

        let response_text = std::fs::read_to_string(output_file.path()).map_err(|e| {
            LlmError::CommandFailed(format!("Failed to read codex output file: {}", e))
        })?;

        serde_json::from_str(&response_text).map_err(|e| {
            LlmError::ParseResponse(format!(
                "Failed to parse Codex structured output: {} — response: {}",
                e,
                redact_api_keys(&truncate_to_token_budget(&response_text, 400))
            ))
        })
    }
}

#[async_trait]
impl LlmProvider for CodexCliProvider {
    fn name(&self) -> &str {
        "codex"
    }

    fn model(&self) -> &str {
        self.model.as_deref().unwrap_or("default")
    }

    fn max_context_tokens(&self) -> usize {
        400_000
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
    let Some(codex_path) = super::resolve_cli_executable("codex") else {
        return BackendStatus {
            installed: false,
            authenticated: false,
        };
    };

    let output = StdCommand::new(codex_path)
        .arg("login")
        .arg("status")
        .output();

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{}\n{}", stdout, stderr).to_lowercase();
            BackendStatus {
                installed: true,
                authenticated: output.status.success() && combined.contains("logged in"),
            }
        }
        Err(_) => BackendStatus {
            installed: false,
            authenticated: false,
        },
    }
}

async fn collect_codex_stream<R>(
    reader: R,
    stream_name: &str,
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
        if let Some(update) = summarize_codex_line(&line, stream_name) {
            if let Some(callback) = &activity_callback {
                callback(update);
            } else {
                super::emit_activity(update);
            }
        }
    }
    Ok(output)
}

fn summarize_codex_line(line: &str, stream_name: &str) -> Option<super::ActivityUpdate> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let parsed = serde_json::from_str::<serde_json::Value>(trimmed).ok()?;
    let event_type = parsed
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);

    match event_type.as_deref() {
        Some("thread.started") => Some(super::ActivityUpdate::info(
            "codex",
            "Started Codex session",
            event_type,
        )),
        Some("turn.started") => Some(super::ActivityUpdate::info(
            "codex",
            "Codex started working",
            event_type,
        )),
        Some("turn.completed") => Some(super::ActivityUpdate::info(
            "codex",
            "Codex finished its turn",
            event_type,
        )),
        Some("item.started") => summarize_codex_item(parsed.get("item")?, stream_name, true),
        Some("item.completed") => summarize_codex_item(parsed.get("item")?, stream_name, false),
        _ => Some(super::ActivityUpdate::info(
            "codex",
            truncate_for_activity(trimmed),
            event_type,
        )),
    }
}

fn summarize_codex_item(
    item: &serde_json::Value,
    stream_name: &str,
    in_progress: bool,
) -> Option<super::ActivityUpdate> {
    let item_type = item
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("item");

    match item_type {
        "agent_message" => {
            let text = item
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Codex produced a response");
            let message = if looks_like_structured_json(text) {
                "Codex prepared structured output".to_string()
            } else {
                truncate_for_activity(text)
            };
            Some(super::ActivityUpdate::info(
                "codex",
                message,
                Some(format!("{}.{}", stream_name, item_type)),
            ))
        }
        "command_execution" => {
            let command = item
                .get("command")
                .and_then(serde_json::Value::as_str)
                .map(pretty_codex_command)
                .unwrap_or_else(|| "a shell command".to_string());
            let message = if in_progress {
                format!("Codex is running {}", command)
            } else {
                format!("Codex finished {}", command)
            };
            Some(super::ActivityUpdate::info(
                "codex",
                message,
                Some(format!("{}.{}", stream_name, item_type)),
            ))
        }
        other => {
            let verb = if in_progress { "Started" } else { "Completed" };
            let detail = item
                .get("path")
                .and_then(serde_json::Value::as_str)
                .or_else(|| item.get("file").and_then(serde_json::Value::as_str))
                .map(truncate_for_activity);
            let base_message =
                if other.contains("search") || other.contains("grep") || other.contains("find") {
                    if in_progress {
                        "Searching the repo".to_string()
                    } else {
                        "Finished searching the repo".to_string()
                    }
                } else if other.contains("read") || other.contains("open") || other.contains("view")
                {
                    if in_progress {
                        "Inspecting a file".to_string()
                    } else {
                        "Finished inspecting a file".to_string()
                    }
                } else {
                    format!("{} {}", verb, humanize_event_name(other))
                };
            let message = detail
                .map(|detail| format!("{}: {}", base_message, detail))
                .unwrap_or(base_message);
            Some(super::ActivityUpdate::info(
                "codex",
                message,
                Some(format!("{}.{}", stream_name, other)),
            ))
        }
    }
}

fn humanize_event_name(value: &str) -> String {
    value.replace('_', " ")
}

fn pretty_codex_command(command: &str) -> String {
    let trimmed = command.trim();
    let zsh_prefix = "/bin/zsh -lc ";
    let sh_prefix = "/bin/sh -lc ";

    let unwrapped = if let Some(rest) = trimmed.strip_prefix(zsh_prefix) {
        rest
    } else if let Some(rest) = trimmed.strip_prefix(sh_prefix) {
        rest
    } else {
        trimmed
    };

    let cleaned = unwrapped
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
        .unwrap_or(unwrapped);

    truncate_for_activity(cleaned)
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

fn looks_like_structured_json(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with('{') && trimmed.ends_with('}')
}
