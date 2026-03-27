#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
//! Live LLM integration tests.
//!
//! These tests hit real LLM APIs and are gated behind `FLOWDIFF_RUN_LIVE_LLM_TESTS=1`.
//! They require valid API keys in the environment.
//!
//! Run with:
//!   FLOWDIFF_RUN_LIVE_LLM_TESTS=1 cargo test --test llm_live -- --nocapture
//!
//! API keys can be loaded from a .env file or set directly in the environment.

mod helpers;

use flowdiff_core::llm::anthropic::AnthropicProvider;
use flowdiff_core::llm::gemini::GeminiProvider;
use flowdiff_core::llm::openai::OpenAIProvider;
use flowdiff_core::llm::LlmProvider;
use helpers::llm_helpers::{load_env, sample_pass1_request, sample_pass2_request, should_run_live};

// ── Anthropic Live Tests ──

#[tokio::test]
async fn test_live_anthropic_pass1() {
    if !should_run_live() {
        eprintln!("Skipping live test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1 to run)");
        return;
    }
    load_env();

    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY must be set");
    let provider = AnthropicProvider::new(api_key, "claude-sonnet-4-6".to_string());

    let request = sample_pass1_request();
    let response = provider.annotate_overview(&request).await.unwrap();

    // Verify structured output
    assert!(!response.groups.is_empty(), "Should have group annotations");
    assert!(
        !response.overall_summary.is_empty(),
        "Should have overall summary"
    );
    assert!(
        !response.suggested_review_order.is_empty(),
        "Should have review order"
    );

    // Verify group IDs match input
    let response_ids: Vec<&str> = response.groups.iter().map(|g| g.id.as_str()).collect();
    assert!(
        response_ids.contains(&"group_1"),
        "Should annotate group_1, got: {:?}",
        response_ids
    );

    // Each group should have meaningful content
    for group in &response.groups {
        assert!(!group.name.is_empty(), "Group name should not be empty");
        assert!(
            !group.summary.is_empty(),
            "Group summary should not be empty"
        );
        assert!(
            !group.review_order_rationale.is_empty(),
            "Review rationale should not be empty"
        );
    }

    eprintln!("Pass 1 response: {:?}", response);
}

#[tokio::test]
async fn test_live_anthropic_pass2() {
    if !should_run_live() {
        eprintln!("Skipping live test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1 to run)");
        return;
    }
    load_env();

    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY must be set");
    let provider = AnthropicProvider::new(api_key, "claude-sonnet-4-6".to_string());

    let request = sample_pass2_request();
    let response = provider.annotate_group(&request).await.unwrap();

    // Verify structured output
    assert_eq!(response.group_id, "group_1");
    assert!(
        !response.flow_narrative.is_empty(),
        "Should have flow narrative"
    );
    assert!(
        !response.file_annotations.is_empty(),
        "Should have file annotations"
    );

    // File annotations should reference actual files
    let annotated_files: Vec<&str> = response
        .file_annotations
        .iter()
        .map(|a| a.file.as_str())
        .collect();
    assert!(
        annotated_files.contains(&"src/routes/users.ts")
            || annotated_files.contains(&"src/services/user-service.ts"),
        "Should annotate at least one input file, got: {:?}",
        annotated_files
    );

    for annotation in &response.file_annotations {
        assert!(!annotation.role_in_flow.is_empty());
        assert!(!annotation.changes_summary.is_empty());
    }

    eprintln!("Pass 2 response: {:?}", response);
}

// ── OpenAI Live Tests ──

#[tokio::test]
async fn test_live_openai_pass1() {
    if !should_run_live() {
        eprintln!("Skipping live test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1 to run)");
        return;
    }
    load_env();

    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
    let provider = OpenAIProvider::new(api_key, "gpt-4.1".to_string());

    let request = sample_pass1_request();
    let response = provider.annotate_overview(&request).await.unwrap();

    assert!(!response.groups.is_empty(), "Should have group annotations");
    assert!(
        !response.overall_summary.is_empty(),
        "Should have overall summary"
    );
    assert!(
        !response.suggested_review_order.is_empty(),
        "Should have review order"
    );

    let response_ids: Vec<&str> = response.groups.iter().map(|g| g.id.as_str()).collect();
    assert!(response_ids.contains(&"group_1"), "Should annotate group_1");

    eprintln!("OpenAI Pass 1 response: {:?}", response);
}

#[tokio::test]
async fn test_live_openai_pass2() {
    if !should_run_live() {
        eprintln!("Skipping live test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1 to run)");
        return;
    }
    load_env();

    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
    let provider = OpenAIProvider::new(api_key, "gpt-4.1".to_string());

    let request = sample_pass2_request();
    let response = provider.annotate_group(&request).await.unwrap();

    assert_eq!(response.group_id, "group_1");
    assert!(!response.flow_narrative.is_empty());
    assert!(!response.file_annotations.is_empty());

    eprintln!("OpenAI Pass 2 response: {:?}", response);
}

// ── Structured Output Compliance Tests ──

#[tokio::test]
async fn test_live_structured_output_compliance_anthropic() {
    if !should_run_live() {
        eprintln!("Skipping live test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1 to run)");
        return;
    }
    load_env();

    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY must be set");
    let provider = AnthropicProvider::new(api_key, "claude-sonnet-4-6".to_string());

    let request = sample_pass1_request();
    let response = provider.annotate_overview(&request).await.unwrap();

    // Verify all required fields are populated (no hallucinated fields beyond schema)
    let json = serde_json::to_value(&response).unwrap();
    assert!(json.is_object());
    let obj = json.as_object().unwrap();

    // Only expected top-level keys
    let expected_keys: std::collections::HashSet<&str> =
        ["groups", "overall_summary", "suggested_review_order"]
            .iter()
            .copied()
            .collect();
    for key in obj.keys() {
        assert!(
            expected_keys.contains(key.as_str()),
            "Unexpected top-level key in response: {}",
            key
        );
    }
}

#[tokio::test]
async fn test_live_structured_output_compliance_openai() {
    if !should_run_live() {
        eprintln!("Skipping live test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1 to run)");
        return;
    }
    load_env();

    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
    let provider = OpenAIProvider::new(api_key, "gpt-4.1".to_string());

    let request = sample_pass1_request();
    let response = provider.annotate_overview(&request).await.unwrap();

    let json = serde_json::to_value(&response).unwrap();
    assert!(json.is_object());
    let obj = json.as_object().unwrap();

    let expected_keys: std::collections::HashSet<&str> =
        ["groups", "overall_summary", "suggested_review_order"]
            .iter()
            .copied()
            .collect();
    for key in obj.keys() {
        assert!(
            expected_keys.contains(key.as_str()),
            "Unexpected top-level key: {}",
            key
        );
    }
}

// ── End-to-End Pipeline Test ──

#[tokio::test]
async fn test_live_end_to_end_pipeline() {
    if !should_run_live() {
        eprintln!("Skipping live test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1 to run)");
        return;
    }
    load_env();

    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY must be set");
    let provider = AnthropicProvider::new(api_key, "claude-sonnet-4-6".to_string());

    // Pass 1: Overview
    let pass1_request = sample_pass1_request();
    let pass1_response = provider.annotate_overview(&pass1_request).await.unwrap();
    assert!(!pass1_response.groups.is_empty());

    // Pass 2: Deep analysis on the first group
    let pass2_request = sample_pass2_request();
    let pass2_response = provider.annotate_group(&pass2_request).await.unwrap();
    assert_eq!(pass2_response.group_id, "group_1");
    assert!(!pass2_response.file_annotations.is_empty());

    // Verify annotations can be combined
    let combined = flowdiff_core::llm::schema::Annotations {
        overview: Some(pass1_response),
        deep_analyses: vec![pass2_response],
    };

    // Should serialize cleanly
    let json = serde_json::to_string_pretty(&combined).unwrap();
    assert!(json.contains("group_1"));
    assert!(json.contains("overall_summary"));
    assert!(json.contains("flow_narrative"));

    // Should deserialize back
    let roundtripped: flowdiff_core::llm::schema::Annotations =
        serde_json::from_str(&json).unwrap();
    assert!(roundtripped.overview.is_some());
    assert_eq!(roundtripped.deep_analyses.len(), 1);

    eprintln!("End-to-end pipeline test passed");
    eprintln!("Combined annotations JSON length: {} bytes", json.len());
}

// ── Error Handling Tests ──

#[tokio::test]
async fn test_live_anthropic_invalid_key() {
    if !should_run_live() {
        eprintln!("Skipping live test");
        return;
    }

    let provider = AnthropicProvider::new(
        "sk-ant-invalid-key".to_string(),
        "claude-sonnet-4-6".to_string(),
    );
    let request = sample_pass1_request();
    let result = provider.annotate_overview(&request).await;

    assert!(result.is_err(), "Should fail with invalid API key");
    let err = result.unwrap_err();
    match err {
        flowdiff_core::llm::LlmError::AuthError(_)
        | flowdiff_core::llm::LlmError::ApiError { .. } => {
            // Expected
        }
        other => {
            // Some API versions return different error codes; that's OK
            eprintln!("Got error type: {:?}", other);
        }
    }
}

#[tokio::test]
async fn test_live_openai_invalid_key() {
    if !should_run_live() {
        eprintln!("Skipping live test");
        return;
    }

    let provider = OpenAIProvider::new("sk-invalid-key".to_string(), "gpt-4.1".to_string());
    let request = sample_pass1_request();
    let result = provider.annotate_overview(&request).await;

    assert!(result.is_err(), "Should fail with invalid API key");
}

// ── Google Gemini Live Tests ──

#[tokio::test]
async fn test_live_gemini_pass1() {
    if !should_run_live() {
        eprintln!("Skipping live test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1 to run)");
        return;
    }
    load_env();

    let api_key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY must be set");
    let provider = GeminiProvider::new(api_key, "gemini-2.5-flash".to_string());

    let request = sample_pass1_request();
    let response = provider.annotate_overview(&request).await.unwrap();

    // Verify structured output
    assert!(!response.groups.is_empty(), "Should have group annotations");
    assert!(
        !response.overall_summary.is_empty(),
        "Should have overall summary"
    );
    assert!(
        !response.suggested_review_order.is_empty(),
        "Should have review order"
    );

    // Verify group IDs match input
    let response_ids: Vec<&str> = response.groups.iter().map(|g| g.id.as_str()).collect();
    assert!(
        response_ids.contains(&"group_1"),
        "Should annotate group_1, got: {:?}",
        response_ids
    );

    // Each group should have meaningful content
    for group in &response.groups {
        assert!(!group.name.is_empty(), "Group name should not be empty");
        assert!(
            !group.summary.is_empty(),
            "Group summary should not be empty"
        );
        assert!(
            !group.review_order_rationale.is_empty(),
            "Review rationale should not be empty"
        );
    }

    eprintln!("Gemini Pass 1 response: {:?}", response);
}

#[tokio::test]
async fn test_live_gemini_pass2() {
    if !should_run_live() {
        eprintln!("Skipping live test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1 to run)");
        return;
    }
    load_env();

    let api_key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY must be set");
    let provider = GeminiProvider::new(api_key, "gemini-2.5-flash".to_string());

    let request = sample_pass2_request();
    let response = provider.annotate_group(&request).await.unwrap();

    // Verify structured output
    assert_eq!(response.group_id, "group_1");
    assert!(
        !response.flow_narrative.is_empty(),
        "Should have flow narrative"
    );
    assert!(
        !response.file_annotations.is_empty(),
        "Should have file annotations"
    );

    // File annotations should reference actual files
    let annotated_files: Vec<&str> = response
        .file_annotations
        .iter()
        .map(|a| a.file.as_str())
        .collect();
    assert!(
        annotated_files.contains(&"src/routes/users.ts")
            || annotated_files.contains(&"src/services/user-service.ts"),
        "Should annotate at least one input file, got: {:?}",
        annotated_files
    );

    for annotation in &response.file_annotations {
        assert!(!annotation.role_in_flow.is_empty());
        assert!(!annotation.changes_summary.is_empty());
    }

    eprintln!("Gemini Pass 2 response: {:?}", response);
}

#[tokio::test]
async fn test_live_structured_output_compliance_gemini() {
    if !should_run_live() {
        eprintln!("Skipping live test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1 to run)");
        return;
    }
    load_env();

    let api_key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY must be set");
    let provider = GeminiProvider::new(api_key, "gemini-2.5-flash".to_string());

    let request = sample_pass1_request();
    let response = provider.annotate_overview(&request).await.unwrap();

    // Verify all required fields are populated (no hallucinated fields beyond schema)
    let json = serde_json::to_value(&response).unwrap();
    assert!(json.is_object());
    let obj = json.as_object().unwrap();

    // Only expected top-level keys
    let expected_keys: std::collections::HashSet<&str> =
        ["groups", "overall_summary", "suggested_review_order"]
            .iter()
            .copied()
            .collect();
    for key in obj.keys() {
        assert!(
            expected_keys.contains(key.as_str()),
            "Unexpected top-level key in Gemini response: {}",
            key
        );
    }
}

#[tokio::test]
async fn test_live_gemini_context_window_handling() {
    if !should_run_live() {
        eprintln!("Skipping live test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1 to run)");
        return;
    }
    load_env();

    let api_key = std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY must be set");
    let provider = GeminiProvider::new(api_key, "gemini-2.5-flash".to_string());

    // Verify the provider reports the correct context window
    assert_eq!(provider.max_context_tokens(), 1_048_576);

    // A normal request should work fine within the context window
    let request = sample_pass1_request();
    let response = provider.annotate_overview(&request).await.unwrap();
    assert!(!response.groups.is_empty());
}

#[tokio::test]
async fn test_live_gemini_invalid_key() {
    if !should_run_live() {
        eprintln!("Skipping live test");
        return;
    }

    let provider = GeminiProvider::new(
        "invalid-gemini-key".to_string(),
        "gemini-2.5-flash".to_string(),
    );
    let request = sample_pass1_request();
    let result = provider.annotate_overview(&request).await;

    assert!(result.is_err(), "Should fail with invalid API key");
    let err = result.unwrap_err();
    match err {
        flowdiff_core::llm::LlmError::AuthError(_)
        | flowdiff_core::llm::LlmError::ApiError { .. } => {
            // Expected — Gemini returns 400 or 403 for invalid keys
        }
        other => {
            eprintln!("Got error type: {:?}", other);
        }
    }
}
