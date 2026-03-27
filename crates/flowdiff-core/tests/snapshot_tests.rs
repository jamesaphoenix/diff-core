#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
//! Snapshot tests for flowdiff JSON output (spec §13.6).
//!
//! Uses the `insta` crate to snapshot the full `AnalysisOutput` JSON for each
//! fixture repo. If the graph construction, ranking, or clustering algorithm
//! changes, review and approve new snapshots with `cargo insta review`.
//!
//! Run with:
//!   cargo test --test snapshot_tests
//!
//! Review pending snapshots:
//!   cargo insta review

mod helpers;

use helpers::repo_builder::{run_pipeline, RepoBuilder};

// ─── Helpers ──────────────────────────────────────────────────────────────

/// Convert AnalysisOutput to a serde_json::Value and redact non-deterministic
/// fields (git SHAs) so snapshots are stable across runs.
fn snapshot_value(output: &flowdiff_core::types::AnalysisOutput) -> serde_json::Value {
    let mut v: serde_json::Value = serde_json::to_value(output).unwrap();
    // Redact git SHAs — these change on every test run because TempDir paths differ
    if let Some(ds) = v.get_mut("diff_source") {
        if let Some(obj) = ds.as_object_mut() {
            if obj.contains_key("base_sha") {
                obj.insert("base_sha".into(), serde_json::json!("[sha]"));
            }
            if obj.contains_key("head_sha") {
                obj.insert("head_sha".into(), serde_json::json!("[sha]"));
            }
        }
    }
    v
}

// ─── Snapshot Tests ───────────────────────────────────────────────────────

/// Snapshot: Simple 5-file Express app with a user creation flow.
///
/// Tests the core happy path: route → service → repo chain produces
/// a flow group with files in correct flow order.
#[test]
fn snapshot_simple_express_app() {
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
    rb.write_file(
        "package.json",
        r#"{"name": "test-app", "version": "1.0.0"}"#,
    );
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

    let output = run_pipeline(rb.path(), "main", "feature/add-users");
    let v = snapshot_value(&output);
    insta::assert_json_snapshot!("simple_express_app", v);
}

/// Snapshot: Python FastAPI app with endpoint + service + repository.
///
/// Verifies entrypoint detection for Python decorators and data flow tracing.
#[test]
fn snapshot_python_fastapi() {
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
    let v = snapshot_value(&output);
    insta::assert_json_snapshot!("python_fastapi", v);
}

/// Snapshot: Cross-cutting refactor — rename a shared utility used by 5 files.
///
/// All 6 files (utility + 5 consumers) should be accounted for, either in
/// flow groups or infrastructure.
#[test]
fn snapshot_cross_cutting_refactor() {
    let rb = RepoBuilder::new();

    // Initial: shared utility + 5 consumers
    rb.write_file(
        "src/utils/format.ts",
        r#"
export function formatDate(d: Date): string {
    return d.toISOString();
}
"#,
    );
    for i in 1..=5 {
        rb.write_file(
            &format!("src/services/service{}.ts", i),
            &format!(
                r#"
import {{ formatDate }} from '../utils/format';

export function process{}(data: any) {{
    return {{ ...data, timestamp: formatDate(new Date()) }};
}}
"#,
                i
            ),
        );
    }
    rb.write_file(
        "package.json",
        r#"{"name": "refactor-app", "version": "1.0.0"}"#,
    );
    rb.commit("Initial commit");
    rb.create_branch("main");

    rb.create_branch("refactor/rename-format");
    rb.checkout("refactor/rename-format");

    // Rename formatDate → formatTimestamp in utility and all consumers
    rb.write_file(
        "src/utils/format.ts",
        r#"
export function formatTimestamp(d: Date): string {
    return d.toISOString();
}
"#,
    );
    for i in 1..=5 {
        rb.write_file(
            &format!("src/services/service{}.ts", i),
            &format!(
                r#"
import {{ formatTimestamp }} from '../utils/format';

export function process{}(data: any) {{
    return {{ ...data, timestamp: formatTimestamp(new Date()) }};
}}
"#,
                i
            ),
        );
    }
    rb.commit("Rename formatDate to formatTimestamp");

    let output = run_pipeline(rb.path(), "main", "refactor/rename-format");
    let v = snapshot_value(&output);
    insta::assert_json_snapshot!("cross_cutting_refactor", v);
}

/// Snapshot: Multi-entrypoint app — HTTP handler + queue worker touching shared code.
///
/// Should produce 2 distinct flow groups, with the shared file assigned to the
/// nearest entrypoint by distance.
#[test]
fn snapshot_multi_entrypoint() {
    let rb = RepoBuilder::new();

    rb.write_file(
        "package.json",
        r#"{"name": "multi-entry", "version": "1.0.0"}"#,
    );
    rb.commit("Initial");
    rb.create_branch("main");

    rb.create_branch("feature/multi");
    rb.checkout("feature/multi");

    // HTTP route entrypoint
    rb.write_file(
        "src/routes/orders.ts",
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

    // Queue consumer entrypoint
    rb.write_file(
        "src/workers/orderWorker.ts",
        r#"
import { processOrder } from '../services/orderService';

export function handleOrderMessage(message: any) {
    const data = JSON.parse(message.body);
    processOrder(data);
    message.ack();
}
"#,
    );

    // Shared service
    rb.write_file(
        "src/services/orderService.ts",
        r#"
import { saveOrder } from '../repositories/orderRepo';
import { sendNotification } from '../utils/notify';

export function processOrder(orderData: any) {
    const order = { id: Date.now(), ...orderData, status: 'processed' };
    saveOrder(order);
    sendNotification('order_created', order);
    return order;
}
"#,
    );

    // Repository
    rb.write_file(
        "src/repositories/orderRepo.ts",
        r#"
const orders: any[] = [];

export function saveOrder(order: any) {
    orders.push(order);
    return order;
}
"#,
    );

    // Notification utility
    rb.write_file(
        "src/utils/notify.ts",
        r#"
export function sendNotification(event: string, payload: any) {
    console.log(`[${event}]`, payload);
}
"#,
    );
    rb.commit("Add order flow with HTTP + worker entrypoints");

    let output = run_pipeline(rb.path(), "main", "feature/multi");
    let v = snapshot_value(&output);
    insta::assert_json_snapshot!("multi_entrypoint", v);
}

/// Snapshot: Mixed language project (TypeScript + Python).
///
/// Verifies both languages are detected and files are grouped correctly.
#[test]
fn snapshot_mixed_language() {
    let rb = RepoBuilder::new();

    rb.write_file(
        "package.json",
        r#"{"name": "mixed-lang", "version": "1.0.0"}"#,
    );
    rb.write_file("requirements.txt", "fastapi\n");
    rb.commit("Initial");
    rb.create_branch("main");

    rb.create_branch("feature/mixed");
    rb.checkout("feature/mixed");

    // TypeScript route
    rb.write_file(
        "frontend/src/routes/dashboard.ts",
        r#"
import express from 'express';
import { fetchStats } from '../services/statsService';

const router = express.Router();

export function getDashboard(req: any, res: any) {
    const stats = fetchStats();
    res.json(stats);
}

router.get('/dashboard', getDashboard);
export default router;
"#,
    );
    rb.write_file(
        "frontend/src/services/statsService.ts",
        r#"
export function fetchStats() {
    return { users: 100, orders: 50 };
}
"#,
    );

    // Python API
    rb.write_file(
        "backend/app/routes/analytics.py",
        r#"
from fastapi import APIRouter
from backend.app.services.analytics_service import compute_analytics

router = APIRouter()

@router.get("/analytics")
async def get_analytics():
    return compute_analytics()
"#,
    );
    rb.write_file(
        "backend/app/services/analytics_service.py",
        r#"
def compute_analytics():
    return {"total_revenue": 5000, "avg_order": 100}
"#,
    );
    rb.commit("Add dashboard + analytics");

    let output = run_pipeline(rb.path(), "main", "feature/mixed");
    let v = snapshot_value(&output);
    insta::assert_json_snapshot!("mixed_language", v);
}

/// Snapshot: Infrastructure-heavy change (config files, Docker, CI).
///
/// Verifies that true infrastructure files land in the infrastructure group
/// with correct sub-group classification.
#[test]
fn snapshot_infrastructure_heavy() {
    let rb = RepoBuilder::new();

    rb.write_file(
        "package.json",
        r#"{"name": "infra-app", "version": "1.0.0"}"#,
    );
    rb.commit("Initial");
    rb.create_branch("main");

    rb.create_branch("chore/infra-update");
    rb.checkout("chore/infra-update");

    rb.write_file("Dockerfile", "FROM node:20-slim\nWORKDIR /app\nCOPY . .\nRUN npm install\nCMD [\"node\", \"index.js\"]\n");
    rb.write_file(
        "docker-compose.yml",
        "version: '3'\nservices:\n  app:\n    build: .\n    ports:\n      - '3000:3000'\n",
    );
    rb.write_file(
        ".env.dev",
        "DATABASE_URL=postgres://localhost/dev\nREDIS_URL=redis://localhost\n",
    );
    rb.write_file(
        "tsconfig.json",
        r#"{"compilerOptions": {"target": "es2020", "module": "commonjs"}}"#,
    );
    rb.write_file(".github/workflows/ci.yml", "name: CI\non: [push]\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n");
    rb.write_file(
        "scripts/deploy.sh",
        "#!/bin/bash\nnpm run build\nnpm run deploy\n",
    );
    rb.write_file(
        "docs/setup.md",
        "# Setup\n\nInstall dependencies with `npm install`.\n",
    );
    rb.write_file(
        "migrations/001_init.sql",
        "CREATE TABLE users (id SERIAL PRIMARY KEY, name TEXT NOT NULL);\n",
    );
    rb.commit("Update infrastructure and config");

    let output = run_pipeline(rb.path(), "main", "chore/infra-update");
    let v = snapshot_value(&output);
    insta::assert_json_snapshot!("infrastructure_heavy", v);
}

/// Snapshot: Go HTTP API with handler → service → repository pattern.
///
/// Verifies Go language support and entrypoint detection.
#[test]
fn snapshot_go_http_api() {
    let rb = RepoBuilder::new();

    rb.write_file("go.mod", "module example.com/api\n\ngo 1.21\n");
    rb.commit("Initial");
    rb.create_branch("main");

    rb.create_branch("feature/go-api");
    rb.checkout("feature/go-api");

    rb.write_file(
        "internal/handlers/user.go",
        r#"package handlers

import (
    "encoding/json"
    "net/http"
    "example.com/api/internal/services"
)

func CreateUser(w http.ResponseWriter, r *http.Request) {
    var input map[string]interface{}
    json.NewDecoder(r.Body).Decode(&input)
    user := services.CreateUser(input)
    json.NewEncoder(w).Encode(user)
}
"#,
    );
    rb.write_file(
        "internal/services/user_service.go",
        r#"package services

import "example.com/api/internal/repositories"

func CreateUser(data map[string]interface{}) map[string]interface{} {
    data["id"] = 1
    repositories.SaveUser(data)
    return data
}
"#,
    );
    rb.write_file(
        "internal/repositories/user_repo.go",
        r#"package repositories

var users []map[string]interface{}

func SaveUser(user map[string]interface{}) {
    users = append(users, user)
}
"#,
    );
    rb.commit("Add Go user API");

    let output = run_pipeline(rb.path(), "main", "feature/go-api");
    let v = snapshot_value(&output);
    insta::assert_json_snapshot!("go_http_api", v);
}

/// Snapshot: Empty diff — no changes between base and head.
///
/// Verifies graceful empty result with zero groups.
#[test]
fn snapshot_empty_diff() {
    let rb = RepoBuilder::new();

    rb.write_file("src/index.ts", "export const VERSION = '1.0.0';\n");
    rb.commit("Initial");
    rb.create_branch("main");

    // Create a branch at the same commit — no diff
    rb.create_branch("feature/no-changes");
    rb.checkout("feature/no-changes");

    let output = run_pipeline(rb.path(), "main", "feature/no-changes");
    let v = snapshot_value(&output);
    insta::assert_json_snapshot!("empty_diff", v);
}
