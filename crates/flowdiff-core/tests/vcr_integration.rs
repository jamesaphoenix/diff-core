//! VCR integration tests.
//!
//! Tests the VCR caching layer with real LLM providers (when API keys are available)
//! and verifies the full record → replay cycle works end-to-end.
//!
//! Live VCR tests are gated behind `FLOWDIFF_RUN_LIVE_LLM_TESTS=1`.
//! Non-live tests use pre-recorded fixtures and run unconditionally.

use flowdiff_core::llm::schema::{
    Pass1GroupInput, Pass1Request, Pass1Response, Pass2FileInput, Pass2Request, Pass2Response,
};
use flowdiff_core::llm::vcr::{CacheEntry, VcrMode, VcrProvider};
use flowdiff_core::llm::LlmProvider;
use tempfile::TempDir;

/// Check if live tests should run.
fn should_run_live() -> bool {
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
                if std::env::var(key).is_err() {
                    std::env::set_var(key, value);
                }
            }
        }
    }
}

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

// ── Non-Live Tests: Pre-recorded Fixture Replay ──

/// Create a pre-recorded cache entry on disk and verify VCR can replay it.
#[tokio::test]
async fn test_replay_from_prerecorded_pass1_fixture() {
    let tmp = TempDir::new().unwrap();

    // Build the expected cache key to determine the filename
    let request = sample_pass1_request();
    let request_json = serde_json::to_string(&request).unwrap();
    let template_hash = VcrProvider::pass1_template_hash();
    let key = VcrProvider::cache_key("fixture", "fixture-v1", &request_json, &template_hash);
    let filename = format!("pass1_{}.json", &key[..16]);

    // Write a pre-recorded fixture
    let fixture_response = Pass1Response {
        groups: vec![flowdiff_core::llm::schema::Pass1GroupAnnotation {
            id: "group_1".to_string(),
            name: "User registration flow".to_string(),
            summary: "Adds a new user registration endpoint with validation and persistence."
                .to_string(),
            review_order_rationale: "Core feature change, review first.".to_string(),
            risk_flags: vec!["new_endpoint".to_string()],
        }],
        overall_summary: "New user registration flow with validation.".to_string(),
        suggested_review_order: vec!["group_1".to_string(), "group_2".to_string()],
    };

    let entry = CacheEntry {
        provider: "fixture".to_string(),
        model: "fixture-v1".to_string(),
        request_hash: key.clone(),
        prompt_template_hash: template_hash,
        recorded_at: "2026-03-19T12:00:00Z".to_string(),
        response: fixture_response.clone(),
    };

    let json = serde_json::to_string_pretty(&entry).unwrap();
    std::fs::write(tmp.path().join(&filename), json).unwrap();

    // Create a dummy provider (won't be called in replay mode)
    use async_trait::async_trait;
    struct DummyProvider;

    #[async_trait]
    impl LlmProvider for DummyProvider {
        fn name(&self) -> &str { "fixture" }
        fn model(&self) -> &str { "fixture-v1" }
        fn max_context_tokens(&self) -> usize { 100_000 }
        async fn annotate_overview(&self, _: &Pass1Request) -> Result<Pass1Response, flowdiff_core::llm::LlmError> {
            panic!("Should not be called in replay mode")
        }
        async fn annotate_group(&self, _: &Pass2Request) -> Result<Pass2Response, flowdiff_core::llm::LlmError> {
            panic!("Should not be called in replay mode")
        }
    }

    let vcr = VcrProvider::new(
        Box::new(DummyProvider),
        tmp.path().to_path_buf(),
        VcrMode::Replay,
    );

    let result = vcr.annotate_overview(&request).await.unwrap();
    assert_eq!(result, fixture_response);
    assert_eq!(result.groups[0].id, "group_1");
    assert_eq!(result.overall_summary, "New user registration flow with validation.");
}

/// Create a pre-recorded cache entry for Pass 2 and verify replay.
#[tokio::test]
async fn test_replay_from_prerecorded_pass2_fixture() {
    let tmp = TempDir::new().unwrap();

    let request = sample_pass2_request();
    let request_json = serde_json::to_string(&request).unwrap();
    let template_hash = VcrProvider::pass2_template_hash();
    let key = VcrProvider::cache_key("fixture", "fixture-v1", &request_json, &template_hash);
    let filename = format!("pass2_{}.json", &key[..16]);

    let fixture_response = Pass2Response {
        group_id: "group_1".to_string(),
        flow_narrative: "Request enters at POST /api/users, validated, then persisted.".to_string(),
        file_annotations: vec![
            flowdiff_core::llm::schema::Pass2FileAnnotation {
                file: "src/routes/users.ts".to_string(),
                role_in_flow: "HTTP entrypoint".to_string(),
                changes_summary: "New POST handler for user registration.".to_string(),
                risks: vec!["No rate limiting".to_string()],
                suggestions: vec!["Add rate limiter middleware".to_string()],
            },
        ],
        cross_cutting_concerns: vec!["Missing error handling for DB failures".to_string()],
    };

    let entry = CacheEntry {
        provider: "fixture".to_string(),
        model: "fixture-v1".to_string(),
        request_hash: key.clone(),
        prompt_template_hash: template_hash,
        recorded_at: "2026-03-19T12:00:00Z".to_string(),
        response: fixture_response.clone(),
    };

    let json = serde_json::to_string_pretty(&entry).unwrap();
    std::fs::write(tmp.path().join(&filename), json).unwrap();

    use async_trait::async_trait;
    struct DummyProvider;

    #[async_trait]
    impl LlmProvider for DummyProvider {
        fn name(&self) -> &str { "fixture" }
        fn model(&self) -> &str { "fixture-v1" }
        fn max_context_tokens(&self) -> usize { 100_000 }
        async fn annotate_overview(&self, _: &Pass1Request) -> Result<Pass1Response, flowdiff_core::llm::LlmError> {
            panic!("Should not be called")
        }
        async fn annotate_group(&self, _: &Pass2Request) -> Result<Pass2Response, flowdiff_core::llm::LlmError> {
            panic!("Should not be called")
        }
    }

    let vcr = VcrProvider::new(
        Box::new(DummyProvider),
        tmp.path().to_path_buf(),
        VcrMode::Replay,
    );

    let result = vcr.annotate_group(&request).await.unwrap();
    assert_eq!(result, fixture_response);
    assert_eq!(result.group_id, "group_1");
    assert!(!result.flow_narrative.is_empty());
    assert_eq!(result.file_annotations.len(), 1);
}

/// VCR in auto mode with no cache falls through to the inner provider.
#[tokio::test]
async fn test_auto_mode_records_on_first_call_replays_on_second() {
    let tmp = TempDir::new().unwrap();

    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let call_count = Arc::new(AtomicUsize::new(0));
    let cc = call_count.clone();

    struct CountingProvider {
        count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl LlmProvider for CountingProvider {
        fn name(&self) -> &str { "counting" }
        fn model(&self) -> &str { "counting-v1" }
        fn max_context_tokens(&self) -> usize { 100_000 }
        async fn annotate_overview(&self, _: &Pass1Request) -> Result<Pass1Response, flowdiff_core::llm::LlmError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(Pass1Response {
                groups: vec![],
                overall_summary: "counted".to_string(),
                suggested_review_order: vec![],
            })
        }
        async fn annotate_group(&self, _: &Pass2Request) -> Result<Pass2Response, flowdiff_core::llm::LlmError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(Pass2Response {
                group_id: "g1".to_string(),
                flow_narrative: "counted".to_string(),
                file_annotations: vec![],
                cross_cutting_concerns: vec![],
            })
        }
    }

    let vcr = VcrProvider::new(
        Box::new(CountingProvider { count: cc }),
        tmp.path().to_path_buf(),
        VcrMode::Auto,
    );

    // First call: cache miss → calls provider
    let r1 = vcr.annotate_overview(&sample_pass1_request()).await.unwrap();
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
    assert_eq!(r1.overall_summary, "counted");

    // Second call: cache hit → no provider call
    let r2 = vcr.annotate_overview(&sample_pass1_request()).await.unwrap();
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
    assert_eq!(r1, r2);

    // Different request type (Pass 2): cache miss → calls provider
    let r3 = vcr.annotate_group(&sample_pass2_request()).await.unwrap();
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
    assert_eq!(r3.flow_narrative, "counted");
}

// ── Live VCR Tests: Record → Replay with Real API ──

#[tokio::test]
async fn test_live_vcr_record_replay_anthropic() {
    if !should_run_live() {
        eprintln!("Skipping live VCR test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1)");
        return;
    }
    load_env();

    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY must be set");
    let tmp = TempDir::new().unwrap();

    // Phase 1: Record
    let provider = flowdiff_core::llm::anthropic::AnthropicProvider::new(
        api_key.clone(),
        "claude-sonnet-4-20250514".to_string(),
    );
    let vcr_record = VcrProvider::new(
        Box::new(provider),
        tmp.path().to_path_buf(),
        VcrMode::Record,
    );

    let request = sample_pass1_request();
    let recorded = vcr_record.annotate_overview(&request).await.unwrap();
    assert!(!recorded.groups.is_empty());
    assert!(!recorded.overall_summary.is_empty());

    // Verify cache file was written
    let entries = vcr_record.list_entries();
    assert_eq!(entries.len(), 1, "Should have one cache entry after recording");

    // Phase 2: Replay (with a different provider that would fail)
    use async_trait::async_trait;
    struct FailProvider;

    #[async_trait]
    impl LlmProvider for FailProvider {
        fn name(&self) -> &str { "anthropic" }
        fn model(&self) -> &str { "claude-sonnet-4-20250514" }
        fn max_context_tokens(&self) -> usize { 200_000 }
        async fn annotate_overview(&self, _: &Pass1Request) -> Result<Pass1Response, flowdiff_core::llm::LlmError> {
            Err(flowdiff_core::llm::LlmError::AuthError("This should not be called".to_string()))
        }
        async fn annotate_group(&self, _: &Pass2Request) -> Result<Pass2Response, flowdiff_core::llm::LlmError> {
            Err(flowdiff_core::llm::LlmError::AuthError("This should not be called".to_string()))
        }
    }

    let vcr_replay = VcrProvider::new(
        Box::new(FailProvider),
        tmp.path().to_path_buf(),
        VcrMode::Replay,
    );

    let replayed = vcr_replay.annotate_overview(&request).await.unwrap();
    assert_eq!(recorded, replayed, "Replayed response should match recorded response exactly");

    eprintln!("VCR record/replay test passed for Anthropic");
    eprintln!("Recorded summary: {}", recorded.overall_summary);
}

#[tokio::test]
async fn test_live_vcr_record_replay_pass2_anthropic() {
    if !should_run_live() {
        eprintln!("Skipping live VCR test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1)");
        return;
    }
    load_env();

    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY must be set");
    let tmp = TempDir::new().unwrap();

    // Record Pass 2
    let provider = flowdiff_core::llm::anthropic::AnthropicProvider::new(
        api_key,
        "claude-sonnet-4-20250514".to_string(),
    );
    let vcr = VcrProvider::new(
        Box::new(provider),
        tmp.path().to_path_buf(),
        VcrMode::Record,
    );

    let request = sample_pass2_request();
    let recorded = vcr.annotate_group(&request).await.unwrap();
    assert_eq!(recorded.group_id, "group_1");
    assert!(!recorded.flow_narrative.is_empty());
    assert!(!recorded.file_annotations.is_empty());

    // Replay with a provider that errors
    use async_trait::async_trait;
    struct FailProvider;

    #[async_trait]
    impl LlmProvider for FailProvider {
        fn name(&self) -> &str { "anthropic" }
        fn model(&self) -> &str { "claude-sonnet-4-20250514" }
        fn max_context_tokens(&self) -> usize { 200_000 }
        async fn annotate_overview(&self, _: &Pass1Request) -> Result<Pass1Response, flowdiff_core::llm::LlmError> {
            panic!("should not be called")
        }
        async fn annotate_group(&self, _: &Pass2Request) -> Result<Pass2Response, flowdiff_core::llm::LlmError> {
            Err(flowdiff_core::llm::LlmError::AuthError("no".to_string()))
        }
    }

    let vcr_replay = VcrProvider::new(
        Box::new(FailProvider),
        tmp.path().to_path_buf(),
        VcrMode::Replay,
    );

    let replayed = vcr_replay.annotate_group(&request).await.unwrap();
    assert_eq!(recorded, replayed);

    eprintln!("VCR Pass 2 record/replay test passed");
}

/// Full end-to-end VCR test: record Pass 1 + Pass 2, then replay both.
#[tokio::test]
async fn test_live_vcr_full_pipeline() {
    if !should_run_live() {
        eprintln!("Skipping live VCR test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1)");
        return;
    }
    load_env();

    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY must be set");
    let tmp = TempDir::new().unwrap();

    // Record both passes
    let provider = flowdiff_core::llm::anthropic::AnthropicProvider::new(
        api_key,
        "claude-sonnet-4-20250514".to_string(),
    );
    let vcr = VcrProvider::new(
        Box::new(provider),
        tmp.path().to_path_buf(),
        VcrMode::Record,
    );

    let pass1_request = sample_pass1_request();
    let pass1_recorded = vcr.annotate_overview(&pass1_request).await.unwrap();

    let pass2_request = sample_pass2_request();
    let pass2_recorded = vcr.annotate_group(&pass2_request).await.unwrap();

    assert_eq!(vcr.list_entries().len(), 2);

    // Replay both passes with a dummy provider
    use async_trait::async_trait;
    struct FailProvider;

    #[async_trait]
    impl LlmProvider for FailProvider {
        fn name(&self) -> &str { "anthropic" }
        fn model(&self) -> &str { "claude-sonnet-4-20250514" }
        fn max_context_tokens(&self) -> usize { 200_000 }
        async fn annotate_overview(&self, _: &Pass1Request) -> Result<Pass1Response, flowdiff_core::llm::LlmError> {
            panic!("replaying, should not call")
        }
        async fn annotate_group(&self, _: &Pass2Request) -> Result<Pass2Response, flowdiff_core::llm::LlmError> {
            panic!("replaying, should not call")
        }
    }

    let vcr_replay = VcrProvider::new(
        Box::new(FailProvider),
        tmp.path().to_path_buf(),
        VcrMode::Replay,
    );

    let pass1_replayed = vcr_replay.annotate_overview(&pass1_request).await.unwrap();
    let pass2_replayed = vcr_replay.annotate_group(&pass2_request).await.unwrap();

    assert_eq!(pass1_recorded, pass1_replayed);
    assert_eq!(pass2_recorded, pass2_replayed);

    // Verify combined annotations roundtrip
    let combined = flowdiff_core::llm::schema::Annotations {
        overview: Some(pass1_replayed),
        deep_analyses: vec![pass2_replayed],
    };
    let json = serde_json::to_string_pretty(&combined).unwrap();
    let roundtripped: flowdiff_core::llm::schema::Annotations =
        serde_json::from_str(&json).unwrap();
    assert!(roundtripped.overview.is_some());
    assert_eq!(roundtripped.deep_analyses.len(), 1);

    eprintln!("Full VCR pipeline test passed (2 cache entries)");
}
