//! Anthropic Messages API client for flowdiff LLM annotations.
//!
//! Implements the `LlmProvider` trait using the Anthropic Messages API.
//! Uses tool_use with forced tool_choice for provider-native structured outputs.
//! Supports Claude models including extended thinking for reasoning models.
//! See spec §5.1 for provider details.

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::schema::{
    judge_json_schema, pass1_json_schema, pass2_json_schema, refinement_json_schema, JudgeResponse,
    Pass1Response, Pass2Response, RefinementResponse,
};
use super::{
    judge_system_prompt, judge_user_prompt, pass1_system_prompt, pass1_user_prompt,
    pass2_system_prompt, pass2_user_prompt, refinement_system_prompt, refinement_user_prompt,
    truncate_to_token_budget, LlmError, LlmProvider,
};
use crate::llm::schema::{JudgeRequest, Pass1Request, Pass2Request, RefinementRequest};

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_API_VERSION: &str = "2023-06-01";

/// Tool name used for structured output extraction.
const STRUCTURED_OUTPUT_TOOL: &str = "structured_output";

/// Anthropic Messages API provider.
#[derive(Debug, Clone)]
pub struct AnthropicProvider {
    api_key: String,
    model: String,
    client: Client,
    /// Base URL (overridable for testing).
    base_url: String,
}

impl AnthropicProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            client: Client::new(),
            base_url: ANTHROPIC_API_URL.to_string(),
        }
    }

    /// Create with a custom base URL (for testing with mock servers).
    ///
    /// Only available with the `test-support` feature or in `#[cfg(test)]` builds.
    /// Not exposed in production to prevent SSRF via configurable API endpoints.
    #[cfg(any(test, feature = "test-support"))]
    pub fn with_base_url(api_key: String, model: String, base_url: String) -> Self {
        Self {
            api_key,
            model,
            client: Client::new(),
            base_url,
        }
    }

    /// Build and send a Messages API request with tool_use for structured output.
    ///
    /// Uses Anthropic's tool_use with forced tool_choice to guarantee the response
    /// matches the provided JSON schema. Falls back to text extraction with markdown
    /// stripping if the response doesn't contain a tool_use block.
    async fn send_structured_message(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        schema: serde_json::Value,
        tool_description: &str,
    ) -> Result<String, LlmError> {
        let max_input = self.max_context_tokens().saturating_sub(4096);
        let truncated_user = truncate_to_token_budget(user_prompt, max_input);

        let request = AnthropicRequest {
            model: self.model.clone(),
            max_tokens: 4096,
            system: Some(system_prompt.to_string()),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: truncated_user,
            }],
            tools: Some(vec![AnthropicTool {
                name: STRUCTURED_OUTPUT_TOOL.to_string(),
                description: tool_description.to_string(),
                input_schema: schema,
            }]),
            tool_choice: Some(AnthropicToolChoice {
                r#type: "tool".to_string(),
                name: Some(STRUCTURED_OUTPUT_TOOL.to_string()),
            }),
        };

        let response = self
            .client
            .post(&self.base_url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_API_VERSION)
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await?;

        let status = response.status().as_u16();

        if status == 429 {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            return Err(LlmError::RateLimited {
                retry_after_secs: retry_after,
            });
        }

        if status == 401 {
            return Err(LlmError::AuthError("Invalid Anthropic API key".to_string()));
        }

        if status == 408 || status == 504 {
            return Err(LlmError::Timeout(120));
        }

        super::check_response_size(&response)?;
        let body = response.text().await?;

        if status != 200 {
            return Err(LlmError::ApiError {
                status,
                message: super::redact_api_keys(&body),
            });
        }

        let api_response: AnthropicResponse = serde_json::from_str(&body).map_err(|e| {
            LlmError::ParseResponse(format!(
                "Failed to parse Anthropic response: {} — body: {}",
                e,
                &body[..body.len().min(500)]
            ))
        })?;

        // Primary path: extract structured input from tool_use block
        for block in &api_response.content {
            if let ContentBlock::ToolUse { input, .. } = block {
                return serde_json::to_string(input).map_err(|e| {
                    LlmError::ParseResponse(format!("Failed to serialize tool_use input: {}", e))
                });
            }
        }

        // Fallback: extract text (for backward compat or if tool_use isn't in response)
        let text = api_response
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");

        if text.is_empty() {
            return Err(LlmError::ParseResponse(
                "Anthropic response contained no tool_use or text content".to_string(),
            ));
        }

        Ok(text)
    }
}

/// Return the max context tokens for known Anthropic models.
fn anthropic_context_window(model: &str) -> usize {
    if model.contains("claude-opus-4-6") || model.contains("claude-sonnet-4-6") {
        200_000
    } else if model.contains("claude-haiku-4-5")
        || model.contains("claude-haiku-4")
        || model.contains("claude-3-5-haiku")
    {
        200_000
    } else if model.contains("claude-sonnet-4") || model.contains("claude-opus-4") {
        200_000
    } else if model.contains("claude-3-5-sonnet") || model.contains("claude-3-7-sonnet") {
        200_000
    } else if model.contains("claude-3-opus") {
        200_000
    } else {
        // Conservative default for unknown models
        100_000
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn max_context_tokens(&self) -> usize {
        anthropic_context_window(&self.model)
    }

    async fn annotate_overview(&self, request: &Pass1Request) -> Result<Pass1Response, LlmError> {
        let system = pass1_system_prompt();
        let user = pass1_user_prompt(request);
        let response_text = self
            .send_structured_message(
                &system,
                &user,
                pass1_json_schema(),
                "Return the overview annotation for the diff analysis",
            )
            .await?;
        parse_json_response::<Pass1Response>(&response_text)
    }

    async fn annotate_group(&self, request: &Pass2Request) -> Result<Pass2Response, LlmError> {
        let system = pass2_system_prompt();
        let user = pass2_user_prompt(request);
        let response_text = self
            .send_structured_message(
                &system,
                &user,
                pass2_json_schema(),
                "Return the deep analysis for the flow group",
            )
            .await?;
        parse_json_response::<Pass2Response>(&response_text)
    }

    async fn evaluate_quality(&self, request: &JudgeRequest) -> Result<JudgeResponse, LlmError> {
        let system = judge_system_prompt();
        let user = judge_user_prompt(request);
        let response_text = self
            .send_structured_message(
                &system,
                &user,
                judge_json_schema(),
                "Return the quality evaluation scores",
            )
            .await?;
        parse_json_response::<JudgeResponse>(&response_text)
    }

    async fn refine_groups(
        &self,
        request: &RefinementRequest,
    ) -> Result<RefinementResponse, LlmError> {
        let system = refinement_system_prompt();
        let user = refinement_user_prompt(request);
        let response_text = self
            .send_structured_message(
                &system,
                &user,
                refinement_json_schema(),
                "Return the refinement operations for the flow groups",
            )
            .await?;
        parse_json_response::<RefinementResponse>(&response_text)
    }
}

/// Parse a JSON response, stripping any markdown fencing the LLM may add.
/// Kept as defensive fallback — with tool_use, responses are already clean JSON.
fn parse_json_response<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, LlmError> {
    let cleaned = strip_markdown_json(text);
    serde_json::from_str(&cleaned).map_err(|e| {
        LlmError::ParseResponse(format!(
            "Failed to parse structured output: {} — response: {}",
            e,
            &cleaned[..cleaned.len().min(500)]
        ))
    })
}

/// Strip markdown code fences from JSON responses.
/// Defensive fallback for text responses (tool_use responses don't need this).
fn strip_markdown_json(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.starts_with("```json") {
        let after_fence = &trimmed[7..]; // Skip ```json
        if let Some(end) = after_fence.rfind("```") {
            return after_fence[..end].trim().to_string();
        }
    }
    if trimmed.starts_with("```") {
        let after_fence = &trimmed[3..]; // Skip ```
        if let Some(end) = after_fence.rfind("```") {
            return after_fence[..end].trim().to_string();
        }
    }
    trimmed.to_string()
}

// ── Anthropic API Types ──

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    /// Tools for structured output via tool_use.
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    /// Force the model to use a specific tool.
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<AnthropicToolChoice>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

/// A tool definition for Anthropic's tool_use feature.
#[derive(Debug, Serialize)]
struct AnthropicTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

/// Tool choice configuration to force a specific tool.
#[derive(Debug, Serialize)]
struct AnthropicToolChoice {
    r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<ContentBlock>,
    #[allow(dead_code)]
    model: Option<String>,
    #[allow(dead_code)]
    stop_reason: Option<String>,
    #[allow(dead_code)]
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking {
        #[allow(dead_code)]
        thinking: String,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        #[allow(dead_code)]
        id: String,
        #[allow(dead_code)]
        name: String,
        input: serde_json::Value,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AnthropicUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
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

    // ── Request Format Tests ──

    #[test]
    fn test_anthropic_request_format_with_tools() {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "result": {"type": "string"}
            }
        });
        let request = AnthropicRequest {
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 4096,
            system: Some("You are a reviewer".to_string()),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: "Review this diff".to_string(),
            }],
            tools: Some(vec![AnthropicTool {
                name: "structured_output".to_string(),
                description: "Return structured result".to_string(),
                input_schema: schema,
            }]),
            tool_choice: Some(AnthropicToolChoice {
                r#type: "tool".to_string(),
                name: Some("structured_output".to_string()),
            }),
        };
        let json = serde_json::to_string(&request).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["model"], "claude-sonnet-4-6");
        assert_eq!(parsed["max_tokens"], 4096);
        assert_eq!(parsed["system"], "You are a reviewer");
        assert_eq!(parsed["messages"][0]["role"], "user");
        // Verify tools are present
        assert_eq!(parsed["tools"][0]["name"], "structured_output");
        assert!(parsed["tools"][0]["input_schema"].is_object());
        // Verify tool_choice forces the tool
        assert_eq!(parsed["tool_choice"]["type"], "tool");
        assert_eq!(parsed["tool_choice"]["name"], "structured_output");
    }

    #[test]
    fn test_anthropic_request_without_tools() {
        let request = AnthropicRequest {
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 4096,
            system: None,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
            }],
            tools: None,
            tool_choice: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.get("system").is_none());
        assert!(parsed.get("tools").is_none());
        assert!(parsed.get("tool_choice").is_none());
    }

    // ── Response Parsing Tests ──

    #[test]
    fn test_parse_tool_use_response() {
        let json = r#"{
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_01abc",
                    "name": "structured_output",
                    "input": {
                        "groups": [{
                            "id": "g1",
                            "name": "Auth flow",
                            "summary": "Changes auth",
                            "review_order_rationale": "Review first",
                            "risk_flags": ["auth_change"]
                        }],
                        "overall_summary": "Auth changes",
                        "suggested_review_order": ["g1"]
                    }
                }
            ],
            "model": "claude-sonnet-4-6",
            "stop_reason": "tool_use"
        }"#;
        let response: AnthropicResponse = serde_json::from_str(json).unwrap();

        // Should have one tool_use block
        let mut found_tool_use = false;
        for block in &response.content {
            if let ContentBlock::ToolUse { input, name, .. } = block {
                found_tool_use = true;
                assert_eq!(name, "structured_output");
                // The input should be directly deserializable
                let pass1: Pass1Response = serde_json::from_value(input.clone()).unwrap();
                assert_eq!(pass1.groups.len(), 1);
                assert_eq!(pass1.groups[0].id, "g1");
                assert_eq!(pass1.overall_summary, "Auth changes");
            }
        }
        assert!(found_tool_use, "Expected a tool_use content block");
    }

    #[test]
    fn test_parse_text_response_fallback() {
        let json = r#"{
            "content": [
                {"type": "text", "text": "Hello, world!"}
            ],
            "model": "claude-sonnet-4-6",
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 5}
        }"#;
        let response: AnthropicResponse = serde_json::from_str(json).unwrap();
        let text: String = response
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "Hello, world!");
    }

    #[test]
    fn test_parse_response_with_thinking_and_tool_use() {
        let json = r#"{
            "content": [
                {"type": "thinking", "thinking": "Let me analyze..."},
                {
                    "type": "tool_use",
                    "id": "toolu_01xyz",
                    "name": "structured_output",
                    "input": {"group_id": "g1", "flow_narrative": "Data flows...", "file_annotations": [], "cross_cutting_concerns": []}
                }
            ],
            "model": "claude-3-7-sonnet-20250219",
            "stop_reason": "tool_use"
        }"#;
        let response: AnthropicResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.content.len(), 2);

        // Should find tool_use block even with thinking block present
        let mut found = false;
        for block in &response.content {
            if let ContentBlock::ToolUse { input, .. } = block {
                let pass2: Pass2Response = serde_json::from_value(input.clone()).unwrap();
                assert_eq!(pass2.group_id, "g1");
                found = true;
            }
        }
        assert!(found);
    }

    #[test]
    fn test_parse_pass1_response_from_text() {
        let json = r#"{
            "groups": [{
                "id": "group_1",
                "name": "User auth flow",
                "summary": "Changes token refresh",
                "review_order_rationale": "Review first",
                "risk_flags": ["auth_change"]
            }],
            "overall_summary": "Auth changes",
            "suggested_review_order": ["group_1"]
        }"#;
        let result: Pass1Response = parse_json_response(json).unwrap();
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups[0].id, "group_1");
        assert_eq!(result.overall_summary, "Auth changes");
    }

    #[test]
    fn test_parse_pass2_response_from_text() {
        let json = r#"{
            "group_id": "group_1",
            "flow_narrative": "Data enters at POST /auth",
            "file_annotations": [{
                "file": "src/auth.rs",
                "role_in_flow": "Entrypoint",
                "changes_summary": "Added rotation",
                "risks": ["Race condition"],
                "suggestions": ["Add mutex"]
            }],
            "cross_cutting_concerns": ["Error handling"]
        }"#;
        let result: Pass2Response = parse_json_response(json).unwrap();
        assert_eq!(result.group_id, "group_1");
        assert_eq!(result.file_annotations.len(), 1);
    }

    // ── Markdown Stripping Tests (defensive fallback) ──

    #[test]
    fn test_strip_markdown_json_fenced() {
        let input = "```json\n{\"key\": \"value\"}\n```";
        assert_eq!(strip_markdown_json(input), r#"{"key": "value"}"#);
    }

    #[test]
    fn test_strip_markdown_plain_fence() {
        let input = "```\n{\"key\": \"value\"}\n```";
        assert_eq!(strip_markdown_json(input), r#"{"key": "value"}"#);
    }

    #[test]
    fn test_strip_markdown_no_fence() {
        let input = r#"{"key": "value"}"#;
        assert_eq!(strip_markdown_json(input), r#"{"key": "value"}"#);
    }

    #[test]
    fn test_strip_markdown_with_whitespace() {
        let input = "  ```json\n  {\"key\": \"value\"}  \n```  ";
        let result = strip_markdown_json(input);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["key"], "value");
    }

    // ── Tool Schema Tests ──

    #[test]
    fn test_tool_schema_included_in_request() {
        let schema = pass1_json_schema();
        let tool = AnthropicTool {
            name: STRUCTURED_OUTPUT_TOOL.to_string(),
            description: "test".to_string(),
            input_schema: schema.clone(),
        };
        let json = serde_json::to_string(&tool).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["name"], "structured_output");
        assert!(parsed["input_schema"].is_object());
        // Schema should contain the response type properties
        let schema_str = serde_json::to_string(&parsed["input_schema"]).unwrap();
        assert!(schema_str.contains("groups"));
    }

    #[test]
    fn test_tool_choice_forces_tool() {
        let choice = AnthropicToolChoice {
            r#type: "tool".to_string(),
            name: Some("structured_output".to_string()),
        };
        let json = serde_json::to_string(&choice).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "tool");
        assert_eq!(parsed["name"], "structured_output");
    }

    // ── Context Window Tests ──

    #[test]
    fn test_context_window_sonnet() {
        assert_eq!(anthropic_context_window("claude-sonnet-4-6"), 200_000);
        assert_eq!(
            anthropic_context_window("claude-3-5-sonnet-20240620"),
            200_000
        );
        assert_eq!(
            anthropic_context_window("claude-3-7-sonnet-20250219"),
            200_000
        );
    }

    #[test]
    fn test_context_window_opus() {
        assert_eq!(anthropic_context_window("claude-opus-4-6"), 200_000);
        assert_eq!(anthropic_context_window("claude-3-opus-20240229"), 200_000);
    }

    #[test]
    fn test_context_window_haiku() {
        assert_eq!(anthropic_context_window("claude-haiku-4-5"), 200_000);
        assert_eq!(
            anthropic_context_window("claude-3-5-haiku-20241022"),
            200_000
        );
        assert_eq!(anthropic_context_window("claude-haiku-4-20250514"), 200_000);
    }

    #[test]
    fn test_context_window_unknown() {
        assert_eq!(anthropic_context_window("some-future-model"), 100_000);
    }

    // ── Provider Construction Tests ──

    #[test]
    fn test_anthropic_provider_new() {
        let provider = AnthropicProvider::new("key".to_string(), "claude-sonnet-4-6".to_string());
        assert_eq!(provider.name(), "anthropic");
        assert_eq!(provider.model(), "claude-sonnet-4-6");
        assert_eq!(provider.max_context_tokens(), 200_000);
    }

    #[test]
    fn test_anthropic_provider_with_base_url() {
        let provider = AnthropicProvider::with_base_url(
            "key".to_string(),
            "claude-sonnet-4-6".to_string(),
            "http://localhost:8080".to_string(),
        );
        assert_eq!(provider.base_url, "http://localhost:8080");
    }

    // ── Invalid Response Tests ──

    #[test]
    fn test_parse_invalid_json() {
        let result = parse_json_response::<Pass1Response>("not valid json");
        assert!(result.is_err());
        match result.unwrap_err() {
            LlmError::ParseResponse(msg) => assert!(msg.contains("Failed to parse")),
            other => panic!("Expected ParseResponse, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_wrong_schema() {
        let json = r#"{"wrong_field": true}"#;
        let result = parse_json_response::<Pass1Response>(json);
        assert!(result.is_err());
    }
}
