//! OpenAI Chat Completions API client for flowdiff LLM annotations.
//!
//! Implements the `LlmProvider` trait using the OpenAI Chat Completions API.
//! Supports GPT-4o, o1, o3-mini, and o3 models.
//! See spec §5.1 for provider details.

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::schema::{JudgeResponse, Pass1Response, Pass2Response};
use super::{
    judge_system_prompt, judge_user_prompt, pass1_system_prompt, pass1_user_prompt,
    pass2_system_prompt, pass2_user_prompt, truncate_to_token_budget, LlmError, LlmProvider,
};
use crate::llm::schema::{JudgeRequest, Pass1Request, Pass2Request};

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
    pub fn with_base_url(api_key: String, model: String, base_url: String) -> Self {
        Self {
            api_key,
            model,
            client: Client::new(),
            base_url,
        }
    }

    /// Check if this model is a reasoning model (o1, o3) that doesn't support system messages.
    fn is_reasoning_model(&self) -> bool {
        self.model.starts_with("o1") || self.model.starts_with("o3")
    }

    /// Build and send a Chat Completions request.
    async fn send_message(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<String, LlmError> {
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

        let body = response.text().await?;

        if status != 200 {
            return Err(LlmError::ApiError {
                status,
                message: body,
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
    if model.starts_with("gpt-4o") {
        128_000
    } else if model.starts_with("o1") {
        200_000
    } else if model.starts_with("o3") {
        200_000
    } else if model.starts_with("gpt-4-turbo") {
        128_000
    } else if model.starts_with("gpt-4") {
        8_192
    } else {
        // Conservative default
        8_192
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
        let response_text = self.send_message(&system, &user).await?;
        parse_json_response::<Pass1Response>(&response_text)
    }

    async fn annotate_group(&self, request: &Pass2Request) -> Result<Pass2Response, LlmError> {
        let system = pass2_system_prompt();
        let user = pass2_user_prompt(request);
        let response_text = self.send_message(&system, &user).await?;
        parse_json_response::<Pass2Response>(&response_text)
    }

    async fn evaluate_quality(
        &self,
        request: &JudgeRequest,
    ) -> Result<JudgeResponse, LlmError> {
        let system = judge_system_prompt();
        let user = judge_user_prompt(request);
        let response_text = self.send_message(&system, &user).await?;
        parse_json_response::<JudgeResponse>(&response_text)
    }
}

/// Parse a JSON response, stripping any markdown fencing the LLM may add.
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
mod tests {
    use super::*;

    // ── Request Format Tests ──

    #[test]
    fn test_openai_request_format_standard() {
        let provider = OpenAIProvider::new("key".to_string(), "gpt-4o".to_string());
        assert!(!provider.is_reasoning_model());

        let request = OpenAIRequest {
            model: "gpt-4o".to_string(),
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
        };
        let json = serde_json::to_string(&request).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["model"], "gpt-4o");
        assert_eq!(parsed["temperature"], 0.0);
        assert_eq!(parsed["max_tokens"], 4096);
        assert!(parsed.get("max_completion_tokens").is_none());
        assert_eq!(parsed["messages"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["messages"][0]["role"], "system");
    }

    #[test]
    fn test_openai_request_format_reasoning() {
        let provider = OpenAIProvider::new("key".to_string(), "o3-mini".to_string());
        assert!(provider.is_reasoning_model());

        // Reasoning models: no system message, no temperature, use max_completion_tokens
        let request = OpenAIRequest {
            model: "o3-mini".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: "System prompt\n\n---\n\nUser prompt".to_string(),
            }],
            temperature: None,
            max_tokens: None,
            max_completion_tokens: Some(4096),
        };
        let json = serde_json::to_string(&request).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["model"], "o3-mini");
        assert!(parsed.get("temperature").is_none());
        assert!(parsed.get("max_tokens").is_none());
        assert_eq!(parsed["max_completion_tokens"], 4096);
        assert_eq!(parsed["messages"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["messages"][0]["role"], "user");
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
            !OpenAIProvider::new("k".to_string(), "gpt-4o".to_string()).is_reasoning_model()
        );
        assert!(
            !OpenAIProvider::new("k".to_string(), "gpt-4-turbo".to_string()).is_reasoning_model()
        );
    }

    // ── Context Window Tests ──

    #[test]
    fn test_context_window_gpt4o() {
        assert_eq!(openai_context_window("gpt-4o"), 128_000);
        assert_eq!(openai_context_window("gpt-4o-2024-05-13"), 128_000);
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
        assert_eq!(openai_context_window("future-model"), 8_192);
    }

    // ── Provider Construction Tests ──

    #[test]
    fn test_openai_provider_new() {
        let provider = OpenAIProvider::new("key".to_string(), "gpt-4o".to_string());
        assert_eq!(provider.name(), "openai");
        assert_eq!(provider.model(), "gpt-4o");
        assert_eq!(provider.max_context_tokens(), 128_000);
    }

    #[test]
    fn test_openai_provider_with_base_url() {
        let provider = OpenAIProvider::with_base_url(
            "key".to_string(),
            "gpt-4o".to_string(),
            "http://localhost:8080".to_string(),
        );
        assert_eq!(provider.base_url, "http://localhost:8080");
    }

    // ── Markdown Stripping Tests ──

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
