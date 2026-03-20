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

// ─── Go Integration Tests ────────────────────────────────────────────────

/// Test: Synthetic Go HTTP API with handler → service → repo pattern.
///
/// Creates a Go app with Gin framework:
///   main.go → handlers/user.go → services/user.go → repositories/user.go
///
/// Verifies:
///   - Go language detection
///   - Import extraction from Go files
///   - Function/struct/interface definitions
///   - Call site detection
///   - HTTP route entrypoint detection
///   - Framework detection (Gin)
///   - Pipeline produces valid groups and JSON output
#[test]
fn test_e2e_go_http_api() {
    let rb = RepoBuilder::new();

    // Initial commit
    rb.write_file("go.mod", "module github.com/example/api\n\ngo 1.21\n");
    rb.commit("Initial commit: go.mod");
    rb.create_branch("main");

    // Feature branch: add Go API
    rb.create_branch("feature/go-api");
    rb.checkout("feature/go-api");

    rb.write_file(
        "cmd/server/main.go",
        r#"
package main

import (
    "github.com/gin-gonic/gin"
    "github.com/example/api/handlers"
)

func main() {
    r := gin.Default()
    handlers.RegisterRoutes(r)
    r.Run(":8080")
}
"#,
    );

    rb.write_file(
        "handlers/user.go",
        r#"
package handlers

import (
    "github.com/gin-gonic/gin"
    "github.com/example/api/services"
)

func RegisterRoutes(r *gin.Engine) {
    r.GET("/users/:id", GetUser)
    r.POST("/users", CreateUser)
}

func GetUser(c *gin.Context) {
    id := c.Param("id")
    user := services.FindUser(id)
    c.JSON(200, user)
}

func CreateUser(c *gin.Context) {
    data := services.ParseInput(c)
    user := services.CreateUser(data)
    c.JSON(201, user)
}
"#,
    );

    rb.write_file(
        "services/user.go",
        r#"
package services

import (
    "github.com/gin-gonic/gin"
    "github.com/example/api/repositories"
)

type UserInput struct {
    Name  string
    Email string
}

func ParseInput(c *gin.Context) UserInput {
    var input UserInput
    c.BindJSON(&input)
    return input
}

func FindUser(id string) *repositories.User {
    return repositories.GetByID(id)
}

func CreateUser(data UserInput) *repositories.User {
    user := repositories.User{
        Name:  data.Name,
        Email: data.Email,
    }
    return repositories.Insert(&user)
}
"#,
    );

    rb.write_file(
        "repositories/user.go",
        r#"
package repositories

type User struct {
    ID    string
    Name  string
    Email string
}

var users = make(map[string]*User)

func GetByID(id string) *User {
    return users[id]
}

func Insert(user *User) *User {
    user.ID = "generated-id"
    users[user.ID] = user
    return user
}
"#,
    );

    rb.commit("Add Go HTTP API with handler-service-repo");

    let result = run_pipeline(rb.path(), "main", "feature/go-api");

    // Verify basic output shape
    assert_valid_json_schema(&result);
    assert_valid_scores(&result);

    // Verify Go files were detected
    assert_language_detected(&result, "go");

    // Verify all changed files are accounted for
    assert_all_files_accounted(&result);

    // Verify there are flow groups
    assert!(
        !result.groups.is_empty(),
        "should produce at least one flow group"
    );

    // Verify CLI entrypoint detection (func main)
    let has_cli_ep = result.groups.iter().any(|g| {
        g.entrypoint.as_ref().map_or(false, |ep| {
            ep.entrypoint_type == flowdiff_core::types::EntrypointType::CliCommand
        })
    });
    assert!(
        has_cli_ep,
        "should detect func main() as CLI entrypoint"
    );

    // Verify Mermaid graph is valid
    assert_valid_mermaid(&result);
}

/// Test: Go test file detection.
///
/// Verifies that `_test.go` files are detected as test file entrypoints,
/// and that Go Test* functions are recognized as test symbols.
#[test]
fn test_e2e_go_test_file_detection() {
    let rb = RepoBuilder::new();

    rb.write_file("go.mod", "module example.com/app\n\ngo 1.21\n");
    rb.commit("Initial commit");
    rb.create_branch("main");

    rb.create_branch("feature/tests");
    rb.checkout("feature/tests");

    rb.write_file(
        "handlers/user.go",
        r#"
package handlers

func GetUser(id string) string {
    return "user-" + id
}
"#,
    );

    rb.write_file(
        "handlers/user_test.go",
        r#"
package handlers

import "testing"

func TestGetUser(t *testing.T) {
    result := GetUser("123")
    if result != "user-123" {
        t.Errorf("unexpected: %s", result)
    }
}

func BenchmarkGetUser(b *testing.B) {
    for i := 0; i < b.N; i++ {
        GetUser("123")
    }
}
"#,
    );

    rb.commit("Add user handler with tests");

    let result = run_pipeline(rb.path(), "main", "feature/tests");

    // Verify test file entrypoint detection
    let test_eps: Vec<_> = result
        .groups
        .iter()
        .flat_map(|g| g.entrypoint.as_ref())
        .filter(|ep| ep.entrypoint_type == flowdiff_core::types::EntrypointType::TestFile)
        .collect();
    assert!(
        !test_eps.is_empty(),
        "should detect _test.go as test file entrypoint"
    );
}

// =====================================================================
// Rust language integration tests
// =====================================================================

/// Test: Synthetic Rust axum API with handler→service→repo pattern.
///
/// Creates a 5-file Rust HTTP API using axum and verifies the full pipeline:
/// language detection, import extraction, definition extraction, call sites,
/// entrypoint detection, framework detection, grouping, and JSON output.
#[test]
fn test_e2e_rust_axum_api() {
    let rb = RepoBuilder::new();

    // Initial commit
    rb.write_file(
        "Cargo.toml",
        r#"[package]
name = "my-api"
version = "0.1.0"
edition = "2021"

[dependencies]
axum = "0.7"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
sqlx = "0.7"
"#,
    );
    rb.commit("Initial commit: Cargo.toml");
    rb.create_branch("main");

    // Feature branch: add Rust API
    rb.create_branch("feature/rust-api");
    rb.checkout("feature/rust-api");

    rb.write_file(
        "src/main.rs",
        r#"
use axum::{Router, routing::get, routing::post};
use crate::handlers;

mod handlers;
mod services;
mod repositories;
mod models;

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/users/:id", get(handlers::get_user))
        .route("/users", post(handlers::create_user));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
"#,
    );

    rb.write_file(
        "src/handlers.rs",
        r#"
use axum::{extract::Path, Json};
use crate::models::User;
use crate::services;

pub async fn get_user(Path(id): Path<u64>) -> Json<User> {
    let user = services::find_user(id).await;
    Json(user)
}

pub async fn create_user(Json(input): Json<CreateUserInput>) -> Json<User> {
    let user = services::create_user(input.name, input.email).await;
    Json(user)
}

#[derive(serde::Deserialize)]
pub struct CreateUserInput {
    pub name: String,
    pub email: String,
}
"#,
    );

    rb.write_file(
        "src/services.rs",
        r#"
use crate::models::User;
use crate::repositories;

pub async fn find_user(id: u64) -> User {
    repositories::get_by_id(id).await
}

pub async fn create_user(name: String, email: String) -> User {
    let user = User {
        id: 0,
        name,
        email,
    };
    repositories::insert(user).await
}
"#,
    );

    rb.write_file(
        "src/repositories.rs",
        r#"
use crate::models::User;
use sqlx::PgPool;

pub async fn get_by_id(id: u64) -> User {
    User {
        id,
        name: "Alice".to_string(),
        email: "alice@example.com".to_string(),
    }
}

pub async fn insert(user: User) -> User {
    User {
        id: 1,
        ..user
    }
}
"#,
    );

    rb.write_file(
        "src/models.rs",
        r#"
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: u64,
    pub name: String,
    pub email: String,
}
"#,
    );

    rb.commit("Add Rust axum HTTP API with handler-service-repo");

    let result = run_pipeline(rb.path(), "main", "feature/rust-api");

    // Verify basic output shape
    assert_valid_json_schema(&result);
    assert_valid_scores(&result);

    // Verify Rust files were detected
    assert_language_detected(&result, "rust");

    // Verify all changed files are accounted for
    assert_all_files_accounted(&result);

    // Verify there are flow groups
    assert!(
        !result.groups.is_empty(),
        "should produce at least one flow group"
    );

    // Verify entrypoint detection (fn main or HTTP routes)
    let has_entrypoint = result.groups.iter().any(|g| {
        g.entrypoint.is_some()
    });
    assert!(
        has_entrypoint,
        "should detect at least one entrypoint"
    );

    // Verify HTTP route detection (axum Router patterns)
    let has_http_ep = result.groups.iter().any(|g| {
        g.entrypoint.as_ref().map_or(false, |ep| {
            ep.entrypoint_type == flowdiff_core::types::EntrypointType::HttpRoute
        })
    });
    assert!(
        has_http_ep,
        "should detect axum HTTP route entrypoints"
    );

    // Verify framework detection (Axum)
    let has_axum = result
        .summary
        .frameworks_detected
        .iter()
        .any(|f| f.contains("Axum") || f.contains("axum"));
    assert!(has_axum, "should detect Axum framework; detected: {:?}", result.summary.frameworks_detected);

    // Verify Mermaid graph is valid
    assert_valid_mermaid(&result);
}

/// Test: Rust test file detection.
///
/// Verifies that `_test.rs` files and functions with test_ prefix are detected.
#[test]
fn test_e2e_rust_test_file_detection() {
    let rb = RepoBuilder::new();

    rb.write_file(
        "Cargo.toml",
        r#"[package]
name = "my-app"
version = "0.1.0"
edition = "2021"
"#,
    );
    rb.commit("Initial commit");
    rb.create_branch("main");

    rb.create_branch("feature/tests");
    rb.checkout("feature/tests");

    rb.write_file(
        "src/lib.rs",
        r#"
pub fn add(a: i32, b: i32) -> i32 {
    a + b
}
"#,
    );

    rb.write_file(
        "src/lib_test.rs",
        r#"
use crate::add;

fn test_add() {
    assert_eq!(add(2, 3), 5);
}

fn test_add_negative() {
    assert_eq!(add(-1, 1), 0);
}
"#,
    );

    rb.commit("Add lib with tests");

    let result = run_pipeline(rb.path(), "main", "feature/tests");

    // Verify test file detection
    let test_eps: Vec<_> = result
        .groups
        .iter()
        .flat_map(|g| g.entrypoint.as_ref())
        .filter(|ep| ep.entrypoint_type == flowdiff_core::types::EntrypointType::TestFile)
        .collect();
    assert!(
        !test_eps.is_empty(),
        "should detect _test.rs as test file entrypoint"
    );
}

// ---------------------------------------------------------------------------
// Java integration tests (Phase 11.2)
// ---------------------------------------------------------------------------

/// Test: Java Spring Boot REST API with controller → service → repository pattern.
///
/// Verifies full pipeline: language detection, file accounting, flow groups,
/// entrypoint detection, framework detection, and Mermaid graph.
#[test]
fn test_e2e_java_spring_boot_api() {
    let rb = RepoBuilder::new();

    // Initial commit
    rb.write_file(
        "pom.xml",
        r#"<project>
    <modelVersion>4.0.0</modelVersion>
    <groupId>com.example</groupId>
    <artifactId>demo</artifactId>
    <version>0.0.1-SNAPSHOT</version>
    <dependencies>
        <dependency>
            <groupId>org.springframework.boot</groupId>
            <artifactId>spring-boot-starter-web</artifactId>
        </dependency>
    </dependencies>
</project>
"#,
    );
    rb.commit("Initial commit: pom.xml");
    rb.create_branch("main");

    // Feature branch: add Spring Boot API
    rb.create_branch("feature/java-api");
    rb.checkout("feature/java-api");

    rb.write_file(
        "src/main/java/com/example/demo/DemoApplication.java",
        r#"
package com.example.demo;

import org.springframework.boot.SpringApplication;
import org.springframework.boot.autoconfigure.SpringBootApplication;

@SpringBootApplication
public class DemoApplication {
    public static void main(String[] args) {
        SpringApplication.run(DemoApplication.class, args);
    }
}
"#,
    );

    rb.write_file(
        "src/main/java/com/example/demo/controller/UserController.java",
        r#"
package com.example.demo.controller;

import java.util.List;
import org.springframework.web.bind.annotation.RestController;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import com.example.demo.model.User;
import com.example.demo.service.UserService;

@RestController
public class UserController {

    private final UserService userService;

    public UserController(UserService userService) {
        this.userService = userService;
    }

    @GetMapping("/users")
    public List<User> getUsers() {
        return userService.findAll();
    }

    @PostMapping("/users")
    public User createUser(@RequestBody User user) {
        return userService.save(user);
    }
}
"#,
    );

    rb.write_file(
        "src/main/java/com/example/demo/service/UserService.java",
        r#"
package com.example.demo.service;

import java.util.List;
import com.example.demo.model.User;
import com.example.demo.repository.UserRepository;

public class UserService {

    private final UserRepository userRepository;

    public UserService(UserRepository userRepository) {
        this.userRepository = userRepository;
    }

    public List<User> findAll() {
        return userRepository.findAll();
    }

    public User save(User user) {
        return userRepository.save(user);
    }
}
"#,
    );

    rb.write_file(
        "src/main/java/com/example/demo/repository/UserRepository.java",
        r#"
package com.example.demo.repository;

import java.util.List;
import java.util.ArrayList;
import com.example.demo.model.User;

public class UserRepository {

    private final List<User> users = new ArrayList<>();

    public List<User> findAll() {
        return users;
    }

    public User save(User user) {
        users.add(user);
        return user;
    }
}
"#,
    );

    rb.write_file(
        "src/main/java/com/example/demo/model/User.java",
        r#"
package com.example.demo.model;

public class User {
    private Long id;
    private String name;
    private String email;

    public User() {}

    public User(String name, String email) {
        this.name = name;
        this.email = email;
    }

    public Long getId() { return id; }
    public void setId(Long id) { this.id = id; }
    public String getName() { return name; }
    public void setName(String name) { this.name = name; }
    public String getEmail() { return email; }
    public void setEmail(String email) { this.email = email; }
}
"#,
    );

    rb.commit("Add Spring Boot REST API with controller-service-repo");

    let result = run_pipeline(rb.path(), "main", "feature/java-api");

    // Verify basic output shape
    assert_valid_json_schema(&result);
    assert_valid_scores(&result);

    // Verify Java files were detected
    assert_language_detected(&result, "java");

    // Verify all changed files are accounted for
    assert_all_files_accounted(&result);

    // Verify there are flow groups
    assert!(
        !result.groups.is_empty(),
        "should produce at least one flow group"
    );

    // Verify entrypoint detection (main or HTTP routes)
    let has_entrypoint = result.groups.iter().any(|g| g.entrypoint.is_some());
    assert!(
        has_entrypoint,
        "should detect at least one entrypoint"
    );

    // Verify Spring Boot framework detection
    let has_spring = result
        .summary
        .frameworks_detected
        .iter()
        .any(|f| f.contains("Spring"));
    assert!(
        has_spring,
        "should detect Spring Boot framework; detected: {:?}",
        result.summary.frameworks_detected
    );

    // Verify Mermaid graph is valid
    assert_valid_mermaid(&result);
}

/// Test: Java test file detection.
///
/// Verifies that *Test.java files and @Test annotated methods are detected as test entrypoints.
#[test]
fn test_e2e_java_test_file_detection() {
    let rb = RepoBuilder::new();

    rb.write_file(
        "pom.xml",
        r#"<project>
    <modelVersion>4.0.0</modelVersion>
    <groupId>com.example</groupId>
    <artifactId>demo</artifactId>
    <version>0.0.1-SNAPSHOT</version>
</project>
"#,
    );
    rb.commit("Initial commit");
    rb.create_branch("main");

    rb.create_branch("feature/tests");
    rb.checkout("feature/tests");

    rb.write_file(
        "src/main/java/com/example/demo/UserService.java",
        r#"
package com.example.demo;

public class UserService {
    public String greet(String name) {
        return "Hello, " + name;
    }
}
"#,
    );

    rb.write_file(
        "src/test/java/com/example/demo/UserServiceTest.java",
        r#"
package com.example.demo;

import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.assertEquals;

public class UserServiceTest {

    @Test
    public void testGreet() {
        UserService svc = new UserService();
        assertEquals("Hello, Alice", svc.greet("Alice"));
    }

    @Test
    public void testGreetEmpty() {
        UserService svc = new UserService();
        assertEquals("Hello, ", svc.greet(""));
    }
}
"#,
    );

    rb.commit("Add UserService and tests");

    let result = run_pipeline(rb.path(), "main", "feature/tests");

    // Verify test file detection
    let test_eps: Vec<_> = result
        .groups
        .iter()
        .flat_map(|g| g.entrypoint.as_ref())
        .filter(|ep| ep.entrypoint_type == flowdiff_core::types::EntrypointType::TestFile)
        .collect();
    assert!(
        !test_eps.is_empty(),
        "should detect *Test.java as test file entrypoint"
    );
}

// ---------------------------------------------------------------------------
// C# integration tests (Phase 11.2)
// ---------------------------------------------------------------------------

/// Test: C# ASP.NET Core Web API with controller → service → repository pattern.
///
/// Verifies full pipeline: language detection, file accounting, flow groups,
/// entrypoint detection, framework detection, and Mermaid graph.
#[test]
fn test_e2e_csharp_aspnet_core_api() {
    let rb = RepoBuilder::new();

    // Initial commit
    rb.write_file(
        "MyApp.csproj",
        r#"<Project Sdk="Microsoft.NET.Sdk.Web">
    <PropertyGroup>
        <TargetFramework>net8.0</TargetFramework>
    </PropertyGroup>
    <ItemGroup>
        <PackageReference Include="Microsoft.EntityFrameworkCore" Version="8.0.0" />
    </ItemGroup>
</Project>
"#,
    );
    rb.commit("Initial commit: csproj");
    rb.create_branch("main");

    // Feature branch: add ASP.NET Core API
    rb.create_branch("feature/csharp-api");
    rb.checkout("feature/csharp-api");

    rb.write_file(
        "Program.cs",
        r#"
using Microsoft.AspNetCore.Builder;
using Microsoft.Extensions.DependencyInjection;

var builder = WebApplication.CreateBuilder(args);
builder.Services.AddControllers();
var app = builder.Build();
app.MapControllers();
app.Run();
"#,
    );

    rb.write_file(
        "Controllers/UsersController.cs",
        r#"
using System.Collections.Generic;
using Microsoft.AspNetCore.Mvc;
using MyApp.Models;
using MyApp.Services;

namespace MyApp.Controllers
{
    [ApiController]
    [Route("api/[controller]")]
    public class UsersController : ControllerBase
    {
        private readonly IUserService _userService;

        public UsersController(IUserService userService)
        {
            _userService = userService;
        }

        [HttpGet]
        public ActionResult<IEnumerable<User>> GetUsers()
        {
            return Ok(_userService.FindAll());
        }

        [HttpPost]
        public ActionResult<User> CreateUser(User user)
        {
            return Ok(_userService.Save(user));
        }
    }
}
"#,
    );

    rb.write_file(
        "Services/UserService.cs",
        r#"
using System.Collections.Generic;
using MyApp.Models;
using MyApp.Repositories;

namespace MyApp.Services
{
    public interface IUserService
    {
        List<User> FindAll();
        User Save(User user);
    }

    public class UserService : IUserService
    {
        private readonly IUserRepository _repository;

        public UserService(IUserRepository repository)
        {
            _repository = repository;
        }

        public List<User> FindAll()
        {
            return _repository.FindAll();
        }

        public User Save(User user)
        {
            return _repository.Save(user);
        }
    }
}
"#,
    );

    rb.write_file(
        "Repositories/UserRepository.cs",
        r#"
using System.Collections.Generic;
using MyApp.Models;

namespace MyApp.Repositories
{
    public interface IUserRepository
    {
        List<User> FindAll();
        User Save(User user);
    }

    public class UserRepository : IUserRepository
    {
        private readonly List<User> _users = new List<User>();

        public List<User> FindAll()
        {
            return _users;
        }

        public User Save(User user)
        {
            _users.Add(user);
            return user;
        }
    }
}
"#,
    );

    rb.write_file(
        "Models/User.cs",
        r#"
namespace MyApp.Models
{
    public record User(int Id, string Name, string Email);
}
"#,
    );

    rb.commit("Add ASP.NET Core Web API with controller-service-repo");

    let result = run_pipeline(rb.path(), "main", "feature/csharp-api");

    // Verify basic output shape
    assert_valid_json_schema(&result);
    assert_valid_scores(&result);

    // Verify C# files were detected
    assert_language_detected(&result, "csharp");

    // Verify all changed files are accounted for
    assert_all_files_accounted(&result);

    // Verify there are flow groups
    assert!(
        !result.groups.is_empty(),
        "should produce at least one flow group"
    );

    // Verify entrypoint detection (Main or HTTP routes)
    let has_entrypoint = result.groups.iter().any(|g| g.entrypoint.is_some());
    assert!(
        has_entrypoint,
        "should detect at least one entrypoint"
    );

    // Verify ASP.NET Core framework detection
    let has_aspnet = result
        .summary
        .frameworks_detected
        .iter()
        .any(|f| f.contains("ASP.NET"));
    assert!(
        has_aspnet,
        "should detect ASP.NET Core framework; detected: {:?}",
        result.summary.frameworks_detected
    );

    // Verify Mermaid graph is valid
    assert_valid_mermaid(&result);
}

/// Test: C# test file detection.
///
/// Verifies that *Test.cs and *Tests.cs files are detected as test entrypoints.
#[test]
fn test_e2e_csharp_test_file_detection() {
    let rb = RepoBuilder::new();

    rb.write_file(
        "MyApp.csproj",
        r#"<Project Sdk="Microsoft.NET.Sdk.Web">
    <PropertyGroup>
        <TargetFramework>net8.0</TargetFramework>
    </PropertyGroup>
</Project>
"#,
    );
    rb.commit("Initial commit");
    rb.create_branch("main");

    rb.create_branch("feature/tests");
    rb.checkout("feature/tests");

    rb.write_file(
        "Services/UserService.cs",
        r#"
namespace MyApp.Services
{
    public class UserService
    {
        public string GetGreeting(string name)
        {
            return $"Hello, {name}!";
        }
    }
}
"#,
    );

    rb.write_file(
        "Tests/UserServiceTests.cs",
        r#"
using Xunit;
using MyApp.Services;

namespace MyApp.Tests
{
    public class UserServiceTests
    {
        [Fact]
        public void GetGreeting_ReturnsExpected()
        {
            var svc = new UserService();
            var result = svc.GetGreeting("World");
            Assert.Equal("Hello, World!", result);
        }
    }
}
"#,
    );

    rb.commit("Add UserService and tests");

    let result = run_pipeline(rb.path(), "main", "feature/tests");

    // Verify test file detection
    let test_eps: Vec<_> = result
        .groups
        .iter()
        .flat_map(|g| g.entrypoint.as_ref())
        .filter(|ep| ep.entrypoint_type == flowdiff_core::types::EntrypointType::TestFile)
        .collect();
    assert!(
        !test_eps.is_empty(),
        "should detect *Tests.cs as test file entrypoint"
    );
}

// ─── PHP E2E Tests ────────────────────────────────────────────────────────

/// Test: Synthetic Laravel REST API with controller → service → model pattern.
///
/// Verifies PHP parsing, import resolution, entrypoint detection (Laravel controllers),
/// framework detection (Laravel), and flow grouping.
#[test]
fn test_e2e_php_laravel_api() {
    let rb = RepoBuilder::new();

    // Initial commit
    rb.write_file(
        "composer.json",
        r#"{"name": "example/demo", "require": {"laravel/framework": "^11.0"}}"#,
    );
    rb.commit("Initial commit: composer.json");
    rb.create_branch("main");

    // Feature branch: add Laravel REST API
    rb.create_branch("feature/php-api");
    rb.checkout("feature/php-api");

    rb.write_file(
        "app/Http/Controllers/UserController.php",
        r#"<?php

namespace App\Http\Controllers;

use Illuminate\Http\Request;
use App\Models\User;
use App\Services\UserService;

class UserController extends Controller
{
    private $userService;

    public function __construct(UserService $userService)
    {
        $this->userService = $userService;
    }

    public function index()
    {
        $users = User::all();
        return response()->json($users);
    }

    public function store(Request $request)
    {
        $data = $request->validated();
        $user = User::create($data);
        return response()->json($user, 201);
    }

    public function show(User $user)
    {
        return response()->json($user);
    }

    public function destroy(User $user)
    {
        $user->delete();
        return response()->json(null, 204);
    }
}
"#,
    );

    rb.write_file(
        "app/Models/User.php",
        r#"<?php

namespace App\Models;

use Illuminate\Database\Eloquent\Model;

class User extends Model
{
    protected $fillable = ['name', 'email'];

    public function posts()
    {
        return $this->hasMany(Post::class);
    }
}
"#,
    );

    rb.write_file(
        "app/Services/UserService.php",
        r#"<?php

namespace App\Services;

use App\Models\User;

class UserService
{
    public function findAll()
    {
        return User::all();
    }

    public function findById($id)
    {
        return User::find($id);
    }

    public function create(array $data)
    {
        return User::create($data);
    }

    public function update(User $user, array $data)
    {
        $user->update($data);
        return $user;
    }

    public function delete(User $user)
    {
        $user->delete();
    }
}
"#,
    );

    rb.write_file(
        "app/Providers/AppServiceProvider.php",
        r#"<?php

namespace App\Providers;

use Illuminate\Support\ServiceProvider;

class AppServiceProvider extends ServiceProvider
{
    public function register()
    {
        //
    }

    public function boot()
    {
        //
    }
}
"#,
    );

    rb.commit("Add Laravel REST API with controller-service-model");

    let result = run_pipeline(rb.path(), "main", "feature/php-api");

    // Verify basic output shape
    assert_valid_json_schema(&result);
    assert_valid_scores(&result);

    // Verify PHP files were detected
    assert_language_detected(&result, "php");

    // Verify all changed files are accounted for
    assert_all_files_accounted(&result);

    // Verify there are flow groups
    assert!(
        !result.groups.is_empty(),
        "should produce at least one flow group"
    );

    // Verify entrypoint detection (controller action methods)
    let has_entrypoint = result.groups.iter().any(|g| g.entrypoint.is_some());
    assert!(
        has_entrypoint,
        "should detect at least one entrypoint (Laravel controller actions)"
    );

    // Verify Laravel framework detection
    let has_laravel = result
        .summary
        .frameworks_detected
        .iter()
        .any(|f| f.contains("Laravel"));
    assert!(
        has_laravel,
        "should detect Laravel framework; detected: {:?}",
        result.summary.frameworks_detected
    );

    // Verify Mermaid graph is valid
    assert_valid_mermaid(&result);

    // Verify JSON roundtrip
    assert_json_roundtrip(&result);
}

/// Test: PHP test file detection.
///
/// Verifies that *Test.php files are detected as test entrypoints.
#[test]
fn test_e2e_php_test_file_detection() {
    let rb = RepoBuilder::new();

    rb.write_file(
        "composer.json",
        r#"{"name": "example/demo", "require-dev": {"phpunit/phpunit": "^11.0"}}"#,
    );
    rb.commit("Initial commit");
    rb.create_branch("main");

    rb.create_branch("feature/tests");
    rb.checkout("feature/tests");

    rb.write_file(
        "app/Services/UserService.php",
        r#"<?php

namespace App\Services;

class UserService
{
    public function greet($name)
    {
        return "Hello, " . $name;
    }
}
"#,
    );

    rb.write_file(
        "tests/Unit/UserServiceTest.php",
        r#"<?php

namespace Tests\Unit;

use PHPUnit\Framework\TestCase;
use App\Services\UserService;

class UserServiceTest extends TestCase
{
    public function test_greet()
    {
        $service = new UserService();
        $result = $service->greet("Alice");
        $this->assertEquals("Hello, Alice", $result);
    }

    public function test_greet_empty()
    {
        $service = new UserService();
        $result = $service->greet("");
        $this->assertEquals("Hello, ", $result);
    }
}
"#,
    );

    rb.commit("Add UserService with PHPUnit tests");

    let result = run_pipeline(rb.path(), "main", "feature/tests");

    // Verify test file detection
    let test_eps: Vec<_> = result
        .groups
        .iter()
        .flat_map(|g| g.entrypoint.as_ref())
        .filter(|ep| ep.entrypoint_type == flowdiff_core::types::EntrypointType::TestFile)
        .collect();
    assert!(
        !test_eps.is_empty(),
        "should detect *Test.php as test file entrypoint"
    );

    // Verify PHP language detected
    assert_language_detected(&result, "php");

    // Verify PHPUnit framework detected
    let has_phpunit = result
        .summary
        .frameworks_detected
        .iter()
        .any(|f| f.contains("PHPUnit"));
    assert!(
        has_phpunit,
        "should detect PHPUnit framework; detected: {:?}",
        result.summary.frameworks_detected
    );
}

/// Test: Ruby Rails REST API with controller→service→model pattern.
///
/// Verifies that the pipeline can:
/// - Parse Ruby source files via tree-sitter
/// - Extract require/require_relative imports, include/extend mixins
/// - Detect class, module, and method definitions
/// - Detect Rails controller action entrypoints
/// - Detect Rails framework from imports
/// - Cluster files into meaningful flow groups
#[test]
fn test_e2e_ruby_rails_api() {
    let rb = RepoBuilder::new();

    // Initial commit
    rb.write_file("Gemfile", "source 'https://rubygems.org'\ngem 'rails', '~> 7.1'\n");
    rb.commit("Initial commit: Gemfile");
    rb.create_branch("main");

    // Feature branch: add Rails REST API
    rb.create_branch("feature/ruby-api");
    rb.checkout("feature/ruby-api");

    rb.write_file(
        "app/controllers/users_controller.rb",
        r#"require 'action_controller'
require_relative '../models/user'
require_relative '../services/user_service'

class UsersController < ApplicationController
  include Authentication

  def index
    @users = User.all()
    respond_to()
  end

  def show
    @user = User.find(params())
  end

  def create
    @user = UserService.new().create(user_params())
    redirect_to(@user)
  end

  def destroy
    @user = User.find(params())
    @user.destroy()
  end

  private

  def user_params
    params().require().permit()
  end
end
"#,
    );

    rb.write_file(
        "app/models/user.rb",
        r#"require 'active_record'

class User < ActiveRecord::Base
  include Validatable

  def full_name
    first_name.to_s()
  end

  def active?
    status == 'active'
  end
end
"#,
    );

    rb.write_file(
        "app/services/user_service.rb",
        r#"require_relative '../models/user'

class UserService
  def create(attrs)
    user = User.new(attrs)
    user.save()
    notify(user)
    user
  end

  def find(id)
    User.find(id)
  end

  private

  def notify(user)
    EventBus.publish('user.created', user)
  end
end
"#,
    );

    rb.write_file(
        "config/routes.rb",
        r#"require 'action_controller'

Rails.application.routes.draw()
"#,
    );

    rb.commit("Add Rails REST API with controller-service-model");

    let result = run_pipeline(rb.path(), "main", "feature/ruby-api");

    // Verify basic output shape
    assert_valid_json_schema(&result);
    assert_valid_scores(&result);

    // Verify Ruby files were detected
    assert_language_detected(&result, "ruby");

    // Verify all changed files are accounted for
    assert_all_files_accounted(&result);

    // Verify there are flow groups
    assert!(
        !result.groups.is_empty(),
        "should produce at least one flow group"
    );

    // Verify entrypoint detection (controller action methods)
    let has_entrypoint = result.groups.iter().any(|g| g.entrypoint.is_some());
    assert!(
        has_entrypoint,
        "should detect at least one entrypoint (Rails controller actions)"
    );

    // Verify Rails framework detection
    let has_rails = result
        .summary
        .frameworks_detected
        .iter()
        .any(|f| f.contains("Rails"));
    assert!(
        has_rails,
        "should detect Rails framework; detected: {:?}",
        result.summary.frameworks_detected
    );

    // Verify Mermaid graph is valid
    assert_valid_mermaid(&result);

    // Verify JSON roundtrip
    assert_json_roundtrip(&result);
}

/// Test: Ruby test file detection.
///
/// Verifies that *_spec.rb and *_test.rb files are detected as test entrypoints.
#[test]
fn test_e2e_ruby_test_file_detection() {
    let rb = RepoBuilder::new();

    rb.write_file("Gemfile", "source 'https://rubygems.org'\ngem 'rspec'\n");
    rb.commit("Initial commit");
    rb.create_branch("main");

    rb.create_branch("feature/tests");
    rb.checkout("feature/tests");

    rb.write_file(
        "app/services/user_service.rb",
        "class UserService\n  def greet(name)\n    name.to_s()\n  end\nend\n",
    );

    rb.write_file(
        "spec/services/user_service_spec.rb",
        r#"require 'rspec'
require_relative '../../app/services/user_service'

RSpec.describe(UserService)

class UserServiceSpec
  def test_greet
    service = UserService.new()
    result = service.greet("Alice")
  end

  def test_greet_empty
    service = UserService.new()
    result = service.greet("")
  end
end
"#,
    );

    rb.commit("Add UserService with RSpec tests");

    let result = run_pipeline(rb.path(), "main", "feature/tests");

    // Verify test file detection
    let test_eps: Vec<_> = result
        .groups
        .iter()
        .flat_map(|g| g.entrypoint.as_ref())
        .filter(|ep| ep.entrypoint_type == flowdiff_core::types::EntrypointType::TestFile)
        .collect();
    assert!(
        !test_eps.is_empty(),
        "should detect *_spec.rb as test file entrypoint"
    );

    // Verify Ruby language detected
    assert_language_detected(&result, "ruby");

    // Verify RSpec framework detected
    let has_rspec = result
        .summary
        .frameworks_detected
        .iter()
        .any(|f| f.contains("RSpec"));
    assert!(
        has_rspec,
        "should detect RSpec framework; detected: {:?}",
        result.summary.frameworks_detected
    );
}

/// Test: Kotlin Ktor REST API with handler→service→repo pattern.
///
/// Verifies that the pipeline can:
/// - Parse Kotlin source files via tree-sitter
/// - Extract import statements (regular, aliased, wildcard)
/// - Detect fun, class, object, val/var definitions
/// - Detect Ktor route handler entrypoints
/// - Detect Ktor framework from imports
/// - Cluster files into meaningful flow groups
#[test]
fn test_e2e_kotlin_ktor_api() {
    let rb = RepoBuilder::new();

    // Initial commit
    rb.write_file("build.gradle.kts", "plugins {\n    kotlin(\"jvm\")\n}\n");
    rb.commit("Initial commit: build.gradle.kts");
    rb.create_branch("main");

    // Feature branch: add Ktor REST API
    rb.create_branch("feature/kotlin-api");
    rb.checkout("feature/kotlin-api");

    rb.write_file(
        "src/main/kotlin/routes/UserRoutes.kt",
        r#"import io.ktor.server.routing.Route
import io.ktor.server.routing.get
import io.ktor.server.routing.post
import io.ktor.server.response.respond
import com.example.services.UserService

fun Route.userRoutes(userService: UserService) {
    get("/users") {
        val users = userService.findAll()
        call.respond(users)
    }

    post("/users") {
        val user = userService.create(call)
        call.respond(user)
    }

    get("/users/{id}") {
        val user = userService.findById(call)
        call.respond(user)
    }
}
"#,
    );

    rb.write_file(
        "src/main/kotlin/services/UserService.kt",
        r#"import com.example.repositories.UserRepository
import com.example.models.User

class UserService(private val repository: UserRepository) {
    fun findAll(): List<User> {
        val users = repository.findAll()
        return users
    }

    fun findById(id: String): User {
        val user = repository.findById(id)
        return user
    }

    fun create(data: Map<String, String>): User {
        val user = repository.save(data)
        return user
    }
}
"#,
    );

    rb.write_file(
        "src/main/kotlin/repositories/UserRepository.kt",
        r#"import org.jetbrains.exposed.sql.Database
import com.example.models.User

class UserRepository(private val db: Database) {
    fun findAll(): List<User> {
        val results = db.query("SELECT * FROM users")
        return results
    }

    fun findById(id: String): User {
        val result = db.query("SELECT * FROM users WHERE id = ?")
        return result
    }

    fun save(data: Map<String, String>): User {
        val result = db.execute("INSERT INTO users ...")
        return result
    }
}
"#,
    );

    rb.write_file(
        "src/main/kotlin/models/User.kt",
        r#"import kotlinx.serialization.Serializable

@Serializable
data class User(
    val id: String,
    val name: String,
    val email: String
)
"#,
    );

    rb.write_file(
        "src/main/kotlin/Application.kt",
        r#"import io.ktor.server.engine.embeddedServer
import io.ktor.server.netty.Netty
import com.example.routes.userRoutes
import com.example.services.UserService
import com.example.repositories.UserRepository

fun main() {
    val repo = UserRepository()
    val service = UserService(repo)
    embeddedServer(Netty, port = 8080) {
        userRoutes(service)
    }
}
"#,
    );

    rb.commit("Add Ktor REST API with routes-service-repo");

    let result = run_pipeline(rb.path(), "main", "feature/kotlin-api");

    // Verify basic output shape
    assert_valid_json_schema(&result);
    assert_valid_scores(&result);

    // Verify Kotlin files were detected
    assert_language_detected(&result, "kotlin");

    // Verify all changed files are accounted for
    assert_all_files_accounted(&result);

    // Verify there are flow groups
    assert!(
        !result.groups.is_empty(),
        "should produce at least one flow group"
    );

    // Verify entrypoint detection (Ktor route handlers or main)
    let has_entrypoint = result.groups.iter().any(|g| g.entrypoint.is_some());
    assert!(
        has_entrypoint,
        "should detect at least one entrypoint (Ktor routes or main)"
    );

    // Verify Ktor framework detection
    let has_ktor = result
        .summary
        .frameworks_detected
        .iter()
        .any(|f| f.contains("Ktor"));
    assert!(
        has_ktor,
        "should detect Ktor framework; detected: {:?}",
        result.summary.frameworks_detected
    );

    // Verify Mermaid graph is valid
    assert_valid_mermaid(&result);

    // Verify JSON roundtrip
    assert_json_roundtrip(&result);
}

/// Test: Kotlin test file detection.
///
/// Verifies that *Test.kt files are detected as test entrypoints.
#[test]
fn test_e2e_kotlin_test_file_detection() {
    let rb = RepoBuilder::new();

    rb.write_file("build.gradle.kts", "plugins {\n    kotlin(\"jvm\")\n}\n");
    rb.commit("Initial commit");
    rb.create_branch("main");

    rb.create_branch("feature/tests");
    rb.checkout("feature/tests");

    rb.write_file(
        "src/main/kotlin/services/UserService.kt",
        r#"import com.example.models.User

class UserService {
    fun greet(name: String): String {
        return "Hello, $name"
    }
}
"#,
    );

    rb.write_file(
        "src/test/kotlin/services/UserServiceTest.kt",
        r#"import org.junit.Test
import com.example.services.UserService

class UserServiceTest {
    fun testGreet() {
        val service = UserService()
        val result = service.greet("Alice")
    }

    fun testGreetEmpty() {
        val service = UserService()
        val result = service.greet("")
    }
}
"#,
    );

    rb.commit("Add UserService with JUnit tests");

    let result = run_pipeline(rb.path(), "main", "feature/tests");

    // Verify test file detection
    let test_eps: Vec<_> = result
        .groups
        .iter()
        .flat_map(|g| g.entrypoint.as_ref())
        .filter(|ep| ep.entrypoint_type == flowdiff_core::types::EntrypointType::TestFile)
        .collect();
    assert!(
        !test_eps.is_empty(),
        "should detect *Test.kt as test file entrypoint"
    );

    // Verify Kotlin language detected
    assert_language_detected(&result, "kotlin");

    // Verify JUnit framework detected
    let has_junit = result
        .summary
        .frameworks_detected
        .iter()
        .any(|f| f.contains("JUnit"));
    assert!(
        has_junit,
        "should detect JUnit framework; detected: {:?}",
        result.summary.frameworks_detected
    );
}

/// Test: Swift Vapor REST API with controller→service→repo pattern.
///
/// Creates a synthetic Swift Vapor app to verify:
/// - Swift file detection (.swift extension)
/// - Import extraction (module-level imports)
/// - Definition extraction (struct, class, protocol, func)
/// - Call site extraction (method calls, function calls)
/// - Entrypoint detection (Vapor route handlers)
/// - Framework detection (Vapor, Fluent)
/// - Semantic grouping and ranking
#[test]
fn test_e2e_swift_vapor_api() {
    let rb = RepoBuilder::new();

    // Initial commit
    rb.write_file("Package.swift", "// swift-tools-version:5.9\nimport PackageDescription\n");
    rb.commit("Initial commit: Package.swift");
    rb.create_branch("main");

    // Feature branch: add Vapor REST API
    rb.create_branch("feature/swift-api");
    rb.checkout("feature/swift-api");

    rb.write_file(
        "Sources/App/Controllers/UserController.swift",
        r#"import Vapor
import Fluent

struct UserController: RouteCollection {
    func boot(routes: RoutesBuilder) throws {
        let users = routes.grouped("users")
        users.get(use: index)
        users.post(use: create)
    }

    func index(req: Request) throws -> EventLoopFuture<[User]> {
        return User.query(on: req.db).all()
    }

    func create(req: Request) throws -> EventLoopFuture<User> {
        let user = try req.content.decode(User.self)
        return user.save(on: req.db).map { user }
    }
}
"#,
    );

    rb.write_file(
        "Sources/App/Services/UserService.swift",
        r#"import Foundation
import Vapor

class UserService {
    let repository: UserRepository

    init(repository: UserRepository) {
        self.repository = repository
    }

    func findAll() -> [User] {
        let users = repository.findAll()
        return users
    }

    func findById(id: UUID) -> User? {
        let user = repository.findById(id: id)
        return user
    }

    func create(name: String) -> User {
        let user = repository.save(name: name)
        return user
    }
}
"#,
    );

    rb.write_file(
        "Sources/App/Repositories/UserRepository.swift",
        r#"import Foundation
import Fluent

class UserRepository {
    let db: Database

    init(db: Database) {
        self.db = db
    }

    func findAll() -> [User] {
        let results = db.query(User.self)
        return results
    }

    func findById(id: UUID) -> User? {
        let result = db.find(User.self, id: id)
        return result
    }

    func save(name: String) -> User {
        let user = User(name: name)
        db.save(user)
        return user
    }
}
"#,
    );

    rb.write_file(
        "Sources/App/Models/User.swift",
        r#"import Foundation
import Fluent

final class User: Model, Content {
    static let schema = "users"

    var id: UUID?
    var name: String

    init() {}

    init(id: UUID? = nil, name: String) {
        self.id = id
        self.name = name
    }
}
"#,
    );

    rb.write_file(
        "Sources/App/configure.swift",
        r#"import Vapor
import Fluent

func configure(app: Application) throws {
    let controller = UserController()
    try app.register(collection: controller)
}
"#,
    );

    rb.commit("Add Vapor REST API with controller-service-repo");

    let result = run_pipeline(rb.path(), "main", "feature/swift-api");

    // Verify basic output shape
    assert_valid_json_schema(&result);
    assert_valid_scores(&result);

    // Verify Swift files were detected
    assert_language_detected(&result, "swift");

    // Verify all changed files are accounted for
    assert_all_files_accounted(&result);

    // Verify there are flow groups
    assert!(
        !result.groups.is_empty(),
        "should produce at least one flow group"
    );

    // Verify entrypoint detection (Vapor route handlers)
    let has_entrypoint = result.groups.iter().any(|g| g.entrypoint.is_some());
    assert!(
        has_entrypoint,
        "should detect at least one entrypoint (Vapor routes)"
    );

    // Verify Vapor framework detection
    let has_vapor = result
        .summary
        .frameworks_detected
        .iter()
        .any(|f| f.contains("Vapor"));
    assert!(
        has_vapor,
        "should detect Vapor framework; detected: {:?}",
        result.summary.frameworks_detected
    );

    // Verify Mermaid graph is valid
    assert_valid_mermaid(&result);

    // Verify JSON roundtrip
    assert_json_roundtrip(&result);
}

/// Test: Swift test file detection.
///
/// Verifies that *Tests.swift and *Test.swift files are detected as test entrypoints.
#[test]
fn test_e2e_swift_test_file_detection() {
    let rb = RepoBuilder::new();

    rb.write_file("Package.swift", "// swift-tools-version:5.9\n");
    rb.commit("Initial commit");
    rb.create_branch("main");

    rb.create_branch("feature/tests");
    rb.checkout("feature/tests");

    rb.write_file(
        "Sources/App/Services/UserService.swift",
        r#"import Foundation

class UserService {
    func greet(name: String) -> String {
        return "Hello, \(name)"
    }
}
"#,
    );

    rb.write_file(
        "Tests/AppTests/UserServiceTests.swift",
        r#"import XCTest
import Foundation

final class UserServiceTests: XCTestCase {
    func testGreet() {
        let service = UserService()
        let result = service.greet(name: "Alice")
    }

    func testGreetEmpty() {
        let service = UserService()
        let result = service.greet(name: "")
    }
}
"#,
    );

    rb.commit("Add UserService with XCTest tests");

    let result = run_pipeline(rb.path(), "main", "feature/tests");

    // Verify test file detection
    let test_eps: Vec<_> = result
        .groups
        .iter()
        .flat_map(|g| g.entrypoint.as_ref())
        .filter(|ep| ep.entrypoint_type == flowdiff_core::types::EntrypointType::TestFile)
        .collect();
    assert!(
        !test_eps.is_empty(),
        "should detect *Tests.swift as test file entrypoint"
    );

    // Verify Swift language detected
    assert_language_detected(&result, "swift");

    // Verify XCTest framework detected
    let has_xctest = result
        .summary
        .frameworks_detected
        .iter()
        .any(|f| f.contains("XCTest"));
    assert!(
        has_xctest,
        "should detect XCTest framework; detected: {:?}",
        result.summary.frameworks_detected
    );
}

// ─── C++ Integration Tests ──────────────────────────────────────────────

/// Test: Synthetic C++ REST API with handler→service→repo pattern.
///
/// Verifies: C++ language detection, #include import resolution,
/// class/function extraction, call graph across files, framework detection,
/// entrypoint detection (main), semantic grouping.
#[test]
fn test_e2e_cpp_http_server() {
    let rb = RepoBuilder::new();

    // Initial commit
    rb.write_file("CMakeLists.txt", "cmake_minimum_required(VERSION 3.14)\nproject(myapp)\n");
    rb.commit("Initial commit: CMakeLists.txt");
    rb.create_branch("main");

    // Feature branch: add HTTP server
    rb.create_branch("feature/cpp-api");
    rb.checkout("feature/cpp-api");

    rb.write_file(
        "src/handlers/user_handler.cpp",
        r#"#include <iostream>
#include "user_handler.hpp"
#include "../services/user_service.hpp"

void UserHandler::handle_list(const Request& req) {
    auto users = service_.list_users();
    send_response(req, users);
}

void UserHandler::handle_create(const Request& req) {
    auto name = parse_body(req);
    auto user = service_.create_user(name);
    send_response(req, user);
}
"#,
    );

    rb.write_file(
        "src/services/user_service.hpp",
        r#"#pragma once
#include <string>
#include <vector>
#include "../models/user.hpp"
#include "../repositories/user_repository.hpp"

class UserService {
public:
    std::vector<User> list_users();
    User create_user(const std::string& name);
private:
    UserRepository repo_;
};
"#,
    );

    rb.write_file(
        "src/services/user_service.cpp",
        r#"#include "user_service.hpp"

std::vector<User> UserService::list_users() {
    auto result = repo_.find_all();
    return result;
}

User UserService::create_user(const std::string& name) {
    auto user = repo_.save(name);
    return user;
}
"#,
    );

    rb.write_file(
        "src/repositories/user_repository.hpp",
        r#"#pragma once
#include <string>
#include <vector>
#include "../models/user.hpp"

class UserRepository {
public:
    std::vector<User> find_all();
    User save(const std::string& name);
};
"#,
    );

    rb.write_file(
        "src/repositories/user_repository.cpp",
        r#"#include "user_repository.hpp"
#include <sqlite3.h>

std::vector<User> UserRepository::find_all() {
    auto db = open_db();
    auto results = query(db, "SELECT * FROM users");
    return results;
}

User UserRepository::save(const std::string& name) {
    auto db = open_db();
    auto result = execute(db, "INSERT INTO users (name) VALUES (?)", name);
    return result;
}
"#,
    );

    rb.write_file(
        "src/models/user.hpp",
        r#"#pragma once
#include <string>

struct User {
    int id;
    std::string name;
};
"#,
    );

    rb.write_file(
        "src/main.cpp",
        r#"#include <iostream>
#include "handlers/user_handler.hpp"

int main() {
    auto handler = UserHandler();
    auto server = init_server();
    register_routes(server, handler);
    start_server(server);
    return 0;
}
"#,
    );

    rb.commit("Add C++ HTTP server with handler-service-repo");

    let result = run_pipeline(rb.path(), "main", "feature/cpp-api");

    // Verify basic output shape
    assert_valid_json_schema(&result);
    assert_valid_scores(&result);

    // Verify C++ files were detected
    assert_language_detected(&result, "cpp");

    // Verify all changed files are accounted for
    assert_all_files_accounted(&result);

    // Verify there are flow groups
    assert!(
        !result.groups.is_empty(),
        "should produce at least one flow group"
    );

    // Verify entrypoint detection (main function)
    let has_entrypoint = result.groups.iter().any(|g| g.entrypoint.is_some());
    assert!(
        has_entrypoint,
        "should detect at least one entrypoint (main)"
    );

    // Verify C++ STL framework detection
    let has_stl = result
        .summary
        .frameworks_detected
        .iter()
        .any(|f| f.contains("STL") || f.contains("C++"));
    assert!(
        has_stl,
        "should detect C++ STL; detected: {:?}",
        result.summary.frameworks_detected
    );

    // Verify Mermaid graph is valid
    assert_valid_mermaid(&result);

    // Verify JSON roundtrip
    assert_json_roundtrip(&result);
}

/// Test: C test file detection.
///
/// Verifies that *_test.c and files in test/ directories are detected as test entrypoints.
#[test]
fn test_e2e_c_test_file_detection() {
    let rb = RepoBuilder::new();

    rb.write_file("Makefile", "all: build\n");
    rb.commit("Initial commit");
    rb.create_branch("main");

    rb.create_branch("feature/tests");
    rb.checkout("feature/tests");

    rb.write_file(
        "src/math.c",
        r#"#include "math.h"

int add(int a, int b) {
    return a + b;
}

int multiply(int a, int b) {
    return a * b;
}
"#,
    );

    rb.write_file(
        "tests/math_test.c",
        r#"#include <stdio.h>
#include "../src/math.h"

void test_add() {
    int result = add(2, 3);
    printf("test_add: %s\n", result == 5 ? "PASS" : "FAIL");
}

void test_multiply() {
    int result = multiply(3, 4);
    printf("test_multiply: %s\n", result == 12 ? "PASS" : "FAIL");
}

int main() {
    test_add();
    test_multiply();
    return 0;
}
"#,
    );

    rb.commit("Add math module with tests");

    let result = run_pipeline(rb.path(), "main", "feature/tests");

    assert_valid_json_schema(&result);
    assert_all_files_accounted(&result);

    // Verify C language detected
    assert_language_detected(&result, "c");

    // Verify test file detected as entrypoint
    let has_test_entrypoint = result.groups.iter().any(|g| {
        g.entrypoint.as_ref().map_or(false, |e| {
            e.entrypoint_type == flowdiff_core::types::EntrypointType::TestFile
                || e.entrypoint_type == flowdiff_core::types::EntrypointType::CliCommand
        })
    });
    assert!(
        has_test_entrypoint,
        "should detect test file entrypoint; groups: {:?}",
        result.groups.iter().map(|g| (&g.name, &g.entrypoint)).collect::<Vec<_>>()
    );
}
