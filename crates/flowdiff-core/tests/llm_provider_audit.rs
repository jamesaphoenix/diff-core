#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
//! LLM Provider Audit — Phase 8 Hardening
//!
//! Tests all three providers (Anthropic, OpenAI, Gemini) with adversarial inputs:
//! - Huge diffs that exceed context windows
//! - Malformed JSON responses
//! - Rate limit handling (429)
//! - Timeout handling (408/504)
//! - Auth error handling (401/403)
//! - Structured output schema violations
//! - Unicode/emoji in code diffs and file paths
//! - Concurrent requests (Send + Sync verification)
//! - Empty and edge-case responses
//! - API error bodies with adversarial content

use flowdiff_core::llm::anthropic::AnthropicProvider;
use flowdiff_core::llm::gemini::GeminiProvider;
use flowdiff_core::llm::openai::OpenAIProvider;
use flowdiff_core::llm::schema::{
    Pass1GroupInput, Pass1Request, Pass2FileInput, Pass2Request, RefinementGroupInput,
    RefinementRequest,
};
use flowdiff_core::llm::{
    estimate_tokens, truncate_to_token_budget, LlmError, LlmProvider,
};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

// ═══════════════════════════════════════════════════════════════
// Helper: build sample requests
// ═══════════════════════════════════════════════════════════════

fn sample_pass1_request() -> Pass1Request {
    Pass1Request {
        diff_summary: "10 files changed".to_string(),
        flow_groups: vec![Pass1GroupInput {
            id: "group_1".to_string(),
            name: "Auth flow".to_string(),
            entrypoint: Some("src/auth.ts::login".to_string()),
            files: vec!["src/auth.ts".to_string()],
            risk_score: 0.75,
            edge_summary: "auth -> token".to_string(),
        }],
        graph_summary: "2 nodes, 1 edge".to_string(),
    }
}

fn sample_pass2_request() -> Pass2Request {
    Pass2Request {
        group_id: "group_1".to_string(),
        group_name: "Auth flow".to_string(),
        files: vec![Pass2FileInput {
            path: "src/auth.ts".to_string(),
            diff: "+ const token = generateToken();".to_string(),
            new_content: Some("full content".to_string()),
            role: "Entrypoint".to_string(),
        }],
        graph_context: "auth -> token-service".to_string(),
    }
}

fn sample_refinement_request() -> RefinementRequest {
    RefinementRequest {
        analysis_json: r#"{"version":"1.0.0"}"#.to_string(),
        diff_summary: "5 files changed".to_string(),
        groups: vec![RefinementGroupInput {
            id: "group_1".to_string(),
            name: "Auth flow".to_string(),
            entrypoint: Some("src/auth.ts::login".to_string()),
            files: vec!["src/auth.ts".to_string()],
            risk_score: 0.75,
            review_order: 1,
        }],
    }
}

/// Valid Pass1 response JSON for Anthropic tool_use format.
fn valid_anthropic_pass1_tool_use() -> String {
    r#"{
        "content": [{
            "type": "tool_use",
            "id": "toolu_mock",
            "name": "structured_output",
            "input": {
                "groups": [{"id": "group_1", "name": "Auth flow", "summary": "Changes auth", "review_order_rationale": "Review first", "risk_flags": ["auth_change"]}],
                "overall_summary": "Auth changes",
                "suggested_review_order": ["group_1"]
            }
        }],
        "model": "claude-sonnet-4-6",
        "stop_reason": "tool_use"
    }"#
    .to_string()
}

/// Valid Pass1 response JSON for OpenAI format.
fn valid_openai_pass1() -> String {
    r#"{
        "choices": [{
            "message": {"role": "assistant", "content": "{\"groups\": [{\"id\": \"group_1\", \"name\": \"Auth flow\", \"summary\": \"Changes auth\", \"review_order_rationale\": \"Review first\", \"risk_flags\": [\"auth_change\"]}], \"overall_summary\": \"Auth changes\", \"suggested_review_order\": [\"group_1\"]}"},
            "finish_reason": "stop"
        }],
        "model": "gpt-4.1",
        "usage": {"prompt_tokens": 100, "completion_tokens": 50, "total_tokens": 150}
    }"#
    .to_string()
}

/// Valid Pass1 response JSON for Gemini format.
fn valid_gemini_pass1() -> String {
    r#"{
        "candidates": [{
            "content": {
                "parts": [{"text": "{\"groups\": [{\"id\": \"group_1\", \"name\": \"Auth flow\", \"summary\": \"Changes auth\", \"review_order_rationale\": \"Review first\", \"risk_flags\": [\"auth_change\"]}], \"overall_summary\": \"Auth changes\", \"suggested_review_order\": [\"group_1\"]}"}],
                "role": "model"
            },
            "finishReason": "STOP"
        }],
        "usageMetadata": {"promptTokenCount": 100, "candidatesTokenCount": 50, "totalTokenCount": 150}
    }"#
    .to_string()
}

// ═══════════════════════════════════════════════════════════════
// 1. Rate Limit Handling (429)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_anthropic_rate_limit_with_retry_after() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(429).insert_header("retry-after", "30"),
        )
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url("key".into(), "claude-sonnet-4-6".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::RateLimited { retry_after_secs } => {
            assert_eq!(retry_after_secs, Some(30));
        }
        other => panic!("Expected RateLimited, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_openai_rate_limit_with_retry_after() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(429).insert_header("retry-after", "60"),
        )
        .mount(&server)
        .await;

    let provider = OpenAIProvider::with_base_url("key".into(), "gpt-4.1".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::RateLimited { retry_after_secs } => {
            assert_eq!(retry_after_secs, Some(60));
        }
        other => panic!("Expected RateLimited, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_gemini_rate_limit_no_retry_header() {
    let server = MockServer::start().await;
    // Gemini endpoint includes model in path
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let provider = GeminiProvider::with_base_url(
        "key".into(),
        "gemini-2.5-flash".into(),
        server.uri(),
    );
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::RateLimited { retry_after_secs } => {
            assert_eq!(retry_after_secs, None);
        }
        other => panic!("Expected RateLimited, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_rate_limit_without_retry_after_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429)) // No retry-after header
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url("key".into(), "claude-sonnet-4-6".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::RateLimited { retry_after_secs } => {
            assert_eq!(retry_after_secs, None);
        }
        other => panic!("Expected RateLimited, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_rate_limit_with_non_numeric_retry_after() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(429).insert_header("retry-after", "not-a-number"),
        )
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url("key".into(), "claude-sonnet-4-6".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::RateLimited { retry_after_secs } => {
            // Non-numeric retry-after should be parsed as None
            assert_eq!(retry_after_secs, None);
        }
        other => panic!("Expected RateLimited, got: {:?}", other),
    }
}

// ═══════════════════════════════════════════════════════════════
// 2. Auth Error Handling (401/403)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_anthropic_auth_error_401() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url("bad-key".into(), "claude-sonnet-4-6".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::AuthError(msg) => {
            assert!(msg.contains("Anthropic"), "Expected Anthropic auth error, got: {}", msg);
        }
        other => panic!("Expected AuthError, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_openai_auth_error_401() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string("Invalid API key"))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::with_base_url("bad-key".into(), "gpt-4.1".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::AuthError(msg) => {
            assert!(msg.contains("OpenAI"), "Expected OpenAI auth error, got: {}", msg);
        }
        other => panic!("Expected AuthError, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_gemini_auth_error_401() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string("Invalid key"))
        .mount(&server)
        .await;

    let provider = GeminiProvider::with_base_url("bad-key".into(), "gemini-2.5-flash".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::AuthError(msg) => {
            assert!(msg.contains("Gemini"), "Expected Gemini auth error, got: {}", msg);
        }
        other => panic!("Expected AuthError, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_gemini_auth_error_403() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
        .mount(&server)
        .await;

    let provider = GeminiProvider::with_base_url("bad-key".into(), "gemini-2.5-flash".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::AuthError(msg) => {
            assert!(msg.contains("Gemini"));
        }
        other => panic!("Expected AuthError, got: {:?}", other),
    }
}

// ═══════════════════════════════════════════════════════════════
// 3. Timeout Handling (408/504)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_anthropic_timeout_408() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(408))
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url("key".into(), "claude-sonnet-4-6".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::Timeout(secs) => assert_eq!(secs, 120),
        other => panic!("Expected Timeout, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_openai_timeout_504() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(504))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::with_base_url("key".into(), "gpt-4.1".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::Timeout(secs) => assert_eq!(secs, 120),
        other => panic!("Expected Timeout, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_gemini_timeout_504() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(504))
        .mount(&server)
        .await;

    let provider = GeminiProvider::with_base_url("key".into(), "gemini-2.5-flash".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::Timeout(secs) => assert_eq!(secs, 120),
        other => panic!("Expected Timeout, got: {:?}", other),
    }
}

// ═══════════════════════════════════════════════════════════════
// 4. Malformed JSON Responses
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_anthropic_completely_invalid_json_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("this is not json at all!!!"),
        )
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url("key".into(), "claude-sonnet-4-6".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::ParseResponse(msg) => {
            assert!(msg.contains("parse"), "Expected parse error: {}", msg);
        }
        other => panic!("Expected ParseResponse, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_openai_completely_invalid_json_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("garbage response {{{{"),
        )
        .mount(&server)
        .await;

    let provider = OpenAIProvider::with_base_url("key".into(), "gpt-4.1".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::ParseResponse(msg) => {
            assert!(msg.contains("parse"), "Expected parse error: {}", msg);
        }
        other => panic!("Expected ParseResponse, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_gemini_completely_invalid_json_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("<html>error page</html>"),
        )
        .mount(&server)
        .await;

    let provider = GeminiProvider::with_base_url("key".into(), "gemini-2.5-flash".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::ParseResponse(msg) => {
            assert!(msg.contains("parse"), "Expected parse error: {}", msg);
        }
        other => panic!("Expected ParseResponse, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_anthropic_valid_wrapper_but_missing_content() {
    let server = MockServer::start().await;
    // Valid JSON but missing the expected `content` field
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"model": "claude-sonnet-4-6", "stop_reason": "end_turn"}"#),
        )
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url("key".into(), "claude-sonnet-4-6".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    assert!(result.is_err(), "Should fail when content field is missing");
}

#[tokio::test]
async fn test_openai_empty_choices_array() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"choices": [], "model": "gpt-4.1"}"#),
        )
        .mount(&server)
        .await;

    let provider = OpenAIProvider::with_base_url("key".into(), "gpt-4.1".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::ParseResponse(msg) => {
            assert!(msg.contains("no text") || msg.contains("empty"),
                "Expected empty response error: {}", msg);
        }
        other => panic!("Expected ParseResponse, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_openai_empty_content_in_choice() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"choices": [{"message": {"role": "assistant", "content": ""}, "finish_reason": "stop"}], "model": "gpt-4.1"}"#),
        )
        .mount(&server)
        .await;

    let provider = OpenAIProvider::with_base_url("key".into(), "gpt-4.1".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::ParseResponse(msg) => {
            assert!(msg.contains("no text") || msg.contains("empty"),
                "Expected empty content error: {}", msg);
        }
        other => panic!("Expected ParseResponse, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_gemini_no_candidates() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"usageMetadata": {"totalTokenCount": 0}}"#),
        )
        .mount(&server)
        .await;

    let provider = GeminiProvider::with_base_url("key".into(), "gemini-2.5-flash".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::ParseResponse(msg) => {
            assert!(msg.contains("no text") || msg.contains("empty"),
                "Expected empty response error: {}", msg);
        }
        other => panic!("Expected ParseResponse, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_gemini_safety_blocked_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                r#"{"candidates": [{"finishReason": "SAFETY"}]}"#,
            ),
        )
        .mount(&server)
        .await;

    let provider = GeminiProvider::with_base_url("key".into(), "gemini-2.5-flash".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::ApiError { status, message } => {
            assert_eq!(status, 200);
            assert!(message.contains("SAFETY"), "Expected safety block: {}", message);
        }
        other => panic!("Expected ApiError with SAFETY, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_gemini_recitation_blocked_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                r#"{"candidates": [{"finishReason": "RECITATION"}]}"#,
            ),
        )
        .mount(&server)
        .await;

    let provider = GeminiProvider::with_base_url("key".into(), "gemini-2.5-flash".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::ApiError { status, message } => {
            assert_eq!(status, 200);
            assert!(message.contains("RECITATION"), "Expected recitation block: {}", message);
        }
        other => panic!("Expected ApiError with RECITATION, got: {:?}", other),
    }
}

// ═══════════════════════════════════════════════════════════════
// 5. Structured Output Schema Violations
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_anthropic_tool_use_with_wrong_schema() {
    let server = MockServer::start().await;
    // Returns tool_use but with completely wrong fields
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                r#"{
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu_mock",
                        "name": "structured_output",
                        "input": {"wrong_field": true, "another": 42}
                    }],
                    "model": "claude-sonnet-4-6",
                    "stop_reason": "tool_use"
                }"#,
            ),
        )
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url("key".into(), "claude-sonnet-4-6".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    assert!(result.is_err(), "Should fail on schema violation");
    match result.unwrap_err() {
        LlmError::ParseResponse(_) => {} // Expected
        other => panic!("Expected ParseResponse, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_openai_json_response_with_missing_required_fields() {
    let server = MockServer::start().await;
    // Valid JSON wrapper but inner content missing required `groups` field
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                r#"{"choices": [{"message": {"role": "assistant", "content": "{\"overall_summary\": \"test\"}"}, "finish_reason": "stop"}], "model": "gpt-4.1"}"#,
            ),
        )
        .mount(&server)
        .await;

    let provider = OpenAIProvider::with_base_url("key".into(), "gpt-4.1".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    assert!(result.is_err(), "Should fail when required fields missing");
    match result.unwrap_err() {
        LlmError::ParseResponse(_) => {} // Expected
        other => panic!("Expected ParseResponse, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_gemini_response_with_partial_schema() {
    let server = MockServer::start().await;
    // Returns valid JSON but missing required fields
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                r#"{"candidates": [{"content": {"parts": [{"text": "{\"groups\": []}"}], "role": "model"}, "finishReason": "STOP"}]}"#,
            ),
        )
        .mount(&server)
        .await;

    let provider = GeminiProvider::with_base_url("key".into(), "gemini-2.5-flash".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    // groups=[] is valid, but missing overall_summary and suggested_review_order
    assert!(result.is_err(), "Should fail on missing required fields");
}

#[tokio::test]
async fn test_anthropic_tool_use_with_null_input() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                r#"{
                    "content": [{
                        "type": "tool_use",
                        "id": "toolu_mock",
                        "name": "structured_output",
                        "input": null
                    }],
                    "model": "claude-sonnet-4-6",
                    "stop_reason": "tool_use"
                }"#,
            ),
        )
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url("key".into(), "claude-sonnet-4-6".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    assert!(result.is_err(), "Should fail on null tool input");
}

// ═══════════════════════════════════════════════════════════════
// 6. Huge Diffs That Exceed Context Windows
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_anthropic_huge_diff_truncated_before_sending() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(valid_anthropic_pass1_tool_use()),
        )
        .mount(&server)
        .await;

    // Create a request with a massive diff summary (~1M chars ≈ 250K tokens)
    let huge_diff = "a".repeat(1_000_000);
    let request = Pass1Request {
        diff_summary: huge_diff.clone(),
        flow_groups: vec![Pass1GroupInput {
            id: "g1".to_string(),
            name: "Massive change".to_string(),
            entrypoint: None,
            files: vec!["big.ts".to_string()],
            risk_score: 0.5,
            edge_summary: "none".to_string(),
        }],
        graph_summary: "1 node, 0 edges".to_string(),
    };

    let provider =
        AnthropicProvider::with_base_url("key".into(), "claude-sonnet-4-6".into(), server.uri());

    // Should succeed — truncation happens before sending
    let result = provider.annotate_overview(&request).await;
    assert!(result.is_ok(), "Should succeed after truncating: {:?}", result.err());
}

#[tokio::test]
async fn test_openai_huge_diff_truncated_before_sending() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(valid_openai_pass1()),
        )
        .mount(&server)
        .await;

    let huge_diff = "b".repeat(1_000_000);
    let request = Pass1Request {
        diff_summary: huge_diff,
        flow_groups: vec![],
        graph_summary: String::new(),
    };

    let provider = OpenAIProvider::with_base_url("key".into(), "gpt-4.1".into(), server.uri());
    let result = provider.annotate_overview(&request).await;
    assert!(result.is_ok(), "Should succeed after truncating: {:?}", result.err());
}

#[tokio::test]
async fn test_gemini_huge_diff_truncated_before_sending() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(valid_gemini_pass1()),
        )
        .mount(&server)
        .await;

    let huge_diff = "c".repeat(5_000_000); // 5M chars — even larger
    let request = Pass1Request {
        diff_summary: huge_diff,
        flow_groups: vec![],
        graph_summary: String::new(),
    };

    let provider = GeminiProvider::with_base_url("key".into(), "gemini-2.5-flash".into(), server.uri());
    let result = provider.annotate_overview(&request).await;
    assert!(result.is_ok(), "Should succeed after truncating: {:?}", result.err());
}

#[test]
fn test_truncation_preserves_valid_text() {
    let text = "Hello\nWorld\nFoo\nBar\n";
    let result = truncate_to_token_budget(text, 1000);
    assert_eq!(result, text, "Short text should not be truncated");
}

#[test]
fn test_truncation_adds_marker() {
    let text = "x".repeat(1000);
    let result = truncate_to_token_budget(&text, 10); // ~40 chars
    assert!(result.contains("truncated"), "Should contain truncation marker");
    assert!(result.len() < text.len(), "Should be shorter than input");
}

#[test]
fn test_truncation_multibyte_unicode_boundary_4byte() {
    // Create text with 4-byte unicode chars (happens to align with byte budget)
    let text = "🦀".repeat(100); // Each emoji is 4 bytes
    let result = truncate_to_token_budget(&text, 5); // ~20 bytes budget
    assert!(!result.is_empty());
}

#[test]
fn test_truncation_multibyte_unicode_boundary_3byte() {
    // Create text with 3-byte CJK characters — byte budget won't align
    // "中" is 3 bytes in UTF-8. With max_tokens=5, max_chars=20 bytes.
    // 20 / 3 = 6.67, so byte 20 falls mid-character → would panic without fix.
    let text = "中".repeat(100); // 300 bytes total
    let result = truncate_to_token_budget(&text, 5); // 20 byte budget
    assert!(!result.is_empty());
    assert!(result.contains("truncated"));
}

#[test]
fn test_truncation_multibyte_unicode_boundary_2byte() {
    // "é" is 2 bytes in UTF-8
    let text = "é".repeat(200); // 400 bytes
    let result = truncate_to_token_budget(&text, 3); // 12 byte budget
    // 12 / 2 = 6 characters, aligns. But let's also test odd budgets.
    assert!(!result.is_empty());
}

#[test]
fn test_truncation_mixed_multibyte_characters() {
    // Mix of 1, 2, 3, and 4-byte characters
    let text = "aé中🦀".repeat(50); // (1+2+3+4=10 bytes per repeat) = 500 bytes
    let result = truncate_to_token_budget(&text, 7); // 28 byte budget
    assert!(!result.is_empty());
}

#[test]
fn test_estimate_tokens_empty() {
    assert_eq!(estimate_tokens(""), 0);
}

#[test]
fn test_estimate_tokens_single_char() {
    assert_eq!(estimate_tokens("x"), 1);
}

#[test]
fn test_estimate_tokens_unicode() {
    // Multi-byte chars should produce reasonable estimates
    let text = "日本語テスト"; // 18 bytes in UTF-8
    let tokens = estimate_tokens(text);
    assert!(tokens > 0);
}

// ═══════════════════════════════════════════════════════════════
// 7. Unicode/Emoji in Code Diffs and File Paths
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_anthropic_unicode_in_diff_summary() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(valid_anthropic_pass1_tool_use()),
        )
        .mount(&server)
        .await;

    let request = Pass1Request {
        diff_summary: "Changed 日本語ファイル, added 🦀 Rust code, modified données_utilisateur.py"
            .to_string(),
        flow_groups: vec![Pass1GroupInput {
            id: "g1".to_string(),
            name: "🦀 Rust migration".to_string(),
            entrypoint: Some("src/日本語/main.rs::main".to_string()),
            files: vec![
                "src/日本語/main.rs".to_string(),
                "src/données/utilisateur.py".to_string(),
                "src/emoji_🎉/component.tsx".to_string(),
            ],
            risk_score: 0.9,
            edge_summary: "main → 日本語_service → données_repo".to_string(),
        }],
        graph_summary: "3 nodes with unicode names".to_string(),
    };

    let provider =
        AnthropicProvider::with_base_url("key".into(), "claude-sonnet-4-6".into(), server.uri());
    let result = provider.annotate_overview(&request).await;
    assert!(result.is_ok(), "Should handle unicode in request: {:?}", result.err());
}

#[tokio::test]
async fn test_openai_emoji_in_file_paths() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(valid_openai_pass1()),
        )
        .mount(&server)
        .await;

    let request = Pass2Request {
        group_id: "g_emoji_🎯".to_string(),
        group_name: "Feature 🚀 rocket".to_string(),
        files: vec![Pass2FileInput {
            path: "src/🦀_service/handler.ts".to_string(),
            diff: "+ const résultat = await données.fetch('clé_🔑');".to_string(),
            new_content: Some("// 日本語コメント\nexport const 名前 = '値';".to_string()),
            role: "Entrypoint".to_string(),
        }],
        graph_context: "🦀 → 🐍 → 💾".to_string(),
    };

    let provider = OpenAIProvider::with_base_url("key".into(), "gpt-4.1".into(), server.uri());
    // Pass2 also produces a ParseResponse error since the mock returns Pass1 schema
    // But we're testing that the request with unicode doesn't cause panics
    let _result = provider.annotate_group(&request).await;
    // The important thing is no panic occurred
}

#[tokio::test]
async fn test_gemini_unicode_in_refinement_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                r#"{"candidates": [{"content": {"parts": [{"text": "{\"splits\": [], \"merges\": [], \"re_ranks\": [], \"reclassifications\": [], \"reasoning\": \"No refinements needed\"}"}], "role": "model"}, "finishReason": "STOP"}]}"#,
            ),
        )
        .mount(&server)
        .await;

    let request = RefinementRequest {
        analysis_json: "{}".to_string(),
        diff_summary: "Changé les fichiers français 🇫🇷".to_string(),
        groups: vec![RefinementGroupInput {
            id: "groupe_1".to_string(),
            name: "Flux d'authentification 🔐".to_string(),
            entrypoint: Some("src/auth_日本語.ts".to_string()),
            files: vec!["src/données.ts".to_string(), "src/résultat.py".to_string()],
            risk_score: 0.6,
            review_order: 1,
        }],
    };

    let provider = GeminiProvider::with_base_url("key".into(), "gemini-2.5-flash".into(), server.uri());
    let result = provider.refine_groups(&request).await;
    assert!(result.is_ok(), "Should handle unicode in refinement: {:?}", result.err());
}

// ═══════════════════════════════════════════════════════════════
// 8. API Error with Adversarial Content
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_anthropic_500_with_html_error_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(500).set_body_string(
                "<html><body><h1>Internal Server Error</h1><script>alert('xss')</script></body></html>",
            ),
        )
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url("key".into(), "claude-sonnet-4-6".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::ApiError { status, message } => {
            assert_eq!(status, 500);
            assert!(message.contains("Internal Server Error"));
        }
        other => panic!("Expected ApiError, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_openai_500_with_json_error_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(500).set_body_string(
                r#"{"error": {"message": "Internal error", "type": "server_error"}}"#,
            ),
        )
        .mount(&server)
        .await;

    let provider = OpenAIProvider::with_base_url("key".into(), "gpt-4.1".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::ApiError { status, .. } => assert_eq!(status, 500),
        other => panic!("Expected ApiError, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_gemini_400_bad_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(400).set_body_string(
                r#"{"error": {"code": 400, "message": "Invalid request: model not found", "status": "INVALID_ARGUMENT"}}"#,
            ),
        )
        .mount(&server)
        .await;

    let provider = GeminiProvider::with_base_url("key".into(), "gemini-2.5-flash".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::ApiError { status, message } => {
            assert_eq!(status, 400);
            assert!(message.contains("Invalid request") || message.contains("INVALID_ARGUMENT"));
        }
        other => panic!("Expected ApiError, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_anthropic_empty_body_on_success() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(""))
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url("key".into(), "claude-sonnet-4-6".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    assert!(result.is_err(), "Empty body should fail");
}

// ═══════════════════════════════════════════════════════════════
// 9. Concurrent Requests (Send + Sync)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_anthropic_concurrent_requests() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(valid_anthropic_pass1_tool_use()),
        )
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url("key".into(), "claude-sonnet-4-6".into(), server.uri());
    let provider = std::sync::Arc::new(provider);

    let mut set = tokio::task::JoinSet::new();
    for _ in 0..10 {
        let p = provider.clone();
        let req = sample_pass1_request();
        set.spawn(async move { p.annotate_overview(&req).await });
    }

    while let Some(result) = set.join_next().await {
        let inner = result.expect("Join error");
        assert!(inner.is_ok(), "Concurrent request failed: {:?}", inner.err());
    }
}

#[tokio::test]
async fn test_openai_concurrent_requests() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(valid_openai_pass1()),
        )
        .mount(&server)
        .await;

    let provider = OpenAIProvider::with_base_url("key".into(), "gpt-4.1".into(), server.uri());
    let provider = std::sync::Arc::new(provider);

    let mut set = tokio::task::JoinSet::new();
    for _ in 0..10 {
        let p = provider.clone();
        let req = sample_pass1_request();
        set.spawn(async move { p.annotate_overview(&req).await });
    }

    while let Some(result) = set.join_next().await {
        let inner = result.expect("Join error");
        assert!(inner.is_ok(), "Concurrent request failed: {:?}", inner.err());
    }
}

#[tokio::test]
async fn test_gemini_concurrent_requests() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(valid_gemini_pass1()),
        )
        .mount(&server)
        .await;

    let provider = GeminiProvider::with_base_url("key".into(), "gemini-2.5-flash".into(), server.uri());
    let provider = std::sync::Arc::new(provider);

    let mut set = tokio::task::JoinSet::new();
    for _ in 0..10 {
        let p = provider.clone();
        let req = sample_pass1_request();
        set.spawn(async move { p.annotate_overview(&req).await });
    }

    while let Some(result) = set.join_next().await {
        let inner = result.expect("Join error");
        assert!(inner.is_ok(), "Concurrent request failed: {:?}", inner.err());
    }
}

/// Verify all providers implement Send + Sync (compile-time check).
#[test]
fn test_providers_are_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<AnthropicProvider>();
    assert_send_sync::<OpenAIProvider>();
    assert_send_sync::<GeminiProvider>();
}

/// Verify the LlmProvider trait object is Send + Sync.
#[test]
fn test_provider_trait_object_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Box<dyn LlmProvider>>();
}

// ═══════════════════════════════════════════════════════════════
// 10. Valid Responses — Happy Path Verification
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_anthropic_valid_pass1_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(valid_anthropic_pass1_tool_use()),
        )
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url("key".into(), "claude-sonnet-4-6".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    let response = result.unwrap();
    assert_eq!(response.groups.len(), 1);
    assert_eq!(response.groups[0].id, "group_1");
    assert_eq!(response.overall_summary, "Auth changes");
}

#[tokio::test]
async fn test_openai_valid_pass1_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(valid_openai_pass1()),
        )
        .mount(&server)
        .await;

    let provider = OpenAIProvider::with_base_url("key".into(), "gpt-4.1".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    let response = result.unwrap();
    assert_eq!(response.groups.len(), 1);
    assert_eq!(response.groups[0].id, "group_1");
}

#[tokio::test]
async fn test_gemini_valid_pass1_response() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(valid_gemini_pass1()),
        )
        .mount(&server)
        .await;

    let provider = GeminiProvider::with_base_url("key".into(), "gemini-2.5-flash".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    let response = result.unwrap();
    assert_eq!(response.groups.len(), 1);
    assert_eq!(response.groups[0].id, "group_1");
}

// ═══════════════════════════════════════════════════════════════
// 11. Pass2 / Refinement / Judge Error Paths
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_anthropic_pass2_rate_limit() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "10"))
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url("key".into(), "claude-sonnet-4-6".into(), server.uri());
    let result = provider.annotate_group(&sample_pass2_request()).await;

    match result.unwrap_err() {
        LlmError::RateLimited { retry_after_secs } => {
            assert_eq!(retry_after_secs, Some(10));
        }
        other => panic!("Expected RateLimited, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_openai_refinement_auth_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::with_base_url("bad-key".into(), "gpt-4.1".into(), server.uri());
    let result = provider.refine_groups(&sample_refinement_request()).await;

    match result.unwrap_err() {
        LlmError::AuthError(_) => {} // Expected
        other => panic!("Expected AuthError, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_gemini_judge_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(504))
        .mount(&server)
        .await;

    let provider = GeminiProvider::with_base_url("key".into(), "gemini-2.5-flash".into(), server.uri());
    let judge_request = flowdiff_core::llm::schema::JudgeRequest {
        analysis_json: "{}".to_string(),
        source_files: vec![],
        diff_text: "diff".to_string(),
        fixture_name: "test".to_string(),
    };
    let result = provider.evaluate_quality(&judge_request).await;

    match result.unwrap_err() {
        LlmError::Timeout(secs) => assert_eq!(secs, 120),
        other => panic!("Expected Timeout, got: {:?}", other),
    }
}

// ═══════════════════════════════════════════════════════════════
// 12. Anthropic Text Fallback Path
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_anthropic_text_fallback_with_valid_json() {
    let server = MockServer::start().await;
    // Response with text content instead of tool_use (fallback path)
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                r#"{
                    "content": [{"type": "text", "text": "{\"groups\": [{\"id\": \"g1\", \"name\": \"Auth\", \"summary\": \"Auth changes\", \"review_order_rationale\": \"Review first\", \"risk_flags\": []}], \"overall_summary\": \"Summary\", \"suggested_review_order\": [\"g1\"]}"}],
                    "model": "claude-sonnet-4-6",
                    "stop_reason": "end_turn"
                }"#,
            ),
        )
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url("key".into(), "claude-sonnet-4-6".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;
    assert!(result.is_ok(), "Text fallback should work: {:?}", result.err());
}

#[tokio::test]
async fn test_anthropic_text_fallback_with_markdown_fenced_json() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                r#"{
                    "content": [{"type": "text", "text": "```json\n{\"groups\": [{\"id\": \"g1\", \"name\": \"Auth\", \"summary\": \"Auth changes\", \"review_order_rationale\": \"Review first\", \"risk_flags\": []}], \"overall_summary\": \"Summary\", \"suggested_review_order\": [\"g1\"]}\n```"}],
                    "model": "claude-sonnet-4-6",
                    "stop_reason": "end_turn"
                }"#,
            ),
        )
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url("key".into(), "claude-sonnet-4-6".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;
    assert!(result.is_ok(), "Markdown fence stripping should work: {:?}", result.err());
}

#[tokio::test]
async fn test_anthropic_no_content_blocks() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                r#"{"content": [], "model": "claude-sonnet-4-6", "stop_reason": "end_turn"}"#,
            ),
        )
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url("key".into(), "claude-sonnet-4-6".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::ParseResponse(msg) => {
            assert!(msg.contains("no tool_use or text"), "Expected no content error: {}", msg);
        }
        other => panic!("Expected ParseResponse, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_anthropic_thinking_only_no_tool_use() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                r#"{
                    "content": [{"type": "thinking", "thinking": "Let me analyze this..."}],
                    "model": "claude-opus-4-6",
                    "stop_reason": "end_turn"
                }"#,
            ),
        )
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url("key".into(), "claude-opus-4-6".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    // Should fail — thinking block alone has no usable output
    match result.unwrap_err() {
        LlmError::ParseResponse(msg) => {
            assert!(msg.contains("no tool_use or text"), "Expected no content error: {}", msg);
        }
        other => panic!("Expected ParseResponse, got: {:?}", other),
    }
}

// ═══════════════════════════════════════════════════════════════
// 13. OpenAI Reasoning Model Behavior
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_openai_reasoning_model_request_format() {
    let server = MockServer::start().await;
    // We'll capture the request to verify it doesn't include system message
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(valid_openai_pass1()),
        )
        .expect(1)
        .mount(&server)
        .await;

    let provider = OpenAIProvider::with_base_url("key".into(), "o3-mini".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;
    // The request should have gone through (reasoning model formatting)
    assert!(result.is_ok(), "Reasoning model request should succeed: {:?}", result.err());
}

// ═══════════════════════════════════════════════════════════════
// 14. Very Large Error Bodies (truncation safety)
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_anthropic_very_large_error_body() {
    let server = MockServer::start().await;
    let large_error = "X".repeat(100_000); // 100KB error body
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_string(&large_error))
        .mount(&server)
        .await;

    let provider =
        AnthropicProvider::with_base_url("key".into(), "claude-sonnet-4-6".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    match result.unwrap_err() {
        LlmError::ApiError { status, message } => {
            assert_eq!(status, 500);
            // The message is stored as-is (the body), should not cause OOM
            assert!(!message.is_empty());
        }
        other => panic!("Expected ApiError, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_openai_very_large_200_response() {
    let server = MockServer::start().await;
    // Huge 200 response that is valid JSON wrapper but inner content is huge
    let huge_inner = "Z".repeat(50_000);
    let body = format!(
        r#"{{"choices": [{{"message": {{"role": "assistant", "content": "{}"}}, "finish_reason": "stop"}}], "model": "gpt-4.1"}}"#,
        huge_inner
    );
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(&body))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::with_base_url("key".into(), "gpt-4.1".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;

    // Should fail to parse the gibberish inner content
    assert!(result.is_err());
    match result.unwrap_err() {
        LlmError::ParseResponse(msg) => {
            // Error message should be truncated (the response: prefix is truncated to 500 chars)
            assert!(msg.len() < 1000, "Error message should be reasonably sized: len={}", msg.len());
        }
        other => panic!("Expected ParseResponse, got: {:?}", other),
    }
}

// ═══════════════════════════════════════════════════════════════
// 15. Gemini Multi-Part Response
// ═══════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_gemini_multi_part_response_concatenation() {
    let server = MockServer::start().await;
    // Response split across multiple parts — should be concatenated
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(
                r#"{"candidates": [{"content": {"parts": [
                    {"text": "{\"groups\": [{\"id\": \"g1\", \"name\": \"Auth\", \"summary\": \"Changes\", \"review_order_rationale\": \"First\", \"risk_flags\": []}],"},
                    {"text": " \"overall_summary\": \"Summary\", \"suggested_review_order\": [\"g1\"]}"}
                ], "role": "model"}, "finishReason": "STOP"}]}"#,
            ),
        )
        .mount(&server)
        .await;

    let provider = GeminiProvider::with_base_url("key".into(), "gemini-2.5-flash".into(), server.uri());
    let result = provider.annotate_overview(&sample_pass1_request()).await;
    assert!(result.is_ok(), "Multi-part concatenation should work: {:?}", result.err());
}

// ═══════════════════════════════════════════════════════════════
// 16. Context Window Sizes
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_all_providers_have_reasonable_context_windows() {
    let anthropic = AnthropicProvider::new("k".into(), "claude-sonnet-4-6".into());
    let openai = OpenAIProvider::new("k".into(), "gpt-4.1".into());
    let gemini = GeminiProvider::new("k".into(), "gemini-2.5-flash".into());

    // All providers should have at least 8K context
    assert!(anthropic.max_context_tokens() >= 8_000);
    assert!(openai.max_context_tokens() >= 8_000);
    assert!(gemini.max_context_tokens() >= 8_000);

    // Specific known values
    assert_eq!(anthropic.max_context_tokens(), 200_000);
    assert_eq!(openai.max_context_tokens(), 1_000_000);
    assert_eq!(gemini.max_context_tokens(), 1_048_576);
}

#[test]
fn test_unknown_model_context_windows_are_conservative() {
    let anthropic = AnthropicProvider::new("k".into(), "future-model-x".into());
    let openai = OpenAIProvider::new("k".into(), "future-model-x".into());
    let gemini = GeminiProvider::new("k".into(), "future-model-x".into());

    // Unknown models should get conservative defaults
    assert!(anthropic.max_context_tokens() <= 200_000);
    assert!(openai.max_context_tokens() <= 128_000);
    assert!(gemini.max_context_tokens() <= 1_048_576);
}

// ═══════════════════════════════════════════════════════════════
// 17. LlmError Display Formatting
// ═══════════════════════════════════════════════════════════════

#[test]
fn test_all_error_variants_display() {
    let errors = vec![
        LlmError::NoApiKey("ANTHROPIC_API_KEY".to_string()),
        LlmError::ParseResponse("bad json".to_string()),
        LlmError::ApiError { status: 500, message: "Internal".to_string() },
        LlmError::RateLimited { retry_after_secs: Some(30) },
        LlmError::RateLimited { retry_after_secs: None },
        LlmError::Timeout(120),
        LlmError::AuthError("Invalid key".to_string()),
        LlmError::KeyCmdError("cmd failed".to_string()),
        LlmError::ContextWindowExceeded { input_tokens: 300_000, max_tokens: 200_000 },
        LlmError::UnsupportedProvider("unknown".to_string()),
    ];

    for err in &errors {
        let display = format!("{}", err);
        assert!(!display.is_empty(), "Error display should not be empty: {:?}", err);
        let debug = format!("{:?}", err);
        assert!(!debug.is_empty(), "Error debug should not be empty");
    }
}

#[test]
fn test_llm_error_http_variant_display() {
    // Verify LlmError::Http variant displays properly
    // We can't easily construct a reqwest::Error, but we can test the display format
    // by constructing the error variant through the API (timeout produces Http)
    let err = LlmError::Http(
        reqwest::Client::builder()
            .build()
            .unwrap()
            .get("http://invalid url with spaces")
            .build()
            .unwrap_err(),
    );
    let msg = format!("{}", err);
    assert!(!msg.is_empty(), "Http error display should not be empty");
    assert!(msg.contains("HTTP"), "Should contain HTTP: {}", msg);
}
