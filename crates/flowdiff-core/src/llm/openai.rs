//! OpenAI Chat Completions API client for flowdiff LLM annotations.
//!
//! Implements the `LlmProvider` trait using the OpenAI Chat Completions API.
//! Uses `response_format: { type: "json_schema" }` for provider-native structured outputs.
//! Supports GPT-4o, o1, o3-mini, and o3 models.
//! See spec §5.1 for provider details.

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::schema::{
    JudgeResponse, Pass1Response, Pass2Response, RefinementResponse,
    pass1_json_schema, pass2_json_schema, judge_json_schema, refinement_json_schema,
    flatten_json_schema,
};
use super::{
    judge_system_prompt, judge_user_prompt, pass1_system_prompt, pass1_user_prompt,
    pass2_system_prompt, pass2_user_prompt, refinement_system_prompt, refinement_user_prompt,
    truncate_to_token_budget, LlmError, LlmProvider,
};
use crate::llm::schema::{JudgeRequest, Pass1Request, Pass2Request, RefinementRequest};

const OPENAI_API_URL: &str = "https://api.openai.com/v1/chat/completions";

/// OpenAI Chat Completions API provider.
#[derive(Debug, Clone)]
pub struct OpenAIProvider {
    api_key: String,
    model: String,
    client: Client,
    /// Base URL (overridable for testing).
    base_url: String,
}

impl OpenAIProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            client: Client::new(),
            base_url: OPENAI_API_URL.to_string(),
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

    /// Check if this model is a reasoning model (o-series, gpt-5.4) that doesn't support system messages.
    fn is_reasoning_model(&self) -> bool {
        self.model.starts_with("o1")
            || self.model.starts_with("o3")
            || self.model.starts_with("o4")
            || self.model.starts_with("gpt-5")
    }

    /// Check if this model supports structured outputs via response_format.
    fn supports_structured_outputs(&self) -> bool {
        self.model.starts_with("gpt-5")
            || self.model.starts_with("gpt-4.1")
            || self.model.starts_with("gpt-4o")
            || self.model.starts_with("o1")
            || self.model.starts_with("o3")
            || self.model.starts_with("o4")
    }

    /// Build and send a Chat Completions request with structured output support.
    async fn send_structured_message(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        schema: serde_json::Value,
        schema_name: &str,
    ) -> Result<String, LlmError> {
        // Flatten schema for OpenAI strict mode: inline $ref, add additionalProperties: false
        let schema = flatten_json_schema(schema);

        let max_input = self.max_context_tokens().saturating_sub(4096);
        let truncated_user = truncate_to_token_budget(user_prompt, max_input);

        let messages = if self.is_reasoning_model() {
            // Reasoning models (o1, o3) don't support system messages.
            // Prepend the system prompt to the user message instead.
            vec![ChatMessage {
                role: "user".to_string(),
                content: format!("{}\n\n---\n\n{}", system_prompt, truncated_user),
            }]
        } else {
            vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: system_prompt.to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: truncated_user,
                },
            ]
        };

        // Build response_format for structured outputs
        let response_format = if self.supports_structured_outputs() {
            Some(OpenAIResponseFormat {
                r#type: "json_schema".to_string(),
                json_schema: Some(OpenAIJsonSchema {
                    name: schema_name.to_string(),
                    strict: true,
                    schema,
                }),
            })
        } else {
            // Older models: use basic json_object mode
            Some(OpenAIResponseFormat {
                r#type: "json_object".to_string(),
                json_schema: None,
            })
        };

        let request = OpenAIRequest {
            model: self.model.clone(),
            messages,
            temperature: if self.is_reasoning_model() {
                None // o1/o3 don't support temperature
            } else {
                Some(0.0)
            },
            max_tokens: if self.is_reasoning_model() {
                None // o1/o3 use max_completion_tokens instead
            } else {
                Some(4096)
            },
            max_completion_tokens: if self.is_reasoning_model() {
                Some(4096)
            } else {
                None
            },
            response_format,
        };

        let response = self
            .client
            .post(&self.base_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
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
            return Err(LlmError::AuthError(
                "Invalid OpenAI API key".to_string(),
            ));
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

        let api_response: OpenAIResponse = serde_json::from_str(&body).map_err(|e| {
            LlmError::ParseResponse(format!(
                "Failed to parse OpenAI response: {} — body: {}",
                e,
                &body[..body.len().min(500)]
            ))
        })?;

        let text = api_response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        if text.is_empty() {
            return Err(LlmError::ParseResponse(
                "OpenAI response contained no text content".to_string(),
            ));
        }

        Ok(text)
    }
}

/// Return the max context tokens for known OpenAI models.
fn openai_context_window(model: &str) -> usize {
    if model.starts_with("gpt-5") {
        1_000_000
    } else if model.starts_with("gpt-4.1") {
        1_000_000
    } else if model.starts_with("o4") {
        200_000
    } else if model.starts_with("o3") {
        200_000
    } else if model.starts_with("o1") {
        200_000
    } else if model.starts_with("gpt-4o") {
        128_000
    } else if model.starts_with("gpt-4-turbo") {
        128_000
    } else if model.starts_with("gpt-4") {
        8_192
    } else {
        // Conservative default
        128_000
    }
}

#[async_trait]
impl LlmProvider for OpenAIProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn max_context_tokens(&self) -> usize {
        openai_context_window(&self.model)
    }

    async fn annotate_overview(&self, request: &Pass1Request) -> Result<Pass1Response, LlmError> {
        let system = pass1_system_prompt();
        let user = pass1_user_prompt(request);
        let response_text = self
            .send_structured_message(&system, &user, pass1_json_schema(), "pass1_response")
            .await?;
        parse_json_response::<Pass1Response>(&response_text)
    }

    async fn annotate_group(&self, request: &Pass2Request) -> Result<Pass2Response, LlmError> {
        let system = pass2_system_prompt();
        let user = pass2_user_prompt(request);
        let response_text = self
            .send_structured_message(&system, &user, pass2_json_schema(), "pass2_response")
            .await?;
        parse_json_response::<Pass2Response>(&response_text)
    }

    async fn evaluate_quality(
        &self,
        request: &JudgeRequest,
    ) -> Result<JudgeResponse, LlmError> {
        let system = judge_system_prompt();
        let user = judge_user_prompt(request);
        let response_text = self
            .send_structured_message(&system, &user, judge_json_schema(), "judge_response")
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
            .send_structured_message(&system, &user, refinement_json_schema(), "refinement_response")
            .await?;
        parse_json_response::<RefinementResponse>(&response_text)
    }
}

/// Parse a JSON response, stripping any markdown fencing the LLM may add.
/// Kept as defensive fallback — with response_format, responses should be clean JSON.
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
/// Defensive fallback — structured outputs shouldn't need this.
fn strip_markdown_json(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.starts_with("```json") {
        let after_fence = &trimmed[7..];
        if let Some(end) = after_fence.rfind("```") {
            return after_fence[..end].trim().to_string();
        }
    }
    if trimmed.starts_with("```") {
        let after_fence = &trimmed[3..];
        if let Some(end) = after_fence.rfind("```") {
            return after_fence[..end].trim().to_string();
        }
    }
    trimmed.to_string()
}

// ── OpenAI API Types ──

#[derive(Debug, Serialize)]
struct OpenAIRequest {
    model: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<u32>,
    /// Structured output format — `json_schema` for guaranteed schema compliance.
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<OpenAIResponseFormat>,
}

/// OpenAI response_format configuration for structured outputs.
#[derive(Debug, Serialize)]
struct OpenAIResponseFormat {
    r#type: String,
    /// JSON schema definition (only for type: "json_schema").
    #[serde(skip_serializing_if = "Option::is_none")]
    json_schema: Option<OpenAIJsonSchema>,
}

/// JSON schema definition within OpenAI's response_format.
#[derive(Debug, Serialize)]
struct OpenAIJsonSchema {
    name: String,
    strict: bool,
    schema: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct OpenAIResponse {
    choices: Vec<OpenAIChoice>,
    #[allow(dead_code)]
    model: Option<String>,
    #[allow(dead_code)]
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    message: ChatMessage,
    #[allow(dead_code)]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OpenAIUsage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod tests {
    use super::*;

    // ── Request Format Tests ──

    #[test]
    fn test_openai_request_format_standard_with_schema() {
        let provider = OpenAIProvider::new("key".to_string(), "gpt-4.1".to_string());
        assert!(!provider.is_reasoning_model());
        assert!(provider.supports_structured_outputs());

        let schema = serde_json::json!({"type": "object", "properties": {}});
        let request = OpenAIRequest {
            model: "gpt-4.1".to_string(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: "You are a reviewer".to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: "Review this diff".to_string(),
                },
            ],
            temperature: Some(0.0),
            max_tokens: Some(4096),
            max_completion_tokens: None,
            response_format: Some(OpenAIResponseFormat {
                r#type: "json_schema".to_string(),
                json_schema: Some(OpenAIJsonSchema {
                    name: "pass1_response".to_string(),
                    strict: true,
                    schema,
                }),
            }),
        };
        let json = serde_json::to_string(&request).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["model"], "gpt-4.1");
        assert_eq!(parsed["temperature"], 0.0);
        assert_eq!(parsed["max_tokens"], 4096);
        // Verify response_format
        assert_eq!(parsed["response_format"]["type"], "json_schema");
        assert_eq!(parsed["response_format"]["json_schema"]["name"], "pass1_response");
        assert_eq!(parsed["response_format"]["json_schema"]["strict"], true);
        assert!(parsed["response_format"]["json_schema"]["schema"].is_object());
    }

    #[test]
    fn test_openai_request_format_reasoning() {
        let provider = OpenAIProvider::new("key".to_string(), "o3-mini".to_string());
        assert!(provider.is_reasoning_model());
        assert!(provider.supports_structured_outputs());

        let request = OpenAIRequest {
            model: "o3-mini".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "System prompt\n\n---\n\nUser prompt".to_string(),
            }],
            temperature: None,
            max_tokens: None,
            max_completion_tokens: Some(4096),
            response_format: Some(OpenAIResponseFormat {
                r#type: "json_schema".to_string(),
                json_schema: Some(OpenAIJsonSchema {
                    name: "test".to_string(),
                    strict: true,
                    schema: serde_json::json!({}),
                }),
            }),
        };
        let json = serde_json::to_string(&request).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["model"], "o3-mini");
        assert!(parsed.get("temperature").is_none());
        assert!(parsed.get("max_tokens").is_none());
        assert_eq!(parsed["max_completion_tokens"], 4096);
        assert_eq!(parsed["messages"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["response_format"]["type"], "json_schema");
    }

    #[test]
    fn test_openai_request_format_no_schema() {
        let request = OpenAIRequest {
            model: "gpt-4".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
            }],
            temperature: Some(0.0),
            max_tokens: Some(4096),
            max_completion_tokens: None,
            response_format: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.get("response_format").is_none());
    }

    // ── Response Parsing Tests ──

    #[test]
    fn test_parse_openai_response() {
        let json = r#"{
            "choices": [{
                "message": {"role": "assistant", "content": "Hello!"},
                "finish_reason": "stop"
            }],
            "model": "gpt-4o-2024-05-13",
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        }"#;
        let response: OpenAIResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.choices[0].message.content, "Hello!");
    }

    #[test]
    fn test_parse_pass1_response_from_text() {
        let json = r#"{
            "groups": [{
                "id": "group_1",
                "name": "Auth flow",
                "summary": "Changes token refresh",
                "review_order_rationale": "Review first",
                "risk_flags": ["auth_change"]
            }],
            "overall_summary": "Auth changes",
            "suggested_review_order": ["group_1"]
        }"#;
        let result: Pass1Response = parse_json_response(json).unwrap();
        assert_eq!(result.groups.len(), 1);
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
                "risks": [],
                "suggestions": []
            }],
            "cross_cutting_concerns": []
        }"#;
        let result: Pass2Response = parse_json_response(json).unwrap();
        assert_eq!(result.group_id, "group_1");
    }

    // ── Reasoning Model Detection Tests ──

    #[test]
    fn test_is_reasoning_model() {
        assert!(OpenAIProvider::new("k".to_string(), "o1".to_string()).is_reasoning_model());
        assert!(
            OpenAIProvider::new("k".to_string(), "o1-preview".to_string()).is_reasoning_model()
        );
        assert!(
            OpenAIProvider::new("k".to_string(), "o3-mini".to_string()).is_reasoning_model()
        );
        assert!(OpenAIProvider::new("k".to_string(), "o3".to_string()).is_reasoning_model());
        assert!(
            OpenAIProvider::new("k".to_string(), "o4-mini".to_string()).is_reasoning_model()
        );
        assert!(
            OpenAIProvider::new("k".to_string(), "gpt-5.4".to_string()).is_reasoning_model()
        );
        assert!(
            OpenAIProvider::new("k".to_string(), "gpt-5.4-mini".to_string()).is_reasoning_model()
        );
        assert!(
            !OpenAIProvider::new("k".to_string(), "gpt-4.1".to_string()).is_reasoning_model()
        );
        assert!(
            !OpenAIProvider::new("k".to_string(), "gpt-4-turbo".to_string()).is_reasoning_model()
        );
    }

    // ── Structured Output Support Detection ──

    #[test]
    fn test_supports_structured_outputs() {
        assert!(OpenAIProvider::new("k".to_string(), "gpt-4.1".to_string()).supports_structured_outputs());
        assert!(OpenAIProvider::new("k".to_string(), "gpt-4o-2024-08-06".to_string()).supports_structured_outputs());
        assert!(OpenAIProvider::new("k".to_string(), "o1".to_string()).supports_structured_outputs());
        assert!(OpenAIProvider::new("k".to_string(), "o3-mini".to_string()).supports_structured_outputs());
        assert!(!OpenAIProvider::new("k".to_string(), "gpt-4-turbo".to_string()).supports_structured_outputs());
        assert!(!OpenAIProvider::new("k".to_string(), "gpt-4".to_string()).supports_structured_outputs());
    }

    // ── Context Window Tests ──

    #[test]
    fn test_context_window_gpt4o() {
        assert_eq!(openai_context_window("gpt-4o"), 128_000);
        assert_eq!(openai_context_window("gpt-4o-2024-05-13"), 128_000);
    }

    #[test]
    fn test_context_window_gpt41() {
        assert_eq!(openai_context_window("gpt-4.1"), 1_000_000);
        assert_eq!(openai_context_window("gpt-4.1-mini"), 1_000_000);
    }

    #[test]
    fn test_context_window_gpt5() {
        assert_eq!(openai_context_window("gpt-5.4"), 1_000_000);
        assert_eq!(openai_context_window("gpt-5.4-mini"), 1_000_000);
    }

    #[test]
    fn test_context_window_o4() {
        assert_eq!(openai_context_window("o4-mini"), 200_000);
    }

    #[test]
    fn test_context_window_reasoning() {
        assert_eq!(openai_context_window("o1"), 200_000);
        assert_eq!(openai_context_window("o1-preview"), 200_000);
        assert_eq!(openai_context_window("o3-mini"), 200_000);
        assert_eq!(openai_context_window("o3"), 200_000);
    }

    #[test]
    fn test_context_window_gpt4_turbo() {
        assert_eq!(openai_context_window("gpt-4-turbo"), 128_000);
        assert_eq!(openai_context_window("gpt-4-turbo-2024-04-09"), 128_000);
    }

    #[test]
    fn test_context_window_gpt4_base() {
        assert_eq!(openai_context_window("gpt-4"), 8_192);
    }

    #[test]
    fn test_context_window_unknown() {
        assert_eq!(openai_context_window("future-model"), 128_000);
    }

    // ── Provider Construction Tests ──

    #[test]
    fn test_openai_provider_new() {
        let provider = OpenAIProvider::new("key".to_string(), "gpt-4.1".to_string());
        assert_eq!(provider.name(), "openai");
        assert_eq!(provider.model(), "gpt-4.1");
        assert_eq!(provider.max_context_tokens(), 1_000_000);
    }

    #[test]
    fn test_openai_provider_with_base_url() {
        let provider = OpenAIProvider::with_base_url(
            "key".to_string(),
            "gpt-4.1".to_string(),
            "http://localhost:8080".to_string(),
        );
        assert_eq!(provider.base_url, "http://localhost:8080");
    }

    // ── Markdown Stripping Tests (defensive fallback) ──

    #[test]
    fn test_strip_markdown_json_fenced() {
        let input = "```json\n{\"key\": \"value\"}\n```";
        assert_eq!(strip_markdown_json(input), r#"{"key": "value"}"#);
    }

    #[test]
    fn test_strip_markdown_no_fence() {
        let input = r#"{"key": "value"}"#;
        assert_eq!(strip_markdown_json(input), r#"{"key": "value"}"#);
    }

    // ── Invalid Response Tests ──

    #[test]
    fn test_parse_invalid_json() {
        let result = parse_json_response::<Pass1Response>("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_empty_response() {
        let result = parse_json_response::<Pass1Response>("");
        assert!(result.is_err());
    }
}
