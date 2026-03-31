//! Benchmark: per-file parse deduplication (Phase 12.1)
//!
//! Compares two approaches:
//! - **double parse** (old): `parse_file()` + `extract_data_flow()` — each calls `parse_tree()` internally
//! - **single parse** (new): `parse_tree_for_path()` once, then `parse_file_with_tree()` + `extract_data_flow_with_tree()`
//!
//! Expected result: single-parse is ~2x faster since tree-sitter parsing dominates.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use diffcore_core::ast::{CallSite, ImportInfo, Language, ParsedFile};
use diffcore_core::flow::{analyze_data_flow, FlowConfig};
use diffcore_core::graph::SymbolGraph;
use diffcore_core::ir::{
    FunctionKind, IrCallExpression, IrConstant, IrExport, IrFile, IrFunctionDef, IrImport,
    IrImportSpecifier, Span,
};
use diffcore_core::query_engine::QueryEngine;

/// A realistic TypeScript source file with imports, classes, functions, and calls.
const TS_SOURCE: &str = r#"
import { Request, Response, NextFunction } from 'express';
import { UserService } from '../services/user.service';
import { Logger } from '../utils/logger';
import { validateInput } from '../middleware/validation';
import { DatabaseConnection } from '../db/connection';

interface UserDTO {
    id: string;
    name: string;
    email: string;
    createdAt: Date;
}

interface CreateUserRequest {
    name: string;
    email: string;
    password: string;
}

class UserController {
    private userService: UserService;
    private logger: Logger;
    private db: DatabaseConnection;

    constructor(userService: UserService, logger: Logger, db: DatabaseConnection) {
        this.userService = userService;
        this.logger = logger;
        this.db = db;
    }

    async getUser(req: Request, res: Response, next: NextFunction): Promise<void> {
        try {
            const userId = req.params.id;
            this.logger.info('Fetching user', { userId });
            const user = await this.userService.findById(userId);
            if (!user) {
                res.status(404).json({ error: 'User not found' });
                return;
            }
            const dto: UserDTO = {
                id: user.id,
                name: user.name,
                email: user.email,
                createdAt: user.createdAt,
            };
            res.json(dto);
        } catch (error) {
            this.logger.error('Failed to fetch user', { error });
            next(error);
        }
    }

    async createUser(req: Request, res: Response, next: NextFunction): Promise<void> {
        try {
            const input: CreateUserRequest = req.body;
            validateInput(input);
            this.logger.info('Creating user', { email: input.email });
            const connection = await this.db.getConnection();
            const user = await this.userService.create(input, connection);
            const result = await this.userService.sendWelcomeEmail(user.email);
            this.logger.info('User created', { userId: user.id, emailSent: result.success });
            res.status(201).json(user);
        } catch (error) {
            this.logger.error('Failed to create user', { error });
            next(error);
        }
    }

    async listUsers(req: Request, res: Response): Promise<void> {
        const page = parseInt(req.query.page as string) || 1;
        const limit = parseInt(req.query.limit as string) || 20;
        const users = await this.userService.findAll({ page, limit });
        const total = await this.userService.count();
        res.json({ users, total, page, limit });
    }

    async deleteUser(req: Request, res: Response, next: NextFunction): Promise<void> {
        try {
            const userId = req.params.id;
            this.logger.warn('Deleting user', { userId });
            await this.userService.delete(userId);
            res.status(204).send();
        } catch (error) {
            this.logger.error('Failed to delete user', { error });
            next(error);
        }
    }
}

export { UserController };
export type { UserDTO, CreateUserRequest };
"#;

/// A realistic Python source file.
const PY_SOURCE: &str = r#"
from fastapi import APIRouter, Depends, HTTPException, status
from sqlalchemy.orm import Session
from typing import List, Optional
from pydantic import BaseModel

from app.database import get_db
from app.models.user import User
from app.services.auth import get_current_user
from app.services.email import send_notification

class UserCreate(BaseModel):
    username: str
    email: str
    full_name: Optional[str] = None

class UserResponse(BaseModel):
    id: int
    username: str
    email: str
    full_name: Optional[str]
    is_active: bool

    class Config:
        from_attributes = True

router = APIRouter(prefix="/users", tags=["users"])

@router.get("/", response_model=List[UserResponse])
async def list_users(
    skip: int = 0,
    limit: int = 100,
    db: Session = Depends(get_db),
    current_user: User = Depends(get_current_user),
):
    users = db.query(User).offset(skip).limit(limit).all()
    return users

@router.post("/", response_model=UserResponse, status_code=status.HTTP_201_CREATED)
async def create_user(
    user_data: UserCreate,
    db: Session = Depends(get_db),
    current_user: User = Depends(get_current_user),
):
    existing = db.query(User).filter(User.email == user_data.email).first()
    if existing:
        raise HTTPException(status_code=400, detail="Email already registered")

    new_user = User(**user_data.dict())
    db.add(new_user)
    db.commit()
    db.refresh(new_user)

    send_notification(new_user.email, "Welcome!")
    return new_user

@router.get("/{user_id}", response_model=UserResponse)
async def get_user(
    user_id: int,
    db: Session = Depends(get_db),
):
    user = db.query(User).filter(User.id == user_id).first()
    if not user:
        raise HTTPException(status_code=404, detail="User not found")
    return user

@router.delete("/{user_id}", status_code=status.HTTP_204_NO_CONTENT)
async def delete_user(
    user_id: int,
    db: Session = Depends(get_db),
    current_user: User = Depends(get_current_user),
):
    user = db.query(User).filter(User.id == user_id).first()
    if not user:
        raise HTTPException(status_code=404, detail="User not found")
    db.delete(user)
    db.commit()
"#;

fn bench_lazy_query_engine(c: &mut Criterion) {
    let mut group = c.benchmark_group("query_engine_init");

    // Benchmark: constructing QueryEngine (should be near-instant with lazy init)
    group.bench_function("new_construction", |b| {
        b.iter(|| {
            black_box(QueryEngine::new().expect("failed to create QueryEngine"));
        });
    });

    // Benchmark: first parse triggers lazy compilation for one language
    group.bench_function("first_parse_typescript", |b| {
        b.iter(|| {
            let engine = QueryEngine::new().expect("failed to create QueryEngine");
            let _ = black_box(engine.parse_file("src/controller.ts", TS_SOURCE));
        });
    });

    // Benchmark: first parse Python (different language, separate compilation)
    group.bench_function("first_parse_python", |b| {
        b.iter(|| {
            let engine = QueryEngine::new().expect("failed to create QueryEngine");
            let _ = black_box(engine.parse_file("app/users.py", PY_SOURCE));
        });
    });

    // Benchmark: second parse reuses cached queries (no recompilation)
    group.bench_function("second_parse_typescript", |b| {
        let engine = QueryEngine::new().expect("failed to create QueryEngine");
        // Warm up: trigger lazy compilation
        let _ = engine.parse_file("src/controller.ts", TS_SOURCE);
        b.iter(|| {
            let _ = black_box(engine.parse_file("src/controller.ts", TS_SOURCE));
        });
    });

    group.finish();
}

fn bench_parse_dedup(c: &mut Criterion) {
    let engine = QueryEngine::new().expect("failed to create QueryEngine");

    let mut group = c.benchmark_group("parse_dedup_typescript");

    // Old approach: two separate parse_tree calls
    group.bench_function("double_parse", |b| {
        b.iter(|| {
            let _ = black_box(engine.parse_file("src/controller.ts", TS_SOURCE));
            let _ = black_box(engine.extract_data_flow("src/controller.ts", TS_SOURCE));
        });
    });

    // New approach: one parse_tree call, reuse tree
    group.bench_function("single_parse", |b| {
        b.iter(|| {
            let (tree, language) = engine
                .parse_tree_for_path("src/controller.ts", TS_SOURCE)
                .unwrap()
                .unwrap();
            let _ = black_box(engine.parse_file_with_tree(
                "src/controller.ts",
                TS_SOURCE,
                &tree,
                language,
            ));
            let _ = black_box(engine.extract_data_flow_with_tree(
                "src/controller.ts",
                TS_SOURCE,
                &tree,
                language,
            ));
        });
    });

    group.finish();

    let mut group = c.benchmark_group("parse_dedup_python");

    group.bench_function("double_parse", |b| {
        b.iter(|| {
            let _ = black_box(engine.parse_file("app/users.py", PY_SOURCE));
            let _ = black_box(engine.extract_data_flow("app/users.py", PY_SOURCE));
        });
    });

    group.bench_function("single_parse", |b| {
        b.iter(|| {
            let (tree, language) = engine
                .parse_tree_for_path("app/users.py", PY_SOURCE)
                .unwrap()
                .unwrap();
            let _ =
                black_box(engine.parse_file_with_tree("app/users.py", PY_SOURCE, &tree, language));
            let _ = black_box(engine.extract_data_flow_with_tree(
                "app/users.py",
                PY_SOURCE,
                &tree,
                language,
            ));
        });
    });

    group.finish();

    // Also benchmark parse_to_ir (the unified pipeline function) vs manual double-parse
    let mut group = c.benchmark_group("parse_to_ir_vs_double");

    group.bench_function(BenchmarkId::new("double_parse", "ts"), |b| {
        b.iter(|| {
            let _ = black_box(engine.parse_file("src/controller.ts", TS_SOURCE));
            let _ = black_box(engine.extract_data_flow("src/controller.ts", TS_SOURCE));
        });
    });

    group.bench_function(BenchmarkId::new("parse_to_ir", "ts"), |b| {
        b.iter(|| {
            let _ = black_box(diffcore_core::pipeline::parse_to_ir(
                &engine,
                "src/controller.ts",
                TS_SOURCE,
                None,
            ));
        });
    });

    group.finish();
}

/// Generate `n` synthetic IrFiles with cross-file imports and calls.
///
/// Each file has 3-5 functions, 1-2 type defs, 1-2 constants, imports from
/// 2-3 other files, and call expressions referencing other files' functions.
fn generate_ir_files(n: usize) -> Vec<IrFile> {
    (0..n)
        .map(|i| {
            let path = format!("src/module_{}.ts", i);
            let functions: Vec<IrFunctionDef> = (0..4)
                .map(|f| IrFunctionDef {
                    name: format!("func_{}_{}", i, f),
                    kind: FunctionKind::Function,
                    span: Span::new(f * 10 + 1, f * 10 + 8),
                    parameters: vec![],
                    is_async: f % 2 == 0,
                    is_exported: f < 2,
                    decorators: vec![],
                })
                .collect();

            let type_defs = vec![diffcore_core::ir::IrTypeDef {
                name: format!("Type_{}", i),
                kind: diffcore_core::ir::TypeDefKind::Interface,
                span: Span::new(50, 60),
                bases: if i > 0 {
                    vec![format!("Type_{}", i - 1)]
                } else {
                    vec![]
                },
                is_exported: true,
                decorators: vec![],
            }];

            let constants = vec![IrConstant {
                name: format!("CONST_{}", i),
                span: Span::single(65),
                is_exported: true,
            }];

            // Import from 2-3 neighboring files
            let imports: Vec<IrImport> = (1..=2)
                .filter_map(|offset| {
                    let target = (i + offset) % n;
                    if target == i {
                        return None;
                    }
                    Some(IrImport {
                        source: format!("./module_{}", target),
                        specifiers: vec![
                            IrImportSpecifier::Named {
                                name: format!("func_{}_0", target),
                                alias: None,
                            },
                            IrImportSpecifier::Named {
                                name: format!("Type_{}", target),
                                alias: None,
                            },
                        ],
                        span: Span::single(1),
                    })
                })
                .collect();

            let exports: Vec<IrExport> = functions
                .iter()
                .filter(|f| f.is_exported)
                .map(|f| IrExport {
                    name: f.name.clone(),
                    is_default: false,
                    is_reexport: false,
                    source: None,
                    span: Span::single(1),
                })
                .collect();

            // Call expressions referencing other files' functions
            let call_expressions: Vec<IrCallExpression> = (1..=3)
                .map(|offset| {
                    let target = (i + offset) % n;
                    IrCallExpression {
                        callee: format!("func_{}_0", target),
                        arguments: vec!["arg1".to_string()],
                        span: Span::single(20 + offset),
                        containing_function: Some(format!("func_{}_0", i)),
                    }
                })
                .collect();

            IrFile {
                path,
                language: diffcore_core::ast::Language::TypeScript,
                functions,
                type_defs,
                constants,
                imports,
                exports,
                call_expressions,
                assignments: vec![],
            }
        })
        .collect()
}

fn bench_graph_building(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph_build_from_ir");

    for &file_count in &[20, 50, 100] {
        let files = generate_ir_files(file_count);

        // Parallel (default — uses rayon global pool)
        group.bench_with_input(
            BenchmarkId::new("parallel", file_count),
            &files,
            |b, files| {
                b.iter(|| black_box(SymbolGraph::build_from_ir(files)));
            },
        );

        // Serial (single-threaded rayon pool)
        group.bench_with_input(
            BenchmarkId::new("serial", file_count),
            &files,
            |b, files| {
                let pool = rayon::ThreadPoolBuilder::new()
                    .num_threads(1)
                    .build()
                    .unwrap();
                b.iter(|| {
                    pool.install(|| black_box(SymbolGraph::build_from_ir(files)));
                });
            },
        );
    }

    group.finish();
}

/// Generate synthetic ParsedFiles with realistic call sites for flow analysis benchmarking.
///
/// Each file has mixed call sites: DB writes, DB reads, event emission, HTTP calls,
/// logging, config reads, and benign calls (collection methods, stdlib).
fn generate_parsed_files_for_flow(n: usize) -> Vec<ParsedFile> {
    let callees = [
        // DB writes
        "db.save",
        "userRepo.insert",
        "model.create",
        "store.update",
        "repo.delete",
        "prisma.user.upsert",
        "collection.bulkInsert",
        // DB reads
        "db.find",
        "userRepo.findOne",
        "model.findById",
        "store.query",
        "repo.findAll",
        // Event emission
        "eventBus.emit",
        "channel.publish",
        "socket.send",
        "dispatcher.dispatch",
        // Event handling
        "eventBus.on",
        "channel.subscribe",
        "socket.listen",
        // HTTP calls
        "fetch",
        "axios.get",
        "axios.post",
        "requests.get",
        "httpx.post",
        // Logging
        "console.log",
        "console.error",
        "logger.info",
        "logger.warn",
        "logging.debug",
        // Config reads
        "process.env.DATABASE_URL",
        "config.get",
        "os.environ",
        // Benign (should not match)
        "arr.push",
        "arr.map",
        "arr.filter",
        "JSON.parse",
        "JSON.stringify",
        "Object.assign",
        "Promise.resolve",
        "Math.floor",
        "str.split",
        "list.append",
    ];

    (0..n)
        .map(|i| {
            let path = format!("src/module_{}.ts", i);
            let call_sites: Vec<CallSite> = callees
                .iter()
                .enumerate()
                .map(|(j, callee)| CallSite {
                    callee: callee.to_string(),
                    line: j + 1,
                    containing_function: Some(format!("handler_{}", j % 4)),
                })
                .collect();

            let imports = vec![
                ImportInfo {
                    source: "express".to_string(),
                    names: vec![],
                    is_default: false,
                    is_namespace: false,
                    line: 1,
                },
                ImportInfo {
                    source: "@prisma/client".to_string(),
                    names: vec![],
                    is_default: false,
                    is_namespace: false,
                    line: 2,
                },
                ImportInfo {
                    source: "react".to_string(),
                    names: vec![],
                    is_default: false,
                    is_namespace: false,
                    line: 3,
                },
            ];

            ParsedFile {
                path,
                language: Language::TypeScript,
                definitions: vec![],
                imports,
                exports: vec![],
                call_sites,
            }
        })
        .collect()
}

fn bench_flow_analysis(c: &mut Criterion) {
    let mut group = c.benchmark_group("flow_analysis");

    for &file_count in &[20, 50, 100] {
        let files = generate_parsed_files_for_flow(file_count);
        let config = FlowConfig::default();

        group.bench_with_input(
            BenchmarkId::new("heuristic_patterns", file_count),
            &(files, config),
            |b, (files, config)| {
                b.iter(|| black_box(analyze_data_flow(files, config)));
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_lazy_query_engine,
    bench_parse_dedup,
    bench_graph_building,
    bench_flow_analysis
);
criterion_main!(benches);
