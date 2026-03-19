//! LLM-as-judge integration tests.
//!
//! Tests the judge evaluator against synthetic fixture codebases.
//! Live LLM tests are gated behind `FLOWDIFF_RUN_LIVE_LLM_TESTS=1`.
//!
//! Run with:
//!   FLOWDIFF_RUN_LIVE_LLM_TESTS=1 cargo test --test llm_judge -- --nocapture
//!
//! Non-live tests use mock providers and VCR replay.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use git2::{Repository, Signature};
use tempfile::TempDir;

use flowdiff_core::ast;
use flowdiff_core::cluster;
use flowdiff_core::entrypoint;
use flowdiff_core::flow::{self, FlowConfig};
use flowdiff_core::git;
use flowdiff_core::graph::SymbolGraph;
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
use flowdiff_core::output::{self, build_analysis_output};
use flowdiff_core::rank::{self, compute_risk_score, compute_surface_area};
use flowdiff_core::types::{AnalysisOutput, GroupRankInput, RankWeights};

// ═══════════════════════════════════════════════════════════════════════════
// Test Infrastructure
// ═══════════════════════════════════════════════════════════════════════════

fn should_run_live() -> bool {
    std::env::var("FLOWDIFF_RUN_LIVE_LLM_TESTS")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(false)
}

fn load_env() {
    let env_path =
        "/Users/jamesaphoenix/Desktop/projects/brightpool/udemy-prompt-engineering-course/.env";
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
    let dir = TempDir::new().expect("failed to create temp dir");
    let repo = Repository::init(dir.path()).expect("failed to init repo");
    let mut config = repo.config().unwrap();
    config.set_str("user.name", "Test User").unwrap();
    config.set_str("user.email", "test@example.com").unwrap();

    // Base commit
    write_file(dir.path(), "package.json", r#"{"name":"test","version":"1.0.0"}"#);
    write_file(dir.path(), "src/app.ts", "import express from 'express';\nconst app = express();\nexport default app;");
    commit(&repo, "Initial commit");

    // Create main branch
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    let _ = repo.branch("main", &head, false);

    // Feature branch
    let _ = repo.branch("feature/users", &head, false);
    let ref_name = "refs/heads/feature/users";
    let obj = repo.revparse_single(ref_name).unwrap();
    repo.checkout_tree(&obj, None).unwrap();
    repo.set_head(ref_name).unwrap();

    write_file(
        dir.path(),
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

    write_file(
        dir.path(),
        "src/services/userService.ts",
        r#"import { insertUser } from '../repositories/userRepo';

export function createUser(data: any) {
    const user = { id: Date.now(), ...data };
    return insertUser(user);
}"#,
    );

    write_file(
        dir.path(),
        "src/repositories/userRepo.ts",
        r#"const users: any[] = [];

export function insertUser(user: any) {
    users.push(user);
    return user;
}"#,
    );

    commit(&repo, "Add user CRUD");

    // Run pipeline
    let output = run_pipeline(dir.path(), "main", "feature/users");

    // Generate diff text
    let repo2 = Repository::open(dir.path()).unwrap();
    let diff_result = git::diff_refs(&repo2, "main", "feature/users").unwrap();
    let diff_text = diff_result
        .files
        .iter()
        .map(|f| format!("--- a/{}\n+++ b/{}\n{}", f.path(), f.path(), f.new_content.as_deref().unwrap_or("")))
        .collect::<Vec<_>>()
        .join("\n\n");

    (dir, output, diff_text)
}

fn write_file(root: &Path, rel_path: &str, content: &str) {
    let full = root.join(rel_path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(full, content).unwrap();
}

fn commit(repo: &Repository, message: &str) {
    let mut index = repo.index().unwrap();
    index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None).unwrap();
    index.write().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    let sig = Signature::now("Test User", "test@example.com").unwrap();
    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents).unwrap();
}

fn run_pipeline(repo_path: &Path, base_ref: &str, head_ref: &str) -> AnalysisOutput {
    let repo = Repository::open(repo_path).expect("failed to open repo");
    let diff_result = git::diff_refs(&repo, base_ref, head_ref).expect("diff_refs failed");

    let mut parsed_files = Vec::new();
    for file_diff in &diff_result.files {
        if let Some(ref content) = file_diff.new_content {
            let path = file_diff.path();
            if let Ok(parsed) = ast::parse_file(path, content) {
                parsed_files.push(parsed);
            }
        }
    }

    let mut graph = SymbolGraph::build(&parsed_files);
    let entrypoints = entrypoint::detect_entrypoints(&parsed_files);
    let flow_analysis = flow::analyze_data_flow(&parsed_files, &FlowConfig::default());
    flow::enrich_graph(&mut graph, &flow_analysis);

    let changed_files: Vec<String> = diff_result.files.iter().map(|f| f.path().to_string()).collect();
    let cluster_result = cluster::cluster_files(&graph, &entrypoints, &changed_files);

    let weights = RankWeights::default();
    let rank_inputs: Vec<GroupRankInput> = cluster_result
        .groups
        .iter()
        .map(|group| {
            let risk_flags = output::compute_group_risk_flags(
                &group.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            );
            let total_add: u32 = group.files.iter().map(|f| f.changes.additions).sum();
            let total_del: u32 = group.files.iter().map(|f| f.changes.deletions).sum();
            GroupRankInput {
                group_id: group.id.clone(),
                risk: compute_risk_score(
                    risk_flags.has_schema_change,
                    risk_flags.has_api_change,
                    risk_flags.has_auth_change,
                    false,
                ),
                centrality: 0.5,
                surface_area: compute_surface_area(total_add, total_del, 1000),
                uncertainty: if risk_flags.has_test_only { 0.1 } else { 0.5 },
            }
        })
        .collect();

    let ranked = rank::rank_groups(&rank_inputs, &weights);
    let diff_source = output::diff_source_branch(
        base_ref,
        head_ref,
        diff_result.base_sha.as_deref(),
        diff_result.head_sha.as_deref(),
    );
    build_analysis_output(&diff_result, diff_source, &parsed_files, &cluster_result, &ranked)
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

    let request = build_judge_request(&output, &source_files, &diff_text, "TS Express API").unwrap();
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
    let vcr2 = VcrProvider::new(Box::new(mock2), tmp.path().to_path_buf(), VcrMode::Replay);
    let replayed = vcr2.evaluate_quality(&request).await.unwrap();
    assert_eq!(replayed.overall_score, 4.0);
    assert_eq!(call_count2.load(Ordering::SeqCst), 0, "Should not call provider in replay");
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
    assert!(files.len() >= 3, "Expected at least 3 source files, got {}", files.len());
    assert!(files.iter().any(|(p, _)| p.contains("routes/users")));
    assert!(files.iter().any(|(p, _)| p.contains("services/userService")));
    assert!(files.iter().any(|(p, _)| p.contains("repositories/userRepo")));
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

    let request = build_judge_request(&output, &source_files, &diff_text, "TS Express API").unwrap();
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
    assert!(!entries.is_empty(), "VCR should have cached at least one entry");
    assert!(
        entries.iter().any(|p| p.file_name().unwrap().to_str().unwrap().starts_with("judge_")),
        "Should have a judge cache entry"
    );
}
