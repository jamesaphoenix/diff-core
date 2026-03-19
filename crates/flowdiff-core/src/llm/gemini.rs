//! Google Gemini API client for flowdiff LLM annotations.
//!
//! Implements the `LlmProvider` trait using the Gemini `generateContent` API.
//! Supports Gemini 2.5 Pro, Gemini 2.5 Flash, and Gemini 2.0 Flash models.
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

const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";

/// Google Gemini API provider.
#[derive(Debug, Clone)]
pub struct GeminiProvider {
    api_key: String,
    model: String,
    client: Client,
    /// Base URL (overridable for testing).
    base_url: String,
}

impl GeminiProvider {
    pub fn new(api_key: String, model: String) -> Self {
        Self {
            api_key,
            model,
            client: Client::new(),
            base_url: GEMINI_API_BASE.to_string(),
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

    /// Build the full endpoint URL for generateContent.
    fn endpoint_url(&self) -> String {
        format!(
            "{}/{}:generateContent",
            self.base_url, self.model
        )
    }

    /// Build and send a generateContent request.
    async fn send_message(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<String, LlmError> {
        let max_input = self.max_context_tokens().saturating_sub(8192); // Reserve tokens for output
        let truncated_user = truncate_to_token_budget(user_prompt, max_input);

        let request = GeminiRequest {
            system_instruction: Some(GeminiContent {
                parts: vec![GeminiPart::Text {
                    text: system_prompt.to_string(),
                }],
                role: None,
            }),
            contents: vec![GeminiContent {
                parts: vec![GeminiPart::Text {
                    text: truncated_user,
                }],
                role: Some("user".to_string()),
            }],
            generation_config: Some(GenerationConfig {
                temperature: Some(0.0),
                max_output_tokens: Some(8192),
                response_mime_type: Some("application/json".to_string()),
            }),
        };

        let response = self
            .client
            .post(&self.endpoint_url())
            .header("x-goog-api-key", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await?;

        let status = response.status().as_u16();

        if status == 429 {
            return Err(LlmError::RateLimited {
                retry_after_secs: None,
            });
        }

        if status == 401 || status == 403 {
            return Err(LlmError::AuthError(
                "Invalid Google Gemini API key".to_string(),
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

        let api_response: GeminiResponse = serde_json::from_str(&body).map_err(|e| {
            LlmError::ParseResponse(format!(
                "Failed to parse Gemini response: {} — body: {}",
                e,
                &body[..body.len().min(500)]
            ))
        })?;

        // Extract text from the first candidate's content parts
        let text = api_response
            .candidates
            .as_ref()
            .and_then(|candidates| candidates.first())
            .and_then(|candidate| candidate.content.as_ref())
            .map(|content| {
                content
                    .parts
                    .iter()
                    .map(|part| {
                        let GeminiPart::Text { text } = part;
                        text.as_str()
                    })
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();

        if text.is_empty() {
            // Check for blocked content or other issues
            if let Some(ref candidates) = api_response.candidates {
                if let Some(candidate) = candidates.first() {
                    if let Some(ref reason) = candidate.finish_reason {
                        if reason == "SAFETY" || reason == "RECITATION" {
                            return Err(LlmError::ApiError {
                                status: 200,
                                message: format!("Content blocked by Gemini safety filter: {}", reason),
                            });
                        }
                    }
                }
            }
            return Err(LlmError::ParseResponse(
                "Gemini response contained no text content".to_string(),
            ));
        }

        Ok(text)
    }
}

/// Return the max context tokens for known Gemini models.
fn gemini_context_window(model: &str) -> usize {
    if model.contains("gemini-2.5-pro") {
        1_048_576
    } else if model.contains("gemini-2.5-flash") {
        1_048_576
    } else if model.contains("gemini-2.0-flash") {
        1_048_576
    } else if model.contains("gemini-1.5-pro") {
        1_048_576
    } else if model.contains("gemini-1.5-flash") {
        1_048_576
    } else {
        // Conservative default for unknown models
        32_000
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    fn name(&self) -> &str {
        "gemini"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn max_context_tokens(&self) -> usize {
        gemini_context_window(&self.model)
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

// ── Gemini API Types ──

#[derive(Debug, Serialize)]
struct GeminiRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    contents: Vec<GeminiContent>,
    #[serde(rename = "generationConfig", skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiContent {
    parts: Vec<GeminiPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum GeminiPart {
    Text { text: String },
}

#[derive(Debug, Serialize)]
struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(rename = "maxOutputTokens", skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(rename = "responseMimeType", skip_serializing_if = "Option::is_none")]
    response_mime_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    #[allow(dead_code)]
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiContent>,
    #[allow(dead_code)]
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GeminiUsageMetadata {
    #[serde(rename = "promptTokenCount")]
    prompt_token_count: Option<u64>,
    #[serde(rename = "candidatesTokenCount")]
    candidates_token_count: Option<u64>,
    #[serde(rename = "totalTokenCount")]
    total_token_count: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Request Format Tests ──

    #[test]
    fn test_gemini_request_format() {
        let request = GeminiRequest {
            system_instruction: Some(GeminiContent {
                parts: vec![GeminiPart::Text {
                    text: "You are a reviewer".to_string(),
                }],
                role: None,
            }),
            contents: vec![GeminiContent {
                parts: vec![GeminiPart::Text {
                    text: "Review this diff".to_string(),
                }],
                role: Some("user".to_string()),
            }],
            generation_config: Some(GenerationConfig {
                temperature: Some(0.0),
                max_output_tokens: Some(8192),
                response_mime_type: Some("application/json".to_string()),
            }),
        };
        let json = serde_json::to_string(&request).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(
            parsed["system_instruction"]["parts"][0]["text"],
            "You are a reviewer"
        );
        assert_eq!(parsed["contents"][0]["parts"][0]["text"], "Review this diff");
        assert_eq!(parsed["contents"][0]["role"], "user");
        assert_eq!(parsed["generationConfig"]["temperature"], 0.0);
        assert_eq!(parsed["generationConfig"]["maxOutputTokens"], 8192);
        assert_eq!(
            parsed["generationConfig"]["responseMimeType"],
            "application/json"
        );
    }

    #[test]
    fn test_gemini_request_without_system() {
        let request = GeminiRequest {
            system_instruction: None,
            contents: vec![GeminiContent {
                parts: vec![GeminiPart::Text {
                    text: "Hello".to_string(),
                }],
                role: Some("user".to_string()),
            }],
            generation_config: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.get("system_instruction").is_none());
        assert!(parsed.get("generationConfig").is_none());
    }

    // ── Response Parsing Tests ──

    #[test]
    fn test_parse_gemini_response_text() {
        let json = r#"{
            "candidates": [
                {
                    "content": {
                        "parts": [{"text": "Hello, world!"}],
                        "role": "model"
                    },
                    "finishReason": "STOP"
                }
            ],
            "usageMetadata": {
                "promptTokenCount": 10,
                "candidatesTokenCount": 5,
                "totalTokenCount": 15
            }
        }"#;
        let response: GeminiResponse = serde_json::from_str(json).unwrap();
        let text: String = response
            .candidates
            .unwrap()
            .first()
            .unwrap()
            .content
            .as_ref()
            .unwrap()
            .parts
            .iter()
            .filter_map(|p| match p {
                GeminiPart::Text { text } => Some(text.clone()),
            })
            .collect();
        assert_eq!(text, "Hello, world!");
    }

    #[test]
    fn test_parse_gemini_response_multi_part() {
        let json = r#"{
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {"text": "Part 1. "},
                            {"text": "Part 2."}
                        ],
                        "role": "model"
                    },
                    "finishReason": "STOP"
                }
            ]
        }"#;
        let response: GeminiResponse = serde_json::from_str(json).unwrap();
        let text: String = response
            .candidates
            .unwrap()
            .first()
            .unwrap()
            .content
            .as_ref()
            .unwrap()
            .parts
            .iter()
            .filter_map(|p| match p {
                GeminiPart::Text { text } => Some(text.clone()),
            })
            .collect();
        assert_eq!(text, "Part 1. Part 2.");
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

    // ── Markdown Stripping Tests ──

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

    // ── Context Window Tests ──

    #[test]
    fn test_context_window_gemini_25_pro() {
        assert_eq!(gemini_context_window("gemini-2.5-pro"), 1_048_576);
        assert_eq!(
            gemini_context_window("gemini-2.5-pro-preview-05-06"),
            1_048_576
        );
    }

    #[test]
    fn test_context_window_gemini_25_flash() {
        assert_eq!(gemini_context_window("gemini-2.5-flash"), 1_048_576);
        assert_eq!(
            gemini_context_window("gemini-2.5-flash-preview-04-17"),
            1_048_576
        );
    }

    #[test]
    fn test_context_window_gemini_20_flash() {
        assert_eq!(gemini_context_window("gemini-2.0-flash"), 1_048_576);
    }

    #[test]
    fn test_context_window_gemini_15() {
        assert_eq!(gemini_context_window("gemini-1.5-pro"), 1_048_576);
        assert_eq!(gemini_context_window("gemini-1.5-flash"), 1_048_576);
    }

    #[test]
    fn test_context_window_unknown() {
        assert_eq!(gemini_context_window("some-future-model"), 32_000);
    }

    // ── Provider Construction Tests ──

    #[test]
    fn test_gemini_provider_new() {
        let provider = GeminiProvider::new("key".to_string(), "gemini-2.5-flash".to_string());
        assert_eq!(provider.name(), "gemini");
        assert_eq!(provider.model(), "gemini-2.5-flash");
        assert_eq!(provider.max_context_tokens(), 1_048_576);
    }

    #[test]
    fn test_gemini_provider_with_base_url() {
        let provider = GeminiProvider::with_base_url(
            "key".to_string(),
            "gemini-2.5-flash".to_string(),
            "http://localhost:8080".to_string(),
        );
        assert_eq!(provider.base_url, "http://localhost:8080");
    }

    #[test]
    fn test_gemini_endpoint_url() {
        let provider = GeminiProvider::new("key".to_string(), "gemini-2.5-flash".to_string());
        let url = provider.endpoint_url();
        assert_eq!(
            url,
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent"
        );
    }

    #[test]
    fn test_gemini_endpoint_url_custom_base() {
        let provider = GeminiProvider::with_base_url(
            "key".to_string(),
            "gemini-2.5-pro".to_string(),
            "http://localhost:8080".to_string(),
        );
        let url = provider.endpoint_url();
        assert_eq!(
            url,
            "http://localhost:8080/gemini-2.5-pro:generateContent"
        );
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

    #[test]
    fn test_parse_empty_response() {
        let result = parse_json_response::<Pass1Response>("");
        assert!(result.is_err());
    }

    // ── Response Edge Cases ──

    #[test]
    fn test_parse_response_no_candidates() {
        let json = r#"{"usageMetadata": {"totalTokenCount": 0}}"#;
        let response: GeminiResponse = serde_json::from_str(json).unwrap();
        assert!(response.candidates.is_none());
    }

    #[test]
    fn test_parse_response_empty_candidates() {
        let json = r#"{"candidates": []}"#;
        let response: GeminiResponse = serde_json::from_str(json).unwrap();
        assert!(response.candidates.unwrap().is_empty());
    }

    #[test]
    fn test_parse_response_with_finish_reason() {
        let json = r#"{
            "candidates": [{
                "content": {
                    "parts": [{"text": "result"}],
                    "role": "model"
                },
                "finishReason": "STOP"
            }]
        }"#;
        let response: GeminiResponse = serde_json::from_str(json).unwrap();
        let candidate = &response.candidates.unwrap()[0];
        assert_eq!(candidate.finish_reason.as_deref(), Some("STOP"));
    }

    #[test]
    fn test_parse_response_safety_blocked() {
        let json = r#"{
            "candidates": [{
                "finishReason": "SAFETY"
            }]
        }"#;
        let response: GeminiResponse = serde_json::from_str(json).unwrap();
        let candidate = &response.candidates.unwrap()[0];
        assert_eq!(candidate.finish_reason.as_deref(), Some("SAFETY"));
        assert!(candidate.content.is_none());
    }

    // ── Serialization Roundtrip Tests ──

    #[test]
    fn test_gemini_content_roundtrip() {
        let content = GeminiContent {
            parts: vec![GeminiPart::Text {
                text: "test content".to_string(),
            }],
            role: Some("user".to_string()),
        };
        let json = serde_json::to_string(&content).unwrap();
        let deserialized: GeminiContent = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.role, Some("user".to_string()));
        match &deserialized.parts[0] {
            GeminiPart::Text { text } => assert_eq!(text, "test content"),
        }
    }

    #[test]
    fn test_generation_config_serialization() {
        let config = GenerationConfig {
            temperature: Some(0.5),
            max_output_tokens: Some(4096),
            response_mime_type: Some("application/json".to_string()),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["temperature"], 0.5);
        assert_eq!(parsed["maxOutputTokens"], 4096);
        assert_eq!(parsed["responseMimeType"], "application/json");
    }
}
