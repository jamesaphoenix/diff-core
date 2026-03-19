//! Synthetic Eval Suite (Phase 7) — validates the full flowdiff pipeline against
//! known-good baselines for realistic fixture codebases.
//!
//! Each fixture defines:
//! - A synthetic git repo with realistic code structure
//! - Expected output baselines (groups, entrypoints, languages, risk ordering)
//! - Scoring functions that produce per-criterion scores in [0.0, 1.0]
//!
#![allow(dead_code)]
//! Run with:
//!   cargo test --test eval_suite
//!
//! Architecture references:
//! - Evaluator-Optimizer pattern: score = f(output), never changes between runs
//! - Golden references: human-curated baselines for what "good" looks like
//! - Regression detection: minimum score thresholds catch pipeline degradation

mod helpers;

use std::collections::HashSet;

use flowdiff_core::output;
use flowdiff_core::types::{
    AnalysisOutput, ChangeStats, DiffSource, DiffType, AnalysisSummary,
    EntrypointType, FileChange, FileRole, FlowGroup, GroupRankInput, InfrastructureGroup,
    RankWeights,
};
use helpers::repo_builder::{find_feature_branch, run_pipeline, RepoBuilder};

// ═══════════════════════════════════════════════════════════════════════════
// Baseline Types
// ═══════════════════════════════════════════════════════════════════════════

/// Expected entrypoint in a fixture baseline.
#[derive(Debug, Clone)]
struct ExpectedEntrypoint {
    /// Substring that must appear in the entrypoint's file path
    file_contains: String,
    /// Expected entrypoint type
    ep_type: EntrypointType,
}

/// Expected flow group in a fixture baseline.
#[derive(Debug, Clone)]
struct ExpectedGroup {
    /// Descriptive label for the expected group (for error messages)
    label: String,
    /// File path substrings that must all appear in the same group
    must_contain: Vec<String>,
    /// File path substrings that must NOT appear in this group
    must_not_contain: Vec<String>,
}

/// Expected risk ordering constraint.
/// "The group containing `higher_risk_file` should be reviewed before `lower_risk_file`."
#[derive(Debug, Clone)]
struct RiskOrderingConstraint {
    higher_risk_file: String,
    lower_risk_file: String,
}

/// Complete baseline for a synthetic fixture.
#[derive(Debug, Clone)]
struct EvalBaseline {
    /// Human-readable fixture name
    name: String,
    /// Expected languages detected
    expected_languages: Vec<String>,
    /// Bounds on number of flow groups (not counting infrastructure)
    min_groups: usize,
    max_groups: usize,
    /// Expected total files changed
    expected_file_count: usize,
    /// Expected entrypoints
    expected_entrypoints: Vec<ExpectedEntrypoint>,
    /// Expected group compositions
    expected_groups: Vec<ExpectedGroup>,
    /// Risk ordering constraints
    risk_ordering: Vec<RiskOrderingConstraint>,
    /// Files expected in infrastructure group (not reachable from entrypoints)
    expected_infrastructure: Vec<String>,
}

// ═══════════════════════════════════════════════════════════════════════════
// Scoring Functions
// ═══════════════════════════════════════════════════════════════════════════

/// Per-criterion eval scores, all in [0.0, 1.0].
#[derive(Debug, Clone)]
struct EvalScores {
    /// Are flow groups semantically coherent? (right files grouped together)
    group_coherence: f64,
    /// Are entrypoints correctly identified?
    entrypoint_accuracy: f64,
    /// Is review ordering logical? (risk ordering constraints satisfied)
    review_ordering: f64,
    /// Are risk scores reasonable? (in valid range, auth > utils, etc.)
    risk_reasonableness: f64,
    /// Are languages correctly detected?
    language_detection: f64,
    /// Is the file count correct?
    file_accounting: f64,
    /// Overall weighted score
    overall: f64,
}

impl EvalScores {
    fn compute_overall(&mut self) {
        // Weighted average matching the spec's ranking weights spirit
        self.overall = 0.25 * self.group_coherence
            + 0.20 * self.entrypoint_accuracy
            + 0.15 * self.review_ordering
            + 0.15 * self.risk_reasonableness
            + 0.15 * self.language_detection
            + 0.10 * self.file_accounting;
    }
}

/// Score the pipeline output against a baseline.
fn score_output(output: &AnalysisOutput, baseline: &EvalBaseline) -> EvalScores {
    let group_coherence = score_group_coherence(output, baseline);
    let entrypoint_accuracy = score_entrypoint_accuracy(output, baseline);
    let review_ordering = score_review_ordering(output, baseline);
    let risk_reasonableness = score_risk_reasonableness(output);
    let language_detection = score_language_detection(output, baseline);
    let file_accounting = score_file_accounting(output, baseline);

    let mut scores = EvalScores {
        group_coherence,
        entrypoint_accuracy,
        review_ordering,
        risk_reasonableness,
        language_detection,
        file_accounting,
        overall: 0.0,
    };
    scores.compute_overall();
    scores
}

/// Score group coherence: do the right files end up in the same groups?
///
/// For each ExpectedGroup, check if all `must_contain` files are in the same group
/// and no `must_not_contain` files are in that group.
fn score_group_coherence(output: &AnalysisOutput, baseline: &EvalBaseline) -> f64 {
    if baseline.expected_groups.is_empty() {
        return 1.0;
    }

    let mut total_score = 0.0;
    let mut total_weight = 0.0;

    for expected in &baseline.expected_groups {
        let weight = expected.must_contain.len().max(1) as f64;
        total_weight += weight;

        // Find which group contains the most must_contain files
        let mut best_match_score: f64 = 0.0;

        for group in &output.groups {
            let group_paths: Vec<&str> = group.files.iter().map(|f| f.path.as_str()).collect();

            // Count how many must_contain files are in this group
            let contained = expected
                .must_contain
                .iter()
                .filter(|mc| group_paths.iter().any(|p| p.contains(mc.as_str())))
                .count();

            // Check must_not_contain violations
            let violations = expected
                .must_not_contain
                .iter()
                .filter(|mnc| group_paths.iter().any(|p| p.contains(mnc.as_str())))
                .count();

            if expected.must_contain.is_empty() {
                continue;
            }

            let contain_score = contained as f64 / expected.must_contain.len() as f64;
            let violation_penalty = if expected.must_not_contain.is_empty() {
                0.0
            } else {
                violations as f64 / expected.must_not_contain.len() as f64
            };

            let group_score = (contain_score - 0.5 * violation_penalty).max(0.0);
            best_match_score = best_match_score.max(group_score);
        }

        // Also check infrastructure group for must_contain files
        if let Some(ref infra) = output.infrastructure_group {
            let contained_in_infra = expected
                .must_contain
                .iter()
                .filter(|mc| infra.files.iter().any(|f| f.contains(mc.as_str())))
                .count();
            // Files in infrastructure that should be in a group penalize the score
            if contained_in_infra > 0 && !expected.must_contain.is_empty() {
                let infra_score =
                    1.0 - (contained_in_infra as f64 / expected.must_contain.len() as f64);
                best_match_score = best_match_score.max(0.0).min(infra_score);
            }
        }

        total_score += best_match_score * weight;
    }

    if total_weight == 0.0 {
        1.0
    } else {
        total_score / total_weight
    }
}

/// Score entrypoint detection accuracy.
fn score_entrypoint_accuracy(output: &AnalysisOutput, baseline: &EvalBaseline) -> f64 {
    if baseline.expected_entrypoints.is_empty() {
        return 1.0;
    }

    // Collect all entrypoints from the output groups
    let detected_entrypoints: Vec<_> = output
        .groups
        .iter()
        .filter_map(|g| g.entrypoint.as_ref())
        .collect();

    let mut matched = 0;
    for expected in &baseline.expected_entrypoints {
        let found = detected_entrypoints.iter().any(|ep| {
            ep.file.contains(&expected.file_contains) && ep.entrypoint_type == expected.ep_type
        });
        if found {
            matched += 1;
        }
    }

    matched as f64 / baseline.expected_entrypoints.len() as f64
}

/// Score review ordering: are risk ordering constraints satisfied?
fn score_review_ordering(output: &AnalysisOutput, baseline: &EvalBaseline) -> f64 {
    if baseline.risk_ordering.is_empty() {
        return 1.0;
    }

    let mut satisfied = 0;
    for constraint in &baseline.risk_ordering {
        let higher_group = output.groups.iter().find(|g| {
            g.files
                .iter()
                .any(|f| f.path.contains(&constraint.higher_risk_file))
        });
        let lower_group = output.groups.iter().find(|g| {
            g.files
                .iter()
                .any(|f| f.path.contains(&constraint.lower_risk_file))
        });

        match (higher_group, lower_group) {
            (Some(h), Some(l)) => {
                // Lower review_order number = reviewed first (higher priority)
                if h.review_order <= l.review_order {
                    satisfied += 1;
                }
            }
            // If one file is in infrastructure, the grouped one should be reviewed first
            (Some(_), None) => {
                satisfied += 1;
            }
            _ => {} // Can't evaluate — don't penalize
        }
    }

    satisfied as f64 / baseline.risk_ordering.len() as f64
}

/// Score risk reasonableness: are scores in valid range and sensible?
fn score_risk_reasonableness(output: &AnalysisOutput) -> f64 {
    if output.groups.is_empty() {
        return 1.0;
    }

    let mut score: f64 = 1.0;
    for group in &output.groups {
        // Risk scores must be in [0.0, 1.0]
        if group.risk_score < 0.0 || group.risk_score > 1.0 {
            score -= 0.25;
        }
        // Review order must be >= 1
        if group.review_order < 1 {
            score -= 0.25;
        }
        // Each group should have at least one file
        if group.files.is_empty() {
            score -= 0.25;
        }
    }

    // Review orders should be unique and form a valid permutation
    let mut orders: Vec<u32> = output.groups.iter().map(|g| g.review_order).collect();
    orders.sort();
    orders.dedup();
    if orders.len() != output.groups.len() {
        score -= 0.1; // Duplicate review orders
    }

    score.max(0.0)
}

/// Score language detection accuracy.
fn score_language_detection(output: &AnalysisOutput, baseline: &EvalBaseline) -> f64 {
    if baseline.expected_languages.is_empty() {
        return 1.0;
    }

    let detected: HashSet<&str> = output
        .summary
        .languages_detected
        .iter()
        .map(|s| s.as_str())
        .collect();

    let mut matched = 0;
    for lang in &baseline.expected_languages {
        if detected.contains(lang.as_str()) {
            matched += 1;
        }
    }

    matched as f64 / baseline.expected_languages.len() as f64
}

/// Score file accounting: correct number of files, all files accounted for.
fn score_file_accounting(output: &AnalysisOutput, baseline: &EvalBaseline) -> f64 {
    let mut score: f64 = 1.0;

    // Check total file count
    if output.summary.total_files_changed as usize != baseline.expected_file_count {
        score -= 0.5;
    }

    // Check all files are accounted for (in groups or infrastructure)
    let total_grouped: usize = output.groups.iter().map(|g| g.files.len()).sum();
    let infra: usize = output
        .infrastructure_group
        .as_ref()
        .map(|i| i.files.len())
        .unwrap_or(0);

    if total_grouped + infra != output.summary.total_files_changed as usize {
        score -= 0.5;
    }

    // Check group count bounds
    let group_count = output.groups.len();
    if group_count < baseline.min_groups || group_count > baseline.max_groups {
        score -= 0.25;
    }

    score.max(0.0)
}

/// Print a formatted eval report for a fixture.
fn print_eval_report(fixture_name: &str, scores: &EvalScores) {
    eprintln!("\n╔══════════════════════════════════════════╗");
    eprintln!("║  Eval: {:<33}║", fixture_name);
    eprintln!("╠══════════════════════════════════════════╣");
    eprintln!(
        "║  Group coherence:    {:.2}                 ║",
        scores.group_coherence
    );
    eprintln!(
        "║  Entrypoint accuracy:{:.2}                 ║",
        scores.entrypoint_accuracy
    );
    eprintln!(
        "║  Review ordering:    {:.2}                 ║",
        scores.review_ordering
    );
    eprintln!(
        "║  Risk reasonableness:{:.2}                 ║",
        scores.risk_reasonableness
    );
    eprintln!(
        "║  Language detection:  {:.2}                ║",
        scores.language_detection
    );
    eprintln!(
        "║  File accounting:    {:.2}                 ║",
        scores.file_accounting
    );
    eprintln!("╠══════════════════════════════════════════╣");
    eprintln!(
        "║  OVERALL:            {:.2}                 ║",
        scores.overall
    );
    eprintln!("╚══════════════════════════════════════════╝");
}

// ═══════════════════════════════════════════════════════════════════════════
// Fixture 1: TypeScript Express API with services + DB layer
// ═══════════════════════════════════════════════════════════════════════════

fn build_fixture_ts_express() -> (RepoBuilder, EvalBaseline) {
    let rb = RepoBuilder::new();

    // Base commit: existing app with health endpoint
    rb.write_file(
        "package.json",
        r#"{"name": "express-api", "version": "1.0.0"}"#,
    );
    rb.write_file(
        "src/app.ts",
        r#"
import express from 'express';
import healthRouter from './routes/health';

const app = express();
app.use('/api', healthRouter);
export default app;
"#,
    );
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
        "src/db/connection.ts",
        r#"
export function getConnection() {
    return { connected: true };
}
"#,
    );
    rb.commit("Initial Express app");
    rb.create_branch("main");

    // Feature branch: add user CRUD with full flow chain
    rb.create_branch("feature/user-crud");
    rb.checkout("feature/user-crud");

    // Route handler (entrypoint)
    rb.write_file(
        "src/routes/users.ts",
        r#"
import express from 'express';
import { createUser, getUser, listUsers } from '../services/userService';
import { validateUserInput } from '../middleware/validation';

const router = express.Router();

export function postUser(req: any, res: any) {
    const user = createUser(req.body);
    res.status(201).json(user);
}

export function getUserById(req: any, res: any) {
    const user = getUser(req.params.id);
    res.json(user);
}

export function getAllUsers(req: any, res: any) {
    const users = listUsers();
    res.json(users);
}

router.post('/users', validateUserInput, postUser);
router.get('/users/:id', getUserById);
router.get('/users', getAllUsers);
export default router;
"#,
    );

    // Service layer
    rb.write_file(
        "src/services/userService.ts",
        r#"
import { insertUser, findUser, findAllUsers } from '../repositories/userRepo';
import { hashPassword } from '../utils/crypto';
import { emitUserCreated } from '../events/userEvents';

export function createUser(data: any) {
    const hashedPassword = hashPassword(data.password);
    const user = { id: Date.now(), ...data, password: hashedPassword };
    const saved = insertUser(user);
    emitUserCreated(saved);
    return saved;
}

export function getUser(id: string) {
    return findUser(id);
}

export function listUsers() {
    return findAllUsers();
}
"#,
    );

    // Repository / persistence layer
    rb.write_file(
        "src/repositories/userRepo.ts",
        r#"
import { getConnection } from '../db/connection';

const users: any[] = [];

export function insertUser(user: any) {
    const conn = getConnection();
    users.push(user);
    return user;
}

export function findUser(id: string) {
    return users.find(u => u.id === id);
}

export function findAllUsers() {
    return [...users];
}
"#,
    );

    // Middleware
    rb.write_file(
        "src/middleware/validation.ts",
        r#"
export function validateUserInput(req: any, res: any, next: any) {
    if (!req.body.email) {
        return res.status(400).json({ error: 'Email required' });
    }
    next();
}
"#,
    );

    // Utility
    rb.write_file(
        "src/utils/crypto.ts",
        r#"
export function hashPassword(password: string): string {
    return 'hashed_' + password;
}
"#,
    );

    // Events
    rb.write_file(
        "src/events/userEvents.ts",
        r#"
export function emitUserCreated(user: any) {
    console.log('User created:', user.id);
}
"#,
    );

    // Auth middleware (separate concern — should be in a different group or infrastructure)
    rb.write_file(
        "src/middleware/auth.ts",
        r#"
export function requireAuth(req: any, res: any, next: any) {
    if (!req.headers.authorization) {
        return res.status(401).json({ error: 'Unauthorized' });
    }
    next();
}
"#,
    );

    // Schema/types file
    rb.write_file(
        "src/types/user.ts",
        r#"
export interface User {
    id: number;
    email: string;
    name: string;
    password: string;
}

export interface CreateUserInput {
    email: string;
    name: string;
    password: string;
}
"#,
    );

    rb.commit("Add user CRUD with full flow chain");

    let baseline = EvalBaseline {
        name: "TypeScript Express API".to_string(),
        expected_languages: vec!["typescript".to_string()],
        min_groups: 1,
        max_groups: 6,
        expected_file_count: 8,
        expected_entrypoints: vec![ExpectedEntrypoint {
            file_contains: "routes/users".to_string(),
            ep_type: EntrypointType::HttpRoute,
        }],
        expected_groups: vec![ExpectedGroup {
            label: "User CRUD flow".to_string(),
            must_contain: vec![
                "routes/users".to_string(),
                "services/userService".to_string(),
                "repositories/userRepo".to_string(),
            ],
            must_not_contain: vec![],
        }],
        risk_ordering: vec![RiskOrderingConstraint {
            higher_risk_file: "middleware/auth".to_string(),
            lower_risk_file: "utils/crypto".to_string(),
        }],
        expected_infrastructure: vec![],
    };

    (rb, baseline)
}

// ═══════════════════════════════════════════════════════════════════════════
// Fixture 2: Python FastAPI with SQLAlchemy + queue workers
// ═══════════════════════════════════════════════════════════════════════════

fn build_fixture_python_fastapi() -> (RepoBuilder, EvalBaseline) {
    let rb = RepoBuilder::new();

    rb.write_file(
        "requirements.txt",
        "fastapi\nuvicorn\nsqlalchemy\ncelery\n",
    );
    rb.write_file("app/__init__.py", "");
    rb.commit("Initial Python project");
    rb.create_branch("main");

    rb.create_branch("feature/order-processing");
    rb.checkout("feature/order-processing");

    rb.write_file(
        "app/routes/orders.py",
        r#"
from fastapi import APIRouter, Depends
from app.services.order_service import create_order, get_order
from app.schemas.order import OrderCreate, OrderResponse
from app.auth.dependencies import get_current_user

router = APIRouter()

@router.post("/orders", response_model=OrderResponse)
async def post_order(order: OrderCreate, user=Depends(get_current_user)):
    return create_order(order, user)

@router.get("/orders/{order_id}", response_model=OrderResponse)
async def read_order(order_id: int, user=Depends(get_current_user)):
    return get_order(order_id, user)
"#,
    );

    rb.write_file(
        "app/services/order_service.py",
        r#"
from app.repositories.order_repo import save_order, find_order
from app.tasks.notification_tasks import send_order_confirmation
from app.models.order import Order

def create_order(order_data, user):
    order = Order(
        user_id=user.id,
        items=order_data.items,
        total=sum(i.price for i in order_data.items),
    )
    saved = save_order(order)
    send_order_confirmation.delay(saved.id)
    return saved

def get_order(order_id, user):
    return find_order(order_id, user.id)
"#,
    );

    rb.write_file(
        "app/repositories/order_repo.py",
        r#"
from sqlalchemy.orm import Session
from app.db.session import get_db

def save_order(order):
    db = get_db()
    db.add(order)
    db.commit()
    db.refresh(order)
    return order

def find_order(order_id, user_id):
    db = get_db()
    return db.query(order).filter_by(id=order_id, user_id=user_id).first()
"#,
    );

    rb.write_file(
        "app/tasks/notification_tasks.py",
        r#"
from celery import shared_task
from app.services.email_service import send_email
from app.repositories.order_repo import find_order

@shared_task
def send_order_confirmation(order_id):
    order = find_order(order_id, None)
    send_email(order.user.email, "Order Confirmed", f"Order {order_id} confirmed")

@shared_task
def send_shipping_notification(order_id, tracking_number):
    order = find_order(order_id, None)
    send_email(order.user.email, "Order Shipped", f"Tracking: {tracking_number}")
"#,
    );

    rb.write_file(
        "app/services/email_service.py",
        r#"
import os

def send_email(to, subject, body):
    smtp_host = os.environ.get("SMTP_HOST", "localhost")
    print(f"Sending email to {to}: {subject}")
"#,
    );

    rb.write_file(
        "app/models/order.py",
        r#"
from sqlalchemy import Column, Integer, Float, ForeignKey
from app.db.base import Base

class Order(Base):
    __tablename__ = "orders"
    id = Column(Integer, primary_key=True)
    user_id = Column(Integer, ForeignKey("users.id"))
    total = Column(Float)
"#,
    );

    rb.write_file(
        "app/schemas/order.py",
        r#"
class OrderItem:
    name: str
    price: float

class OrderCreate:
    items: list

class OrderResponse:
    id: int
    total: float
    status: str
"#,
    );

    rb.write_file(
        "app/auth/dependencies.py",
        r#"
from fastapi import Depends, HTTPException

def get_current_user():
    return {"id": 1, "email": "test@test.com"}
"#,
    );

    rb.write_file(
        "app/db/session.py",
        r#"
from sqlalchemy import create_engine
from sqlalchemy.orm import sessionmaker
import os

engine = create_engine(os.environ.get("DATABASE_URL", "sqlite:///./test.db"))
SessionLocal = sessionmaker(bind=engine)

def get_db():
    return SessionLocal()
"#,
    );

    rb.write_file(
        "app/db/base.py",
        r#"
from sqlalchemy.ext.declarative import declarative_base

Base = declarative_base()
"#,
    );

    rb.commit("Add order processing with queue worker");

    let baseline = EvalBaseline {
        name: "Python FastAPI + Celery".to_string(),
        expected_languages: vec!["python".to_string()],
        min_groups: 1,
        max_groups: 8,
        expected_file_count: 10,
        expected_entrypoints: vec![ExpectedEntrypoint {
            file_contains: "routes/orders".to_string(),
            ep_type: EntrypointType::HttpRoute,
        }],
        expected_groups: vec![ExpectedGroup {
            label: "Order API flow".to_string(),
            must_contain: vec![
                "routes/orders".to_string(),
                "services/order_service".to_string(),
                "repositories/order_repo".to_string(),
            ],
            must_not_contain: vec![],
        }],
        risk_ordering: vec![],
        expected_infrastructure: vec![],
    };

    (rb, baseline)
}

// ═══════════════════════════════════════════════════════════════════════════
// Fixture 3: Next.js Fullstack with React pages + API routes + Prisma
// ═══════════════════════════════════════════════════════════════════════════

fn build_fixture_nextjs_fullstack() -> (RepoBuilder, EvalBaseline) {
    let rb = RepoBuilder::new();

    rb.write_file("package.json", r#"{"name": "nextjs-app", "version": "1.0.0", "dependencies": {"next": "14.0.0", "@prisma/client": "5.0.0"}}"#);
    rb.write_file(
        "next.config.js",
        "module.exports = { reactStrictMode: true };\n",
    );
    rb.commit("Initial Next.js project");
    rb.create_branch("main");

    rb.create_branch("feature/product-listing");
    rb.checkout("feature/product-listing");

    rb.write_file(
        "src/app/api/products/route.ts",
        r#"
import { NextResponse } from 'next/server';
import { getProducts, createProduct } from '@/services/productService';

export async function GET(request: Request) {
    const products = await getProducts();
    return NextResponse.json(products);
}

export async function POST(request: Request) {
    const body = await request.json();
    const product = await createProduct(body);
    return NextResponse.json(product, { status: 201 });
}
"#,
    );

    rb.write_file(
        "src/app/api/products/[id]/route.ts",
        r#"
import { NextResponse } from 'next/server';
import { getProductById, updateProduct } from '@/services/productService';

export async function GET(request: Request, { params }: { params: { id: string } }) {
    const product = await getProductById(params.id);
    return NextResponse.json(product);
}

export async function PUT(request: Request, { params }: { params: { id: string } }) {
    const body = await request.json();
    const product = await updateProduct(params.id, body);
    return NextResponse.json(product);
}
"#,
    );

    rb.write_file(
        "src/services/productService.ts",
        r#"
import { prisma } from '@/lib/prisma';
import { Product, CreateProductInput } from '@/types/product';

export async function getProducts(): Promise<Product[]> {
    return prisma.product.findMany();
}

export async function getProductById(id: string): Promise<Product | null> {
    return prisma.product.findUnique({ where: { id } });
}

export async function createProduct(data: CreateProductInput): Promise<Product> {
    return prisma.product.create({ data });
}

export async function updateProduct(id: string, data: Partial<CreateProductInput>): Promise<Product> {
    return prisma.product.update({ where: { id }, data });
}
"#,
    );

    rb.write_file(
        "src/lib/prisma.ts",
        r#"
import { PrismaClient } from '@prisma/client';

export const prisma = new PrismaClient();
"#,
    );

    rb.write_file(
        "src/app/products/page.tsx",
        r#"
import { ProductList } from '@/components/ProductList';
import { fetchProducts } from '@/lib/api';

export default async function ProductsPage() {
    const products = await fetchProducts();
    return <ProductList products={products} />;
}
"#,
    );

    rb.write_file(
        "src/components/ProductList.tsx",
        r#"
import { Product } from '@/types/product';
import { ProductCard } from './ProductCard';

interface Props {
    products: Product[];
}

export function ProductList({ products }: Props) {
    return (
        <div>
            {products.map(p => <ProductCard key={p.id} product={p} />)}
        </div>
    );
}
"#,
    );

    rb.write_file(
        "src/components/ProductCard.tsx",
        r#"
import { Product } from '@/types/product';
import { formatPrice } from '@/utils/format';

interface Props {
    product: Product;
}

export function ProductCard({ product }: Props) {
    return (
        <div>
            <h3>{product.name}</h3>
            <p>{formatPrice(product.price)}</p>
        </div>
    );
}
"#,
    );

    rb.write_file(
        "src/lib/api.ts",
        r#"
export async function fetchProducts() {
    const res = await fetch('/api/products');
    return res.json();
}

export async function fetchProduct(id: string) {
    const res = await fetch(`/api/products/${id}`);
    return res.json();
}
"#,
    );

    rb.write_file(
        "src/types/product.ts",
        r#"
export interface Product {
    id: string;
    name: string;
    price: number;
    description: string;
}

export interface CreateProductInput {
    name: string;
    price: number;
    description: string;
}
"#,
    );

    rb.write_file(
        "src/utils/format.ts",
        r#"
export function formatPrice(price: number): string {
    return `$${price.toFixed(2)}`;
}
"#,
    );

    rb.write_file(
        "prisma/schema.prisma",
        r#"
generator client {
    provider = "prisma-client-js"
}

datasource db {
    provider = "postgresql"
    url      = env("DATABASE_URL")
}

model Product {
    id          String @id @default(cuid())
    name        String
    price       Float
    description String
    createdAt   DateTime @default(now())
}
"#,
    );

    rb.commit("Add product listing with API + UI");

    let baseline = EvalBaseline {
        name: "Next.js Fullstack".to_string(),
        expected_languages: vec!["typescript".to_string()],
        min_groups: 1,
        max_groups: 8,
        expected_file_count: 11,
        expected_entrypoints: vec![ExpectedEntrypoint {
            file_contains: "api/products/route".to_string(),
            ep_type: EntrypointType::HttpRoute,
        }],
        expected_groups: vec![
            ExpectedGroup {
                label: "Product API flow".to_string(),
                must_contain: vec![
                    "api/products/route".to_string(),
                    "services/productService".to_string(),
                ],
                must_not_contain: vec![],
            },
            ExpectedGroup {
                label: "Product UI flow".to_string(),
                must_contain: vec![
                    "components/ProductList".to_string(),
                    "components/ProductCard".to_string(),
                ],
                must_not_contain: vec![],
            },
        ],
        risk_ordering: vec![],
        expected_infrastructure: vec!["prisma/schema.prisma".to_string()],
    };

    (rb, baseline)
}

// ═══════════════════════════════════════════════════════════════════════════
// Fixture 4: Rust CLI with modules + lib
// ═══════════════════════════════════════════════════════════════════════════

fn build_fixture_rust_cli() -> (RepoBuilder, EvalBaseline) {
    let rb = RepoBuilder::new();

    rb.write_file(
        "Cargo.toml",
        r#"[package]
name = "mycli"
version = "0.1.0"
edition = "2021"

[dependencies]
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
"#,
    );
    rb.write_file("src/main.rs", "fn main() { println!(\"Hello\"); }\n");
    rb.commit("Initial Rust CLI");
    rb.create_branch("main");

    rb.create_branch("feature/analyze-command");
    rb.checkout("feature/analyze-command");

    rb.write_file(
        "src/main.rs",
        r#"
mod cli;
mod analyzer;
mod output;
mod config;

use cli::Args;

fn main() {
    let args = Args::parse();
    let config = config::load_config(&args.config_path);
    let result = analyzer::analyze(&args.input, &config);
    output::print_result(&result, args.format);
}
"#,
    );

    rb.write_file(
        "src/cli.rs",
        r#"
use clap::Parser;

#[derive(Parser)]
pub struct Args {
    pub input: String,
    #[arg(short, long, default_value = "json")]
    pub format: String,
    #[arg(short, long)]
    pub config_path: Option<String>,
}

impl Args {
    pub fn parse() -> Self {
        <Self as Parser>::parse()
    }
}
"#,
    );

    rb.write_file(
        "src/analyzer.rs",
        r#"
use crate::config::Config;

pub struct AnalysisResult {
    pub findings: Vec<Finding>,
    pub summary: String,
}

pub struct Finding {
    pub file: String,
    pub line: usize,
    pub message: String,
    pub severity: Severity,
}

pub enum Severity {
    Error,
    Warning,
    Info,
}

pub fn analyze(input: &str, config: &Config) -> AnalysisResult {
    let findings = scan_files(input, config);
    let summary = format!("Found {} findings", findings.len());
    AnalysisResult { findings, summary }
}

fn scan_files(path: &str, config: &Config) -> Vec<Finding> {
    vec![]
}
"#,
    );

    rb.write_file(
        "src/output.rs",
        r#"
use crate::analyzer::AnalysisResult;

pub fn print_result(result: &AnalysisResult, format: String) {
    match format.as_str() {
        "json" => print_json(result),
        "text" => print_text(result),
        _ => print_text(result),
    }
}

fn print_json(result: &AnalysisResult) {
    println!("{}", result.summary);
}

fn print_text(result: &AnalysisResult) {
    println!("{}", result.summary);
}
"#,
    );

    rb.write_file(
        "src/config.rs",
        r#"
use std::fs;

pub struct Config {
    pub rules: Vec<String>,
    pub ignore_patterns: Vec<String>,
}

pub fn load_config(path: &Option<String>) -> Config {
    match path {
        Some(p) => {
            let content = fs::read_to_string(p).unwrap_or_default();
            Config {
                rules: vec![],
                ignore_patterns: vec![],
            }
        }
        None => Config {
            rules: vec![],
            ignore_patterns: vec![],
        },
    }
}
"#,
    );

    rb.commit("Add analyze command with modules");

    // Note: tree-sitter Rust grammar is not included, so language detection won't find "rust".
    let baseline = EvalBaseline {
        name: "Rust CLI".to_string(),
        expected_languages: vec![], // No Rust grammar in tree-sitter deps
        min_groups: 0,
        max_groups: 5,
        expected_file_count: 5,
        expected_entrypoints: vec![], // No Rust entrypoint detection yet
        expected_groups: vec![],
        risk_ordering: vec![],
        expected_infrastructure: vec![],
    };

    (rb, baseline)
}

// ═══════════════════════════════════════════════════════════════════════════
// Fixture 5: Multi-language monorepo (TS frontend + Python backend)
// ═══════════════════════════════════════════════════════════════════════════

fn build_fixture_multi_language_monorepo() -> (RepoBuilder, EvalBaseline) {
    let rb = RepoBuilder::new();

    rb.write_file("README.md", "# Monorepo\n");
    rb.write_file(
        "packages/frontend/package.json",
        r#"{"name": "@app/frontend"}"#,
    );
    rb.write_file("packages/backend/requirements.txt", "fastapi\n");
    rb.commit("Initial monorepo");
    rb.create_branch("main");

    rb.create_branch("feature/user-profile");
    rb.checkout("feature/user-profile");

    // === Frontend (TypeScript/React) ===
    rb.write_file(
        "packages/frontend/src/pages/ProfilePage.tsx",
        r#"
import { UserProfile } from '../components/UserProfile';
import { useUser } from '../hooks/useUser';

export default function ProfilePage() {
    const { user, loading } = useUser();
    if (loading) return <div>Loading...</div>;
    return <UserProfile user={user} />;
}
"#,
    );

    rb.write_file(
        "packages/frontend/src/components/UserProfile.tsx",
        r#"
import { User } from '../types/user';
import { formatDate } from '../utils/format';

interface Props {
    user: User;
}

export function UserProfile({ user }: Props) {
    return (
        <div>
            <h1>{user.name}</h1>
            <p>Joined: {formatDate(user.createdAt)}</p>
        </div>
    );
}
"#,
    );

    rb.write_file(
        "packages/frontend/src/hooks/useUser.ts",
        r#"
import { useState, useEffect } from 'react';
import { fetchCurrentUser } from '../api/userApi';

export function useUser() {
    const [user, setUser] = useState(null);
    const [loading, setLoading] = useState(true);
    useEffect(() => {
        fetchCurrentUser().then(u => { setUser(u); setLoading(false); });
    }, []);
    return { user, loading };
}
"#,
    );

    rb.write_file(
        "packages/frontend/src/api/userApi.ts",
        r#"
export async function fetchCurrentUser() {
    const res = await fetch('/api/users/me');
    return res.json();
}

export async function updateProfile(data: any) {
    const res = await fetch('/api/users/me', { method: 'PUT', body: JSON.stringify(data) });
    return res.json();
}
"#,
    );

    rb.write_file(
        "packages/frontend/src/types/user.ts",
        r#"
export interface User {
    id: string;
    name: string;
    email: string;
    createdAt: string;
}
"#,
    );

    rb.write_file(
        "packages/frontend/src/utils/format.ts",
        r#"
export function formatDate(date: string): string {
    return new Date(date).toLocaleDateString();
}
"#,
    );

    // === Backend (Python/FastAPI) ===
    rb.write_file(
        "packages/backend/app/routes/users.py",
        r#"
from fastapi import APIRouter, Depends
from app.services.user_service import get_current_user_profile, update_user_profile
from app.auth.deps import get_authenticated_user

router = APIRouter()

@router.get("/api/users/me")
async def get_profile(user=Depends(get_authenticated_user)):
    return get_current_user_profile(user.id)

@router.put("/api/users/me")
async def update_profile(data: dict, user=Depends(get_authenticated_user)):
    return update_user_profile(user.id, data)
"#,
    );

    rb.write_file(
        "packages/backend/app/services/user_service.py",
        r#"
from app.repositories.user_repo import find_user, update_user

def get_current_user_profile(user_id):
    user = find_user(user_id)
    return {"id": user.id, "name": user.name, "email": user.email}

def update_user_profile(user_id, data):
    return update_user(user_id, data)
"#,
    );

    rb.write_file(
        "packages/backend/app/repositories/user_repo.py",
        r#"
def find_user(user_id):
    return type('User', (), {'id': user_id, 'name': 'Test', 'email': 'test@test.com'})()

def update_user(user_id, data):
    return {"id": user_id, **data}
"#,
    );

    rb.write_file(
        "packages/backend/app/auth/deps.py",
        r#"
def get_authenticated_user():
    return type('User', (), {'id': 1})()
"#,
    );

    rb.commit("Add user profile feature across frontend + backend");

    let baseline = EvalBaseline {
        name: "Multi-language Monorepo".to_string(),
        expected_languages: vec!["python".to_string(), "typescript".to_string()],
        min_groups: 1,
        max_groups: 10,
        expected_file_count: 10,
        expected_entrypoints: vec![ExpectedEntrypoint {
            file_contains: "backend/app/routes/users".to_string(),
            ep_type: EntrypointType::HttpRoute,
        }],
        expected_groups: vec![
            ExpectedGroup {
                label: "Frontend profile flow".to_string(),
                must_contain: vec!["ProfilePage".to_string(), "UserProfile".to_string()],
                must_not_contain: vec!["backend".to_string()],
            },
            ExpectedGroup {
                label: "Backend user API".to_string(),
                must_contain: vec![
                    "backend/app/routes/users".to_string(),
                    "backend/app/services/user_service".to_string(),
                ],
                must_not_contain: vec!["frontend".to_string()],
            },
        ],
        risk_ordering: vec![],
        expected_infrastructure: vec![],
    };

    (rb, baseline)
}

// ═══════════════════════════════════════════════════════════════════════════
// Eval Tests
// ═══════════════════════════════════════════════════════════════════════════

/// Minimum acceptable overall score for any fixture.
const MIN_OVERALL_SCORE: f64 = 0.50;

// --- Individual fixture eval tests ---

#[test]
fn test_eval_ts_express_api() {
    let (rb, baseline) = build_fixture_ts_express();
    let branch = find_feature_branch(rb.path());
    let output = run_pipeline(rb.path(), "main", &branch);

    let scores = score_output(&output, &baseline);
    print_eval_report(&baseline.name, &scores);

    assert!(
        scores.overall >= MIN_OVERALL_SCORE,
        "[{}] overall {:.2} < {:.2}",
        baseline.name,
        scores.overall,
        MIN_OVERALL_SCORE,
    );
    assert!(scores.language_detection >= 1.0, "Should detect TypeScript");
    assert!(scores.risk_reasonableness >= 0.5);
    assert!(scores.file_accounting >= 0.25);
}

#[test]
fn test_eval_python_fastapi() {
    let (rb, baseline) = build_fixture_python_fastapi();
    let branch = find_feature_branch(rb.path());
    let output = run_pipeline(rb.path(), "main", &branch);

    let scores = score_output(&output, &baseline);
    print_eval_report(&baseline.name, &scores);

    assert!(
        scores.overall >= MIN_OVERALL_SCORE,
        "[{}] overall {:.2} < {:.2}",
        baseline.name,
        scores.overall,
        MIN_OVERALL_SCORE,
    );
    assert!(scores.language_detection >= 1.0, "Should detect Python");
    assert!(scores.risk_reasonableness >= 0.5);
}

#[test]
fn test_eval_nextjs_fullstack() {
    let (rb, baseline) = build_fixture_nextjs_fullstack();
    let branch = find_feature_branch(rb.path());
    let output = run_pipeline(rb.path(), "main", &branch);

    let scores = score_output(&output, &baseline);
    print_eval_report(&baseline.name, &scores);

    assert!(
        scores.overall >= MIN_OVERALL_SCORE,
        "[{}] overall {:.2} < {:.2}",
        baseline.name,
        scores.overall,
        MIN_OVERALL_SCORE,
    );
    assert!(scores.language_detection >= 1.0, "Should detect TypeScript");
    assert!(scores.risk_reasonableness >= 0.5);
}

#[test]
fn test_eval_rust_cli() {
    let (rb, baseline) = build_fixture_rust_cli();
    let branch = find_feature_branch(rb.path());
    let output = run_pipeline(rb.path(), "main", &branch);

    let scores = score_output(&output, &baseline);
    print_eval_report(&baseline.name, &scores);

    // Rust has no tree-sitter grammar in deps, so scores are naturally lower
    assert!(
        scores.overall >= 0.30,
        "[{}] overall {:.2} < 0.30",
        baseline.name,
        scores.overall
    );
    assert!(scores.risk_reasonableness >= 0.5);
}

#[test]
fn test_eval_multi_language_monorepo() {
    let (rb, baseline) = build_fixture_multi_language_monorepo();
    let branch = find_feature_branch(rb.path());
    let output = run_pipeline(rb.path(), "main", &branch);

    let scores = score_output(&output, &baseline);
    print_eval_report(&baseline.name, &scores);

    assert!(
        scores.overall >= MIN_OVERALL_SCORE,
        "[{}] overall {:.2} < {:.2}",
        baseline.name,
        scores.overall,
        MIN_OVERALL_SCORE,
    );
    // Must detect both languages
    assert!(
        scores.language_detection >= 1.0,
        "Should detect both TS and Python"
    );
    assert!(scores.risk_reasonableness >= 0.5);
}

// --- Cross-fixture consistency tests ---

/// All fixtures should produce deterministic results.
#[test]
fn test_eval_all_fixtures_deterministic() {
    let fixtures: Vec<fn() -> (RepoBuilder, EvalBaseline)> = vec![
        build_fixture_ts_express,
        build_fixture_python_fastapi,
        build_fixture_nextjs_fullstack,
    ];

    for fixture_fn in fixtures {
        let (rb, baseline) = fixture_fn();
        let branch = find_feature_branch(rb.path());

        let output1 = run_pipeline(rb.path(), "main", &branch);
        let output2 = run_pipeline(rb.path(), "main", &branch);

        let json1 = output::to_json(&output1).unwrap();
        let json2 = output::to_json(&output2).unwrap();

        assert_eq!(
            json1, json2,
            "[{}] pipeline output not deterministic",
            baseline.name,
        );
    }
}

/// All fixtures should produce valid JSON that roundtrips cleanly.
#[test]
fn test_eval_all_fixtures_json_roundtrip() {
    let fixtures: Vec<fn() -> (RepoBuilder, EvalBaseline)> = vec![
        build_fixture_ts_express,
        build_fixture_python_fastapi,
        build_fixture_nextjs_fullstack,
        build_fixture_rust_cli,
        build_fixture_multi_language_monorepo,
    ];

    for fixture_fn in fixtures {
        let (rb, baseline) = fixture_fn();
        let branch = find_feature_branch(rb.path());
        let output = run_pipeline(rb.path(), "main", &branch);

        let json = output::to_json(&output).unwrap();
        let parsed: AnalysisOutput = serde_json::from_str(&json).unwrap();
        let json2 = output::to_json(&parsed).unwrap();

        assert_eq!(json, json2, "[{}] JSON roundtrip not stable", baseline.name,);
    }
}

/// Every fixture's output must account for all files (no files lost).
#[test]
fn test_eval_all_fixtures_file_accounting() {
    let fixtures: Vec<fn() -> (RepoBuilder, EvalBaseline)> = vec![
        build_fixture_ts_express,
        build_fixture_python_fastapi,
        build_fixture_nextjs_fullstack,
        build_fixture_rust_cli,
        build_fixture_multi_language_monorepo,
    ];

    for fixture_fn in fixtures {
        let (rb, baseline) = fixture_fn();
        let branch = find_feature_branch(rb.path());
        let output = run_pipeline(rb.path(), "main", &branch);

        let total_grouped: usize = output.groups.iter().map(|g| g.files.len()).sum();
        let infra: usize = output
            .infrastructure_group
            .as_ref()
            .map(|i| i.files.len())
            .unwrap_or(0);

        assert_eq!(
            total_grouped + infra,
            output.summary.total_files_changed as usize,
            "[{}] file accounting: grouped({}) + infra({}) != total({})",
            baseline.name,
            total_grouped,
            infra,
            output.summary.total_files_changed,
        );
    }
}

/// All risk scores must be in [0.0, 1.0].
#[test]
fn test_eval_all_fixtures_risk_bounds() {
    let fixtures: Vec<fn() -> (RepoBuilder, EvalBaseline)> = vec![
        build_fixture_ts_express,
        build_fixture_python_fastapi,
        build_fixture_nextjs_fullstack,
        build_fixture_rust_cli,
        build_fixture_multi_language_monorepo,
    ];

    for fixture_fn in fixtures {
        let (rb, baseline) = fixture_fn();
        let branch = find_feature_branch(rb.path());
        let output = run_pipeline(rb.path(), "main", &branch);

        for group in &output.groups {
            assert!(
                group.risk_score >= 0.0 && group.risk_score <= 1.0,
                "[{}] group '{}' risk_score {} out of bounds",
                baseline.name,
                group.name,
                group.risk_score,
            );
            assert!(
                group.review_order >= 1,
                "[{}] group '{}' review_order {} < 1",
                baseline.name,
                group.name,
                group.review_order,
            );
        }
    }
}

/// Mermaid diagrams should be generated for all groups across all fixtures.
#[test]
fn test_eval_all_fixtures_mermaid() {
    let fixtures: Vec<fn() -> (RepoBuilder, EvalBaseline)> = vec![
        build_fixture_ts_express,
        build_fixture_python_fastapi,
        build_fixture_nextjs_fullstack,
        build_fixture_multi_language_monorepo,
    ];

    for fixture_fn in fixtures {
        let (rb, baseline) = fixture_fn();
        let branch = find_feature_branch(rb.path());
        let output = run_pipeline(rb.path(), "main", &branch);

        for group in &output.groups {
            let mermaid = output::generate_mermaid(group);
            assert!(
                mermaid.starts_with("graph TD"),
                "[{}] group '{}' Mermaid should start with 'graph TD', got: {}",
                baseline.name,
                group.name,
                &mermaid[..mermaid.len().min(50)],
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Property-Based Tests for Scoring Functions
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod scoring_properties {
    use super::*;
    use proptest::prelude::*;

    /// Generate a random AnalysisOutput for property testing.
    fn arb_analysis_output() -> impl Strategy<Value = AnalysisOutput> {
        let arb_file_change = (
            "[a-z]{1,5}/[a-z]{1,5}\\.[a-z]{2,3}",
            0u32..10,
            0u32..100,
            0u32..100,
        )
            .prop_map(|(path, pos, adds, dels)| FileChange {
                path,
                flow_position: pos,
                role: FileRole::Utility,
                changes: ChangeStats {
                    additions: adds,
                    deletions: dels,
                },
                symbols_changed: vec![],
            });

        let arb_group = (
            prop::collection::vec(arb_file_change.clone(), 1..6),
            prop::num::f64::POSITIVE | prop::num::f64::ZERO,
            1u32..20,
        )
            .prop_map(|(files, risk_raw, order)| FlowGroup {
                id: format!("group_{}", order),
                name: format!("Group {}", order),
                entrypoint: None,
                files,
                edges: vec![],
                risk_score: risk_raw.min(1.0),
                review_order: order,
            });

        (prop::collection::vec(arb_group, 0..5), 0u32..50).prop_map(|(groups, extra_files)| {
            let total_in_groups: u32 = groups.iter().map(|g| g.files.len() as u32).sum();
            let total = total_in_groups + extra_files;
            AnalysisOutput {
                version: "1.0.0".to_string(),
                diff_source: DiffSource {
                    diff_type: DiffType::BranchComparison,
                    base: Some("main".to_string()),
                    head: Some("feature".to_string()),
                    base_sha: None,
                    head_sha: None,
                },
                summary: AnalysisSummary {
                    total_files_changed: total,
                    total_groups: groups.len() as u32,
                    languages_detected: vec!["typescript".to_string()],
                    frameworks_detected: vec![],
                },
                groups,
                infrastructure_group: if extra_files > 0 {
                    Some(InfrastructureGroup {
                        files: (0..extra_files)
                            .map(|i| format!("infra_{}.ts", i))
                            .collect(),
                        reason: "Not reachable".to_string(),
                    })
                } else {
                    None
                },
                annotations: None,
            }
        })
    }

    fn arb_baseline() -> impl Strategy<Value = EvalBaseline> {
        Just(EvalBaseline {
            name: "test".to_string(),
            expected_languages: vec!["typescript".to_string()],
            min_groups: 0,
            max_groups: 10,
            expected_file_count: 5,
            expected_entrypoints: vec![],
            expected_groups: vec![],
            risk_ordering: vec![],
            expected_infrastructure: vec![],
        })
    }

    proptest! {
        /// All scoring functions must return values in [0.0, 1.0].
        #[test]
        fn score_bounds(output in arb_analysis_output(), baseline in arb_baseline()) {
            let scores = score_output(&output, &baseline);
            prop_assert!(scores.group_coherence >= 0.0 && scores.group_coherence <= 1.0,
                "group_coherence out of bounds: {}", scores.group_coherence);
            prop_assert!(scores.entrypoint_accuracy >= 0.0 && scores.entrypoint_accuracy <= 1.0,
                "entrypoint_accuracy out of bounds: {}", scores.entrypoint_accuracy);
            prop_assert!(scores.review_ordering >= 0.0 && scores.review_ordering <= 1.0,
                "review_ordering out of bounds: {}", scores.review_ordering);
            prop_assert!(scores.risk_reasonableness >= 0.0 && scores.risk_reasonableness <= 1.0,
                "risk_reasonableness out of bounds: {}", scores.risk_reasonableness);
            prop_assert!(scores.language_detection >= 0.0 && scores.language_detection <= 1.0,
                "language_detection out of bounds: {}", scores.language_detection);
            prop_assert!(scores.file_accounting >= 0.0 && scores.file_accounting <= 1.0,
                "file_accounting out of bounds: {}", scores.file_accounting);
            prop_assert!(scores.overall >= 0.0 && scores.overall <= 1.0,
                "overall out of bounds: {}", scores.overall);
        }

        /// Overall score is a weighted average, so it should be between min and max individual scores.
        #[test]
        fn overall_between_min_max(output in arb_analysis_output(), baseline in arb_baseline()) {
            let scores = score_output(&output, &baseline);
            let all = vec![
                scores.group_coherence,
                scores.entrypoint_accuracy,
                scores.review_ordering,
                scores.risk_reasonableness,
                scores.language_detection,
                scores.file_accounting,
            ];
            let min_score = all.iter().cloned().fold(f64::INFINITY, f64::min);
            let max_score = all.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            prop_assert!(scores.overall >= min_score - 0.01,
                "overall {} < min individual {}", scores.overall, min_score);
            prop_assert!(scores.overall <= max_score + 0.01,
                "overall {} > max individual {}", scores.overall, max_score);
        }

        /// Empty output (no groups, no files) should not cause panics.
        #[test]
        fn empty_output_no_panic(baseline in arb_baseline()) {
            let output = AnalysisOutput {
                version: "1.0.0".to_string(),
                diff_source: DiffSource {
                    diff_type: DiffType::BranchComparison,
                    base: Some("main".to_string()),
                    head: Some("feature".to_string()),
                    base_sha: None,
                    head_sha: None,
                },
                summary: AnalysisSummary {
                    total_files_changed: 0,
                    total_groups: 0,
                    languages_detected: vec![],
                    frameworks_detected: vec![],
                },
                groups: vec![],
                infrastructure_group: None,
                annotations: None,
            };
            let scores = score_output(&output, &baseline);
            // Should not panic and should produce valid scores
            prop_assert!(scores.overall >= 0.0 && scores.overall <= 1.0);
        }

        /// Scoring the same output twice must give the same result (determinism).
        #[test]
        fn scoring_deterministic(output in arb_analysis_output(), baseline in arb_baseline()) {
            let scores1 = score_output(&output, &baseline);
            let scores2 = score_output(&output, &baseline);
            prop_assert!((scores1.overall - scores2.overall).abs() < f64::EPSILON,
                "scoring not deterministic: {} vs {}", scores1.overall, scores2.overall);
        }

        /// Perfect baseline match should score >= 0.8.
        #[test]
        fn perfect_match_high_score(n_groups in 1usize..4) {
            let groups: Vec<FlowGroup> = (0..n_groups).map(|i| {
                FlowGroup {
                    id: format!("group_{}", i),
                    name: format!("Group {}", i),
                    entrypoint: None,
                    files: vec![FileChange {
                        path: format!("file_{}.ts", i),
                        flow_position: 0,
                        role: FileRole::Utility,
                        changes: ChangeStats { additions: 10, deletions: 5 },
                        symbols_changed: vec![],
                    }],
                    edges: vec![],
                    risk_score: 0.5,
                    review_order: (i + 1) as u32,
                }
            }).collect();

            let output = AnalysisOutput {
                version: "1.0.0".to_string(),
                diff_source: DiffSource {
                    diff_type: DiffType::BranchComparison,
                    base: Some("main".to_string()),
                    head: Some("feature".to_string()),
                    base_sha: None,
                    head_sha: None,
                },
                summary: AnalysisSummary {
                    total_files_changed: n_groups as u32,
                    total_groups: n_groups as u32,
                    languages_detected: vec!["typescript".to_string()],
                    frameworks_detected: vec![],
                },
                groups,
                infrastructure_group: None,
                annotations: None,
            };

            let baseline = EvalBaseline {
                name: "perfect".to_string(),
                expected_languages: vec!["typescript".to_string()],
                min_groups: 1,
                max_groups: n_groups + 2,
                expected_file_count: n_groups,
                expected_entrypoints: vec![],
                expected_groups: vec![],
                risk_ordering: vec![],
                expected_infrastructure: vec![],
            };

            let scores = score_output(&output, &baseline);
            prop_assert!(scores.overall >= 0.8,
                "perfect match scored only {:.2}", scores.overall);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Aggregate Eval Report
// ═══════════════════════════════════════════════════════════════════════════

/// Run all fixture evals and print an aggregate report.
/// This test always passes — it's for observing score trends.
#[test]
fn test_eval_aggregate_report() {
    let fixtures: Vec<(fn() -> (RepoBuilder, EvalBaseline), &str)> = vec![
        (build_fixture_ts_express, "TS Express API"),
        (build_fixture_python_fastapi, "Python FastAPI"),
        (build_fixture_nextjs_fullstack, "Next.js Fullstack"),
        (build_fixture_rust_cli, "Rust CLI"),
        (
            build_fixture_multi_language_monorepo,
            "Multi-lang Monorepo",
        ),
    ];

    let mut all_scores: Vec<(String, EvalScores)> = Vec::new();

    for (fixture_fn, label) in &fixtures {
        let (rb, baseline) = fixture_fn();
        let branch = find_feature_branch(rb.path());
        let output = run_pipeline(rb.path(), "main", &branch);
        let scores = score_output(&output, &baseline);
        all_scores.push((label.to_string(), scores));
    }

    // Print aggregate report
    eprintln!("\n╔═══════════════════════════════════════════════════════════════════╗");
    eprintln!("║                    EVAL SUITE AGGREGATE REPORT                   ║");
    eprintln!("╠═══════════════════════════════════════════════════════════════════╣");
    eprintln!(
        "║ {:.<25} {:>6} {:>6} {:>6} {:>6} {:>6} {:>7} ║",
        "Fixture", "GrpCo", "EntPt", "Order", "Risk", "Lang", "TOTAL"
    );
    eprintln!("╠═══════════════════════════════════════════════════════════════════╣");

    let mut total_overall = 0.0;
    for (name, scores) in &all_scores {
        eprintln!(
            "║ {:.<25} {:>5.2} {:>6.2} {:>6.2} {:>6.2} {:>6.2} {:>7.2} ║",
            name,
            scores.group_coherence,
            scores.entrypoint_accuracy,
            scores.review_ordering,
            scores.risk_reasonableness,
            scores.language_detection,
            scores.overall,
        );
        total_overall += scores.overall;
    }

    let avg_overall = total_overall / all_scores.len() as f64;
    eprintln!("╠═══════════════════════════════════════════════════════════════════╣");
    eprintln!("║ {:.<25} {:>39.2} ║", "AVERAGE", avg_overall);
    eprintln!("╚═══════════════════════════════════════════════════════════════════╝");
}
