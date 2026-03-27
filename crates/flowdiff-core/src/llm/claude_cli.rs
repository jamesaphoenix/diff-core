use std::path::PathBuf;
use std::process::Command as StdCommand;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
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
        let status = detect_status();
        if !status.installed {
            return Err(LlmError::CommandUnavailable(
                "claude CLI is not installed".to_string(),
            ));
        }
        if !status.authenticated {
            return Err(LlmError::AuthError(
                "Claude Code is installed but not logged in. Run `claude auth login`.".to_string(),
            ));
        }

        let schema_text = serde_json::to_string(&schema::flatten_json_schema(json_schema))
            .map_err(|e| LlmError::ParseResponse(format!("Failed to serialize schema: {}", e)))?;

        let mut command = Command::new("claude");
        command
            .current_dir(self.workdir())
            .arg("--print")
            .arg("--output-format")
            .arg("json")
            .arg("--json-schema")
            .arg(schema_text)
            .arg("--system-prompt")
            .arg(format!("{}\n\n{}", system_prompt, AGENT_ADDENDUM))
            .arg(user_prompt);

        if let Some(model) = self.selected_model() {
            command.arg("--model").arg(model);
        }

        let output = timeout(Duration::from_secs(CLI_TIMEOUT_SECS), command.output())
            .await
            .map_err(|_| LlmError::Timeout(CLI_TIMEOUT_SECS))?
            .map_err(|e| LlmError::CommandFailed(format!("Failed to launch claude: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let combined = format!("{}\n{}", stderr, stdout);
            let message = redact_api_keys(&truncate_to_token_budget(&combined, 400));
            return Err(LlmError::CommandFailed(format!(
                "claude --print exited with {}: {}",
                output.status, message
            )));
        }

        let response_text = String::from_utf8_lossy(&output.stdout).to_string();
        let parsed: serde_json::Value = serde_json::from_str(&response_text).map_err(|e| {
            LlmError::ParseResponse(format!(
                "Failed to parse Claude response envelope: {} — response: {}",
                e,
                redact_api_keys(&truncate_to_token_budget(&response_text, 400))
            ))
        })?;

        let structured = parsed.get("structured_output").cloned().ok_or_else(|| {
            LlmError::ParseResponse(format!(
                "Claude response did not include structured_output: {}",
                redact_api_keys(&truncate_to_token_budget(&response_text, 400))
            ))
        })?;

        serde_json::from_value(structured).map_err(|e| {
            LlmError::ParseResponse(format!(
                "Failed to parse Claude structured output: {} — response: {}",
                e,
                redact_api_keys(&truncate_to_token_budget(&response_text, 400))
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
    let output = StdCommand::new("claude").arg("auth").arg("status").output();

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
