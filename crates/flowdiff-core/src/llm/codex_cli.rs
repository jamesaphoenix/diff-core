use std::path::PathBuf;
use std::process::Command as StdCommand;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use tempfile::NamedTempFile;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

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
        let status = detect_status();
        if !status.installed {
            return Err(LlmError::CommandUnavailable(
                "codex CLI is not installed".to_string(),
            ));
        }
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

        let mut command = Command::new("codex");
        command
            .arg("exec")
            .arg("-C")
            .arg(self.workdir())
            .arg("--sandbox")
            .arg("read-only")
            .arg("--skip-git-repo-check")
            .arg("--output-schema")
            .arg(schema_file.path())
            .arg("-o")
            .arg(output_file.path())
            .arg(prompt);

        if let Some(model) = self.selected_model() {
            command.arg("--model").arg(model);
        }

        let output = timeout(Duration::from_secs(CLI_TIMEOUT_SECS), command.output())
            .await
            .map_err(|_| LlmError::Timeout(CLI_TIMEOUT_SECS))?
            .map_err(|e| LlmError::CommandFailed(format!("Failed to launch codex: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let combined = format!("{}\n{}", stderr, stdout);
            let message = redact_api_keys(&truncate_to_token_budget(&combined, 400));
            return Err(LlmError::CommandFailed(format!(
                "codex exec exited with {}: {}",
                output.status, message
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
    let output = StdCommand::new("codex").arg("login").arg("status").output();

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
