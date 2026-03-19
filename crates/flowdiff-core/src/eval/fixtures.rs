//! Synthetic fixture codebases and pipeline runner for eval suite.
//!
//! Provides `RepoBuilder` for creating temporary git repos with known file structures,
//! `run_pipeline` for executing the full flowdiff analysis pipeline, and 5 fixture
//! builders that each produce a (RepoBuilder, EvalBaseline) pair.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

use git2::{Repository, Signature};
use tempfile::TempDir;

use crate::ast;
use crate::cluster;
use crate::entrypoint;
use crate::flow::{self, FlowConfig};
use crate::graph::SymbolGraph;
use crate::output::{self, build_analysis_output};
use crate::rank::{self, compute_risk_score, compute_surface_area};
use crate::types::{AnalysisOutput, EntrypointType, GroupRankInput, RankWeights};

use super::scoring::{
    EvalBaseline, ExpectedEntrypoint, ExpectedGroup, RiskOrderingConstraint,
};

// ═══════════════════════════════════════════════════════════════════════════
// Fixture Names
// ═══════════════════════════════════════════════════════════════════════════

/// Short names for each fixture, used by `--fixture` CLI flag.
pub const FIXTURE_NAMES: &[&str] = &[
    "ts-express",
    "python-fastapi",
    "nextjs-fullstack",
    "rust-cli",
    "multi-language",
];

/// Get display name for a fixture.
pub fn fixture_display_name(name: &str) -> &str {
    match name {
        "ts-express" => "TS Express API",
        "python-fastapi" => "Python FastAPI",
        "nextjs-fullstack" => "Next.js Fullstack",
        "rust-cli" => "Rust CLI",
        "multi-language" => "Multi-lang Monorepo",
        _ => name,
    }
}

/// Build a specific fixture by short name.
///
/// Returns `None` if the fixture name is not recognized.
pub fn build_fixture(name: &str) -> Option<(RepoBuilder, EvalBaseline)> {
    match name {
        "ts-express" => Some(build_fixture_ts_express()),
        "python-fastapi" => Some(build_fixture_python_fastapi()),
        "nextjs-fullstack" => Some(build_fixture_nextjs_fullstack()),
        "rust-cli" => Some(build_fixture_rust_cli()),
        "multi-language" => Some(build_fixture_multi_language_monorepo()),
        _ => None,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// RepoBuilder
// ═══════════════════════════════════════════════════════════════════════════

/// Create a git repo, commit initial files, apply changes on a branch, and return the repo + dir.
pub struct RepoBuilder {
    dir: TempDir,
    repo: Repository,
}

impl RepoBuilder {
    pub fn new() -> Self {
        let dir = TempDir::new().expect("failed to create temp dir");
        let repo = Repository::init(dir.path()).expect("failed to init repo");

        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test User").unwrap();
        config.set_str("user.email", "test@example.com").unwrap();

        Self { dir, repo }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Write a file relative to the repo root.
    pub fn write_file(&self, rel_path: &str, content: &str) {
        let full = self.dir.path().join(rel_path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full, content).unwrap();
    }

    /// Stage all changes and commit with a message. Returns the commit OID.
    pub fn commit(&self, message: &str) -> git2::Oid {
        let mut index = self.repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();

        let tree_oid = index.write_tree().unwrap();
        let tree = self.repo.find_tree(tree_oid).unwrap();
        let sig = Signature::now("Test User", "test@example.com").unwrap();

        let parent = self
            .repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();

        self.repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
            .unwrap()
    }

    /// Create a branch at the current HEAD. No-op if it already exists.
    pub fn create_branch(&self, name: &str) {
        let head = self.repo.head().unwrap().peel_to_commit().unwrap();
        let _ = self.repo.branch(name, &head, false);
    }

    /// Checkout a branch by name.
    pub fn checkout(&self, name: &str) {
        let ref_name = format!("refs/heads/{}", name);
        let obj = self.repo.revparse_single(&ref_name).unwrap();
        self.repo.checkout_tree(&obj, None).unwrap();
        self.repo.set_head(&ref_name).unwrap();
    }

    /// Get a reference to the underlying git2 Repository.
    pub fn repo(&self) -> &Repository {
        &self.repo
    }

    /// Get a reference to the underlying TempDir (for keeping it alive).
    pub fn temp_dir(&self) -> &TempDir {
        &self.dir
    }
}

impl Default for RepoBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Pipeline Runner
// ═══════════════════════════════════════════════════════════════════════════

/// Run the full pipeline on a repo diff between two refs.
pub fn run_pipeline(repo_path: &Path, base_ref: &str, head_ref: &str) -> AnalysisOutput {
    let repo = Repository::open(repo_path).expect("failed to open repo");
    let diff_result = crate::git::diff_refs(&repo, base_ref, head_ref).expect("diff_refs failed");

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

    let changed_files: Vec<String> = diff_result
        .files
        .iter()
        .map(|f| f.path().to_string())
        .collect();
    let cluster_result = cluster::cluster_files(&graph, &entrypoints, &changed_files);

    let weights = RankWeights::default();
    let rank_inputs: Vec<GroupRankInput> = cluster_result
        .groups
        .iter()
        .map(|group| {
            let risk_flags = output::compute_group_risk_flags(
                &group
                    .files
                    .iter()
                    .map(|f| f.path.as_str())
                    .collect::<Vec<_>>(),
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
                uncertainty: if risk_flags.has_test_only {
                    0.1
                } else {
                    0.5
                },
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

/// Find the feature branch name in a repo (first non-main branch).
pub fn find_feature_branch(repo_path: &Path) -> String {
    let repo = Repository::open(repo_path).unwrap();
    let branches = repo
        .branches(Some(git2::BranchType::Local))
        .unwrap();
    for branch in branches {
        let (branch, _) = branch.unwrap();
        let name = branch.name().unwrap().unwrap().to_string();
        if name != "main" {
            return name;
        }
    }
    panic!("No feature branch found");
}

// ═══════════════════════════════════════════════════════════════════════════
// Fixture 1: TypeScript Express API with services + DB layer
// ═══════════════════════════════════════════════════════════════════════════

pub fn build_fixture_ts_express() -> (RepoBuilder, EvalBaseline) {
    let rb = RepoBuilder::new();

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

    rb.create_branch("feature/user-crud");
    rb.checkout("feature/user-crud");

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

    rb.write_file(
        "src/utils/crypto.ts",
        r#"
export function hashPassword(password: string): string {
    return 'hashed_' + password;
}
"#,
    );

    rb.write_file(
        "src/events/userEvents.ts",
        r#"
export function emitUserCreated(user: any) {
    console.log('User created:', user.id);
}
"#,
    );

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

pub fn build_fixture_python_fastapi() -> (RepoBuilder, EvalBaseline) {
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

pub fn build_fixture_nextjs_fullstack() -> (RepoBuilder, EvalBaseline) {
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

pub fn build_fixture_rust_cli() -> (RepoBuilder, EvalBaseline) {
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

pub fn build_fixture_multi_language_monorepo() -> (RepoBuilder, EvalBaseline) {
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_fixture_names_count() {
        assert_eq!(FIXTURE_NAMES.len(), 5);
    }

    #[test]
    fn test_all_fixtures_buildable() {
        for name in FIXTURE_NAMES {
            let result = build_fixture(name);
            assert!(result.is_some(), "fixture '{}' should be buildable", name);
        }
    }

    #[test]
    fn test_unknown_fixture_returns_none() {
        assert!(build_fixture("nonexistent").is_none());
    }

    #[test]
    fn test_fixture_display_names() {
        assert_eq!(fixture_display_name("ts-express"), "TS Express API");
        assert_eq!(fixture_display_name("python-fastapi"), "Python FastAPI");
        assert_eq!(fixture_display_name("nextjs-fullstack"), "Next.js Fullstack");
        assert_eq!(fixture_display_name("rust-cli"), "Rust CLI");
        assert_eq!(fixture_display_name("multi-language"), "Multi-lang Monorepo");
        assert_eq!(fixture_display_name("unknown"), "unknown");
    }

    #[test]
    fn test_repo_builder_creates_repo() {
        let rb = RepoBuilder::new();
        assert!(rb.path().exists());
        assert!(rb.path().join(".git").exists());
    }

    #[test]
    fn test_repo_builder_write_and_commit() {
        let rb = RepoBuilder::new();
        rb.write_file("test.txt", "hello");
        let oid = rb.commit("test commit");
        assert!(!oid.is_zero());
    }

    #[test]
    fn test_run_pipeline_ts_express() {
        let (rb, baseline) = build_fixture_ts_express();
        let branch = find_feature_branch(rb.path());
        let output = run_pipeline(rb.path(), "main", &branch);
        assert_eq!(output.summary.total_files_changed as usize, baseline.expected_file_count);
    }

    #[test]
    fn test_find_feature_branch() {
        let rb = RepoBuilder::new();
        rb.write_file("a.txt", "a");
        rb.commit("init");
        rb.create_branch("main");
        rb.create_branch("feature/test");
        rb.checkout("feature/test");
        rb.write_file("b.txt", "b");
        rb.commit("feature commit");

        let branch = find_feature_branch(rb.path());
        assert_eq!(branch, "feature/test");
    }
}
