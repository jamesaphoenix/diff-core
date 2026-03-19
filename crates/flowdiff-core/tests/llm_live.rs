//! Live LLM integration tests.
//!
//! These tests hit real LLM APIs and are gated behind `FLOWDIFF_RUN_LIVE_LLM_TESTS=1`.
//! They require valid API keys in the environment.
//!
//! Run with:
//!   FLOWDIFF_RUN_LIVE_LLM_TESTS=1 cargo test --test llm_live -- --nocapture
//!
//! API keys can be loaded from a .env file or set directly in the environment.

use flowdiff_core::llm::anthropic::AnthropicProvider;
use flowdiff_core::llm::gemini::GeminiProvider;
use flowdiff_core::llm::openai::OpenAIProvider;
use flowdiff_core::llm::schema::{
    Pass1GroupInput, Pass1Request, Pass2FileInput, Pass2Request,
};
use flowdiff_core::llm::LlmProvider;

/// Check if live tests should run.
fn should_run() -> bool {
    std::env::var("FLOWDIFF_RUN_LIVE_LLM_TESTS")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false)
}

/// Load .env file if it exists.
fn load_env() {
    let env_path = "/Users/jamesaphoenix/Desktop/projects/brightpool/udemy-prompt-engineering-course/.env";
    if let Ok(contents) = std::fs::read_to_string(env_path) {
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim();
                // Only set if not already in env (don't override explicit env vars)
                if std::env::var(key).is_err() {
                    std::env::set_var(key, value);
                }
            }
        }
    }
}

/// Build a sample Pass 1 request for testing.
fn sample_pass1_request() -> Pass1Request {
    Pass1Request {
        diff_summary: "12 files changed across 3 modules. Changes include a new user registration \
            endpoint, updated validation logic, and a database migration for the users table."
            .to_string(),
        flow_groups: vec![
            Pass1GroupInput {
                id: "group_1".to_string(),
                name: "POST /api/users registration flow".to_string(),
                entrypoint: Some("src/routes/users.ts::POST".to_string()),
                files: vec![
                    "src/routes/users.ts".to_string(),
                    "src/services/user-service.ts".to_string(),
                    "src/repositories/user-repo.ts".to_string(),
                ],
                risk_score: 0.78,
                edge_summary: "users.ts calls user-service.ts, user-service.ts calls user-repo.ts"
                    .to_string(),
            },
            Pass1GroupInput {
                id: "group_2".to_string(),
                name: "User validation utilities".to_string(),
                entrypoint: None,
                files: vec![
                    "src/utils/validation.ts".to_string(),
                    "src/types/user.ts".to_string(),
                ],
                risk_score: 0.35,
                edge_summary: "validation.ts imports types from user.ts".to_string(),
            },
        ],
        graph_summary: "5 nodes, 4 edges. Primary flow: route → service → repo. \
            Shared utility: validation used by both route and service."
            .to_string(),
    }
}

/// Build a sample Pass 2 request for testing.
fn sample_pass2_request() -> Pass2Request {
    Pass2Request {
        group_id: "group_1".to_string(),
        group_name: "POST /api/users registration flow".to_string(),
        files: vec![
            Pass2FileInput {
                path: "src/routes/users.ts".to_string(),
                diff: r#"+ import { createUser } from '../services/user-service';
+ import { validateUserInput } from '../utils/validation';
+
+ export async function POST(req: Request) {
+   const body = await req.json();
+   const validated = validateUserInput(body);
+   const user = await createUser(validated);
+   return Response.json(user, { status: 201 });
+ }"#
                .to_string(),
                new_content: None,
                role: "Entrypoint".to_string(),
            },
            Pass2FileInput {
                path: "src/services/user-service.ts".to_string(),
                diff: r#"+ import { UserRepository } from '../repositories/user-repo';
+
+ export async function createUser(data: UserInput): Promise<User> {
+   const existing = await UserRepository.findByEmail(data.email);
+   if (existing) throw new Error('User already exists');
+   return UserRepository.insert(data);
+ }"#
                .to_string(),
                new_content: None,
                role: "Service".to_string(),
            },
        ],
        graph_context: "route.ts -> user-service.ts -> user-repo.ts (calls chain)".to_string(),
    }
}

// ── Anthropic Live Tests ──

#[tokio::test]
async fn test_live_anthropic_pass1() {
    if !should_run() {
        eprintln!("Skipping live test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1 to run)");
        return;
    }
    load_env();

    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY must be set");
    let provider = AnthropicProvider::new(api_key, "claude-sonnet-4-20250514".to_string());

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
        assert!(!group.summary.is_empty(), "Group summary should not be empty");
        assert!(
            !group.review_order_rationale.is_empty(),
            "Review rationale should not be empty"
        );
    }

    eprintln!("Pass 1 response: {:?}", response);
}

#[tokio::test]
async fn test_live_anthropic_pass2() {
    if !should_run() {
        eprintln!("Skipping live test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1 to run)");
        return;
    }
    load_env();

    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY must be set");
    let provider = AnthropicProvider::new(api_key, "claude-sonnet-4-20250514".to_string());

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
    if !should_run() {
        eprintln!("Skipping live test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1 to run)");
        return;
    }
    load_env();

    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
    let provider = OpenAIProvider::new(api_key, "gpt-4o".to_string());

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
    assert!(
        response_ids.contains(&"group_1"),
        "Should annotate group_1"
    );

    eprintln!("OpenAI Pass 1 response: {:?}", response);
}

#[tokio::test]
async fn test_live_openai_pass2() {
    if !should_run() {
        eprintln!("Skipping live test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1 to run)");
        return;
    }
    load_env();

    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
    let provider = OpenAIProvider::new(api_key, "gpt-4o".to_string());

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
    if !should_run() {
        eprintln!("Skipping live test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1 to run)");
        return;
    }
    load_env();

    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY must be set");
    let provider = AnthropicProvider::new(api_key, "claude-sonnet-4-20250514".to_string());

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
    if !should_run() {
        eprintln!("Skipping live test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1 to run)");
        return;
    }
    load_env();

    let api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY must be set");
    let provider = OpenAIProvider::new(api_key, "gpt-4o".to_string());

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
    if !should_run() {
        eprintln!("Skipping live test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1 to run)");
        return;
    }
    load_env();

    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY must be set");
    let provider = AnthropicProvider::new(api_key, "claude-sonnet-4-20250514".to_string());

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
    if !should_run() {
        eprintln!("Skipping live test");
        return;
    }

    let provider = AnthropicProvider::new("sk-ant-invalid-key".to_string(), "claude-sonnet-4-20250514".to_string());
    let request = sample_pass1_request();
    let result = provider.annotate_overview(&request).await;

    assert!(result.is_err(), "Should fail with invalid API key");
    let err = result.unwrap_err();
    match err {
        flowdiff_core::llm::LlmError::AuthError(_) | flowdiff_core::llm::LlmError::ApiError { .. } => {
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
    if !should_run() {
        eprintln!("Skipping live test");
        return;
    }

    let provider = OpenAIProvider::new("sk-invalid-key".to_string(), "gpt-4o".to_string());
    let request = sample_pass1_request();
    let result = provider.annotate_overview(&request).await;

    assert!(result.is_err(), "Should fail with invalid API key");
}

// ── Google Gemini Live Tests ──

#[tokio::test]
async fn test_live_gemini_pass1() {
    if !should_run() {
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
        assert!(!group.summary.is_empty(), "Group summary should not be empty");
        assert!(
            !group.review_order_rationale.is_empty(),
            "Review rationale should not be empty"
        );
    }

    eprintln!("Gemini Pass 1 response: {:?}", response);
}

#[tokio::test]
async fn test_live_gemini_pass2() {
    if !should_run() {
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
    if !should_run() {
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
    if !should_run() {
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
    if !should_run() {
        eprintln!("Skipping live test");
        return;
    }

    let provider =
        GeminiProvider::new("invalid-gemini-key".to_string(), "gemini-2.5-flash".to_string());
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
