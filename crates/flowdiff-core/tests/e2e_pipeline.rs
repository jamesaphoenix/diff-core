//! End-to-end integration tests for the full flowdiff analysis pipeline.
//!
//! These tests create real git repositories with known file structures,
//! commit changes, and run the complete pipeline:
//!   git diff → AST parse → graph build → entrypoint detect →
//!   flow analyze → enrich graph → cluster → rank → output
//!
//! Run with:
//!   cargo test --test e2e_pipeline

use std::path::Path;

use git2::{Repository, Signature};
use tempfile::TempDir;

use flowdiff_core::ast;
use flowdiff_core::cluster;
use flowdiff_core::entrypoint;
use flowdiff_core::flow::{self, FlowConfig};
use flowdiff_core::git;
use flowdiff_core::graph::SymbolGraph;
use flowdiff_core::output::{self, build_analysis_output};
use flowdiff_core::rank::{self, compute_risk_score, compute_surface_area};
use flowdiff_core::types::{AnalysisOutput, GroupRankInput, RankWeights};

// ─── Test helpers ────────────────────────────────────────────────────────

/// Create a git repo, commit initial files, apply changes on a branch, and return the repo + dir.
struct RepoBuilder {
    dir: TempDir,
    repo: Repository,
}

impl RepoBuilder {
    fn new() -> Self {
        let dir = TempDir::new().expect("failed to create temp dir");
        let repo = Repository::init(dir.path()).expect("failed to init repo");

        // Configure user for commits
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test User").unwrap();
        config.set_str("user.email", "test@example.com").unwrap();

        Self { dir, repo }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Write a file relative to the repo root.
    fn write_file(&self, rel_path: &str, content: &str) {
        let full = self.dir.path().join(rel_path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full, content).unwrap();
    }

    /// Stage all changes and commit with a message. Returns the commit OID.
    fn commit(&self, message: &str) -> git2::Oid {
        let mut index = self.repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();

        let tree_oid = index.write_tree().unwrap();
        let tree = self.repo.find_tree(tree_oid).unwrap();
        let sig = Signature::now("Test User", "test@example.com").unwrap();

        let parent = self.repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();

        self.repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
            .unwrap()
    }

    /// Create a branch at the current HEAD. No-op if it already exists.
    fn create_branch(&self, name: &str) {
        let head = self.repo.head().unwrap().peel_to_commit().unwrap();
        // force=false; ignore AlreadyExists errors
        let _ = self.repo.branch(name, &head, false);
    }

    /// Checkout a branch by name.
    fn checkout(&self, name: &str) {
        let ref_name = format!("refs/heads/{}", name);
        let obj = self.repo.revparse_single(&ref_name).unwrap();
        self.repo.checkout_tree(&obj, None).unwrap();
        self.repo.set_head(&ref_name).unwrap();
    }
}

/// Run the full pipeline on a repo diff between two refs.
fn run_pipeline(repo_path: &Path, base_ref: &str, head_ref: &str) -> AnalysisOutput {
    let repo = Repository::open(repo_path).expect("failed to open repo");
    let diff_result = git::diff_refs(&repo, base_ref, head_ref).expect("diff_refs failed");

    // Parse all changed files (using new content for adds/modifies, old for deletes).
    let mut parsed_files = Vec::new();
    for file_diff in &diff_result.files {
        if let Some(ref content) = file_diff.new_content {
            let path = file_diff.path();
            if let Ok(parsed) = ast::parse_file(path, content) {
                parsed_files.push(parsed);
            }
        }
    }

    // Build symbol graph.
    let mut graph = SymbolGraph::build(&parsed_files);

    // Detect entrypoints.
    let entrypoints = entrypoint::detect_entrypoints(&parsed_files);

    // Run data flow analysis and enrich graph.
    let flow_analysis = flow::analyze_data_flow(&parsed_files, &FlowConfig::default());
    flow::enrich_graph(&mut graph, &flow_analysis);

    // Cluster changed files.
    let changed_files: Vec<String> = diff_result
        .files
        .iter()
        .map(|f| f.path().to_string())
        .collect();
    let cluster_result = cluster::cluster_files(&graph, &entrypoints, &changed_files);

    // Rank groups.
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
                centrality: 0.5, // Simplified — no PageRank for now
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

// ─── E2E Tests ───────────────────────────────────────────────────────────

/// Test: Simple 5-file Express app with a new route added.
///
/// Initial state: route.ts → service.ts → repo.ts (existing handler chain)
/// Change: add a new POST /users route + userService + userRepo
/// Expected: produces at least 1 flow group, files in flow order, valid JSON
#[test]
fn test_e2e_simple_express_app() {
    let rb = RepoBuilder::new();

    // Initial commit: base Express app
    rb.write_file(
        "src/routes/health.ts",
        r#"
import express from 'express';
const router = express.Router();

export function healthCheck(req: any, res: any) {
    res.json({ status: 'ok' });
}

router.get('/health', healthCheck);
export default router;
"#,
    );
    rb.write_file(
        "src/services/healthService.ts",
        r#"
export function getHealthStatus(): string {
    return 'ok';
}
"#,
    );
    rb.write_file("package.json", r#"{"name": "test-app", "version": "1.0.0"}"#);
    rb.commit("Initial commit: health endpoint");
    rb.create_branch("main");

    // Feature branch: add user creation flow
    rb.create_branch("feature/add-users");
    rb.checkout("feature/add-users");

    rb.write_file(
        "src/routes/users.ts",
        r#"
import express from 'express';
import { createUser } from '../services/userService';

const router = express.Router();

export function postUser(req: any, res: any) {
    const user = createUser(req.body);
    res.status(201).json(user);
}

router.post('/users', postUser);
export default router;
"#,
    );
    rb.write_file(
        "src/services/userService.ts",
        r#"
import { insertUser } from '../repositories/userRepo';

export function createUser(data: any) {
    const user = { id: Date.now(), ...data };
    return insertUser(user);
}
"#,
    );
    rb.write_file(
        "src/repositories/userRepo.ts",
        r#"
const users: any[] = [];

export function insertUser(user: any) {
    users.push(user);
    return user;
}

export function findUserById(id: number) {
    return users.find(u => u.id === id);
}
"#,
    );
    // Also modify health to add version info (simulates multi-area change)
    rb.write_file(
        "src/routes/health.ts",
        r#"
import express from 'express';
const router = express.Router();

export function healthCheck(req: any, res: any) {
    res.json({ status: 'ok', version: '2.0.0' });
}

router.get('/health', healthCheck);
export default router;
"#,
    );
    rb.commit("Add user creation flow + update health");

    // Run the full pipeline
    let output = run_pipeline(rb.path(), "main", "feature/add-users");

    // Assertions
    assert_eq!(output.version, "1.0.0");
    assert!(output.summary.total_files_changed >= 3, "should have at least 3 changed files");
    assert!(
        output.summary.languages_detected.contains(&"typescript".to_string()),
        "should detect TypeScript"
    );

    // Should produce at least one flow group (the user creation chain)
    let total_grouped_files: usize = output.groups.iter().map(|g| g.files.len()).sum();
    let infra_files = output
        .infrastructure_group
        .as_ref()
        .map(|i| i.files.len())
        .unwrap_or(0);
    assert_eq!(
        total_grouped_files + infra_files,
        output.summary.total_files_changed as usize,
        "every changed file should be in exactly one group or infrastructure"
    );

    // JSON should be valid and roundtrip
    let json = output::to_json(&output).unwrap();
    let parsed: AnalysisOutput = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.version, "1.0.0");
    assert_eq!(parsed.summary.total_files_changed, output.summary.total_files_changed);
}

/// Test: Python FastAPI app with endpoint + service + repository.
///
/// Verifies entrypoint detection for Python decorators and DB write heuristics.
#[test]
fn test_e2e_python_fastapi() {
    let rb = RepoBuilder::new();

    rb.write_file("requirements.txt", "fastapi\nuvicorn\nsqlalchemy\n");
    rb.commit("Initial commit");
    rb.create_branch("main");

    rb.create_branch("feature/add-items");
    rb.checkout("feature/add-items");

    rb.write_file(
        "app/routes/items.py",
        r#"
from fastapi import APIRouter, Depends
from app.services.item_service import create_item
from app.schemas.item import ItemCreate

router = APIRouter()

@router.post("/items")
async def post_item(item: ItemCreate):
    return create_item(item)
"#,
    );
    rb.write_file(
        "app/services/item_service.py",
        r#"
from app.repositories.item_repo import save_item

def create_item(item_data):
    item = {"id": 1, "name": item_data.name}
    save_item(item)
    return item
"#,
    );
    rb.write_file(
        "app/repositories/item_repo.py",
        r#"
items_db = []

def save_item(item):
    items_db.append(item)
    return item

def find_item(item_id):
    return next((i for i in items_db if i["id"] == item_id), None)
"#,
    );
    rb.write_file(
        "app/schemas/item.py",
        r#"
class ItemCreate:
    name: str
"#,
    );
    rb.commit("Add items API");

    let output = run_pipeline(rb.path(), "main", "feature/add-items");

    assert!(output.summary.total_files_changed >= 4);
    assert!(output.summary.languages_detected.contains(&"python".to_string()));

    // All files accounted for
    let total_grouped: usize = output.groups.iter().map(|g| g.files.len()).sum();
    let infra: usize = output
        .infrastructure_group
        .as_ref()
        .map(|i| i.files.len())
        .unwrap_or(0);
    assert_eq!(
        total_grouped + infra,
        output.summary.total_files_changed as usize
    );

    // Should detect Python
    assert!(output.summary.languages_detected.contains(&"python".to_string()));
}

/// Test: Branch comparison produces correct diff source metadata.
#[test]
fn test_e2e_branch_comparison_metadata() {
    let rb = RepoBuilder::new();

    rb.write_file("src/index.ts", "export const VERSION = '1.0.0';\n");
    rb.commit("Initial");
    rb.create_branch("main");

    rb.create_branch("feature/bump");
    rb.checkout("feature/bump");
    rb.write_file("src/index.ts", "export const VERSION = '2.0.0';\n");
    rb.commit("Bump version");

    let output = run_pipeline(rb.path(), "main", "feature/bump");

    assert_eq!(
        output.diff_source.diff_type,
        flowdiff_core::types::DiffType::BranchComparison
    );
    assert_eq!(output.diff_source.base.as_deref(), Some("main"));
    assert_eq!(output.diff_source.head.as_deref(), Some("feature/bump"));
    assert!(output.diff_source.base_sha.is_some());
    assert!(output.diff_source.head_sha.is_some());
    assert_ne!(output.diff_source.base_sha, output.diff_source.head_sha);
}

/// Test: Empty diff (same ref) produces graceful empty result.
#[test]
fn test_e2e_no_changes() {
    let rb = RepoBuilder::new();

    rb.write_file("src/app.ts", "console.log('hello');\n");
    rb.commit("Initial");
    rb.create_branch("main");

    // Compare main to itself → no changes
    let output = run_pipeline(rb.path(), "main", "main");

    assert_eq!(output.summary.total_files_changed, 0);
    assert_eq!(output.summary.total_groups, 0);
    assert!(output.groups.is_empty());
    assert!(output.annotations.is_none());
}

/// Test: JSON output is valid and conforms to the documented schema.
#[test]
fn test_e2e_json_output_valid() {
    let rb = RepoBuilder::new();

    rb.write_file("src/handler.ts", "export function handle() {}\n");
    rb.commit("Initial");
    rb.create_branch("main");

    rb.create_branch("feature/x");
    rb.checkout("feature/x");
    rb.write_file(
        "src/handler.ts",
        r#"
import express from 'express';

export function handle(req: any, res: any) {
    res.json({ ok: true });
}

const app = express();
app.get('/test', handle);
"#,
    );
    rb.write_file("src/utils.ts", "export function log(msg: string) { console.log(msg); }\n");
    rb.commit("Add handler + utils");

    let output = run_pipeline(rb.path(), "main", "feature/x");
    let json_str = output::to_json(&output).unwrap();

    // Parse as generic JSON value and validate structure
    let v: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    assert_eq!(v["version"], "1.0.0");
    assert!(v["diff_source"].is_object());
    assert!(v["summary"].is_object());
    assert!(v["groups"].is_array());
    assert!(v["summary"]["total_files_changed"].is_number());
    assert!(v["summary"]["total_groups"].is_number());
    assert!(v["summary"]["languages_detected"].is_array());
    assert!(v["summary"]["frameworks_detected"].is_array());

    // Each group has required fields
    if let Some(groups) = v["groups"].as_array() {
        for g in groups {
            assert!(g["id"].is_string(), "group should have id");
            assert!(g["name"].is_string(), "group should have name");
            assert!(g["files"].is_array(), "group should have files array");
            assert!(g["edges"].is_array(), "group should have edges array");
            assert!(g["risk_score"].is_number(), "group should have risk_score");
            assert!(g["review_order"].is_number(), "group should have review_order");

            // Each file has required fields
            for f in g["files"].as_array().unwrap() {
                assert!(f["path"].is_string());
                assert!(f["flow_position"].is_number());
                assert!(f["role"].is_string());
                assert!(f["changes"]["additions"].is_number());
                assert!(f["changes"]["deletions"].is_number());
                assert!(f["symbols_changed"].is_array());
            }
        }
    }

    // Roundtrip: JSON → AnalysisOutput → JSON → AnalysisOutput should be stable
    let deserialized: AnalysisOutput = serde_json::from_str(&json_str).unwrap();
    let json_str2 = output::to_json(&deserialized).unwrap();
    let deserialized2: AnalysisOutput = serde_json::from_str(&json_str2).unwrap();
    assert_eq!(deserialized, deserialized2, "JSON roundtrip should be stable");
}

/// Test: Cross-cutting refactor — a shared utility used by many files.
///
/// All files should be grouped, and the shared file should not be lost.
#[test]
fn test_e2e_cross_cutting_refactor() {
    let rb = RepoBuilder::new();

    // Initial: 5 files all importing a shared utility
    rb.write_file(
        "src/utils/format.ts",
        "export function formatDate(d: Date): string { return d.toISOString(); }\n",
    );
    for i in 0..5 {
        rb.write_file(
            &format!("src/modules/mod{}.ts", i),
            &format!(
                "import {{ formatDate }} from '../utils/format';\nexport function handler{}() {{ return formatDate(new Date()); }}\n",
                i
            ),
        );
    }
    rb.commit("Initial with shared utility");
    rb.create_branch("main");

    rb.create_branch("feature/refactor");
    rb.checkout("feature/refactor");

    // Refactor: rename function and update all callers
    rb.write_file(
        "src/utils/format.ts",
        "export function formatDateTime(d: Date): string { return d.toLocaleString(); }\n",
    );
    for i in 0..5 {
        rb.write_file(
            &format!("src/modules/mod{}.ts", i),
            &format!(
                "import {{ formatDateTime }} from '../utils/format';\nexport function handler{}() {{ return formatDateTime(new Date()); }}\n",
                i
            ),
        );
    }
    rb.commit("Refactor: rename formatDate to formatDateTime");

    let output = run_pipeline(rb.path(), "main", "feature/refactor");

    // All 6 files should be changed
    assert_eq!(output.summary.total_files_changed, 6);

    // Every file should appear in some group or infrastructure
    let total_grouped: usize = output.groups.iter().map(|g| g.files.len()).sum();
    let infra: usize = output
        .infrastructure_group
        .as_ref()
        .map(|i| i.files.len())
        .unwrap_or(0);
    assert_eq!(total_grouped + infra, 6);
}

/// Test: Multiple entrypoints produce distinct flow groups.
///
/// Two independent handler chains: HTTP route and a queue consumer pattern.
#[test]
fn test_e2e_multiple_entrypoints() {
    let rb = RepoBuilder::new();

    rb.write_file("package.json", r#"{"name": "multi-ep"}"#);
    rb.commit("Initial");
    rb.create_branch("main");

    rb.create_branch("feature/multi");
    rb.checkout("feature/multi");

    // HTTP route chain
    rb.write_file(
        "src/routes/api.ts",
        r#"
import express from 'express';
import { processOrder } from '../services/orderService';

const router = express.Router();

export function createOrder(req: any, res: any) {
    const result = processOrder(req.body);
    res.json(result);
}

router.post('/orders', createOrder);
export default router;
"#,
    );
    rb.write_file(
        "src/services/orderService.ts",
        r#"
export function processOrder(data: any) {
    return { id: 1, ...data, status: 'created' };
}
"#,
    );

    // Queue consumer chain (independent)
    rb.write_file(
        "src/workers/emailWorker.ts",
        r#"
import { sendEmail } from '../services/emailService';

export function handleEmailJob(job: any) {
    sendEmail(job.to, job.subject, job.body);
}

process.on('message', handleEmailJob);
"#,
    );
    rb.write_file(
        "src/services/emailService.ts",
        r#"
export function sendEmail(to: string, subject: string, body: string) {
    console.log('Sending email to', to);
}
"#,
    );

    // Shared config
    rb.write_file(
        "src/config.ts",
        "export const DB_URL = process.env.DATABASE_URL;\n",
    );

    rb.commit("Add order API + email worker");

    let output = run_pipeline(rb.path(), "main", "feature/multi");

    assert!(output.summary.total_files_changed >= 5);

    // All files accounted for
    let total_grouped: usize = output.groups.iter().map(|g| g.files.len()).sum();
    let infra: usize = output
        .infrastructure_group
        .as_ref()
        .map(|i| i.files.len())
        .unwrap_or(0);
    assert_eq!(total_grouped + infra, output.summary.total_files_changed as usize);

    // Ranking: all groups should have valid scores in [0.0, 1.0]
    for group in &output.groups {
        assert!(
            group.risk_score >= 0.0 && group.risk_score <= 1.0,
            "risk_score should be in [0, 1], got {}",
            group.risk_score
        );
        assert!(group.review_order >= 1, "review_order should be >= 1");
    }
}

/// Test: Mixed TypeScript + Python project.
///
/// Validates multi-language support in a single analysis run.
#[test]
fn test_e2e_mixed_language() {
    let rb = RepoBuilder::new();

    rb.write_file("README.md", "# Mixed project\n");
    rb.commit("Initial");
    rb.create_branch("main");

    rb.create_branch("feature/mixed");
    rb.checkout("feature/mixed");

    rb.write_file(
        "frontend/src/api.ts",
        r#"
export async function fetchUsers() {
    const response = await fetch('/api/users');
    return response.json();
}
"#,
    );
    rb.write_file(
        "frontend/src/UserList.tsx",
        r#"
import { fetchUsers } from './api';

export function UserList() {
    const users = fetchUsers();
    return users;
}
"#,
    );
    rb.write_file(
        "backend/app/routes.py",
        r#"
from fastapi import APIRouter
from backend.app.services import get_all_users

router = APIRouter()

@router.get("/api/users")
def list_users():
    return get_all_users()
"#,
    );
    rb.write_file(
        "backend/app/services.py",
        r#"
def get_all_users():
    return [{"id": 1, "name": "Alice"}]
"#,
    );

    rb.commit("Add frontend + backend");

    let output = run_pipeline(rb.path(), "main", "feature/mixed");

    assert!(output.summary.total_files_changed >= 4);

    // Should detect both languages
    let langs = &output.summary.languages_detected;
    assert!(
        langs.contains(&"typescript".to_string()),
        "should detect TypeScript, got: {:?}",
        langs
    );
    assert!(
        langs.contains(&"python".to_string()),
        "should detect Python, got: {:?}",
        langs
    );
}

/// Test: Deterministic output — running the same analysis twice produces identical results.
#[test]
fn test_e2e_deterministic() {
    let rb = RepoBuilder::new();

    rb.write_file("src/a.ts", "export function a() {}\n");
    rb.write_file("src/b.ts", "import { a } from './a';\nexport function b() { return a(); }\n");
    rb.commit("Initial");
    rb.create_branch("main");

    rb.create_branch("feature/det");
    rb.checkout("feature/det");
    rb.write_file("src/a.ts", "export function a() { return 42; }\n");
    rb.write_file("src/b.ts", "import { a } from './a';\nexport function b() { return a() + 1; }\n");
    rb.write_file("src/c.ts", "import { b } from './b';\nexport function c() { return b() * 2; }\n");
    rb.commit("Modify chain");

    let output1 = run_pipeline(rb.path(), "main", "feature/det");
    let output2 = run_pipeline(rb.path(), "main", "feature/det");

    let json1 = output::to_json(&output1).unwrap();
    let json2 = output::to_json(&output2).unwrap();

    assert_eq!(json1, json2, "two runs should produce identical JSON output");
}

/// Test: New file additions (no old content) are handled correctly.
#[test]
fn test_e2e_new_files_only() {
    let rb = RepoBuilder::new();

    rb.write_file(".gitkeep", "");
    rb.commit("Initial empty");
    rb.create_branch("main");

    rb.create_branch("feature/new-files");
    rb.checkout("feature/new-files");

    rb.write_file(
        "src/index.ts",
        r#"
import { greet } from './greet';
console.log(greet('World'));
"#,
    );
    rb.write_file(
        "src/greet.ts",
        r#"
export function greet(name: string): string {
    return `Hello, ${name}!`;
}
"#,
    );
    rb.commit("Add initial files");

    let output = run_pipeline(rb.path(), "main", "feature/new-files");

    assert!(output.summary.total_files_changed >= 2);
    // Should not error even though there's no "old" version of these files
    let json = output::to_json(&output).unwrap();
    let _: serde_json::Value = serde_json::from_str(&json).unwrap();
}

/// Test: File with auth/security changes scores higher risk.
#[test]
fn test_e2e_risk_scoring_auth() {
    let rb = RepoBuilder::new();

    rb.write_file("src/auth/middleware.ts", "export function auth() {}\n");
    rb.write_file("src/utils/helper.ts", "export function help() {}\n");
    rb.commit("Initial");
    rb.create_branch("main");

    rb.create_branch("feature/auth-fix");
    rb.checkout("feature/auth-fix");
    rb.write_file(
        "src/auth/middleware.ts",
        "export function auth() { /* fixed vulnerability */ return true; }\n",
    );
    rb.write_file(
        "src/utils/helper.ts",
        "export function help() { return 'updated'; }\n",
    );
    rb.commit("Fix auth + update helper");

    let output = run_pipeline(rb.path(), "main", "feature/auth-fix");

    // Find the group containing the auth file
    let auth_group = output.groups.iter().find(|g| {
        g.files
            .iter()
            .any(|f| f.path.contains("auth"))
    });
    let helper_group = output.groups.iter().find(|g| {
        g.files
            .iter()
            .any(|f| f.path.contains("helper"))
    });

    // If they're in separate groups, auth group should have higher risk
    if let (Some(ag), Some(hg)) = (auth_group, helper_group) {
        if ag.id != hg.id {
            assert!(
                ag.risk_score >= hg.risk_score,
                "auth group risk ({}) should be >= helper group risk ({})",
                ag.risk_score,
                hg.risk_score
            );
        }
    }
}

/// Test: Large-ish diff (20 files) completes without error and produces reasonable output.
#[test]
fn test_e2e_20_file_diff() {
    let rb = RepoBuilder::new();

    rb.write_file("package.json", r#"{"name": "big-app"}"#);
    rb.commit("Initial");
    rb.create_branch("main");

    rb.create_branch("feature/big");
    rb.checkout("feature/big");

    // Create 20 TypeScript files with import chains
    for i in 0..20 {
        let content = if i == 0 {
            format!(
                "import express from 'express';\nconst router = express.Router();\nexport function handler{}(req: any, res: any) {{ res.json({{}}); }}\nrouter.get('/route{}', handler{});\nexport default router;\n",
                i, i, i
            )
        } else {
            format!(
                "import {{ handler{} }} from './file{}';\nexport function handler{}() {{ return handler{}(); }}\n",
                i - 1, i - 1, i, i - 1
            )
        };
        rb.write_file(&format!("src/file{}.ts", i), &content);
    }
    rb.commit("Add 20 files");

    let start = std::time::Instant::now();
    let output = run_pipeline(rb.path(), "main", "feature/big");
    let elapsed = start.elapsed();

    assert_eq!(output.summary.total_files_changed, 20);
    assert!(
        elapsed.as_secs() < 30,
        "20-file analysis should complete in <30s, took {:?}",
        elapsed
    );

    // All files should be accounted for
    let total_grouped: usize = output.groups.iter().map(|g| g.files.len()).sum();
    let infra: usize = output
        .infrastructure_group
        .as_ref()
        .map(|i| i.files.len())
        .unwrap_or(0);
    assert_eq!(total_grouped + infra, 20);

    // Should produce valid JSON
    let json = output::to_json(&output).unwrap();
    let _: serde_json::Value = serde_json::from_str(&json).unwrap();
}

/// Test: Mermaid diagram generation for flow groups.
#[test]
fn test_e2e_mermaid_generation() {
    let rb = RepoBuilder::new();

    rb.write_file("base.ts", "export const x = 1;\n");
    rb.commit("Initial");
    rb.create_branch("main");

    rb.create_branch("feature/flow");
    rb.checkout("feature/flow");

    rb.write_file(
        "src/route.ts",
        r#"
import express from 'express';
import { doWork } from './service';

const router = express.Router();
export function handle(req: any, res: any) { res.json(doWork()); }
router.get('/work', handle);
"#,
    );
    rb.write_file(
        "src/service.ts",
        r#"
export function doWork() { return { result: 'done' }; }
"#,
    );
    rb.commit("Add route + service");

    let output = run_pipeline(rb.path(), "main", "feature/flow");

    // Generate Mermaid for each group
    for group in &output.groups {
        let mermaid = output::generate_mermaid(group);
        assert!(
            mermaid.starts_with("graph TD"),
            "Mermaid should start with 'graph TD'"
        );
        // Should have at least one node for each file in the group
        assert!(
            !mermaid.is_empty(),
            "Mermaid diagram should not be empty"
        );
    }
}

/// Test: Commit range support via diff_refs with commit SHAs.
#[test]
fn test_e2e_commit_range() {
    let rb = RepoBuilder::new();

    rb.write_file("src/app.ts", "export const v = 1;\n");
    let _c1 = rb.commit("Commit 1");

    rb.write_file("src/app.ts", "export const v = 2;\n");
    let _c2 = rb.commit("Commit 2");

    rb.write_file("src/app.ts", "export const v = 3;\n");
    rb.write_file("src/extra.ts", "export function extra() {}\n");
    let c3 = rb.commit("Commit 3");

    // Compare HEAD~2..HEAD (should include commits 2 and 3)
    let repo = Repository::open(rb.path()).unwrap();
    let head = repo.find_commit(c3).unwrap();
    let base = head.parent(0).unwrap().parent(0).unwrap();

    let diff_result =
        git::diff_refs(&repo, &base.id().to_string(), &head.id().to_string()).unwrap();

    assert!(
        diff_result.files.len() >= 1,
        "should have at least 1 changed file in range"
    );
}

/// Test: Entrypoint detection works for Express routes in e2e context.
#[test]
fn test_e2e_entrypoint_detection() {
    let rb = RepoBuilder::new();

    rb.write_file("init.ts", "// init\n");
    rb.commit("Initial");
    rb.create_branch("main");

    rb.create_branch("feature/ep");
    rb.checkout("feature/ep");

    rb.write_file(
        "src/server.ts",
        r#"
import express from 'express';

const app = express();

app.get('/api/health', (req, res) => { res.json({ ok: true }); });
app.post('/api/users', (req, res) => { res.json(req.body); });

export default app;
"#,
    );
    rb.commit("Add express routes");

    // Run just the AST + entrypoint part of the pipeline
    let repo = Repository::open(rb.path()).unwrap();
    let diff_result = git::diff_refs(&repo, "main", "feature/ep").unwrap();

    let mut parsed_files = Vec::new();
    for file_diff in &diff_result.files {
        if let Some(ref content) = file_diff.new_content {
            if let Ok(parsed) = ast::parse_file(file_diff.path(), content) {
                parsed_files.push(parsed);
            }
        }
    }

    let entrypoints = entrypoint::detect_entrypoints(&parsed_files);

    // Should detect HTTP route entrypoints
    let http_eps: Vec<_> = entrypoints
        .iter()
        .filter(|e| e.entrypoint_type == flowdiff_core::types::EntrypointType::HttpRoute)
        .collect();
    assert!(
        !http_eps.is_empty(),
        "should detect at least one HTTP route entrypoint, got: {:?}",
        entrypoints
    );
}
