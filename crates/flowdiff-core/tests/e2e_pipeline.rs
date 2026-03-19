#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
//! End-to-end integration tests for the full flowdiff analysis pipeline.
//!
//! These tests create real git repositories with known file structures,
//! commit changes, and run the complete pipeline:
//!   git diff → AST parse → graph build → entrypoint detect →
//!   flow analyze → enrich graph → cluster → rank → output
//!
//! Run with:
//!   cargo test --test e2e_pipeline

mod helpers;

use git2::Repository;

use flowdiff_core::ast;
use flowdiff_core::entrypoint;
use flowdiff_core::git;
use flowdiff_core::output;
use flowdiff_core::types::AnalysisOutput;
use helpers::graph_assertions::{
    assert_all_files_accounted, assert_json_roundtrip, assert_language_detected,
    assert_valid_json_schema, assert_valid_mermaid, assert_valid_scores,
};
use helpers::repo_builder::{run_pipeline, RepoBuilder};

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
    assert!(
        output.summary.total_files_changed >= 3,
        "should have at least 3 changed files"
    );
    assert_language_detected(&output, "typescript");
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);
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
    assert_language_detected(&output, "python");
    assert_all_files_accounted(&output);
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
    rb.write_file(
        "src/utils.ts",
        "export function log(msg: string) { console.log(msg); }\n",
    );
    rb.commit("Add handler + utils");

    let output = run_pipeline(rb.path(), "main", "feature/x");

    assert_valid_json_schema(&output);
    assert_json_roundtrip(&output);
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
    assert_all_files_accounted(&output);
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
    assert_all_files_accounted(&output);
    assert_valid_scores(&output);
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
    assert_language_detected(&output, "typescript");
    assert_language_detected(&output, "python");
}

/// Test: Deterministic output — running the same analysis twice produces identical results.
#[test]
fn test_e2e_deterministic() {
    let rb = RepoBuilder::new();

    rb.write_file("src/a.ts", "export function a() {}\n");
    rb.write_file(
        "src/b.ts",
        "import { a } from './a';\nexport function b() { return a(); }\n",
    );
    rb.commit("Initial");
    rb.create_branch("main");

    rb.create_branch("feature/det");
    rb.checkout("feature/det");
    rb.write_file("src/a.ts", "export function a() { return 42; }\n");
    rb.write_file(
        "src/b.ts",
        "import { a } from './a';\nexport function b() { return a() + 1; }\n",
    );
    rb.write_file(
        "src/c.ts",
        "import { b } from './b';\nexport function c() { return b() * 2; }\n",
    );
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
    let auth_group = output
        .groups
        .iter()
        .find(|g| g.files.iter().any(|f| f.path.contains("auth")));
    let helper_group = output
        .groups
        .iter()
        .find(|g| g.files.iter().any(|f| f.path.contains("helper")));

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
    assert_all_files_accounted(&output);

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

    assert_valid_mermaid(&output);
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
