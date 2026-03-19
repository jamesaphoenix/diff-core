//! LLM-as-judge integration tests.
//!
//! Tests the judge evaluator against synthetic fixture codebases.
//! Live LLM tests are gated behind `FLOWDIFF_RUN_LIVE_LLM_TESTS=1`.
//!
//! Run with:
//!   FLOWDIFF_RUN_LIVE_LLM_TESTS=1 cargo test --test llm_judge -- --nocapture
//!
//! Non-live tests use mock providers and VCR replay.

mod helpers;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use tempfile::TempDir;

use flowdiff_core::git;
use flowdiff_core::llm::judge::{
    build_judge_request, collect_source_files, normalize_judge_scores, validate_judge_response,
    JUDGE_CRITERIA,
};
use flowdiff_core::llm::schema::{
    JudgeCriterionScore, JudgeRequest, JudgeResponse, Pass1Request, Pass1Response, Pass2Request,
    Pass2Response,
};
use flowdiff_core::llm::vcr::{VcrMode, VcrProvider};
use flowdiff_core::llm::{LlmError, LlmProvider};
use flowdiff_core::types::AnalysisOutput;
use helpers::llm_helpers::{load_env, should_run_live};
use helpers::repo_builder::{run_pipeline, RepoBuilder};

// ═══════════════════════════════════════════════════════════════════════════
// Test Infrastructure
// ═══════════════════════════════════════════════════════════════════════════

/// Mock LLM provider that returns a valid judge response.
struct MockJudgeProvider {
    call_count: Arc<AtomicUsize>,
}

impl MockJudgeProvider {
    fn new(call_count: Arc<AtomicUsize>) -> Self {
        Self { call_count }
    }
}

#[async_trait]
impl LlmProvider for MockJudgeProvider {
    fn name(&self) -> &str {
        "mock"
    }
    fn model(&self) -> &str {
        "mock-judge-v1"
    }
    fn max_context_tokens(&self) -> usize {
        100_000
    }

    async fn annotate_overview(&self, _: &Pass1Request) -> Result<Pass1Response, LlmError> {
        unimplemented!("Mock judge provider only supports evaluate_quality")
    }

    async fn annotate_group(&self, _: &Pass2Request) -> Result<Pass2Response, LlmError> {
        unimplemented!("Mock judge provider only supports evaluate_quality")
    }

    async fn evaluate_quality(&self, _: &JudgeRequest) -> Result<JudgeResponse, LlmError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Ok(JudgeResponse {
            criteria: JUDGE_CRITERIA
                .iter()
                .map(|c| JudgeCriterionScore {
                    criterion: c.to_string(),
                    score: 4,
                    explanation: format!("{} is well done", c),
                })
                .collect(),
            overall_score: 4.0,
            failure_explanations: vec![],
            strengths: vec!["Good grouping".to_string(), "Clear flow".to_string()],
        })
    }
}

/// Create a simple Express app fixture, run the pipeline, and return output.
fn build_express_fixture_and_analyze() -> (TempDir, AnalysisOutput, String) {
    let rb = RepoBuilder::new();

    // Base commit
    rb.write_file(
        "package.json",
        r#"{"name":"test","version":"1.0.0"}"#,
    );
    rb.write_file(
        "src/app.ts",
        "import express from 'express';\nconst app = express();\nexport default app;",
    );
    rb.commit("Initial commit");
    rb.create_branch("main");

    // Feature branch
    rb.create_branch("feature/users");
    rb.checkout("feature/users");

    rb.write_file(
        "src/routes/users.ts",
        r#"import express from 'express';
import { createUser } from '../services/userService';

const router = express.Router();

export function postUser(req: any, res: any) {
    const user = createUser(req.body);
    res.status(201).json(user);
}

router.post('/users', postUser);
export default router;"#,
    );

    rb.write_file(
        "src/services/userService.ts",
        r#"import { insertUser } from '../repositories/userRepo';

export function createUser(data: any) {
    const user = { id: Date.now(), ...data };
    return insertUser(user);
}"#,
    );

    rb.write_file(
        "src/repositories/userRepo.ts",
        r#"const users: any[] = [];

export function insertUser(user: any) {
    users.push(user);
    return user;
}"#,
    );

    rb.commit("Add user CRUD");

    // Run pipeline
    let output = run_pipeline(rb.path(), "main", "feature/users");

    // Generate diff text
    let repo2 = git2::Repository::open(rb.path()).unwrap();
    let diff_result = git::diff_refs(&repo2, "main", "feature/users").unwrap();
    let diff_text = diff_result
        .files
        .iter()
        .map(|f| {
            format!(
                "--- a/{}\n+++ b/{}\n{}",
                f.path(),
                f.path(),
                f.new_content.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    // Extract TempDir to keep it alive (RepoBuilder owns it)
    // We need to return the TempDir separately since we need the path to persist
    let dir = TempDir::new().unwrap();
    // Copy the repo contents to a new temp dir so the RepoBuilder can be dropped
    copy_dir_recursive(rb.path(), dir.path());

    (dir, output, diff_text)
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            std::fs::create_dir_all(&dst_path).unwrap();
            copy_dir_recursive(&src_path, &dst_path);
        } else {
            std::fs::copy(&src_path, &dst_path).unwrap();
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Mock Provider Tests
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_mock_judge_returns_valid_response() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let provider = MockJudgeProvider::new(call_count.clone());

    let (dir, output, diff_text) = build_express_fixture_and_analyze();
    let source_files = collect_source_files(dir.path());

    let request =
        build_judge_request(&output, &source_files, &diff_text, "TS Express API").unwrap();
    let response = provider.evaluate_quality(&request).await.unwrap();

    assert_eq!(response.criteria.len(), 5);
    assert_eq!(response.overall_score, 4.0);
    assert!(response.failure_explanations.is_empty());
    assert!(!response.strengths.is_empty());
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn test_mock_judge_validates_cleanly() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let provider = MockJudgeProvider::new(call_count);

    let (dir, output, diff_text) = build_express_fixture_and_analyze();
    let source_files = collect_source_files(dir.path());

    let request = build_judge_request(&output, &source_files, &diff_text, "test").unwrap();
    let response = provider.evaluate_quality(&request).await.unwrap();

    let errors = validate_judge_response(&response);
    assert!(errors.is_empty(), "Validation errors: {:?}", errors);
}

#[tokio::test]
async fn test_mock_judge_normalization() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let provider = MockJudgeProvider::new(call_count);

    let (dir, output, diff_text) = build_express_fixture_and_analyze();
    let source_files = collect_source_files(dir.path());

    let request = build_judge_request(&output, &source_files, &diff_text, "test").unwrap();
    let response = provider.evaluate_quality(&request).await.unwrap();
    let normalized = normalize_judge_scores(&response);

    // All 4/5 = 0.75 normalized
    assert!((normalized.overall - 0.75).abs() < f64::EPSILON);
    assert!((normalized.group_coherence - 0.75).abs() < f64::EPSILON);
}

// ═══════════════════════════════════════════════════════════════════════════
// VCR Tests — Record/Replay with Mock
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_vcr_judge_record_replay() {
    let tmp = TempDir::new().unwrap();
    let call_count = Arc::new(AtomicUsize::new(0));
    let mock = MockJudgeProvider::new(call_count.clone());

    let (dir, output, diff_text) = build_express_fixture_and_analyze();
    let source_files = collect_source_files(dir.path());
    let request = build_judge_request(&output, &source_files, &diff_text, "test").unwrap();

    // Record
    let vcr = VcrProvider::new(Box::new(mock), tmp.path().to_path_buf(), VcrMode::Record);
    let response = vcr.evaluate_quality(&request).await.unwrap();
    assert_eq!(response.overall_score, 4.0);
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    // Replay with different mock
    let call_count2 = Arc::new(AtomicUsize::new(0));
    let mock2 = MockJudgeProvider::new(call_count2.clone());
    let vcr2 = VcrProvider::new(
        Box::new(mock2),
        tmp.path().to_path_buf(),
        VcrMode::Replay,
    );
    let replayed = vcr2.evaluate_quality(&request).await.unwrap();
    assert_eq!(replayed.overall_score, 4.0);
    assert_eq!(
        call_count2.load(Ordering::SeqCst),
        0,
        "Should not call provider in replay"
    );
}

#[tokio::test]
async fn test_vcr_judge_auto_mode() {
    let tmp = TempDir::new().unwrap();
    let call_count = Arc::new(AtomicUsize::new(0));
    let mock = MockJudgeProvider::new(call_count.clone());

    let (dir, output, diff_text) = build_express_fixture_and_analyze();
    let source_files = collect_source_files(dir.path());
    let request = build_judge_request(&output, &source_files, &diff_text, "test").unwrap();

    let vcr = VcrProvider::new(Box::new(mock), tmp.path().to_path_buf(), VcrMode::Auto);

    // First call: miss → provider called
    let r1 = vcr.evaluate_quality(&request).await.unwrap();
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    // Second call: cached → provider not called
    let r2 = vcr.evaluate_quality(&request).await.unwrap();
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
    assert_eq!(r1, r2);
}

// ═══════════════════════════════════════════════════════════════════════════
// Fixture Integration Tests (non-live)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_fixture_source_file_collection() {
    let (dir, _, _) = build_express_fixture_and_analyze();
    let files = collect_source_files(dir.path());

    // Should have at least the files we created
    assert!(
        files.len() >= 3,
        "Expected at least 3 source files, got {}",
        files.len()
    );
    assert!(files.iter().any(|(p, _)| p.contains("routes/users")));
    assert!(files.iter().any(|(p, _)| p.contains("services/userService")));
    assert!(files
        .iter()
        .any(|(p, _)| p.contains("repositories/userRepo")));
}

#[test]
fn test_fixture_judge_request_construction() {
    let (dir, output, diff_text) = build_express_fixture_and_analyze();
    let source_files = collect_source_files(dir.path());

    let request = build_judge_request(&output, &source_files, &diff_text, "TS Express").unwrap();
    assert_eq!(request.fixture_name, "TS Express");
    assert!(!request.analysis_json.is_empty());
    assert!(!request.source_files.is_empty());
    assert!(request.analysis_json.contains("groups"));
}

#[test]
fn test_judge_request_contains_all_analysis_fields() {
    let (dir, output, diff_text) = build_express_fixture_and_analyze();
    let source_files = collect_source_files(dir.path());

    let request = build_judge_request(&output, &source_files, &diff_text, "test").unwrap();

    // Verify the serialized output has key fields
    let parsed: serde_json::Value = serde_json::from_str(&request.analysis_json).unwrap();
    assert!(parsed.get("version").is_some());
    assert!(parsed.get("summary").is_some());
    assert!(parsed.get("groups").is_some());
}

// ═══════════════════════════════════════════════════════════════════════════
// Live LLM Tests (gated behind FLOWDIFF_RUN_LIVE_LLM_TESTS=1)
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_live_anthropic_judge() {
    if !should_run_live() {
        eprintln!("Skipping live Anthropic judge test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1)");
        return;
    }
    load_env();

    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY not set");
    let provider = flowdiff_core::llm::anthropic::AnthropicProvider::new(
        api_key,
        "claude-sonnet-4-20250514".to_string(),
    );

    let (dir, output, diff_text) = build_express_fixture_and_analyze();
    let source_files = collect_source_files(dir.path());

    let request =
        build_judge_request(&output, &source_files, &diff_text, "TS Express API").unwrap();
    let response = provider.evaluate_quality(&request).await.unwrap();

    eprintln!("\n=== Live Anthropic Judge Response ===");
    flowdiff_core::llm::judge::print_judge_report("TS Express API", &response);

    // Validate response structure
    let errors = validate_judge_response(&response);
    assert!(errors.is_empty(), "Validation errors: {:?}", errors);

    // Basic sanity checks
    assert_eq!(response.criteria.len(), 5);
    assert!(response.overall_score >= 1.0 && response.overall_score <= 5.0);

    // The Express fixture should get decent scores
    let normalized = normalize_judge_scores(&response);
    assert!(
        normalized.overall >= 0.25,
        "Expected overall normalized >= 0.25, got {}",
        normalized.overall
    );
}

#[tokio::test]
async fn test_live_anthropic_judge_with_vcr() {
    if !should_run_live() {
        eprintln!("Skipping live Anthropic VCR judge test");
        return;
    }
    load_env();

    let api_key = std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY not set");
    let inner = flowdiff_core::llm::anthropic::AnthropicProvider::new(
        api_key,
        "claude-sonnet-4-20250514".to_string(),
    );

    let cache_dir = TempDir::new().unwrap();
    let vcr = VcrProvider::new(
        Box::new(inner),
        cache_dir.path().to_path_buf(),
        VcrMode::Auto,
    );

    let (dir, output, diff_text) = build_express_fixture_and_analyze();
    let source_files = collect_source_files(dir.path());
    let request = build_judge_request(&output, &source_files, &diff_text, "VCR Test").unwrap();

    // First call: real API
    let r1 = vcr.evaluate_quality(&request).await.unwrap();
    assert!(r1.overall_score >= 1.0 && r1.overall_score <= 5.0);

    // Second call: should be from cache (same result)
    let r2 = vcr.evaluate_quality(&request).await.unwrap();
    assert_eq!(r1, r2, "VCR replay should return identical response");

    // Verify cache file was created
    let entries = vcr.list_entries();
    assert!(
        !entries.is_empty(),
        "VCR should have cached at least one entry"
    );
    assert!(
        entries
            .iter()
            .any(|p| p
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("judge_")),
        "Should have a judge cache entry"
    );
}
