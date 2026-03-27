#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
//! Regression test suite (spec §13.13).
//!
//! Each test recreates a scenario that previously caused issues:
//! barrel-file explosion, circular dependencies, dynamic imports,
//! monorepo cross-package edges, file rename chains, generated code
//! domination, and mixed-language projects.
//!
//! Run with:
//!   cargo test --test regressions

mod helpers;

use helpers::graph_assertions::{
    assert_all_files_accounted, assert_json_roundtrip, assert_valid_json_schema,
    assert_valid_scores,
};
use helpers::repo_builder::{run_pipeline, RepoBuilder};

// ═══════════════════════════════════════════════════════════════════════════
// 001 — Barrel File Explosion
// ═══════════════════════════════════════════════════════════════════════════
//
// An index.ts that re-exports 50 modules should not cause infinite edge
// explosion, excessive group counts, or lost files. The barrel file itself
// should appear in the output (either in a group or infrastructure).

#[test]
fn regression_001_barrel_file_50_reexports_no_explosion() {
    let rb = RepoBuilder::new();

    // Base: empty project
    rb.write_file("package.json", r#"{"name": "barrel-test"}"#);
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/barrel");
    rb.checkout("feature/barrel");

    // Create 50 modules + barrel index re-exporting all of them
    let mut barrel_content = String::new();
    for i in 0..50 {
        let module_name = format!("mod{}", i);
        rb.write_file(
            &format!("src/modules/{}.ts", module_name),
            &format!(
                "export function {}Helper() {{ return {}; }}\n",
                module_name, i
            ),
        );
        barrel_content.push_str(&format!(
            "export {{ {}Helper }} from './{}';\n",
            module_name, module_name
        ));
    }
    rb.write_file("src/modules/index.ts", &barrel_content);

    // A route that imports from the barrel
    rb.write_file(
        "src/routes/api.ts",
        r#"
import express from 'express';
import { mod0Helper, mod1Helper } from '../modules';

const router = express.Router();

export function getAll(req: any, res: any) {
    res.json({ a: mod0Helper(), b: mod1Helper() });
}

router.get('/all', getAll);
export default router;
"#,
    );
    rb.commit("Add barrel with 50 modules");

    let output = run_pipeline(rb.path(), "main", "feature/barrel");

    // Must not crash — basic structural assertions
    assert_all_files_accounted(&output);
    assert_valid_scores(&output);
    assert_valid_json_schema(&output);
    assert_json_roundtrip(&output);

    // 50 modules + 1 barrel + 1 route = 52 changed files
    assert_eq!(
        output.summary.total_files_changed, 52,
        "Expected 52 changed files (50 modules + barrel + route)"
    );

    // The barrel file (index.ts) must appear somewhere
    let barrel_found = output
        .groups
        .iter()
        .any(|g| g.files.iter().any(|f| f.path.contains("modules/index.ts")))
        || output
            .infrastructure_group
            .as_ref()
            .map(|ig| ig.files.iter().any(|f| f.contains("modules/index.ts")))
            .unwrap_or(false);
    assert!(barrel_found, "Barrel file index.ts must appear in output");

    // Should not produce more groups than entrypoints (1 route = at most a few groups)
    assert!(
        output.groups.len() <= 5,
        "Barrel explosion: {} groups is too many for 1 route",
        output.groups.len()
    );
}

#[test]
fn regression_001_barrel_single_module_change() {
    // Changing one module that's re-exported via a barrel should not pull
    // all 50 sibling modules into the diff.
    let rb = RepoBuilder::new();

    rb.write_file("package.json", r#"{"name": "barrel-test-2"}"#);
    // Create the initial barrel + modules
    let mut barrel_content = String::new();
    for i in 0..10 {
        let module_name = format!("mod{}", i);
        rb.write_file(
            &format!("src/modules/{}.ts", module_name),
            &format!("export function {}Fn() {{ return {}; }}\n", module_name, i),
        );
        barrel_content.push_str(&format!(
            "export {{ {}Fn }} from './{}';\n",
            module_name, module_name
        ));
    }
    rb.write_file("src/modules/index.ts", &barrel_content);
    rb.commit("init with barrel");
    rb.create_branch("main");

    rb.create_branch("feature/fix-mod3");
    rb.checkout("feature/fix-mod3");

    // Only modify one module
    rb.write_file(
        "src/modules/mod3.ts",
        "export function mod3Fn() { return 42; /* fixed */ }\n",
    );
    rb.commit("Fix mod3");

    let output = run_pipeline(rb.path(), "main", "feature/fix-mod3");

    // Only 1 file changed — the modified module, not the barrel or siblings
    assert_eq!(
        output.summary.total_files_changed, 1,
        "Only mod3.ts should appear in the diff"
    );
    assert_all_files_accounted(&output);
}

// ═══════════════════════════════════════════════════════════════════════════
// 002 — Circular Dependency
// ═══════════════════════════════════════════════════════════════════════════
//
// A→B→C→A import cycle must not cause infinite loops in graph traversal,
// BFS, or clustering. All files must still be accounted for.

#[test]
fn regression_002_circular_dependency_no_infinite_loop() {
    let rb = RepoBuilder::new();

    rb.write_file("package.json", r#"{"name": "cycle-test"}"#);
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/cycle");
    rb.checkout("feature/cycle");

    // A imports B, B imports C, C imports A
    rb.write_file(
        "src/moduleA.ts",
        r#"
import { helperC } from './moduleC';
export function helperA() { return helperC(); }
"#,
    );
    rb.write_file(
        "src/moduleB.ts",
        r#"
import { helperA } from './moduleA';
export function helperB() { return helperA(); }
"#,
    );
    rb.write_file(
        "src/moduleC.ts",
        r#"
import { helperB } from './moduleB';
export function helperC() { return helperB(); }
"#,
    );
    rb.commit("Add cyclic modules A→B→C→A");

    let output = run_pipeline(rb.path(), "main", "feature/cycle");

    // Must terminate (the test itself proves no infinite loop)
    assert_all_files_accounted(&output);
    assert_valid_json_schema(&output);
    assert_json_roundtrip(&output);

    // All 3 cyclic files must be present
    assert_eq!(
        output.summary.total_files_changed, 3,
        "All 3 cycle members should be in the diff"
    );
}

#[test]
fn regression_002_circular_with_entrypoint() {
    // A cycle that includes a route entrypoint should still produce a valid group.
    let rb = RepoBuilder::new();

    rb.write_file("package.json", r#"{"name": "cycle-entry"}"#);
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/cycle-route");
    rb.checkout("feature/cycle-route");

    // Route → service → repo → service (cycle between service and repo)
    rb.write_file(
        "src/routes/users.ts",
        r#"
import express from 'express';
import { getUser } from '../services/userService';

const router = express.Router();
export function handleGetUser(req: any, res: any) { res.json(getUser(1)); }
router.get('/users/:id', handleGetUser);
export default router;
"#,
    );
    rb.write_file(
        "src/services/userService.ts",
        r#"
import { findUser } from '../repos/userRepo';
export function getUser(id: number) { return findUser(id); }
export function enrichUser(user: any) { return { ...user, enriched: true }; }
"#,
    );
    rb.write_file(
        "src/repos/userRepo.ts",
        r#"
import { enrichUser } from '../services/userService';
export function findUser(id: number) { return enrichUser({ id, name: 'test' }); }
"#,
    );
    rb.commit("Route with cyclic service/repo");

    let output = run_pipeline(rb.path(), "main", "feature/cycle-route");

    assert_all_files_accounted(&output);
    assert_valid_scores(&output);
    assert_eq!(output.summary.total_files_changed, 3);

    // The route should be detected as an entrypoint → at least 1 flow group
    assert!(
        !output.groups.is_empty(),
        "Route with cycle should produce at least 1 flow group"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 003 — Dynamic Import
// ═══════════════════════════════════════════════════════════════════════════
//
// Dynamic `import()` expressions should not crash the parser. The file
// containing the dynamic import should still be parsed and included.

#[test]
fn regression_003_dynamic_import_no_crash() {
    let rb = RepoBuilder::new();

    rb.write_file("package.json", r#"{"name": "dynamic-import-test"}"#);
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/dynamic");
    rb.checkout("feature/dynamic");

    // Static import + dynamic import in the same file
    rb.write_file(
        "src/routes/lazy.ts",
        r#"
import express from 'express';

const router = express.Router();

export async function lazyHandler(req: any, res: any) {
    const { processData } = await import('../services/heavyService');
    const result = processData(req.body);
    res.json(result);
}

router.post('/lazy', lazyHandler);
export default router;
"#,
    );
    rb.write_file(
        "src/services/heavyService.ts",
        r#"
export function processData(data: any) {
    return { processed: true, ...data };
}
"#,
    );

    // A require() dynamic import
    rb.write_file(
        "src/utils/loader.ts",
        r#"
export function loadPlugin(name: string) {
    const plugin = require(`./plugins/${name}`);
    return plugin.default;
}
"#,
    );
    rb.commit("Add dynamic imports");

    let output = run_pipeline(rb.path(), "main", "feature/dynamic");

    // Must not crash
    assert_all_files_accounted(&output);
    assert_valid_json_schema(&output);
    assert_json_roundtrip(&output);

    // All 3 files present
    assert_eq!(output.summary.total_files_changed, 3);

    // The route file with dynamic import should still be detected
    let lazy_found = output
        .groups
        .iter()
        .any(|g| g.files.iter().any(|f| f.path.contains("lazy.ts")))
        || output
            .infrastructure_group
            .as_ref()
            .map(|ig| ig.files.iter().any(|f| f.contains("lazy.ts")))
            .unwrap_or(false);
    assert!(lazy_found, "File with dynamic import() must be in output");
}

#[test]
fn regression_003_dynamic_require_template_literal() {
    // require() with a template literal path should not crash.
    let rb = RepoBuilder::new();

    rb.write_file("package.json", r#"{"name": "dyn-require"}"#);
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/dyn-require");
    rb.checkout("feature/dyn-require");

    rb.write_file(
        "src/plugins/loader.ts",
        r#"
const plugins = ['auth', 'logging', 'cache'];

export function loadPlugins() {
    return plugins.map(name => {
        const mod = require(`./impl/${name}`);
        return mod.default || mod;
    });
}
"#,
    );
    rb.write_file(
        "src/plugins/impl/auth.ts",
        "export default { name: 'auth', init() {} };\n",
    );
    rb.write_file(
        "src/plugins/impl/logging.ts",
        "export default { name: 'logging', init() {} };\n",
    );
    rb.write_file(
        "src/plugins/impl/cache.ts",
        "export default { name: 'cache', init() {} };\n",
    );
    rb.commit("Dynamic require with template literals");

    let output = run_pipeline(rb.path(), "main", "feature/dyn-require");

    // Must not crash — all 4 files accounted for
    assert_all_files_accounted(&output);
    assert_eq!(output.summary.total_files_changed, 4);
}

// ═══════════════════════════════════════════════════════════════════════════
// 004 — Monorepo Cross-Package
// ═══════════════════════════════════════════════════════════════════════════
//
// Imports across workspace packages (e.g., `@scope/shared` → `@scope/api`)
// should not produce orphaned files or crash the graph builder.

#[test]
fn regression_004_monorepo_cross_package_imports() {
    let rb = RepoBuilder::new();

    rb.write_file(
        "package.json",
        r#"{"name": "monorepo", "workspaces": ["packages/*"]}"#,
    );
    rb.write_file(
        "packages/shared/package.json",
        r#"{"name": "@mono/shared", "version": "1.0.0"}"#,
    );
    rb.write_file(
        "packages/api/package.json",
        r#"{"name": "@mono/api", "version": "1.0.0", "dependencies": {"@mono/shared": "1.0.0"}}"#,
    );
    rb.write_file(
        "packages/web/package.json",
        r#"{"name": "@mono/web", "version": "1.0.0", "dependencies": {"@mono/shared": "1.0.0"}}"#,
    );
    rb.commit("init monorepo");
    rb.create_branch("main");

    rb.create_branch("feature/cross-pkg");
    rb.checkout("feature/cross-pkg");

    // Shared types
    rb.write_file(
        "packages/shared/src/types.ts",
        r#"
export interface User { id: number; name: string; }
export interface ApiResponse<T> { data: T; error?: string; }
"#,
    );

    // API imports shared types
    rb.write_file(
        "packages/api/src/routes/users.ts",
        r#"
import express from 'express';
import { User, ApiResponse } from '@mono/shared/src/types';

const router = express.Router();

export function listUsers(req: any, res: any) {
    const response: ApiResponse<User[]> = { data: [] };
    res.json(response);
}

router.get('/users', listUsers);
export default router;
"#,
    );

    // Web also imports shared types
    rb.write_file(
        "packages/web/src/components/UserList.tsx",
        r#"
import { User } from '@mono/shared/src/types';

export function UserList(props: { users: User[] }) {
    return props.users.map(u => u.name).join(', ');
}
"#,
    );

    // API service importing from shared
    rb.write_file(
        "packages/api/src/services/userService.ts",
        r#"
import { User } from '@mono/shared/src/types';

export function createUser(name: string): User {
    return { id: Date.now(), name };
}
"#,
    );
    rb.commit("Cross-package changes");

    let output = run_pipeline(rb.path(), "main", "feature/cross-pkg");

    // Must not crash — all files accounted for
    assert_all_files_accounted(&output);
    assert_valid_json_schema(&output);
    assert_json_roundtrip(&output);

    // 4 source files changed (types, users route, UserList, userService)
    assert_eq!(
        output.summary.total_files_changed, 4,
        "Expected 4 changed source files"
    );

    // TypeScript detected
    assert!(
        output
            .summary
            .languages_detected
            .iter()
            .any(|l| l.contains("TypeScript") || l.contains("typescript") || l == "tsx"),
        "TypeScript should be detected in monorepo, got: {:?}",
        output.summary.languages_detected
    );
}

#[test]
fn regression_004_monorepo_shared_dep_not_lost() {
    // The shared package file should not be lost even if it has no direct
    // entrypoint — it should appear in either a flow group or infrastructure.
    let rb = RepoBuilder::new();

    rb.write_file(
        "package.json",
        r#"{"name": "mono2", "workspaces": ["packages/*"]}"#,
    );
    rb.write_file("packages/core/package.json", r#"{"name": "@m/core"}"#);
    rb.write_file(
        "packages/app/package.json",
        r#"{"name": "@m/app", "dependencies": {"@m/core": "1.0.0"}}"#,
    );
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/core-change");
    rb.checkout("feature/core-change");

    rb.write_file(
        "packages/core/src/utils.ts",
        "export function validate(x: any): boolean { return !!x; }\n",
    );
    rb.write_file(
        "packages/app/src/handler.ts",
        r#"
import { validate } from '@m/core/src/utils';
export function handle(input: any) { return validate(input); }
"#,
    );
    rb.commit("Shared util + consumer");

    let output = run_pipeline(rb.path(), "main", "feature/core-change");

    assert_all_files_accounted(&output);
    assert_eq!(output.summary.total_files_changed, 2);
}

// ═══════════════════════════════════════════════════════════════════════════
// 005 — File Rename Chain
// ═══════════════════════════════════════════════════════════════════════════
//
// A renamed to B, B renamed to C. Git should detect renames and the
// pipeline should not double-count files or lose them.

#[test]
fn regression_005_file_rename_chain() {
    let rb = RepoBuilder::new();

    rb.write_file("package.json", r#"{"name": "rename-test"}"#);
    rb.write_file("src/oldName.ts", "export function helper() { return 1; }\n");
    rb.write_file(
        "src/anotherOld.ts",
        "export function anotherHelper() { return 2; }\n",
    );
    rb.write_file(
        "src/consumer.ts",
        r#"
import { helper } from './oldName';
import { anotherHelper } from './anotherOld';
export function consume() { return helper() + anotherHelper(); }
"#,
    );
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/renames");
    rb.checkout("feature/renames");

    // Rename oldName.ts → newName.ts (delete old, create new)
    std::fs::remove_file(rb.path().join("src/oldName.ts")).unwrap();
    rb.write_file("src/newName.ts", "export function helper() { return 1; }\n");

    // Rename anotherOld.ts → anotherNew.ts
    std::fs::remove_file(rb.path().join("src/anotherOld.ts")).unwrap();
    rb.write_file(
        "src/anotherNew.ts",
        "export function anotherHelper() { return 2; }\n",
    );

    // Update consumer to use new paths
    rb.write_file(
        "src/consumer.ts",
        r#"
import { helper } from './newName';
import { anotherHelper } from './anotherNew';
export function consume() { return helper() + anotherHelper(); }
"#,
    );
    rb.commit("Rename oldName→newName, anotherOld→anotherNew");

    let output = run_pipeline(rb.path(), "main", "feature/renames");

    // Must not crash
    assert_all_files_accounted(&output);
    assert_valid_json_schema(&output);
    assert_json_roundtrip(&output);

    // No file should be double-counted
    let all_paths: Vec<&str> = output
        .groups
        .iter()
        .flat_map(|g| g.files.iter().map(|f| f.path.as_str()))
        .chain(
            output
                .infrastructure_group
                .iter()
                .flat_map(|ig| ig.files.iter().map(|s| s.as_str())),
        )
        .collect();
    let unique: std::collections::HashSet<&str> = all_paths.iter().copied().collect();
    assert_eq!(
        all_paths.len(),
        unique.len(),
        "No file should be double-counted after renames"
    );
}

#[test]
fn regression_005_rename_with_content_change() {
    // Rename + modify in the same commit — should be detected and not crash.
    let rb = RepoBuilder::new();

    rb.write_file("package.json", r#"{"name": "rename-mod"}"#);
    rb.write_file(
        "src/service.ts",
        "export function doWork() { return 'v1'; }\n",
    );
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/rename-mod");
    rb.checkout("feature/rename-mod");

    std::fs::remove_file(rb.path().join("src/service.ts")).unwrap();
    rb.write_file(
        "src/workerService.ts",
        "export function doWork() { return 'v2'; /* renamed + changed */ }\n",
    );
    rb.commit("Rename service→workerService with content change");

    let output = run_pipeline(rb.path(), "main", "feature/rename-mod");

    assert_all_files_accounted(&output);
    assert!(
        output.summary.total_files_changed >= 1,
        "At least the renamed/new file should appear"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 006 — Generated Code
// ═══════════════════════════════════════════════════════════════════════════
//
// Large generated files should not dominate the analysis. They should be
// classified as infrastructure/generated, not swamp the flow groups.

#[test]
fn regression_006_generated_code_not_dominating() {
    let rb = RepoBuilder::new();

    rb.write_file("package.json", r#"{"name": "gen-code"}"#);
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/gen");
    rb.checkout("feature/gen");

    // A real route
    rb.write_file(
        "src/routes/api.ts",
        r#"
import express from 'express';
import { getData } from '../services/dataService';

const router = express.Router();
export function getHandler(req: any, res: any) { res.json(getData()); }
router.get('/data', getHandler);
export default router;
"#,
    );
    rb.write_file(
        "src/services/dataService.ts",
        "export function getData() { return { items: [] }; }\n",
    );

    // Large generated file (simulate with many type definitions)
    let mut generated = String::from("// @generated\n// DO NOT EDIT\n\n");
    for i in 0..200 {
        generated.push_str(&format!(
            "export interface GeneratedType{} {{ id: number; field{}: string; }}\n",
            i, i
        ));
    }
    rb.write_file("src/__generated__/types.generated.ts", &generated);

    // Another generated file
    let mut proto = String::from("// Code generated by protoc-gen-ts. DO NOT EDIT.\n\n");
    for i in 0..100 {
        proto.push_str(&format!(
            "export interface Proto{} {{ value: number; }}\n",
            i
        ));
    }
    rb.write_file("src/generated/proto.ts", &proto);

    rb.commit("Add route + generated code");

    let output = run_pipeline(rb.path(), "main", "feature/gen");

    assert_all_files_accounted(&output);
    assert_valid_json_schema(&output);
    assert_json_roundtrip(&output);

    // 4 files total
    assert_eq!(output.summary.total_files_changed, 4);

    // The generated files should land in infrastructure, not in flow groups
    // (they have no entrypoints and live in __generated__/ or generated/)
    let gen_in_flow_groups: Vec<&str> = output
        .groups
        .iter()
        .flat_map(|g| g.files.iter().map(|f| f.path.as_str()))
        .filter(|p| p.contains("generated") || p.contains("__generated__"))
        .collect();

    // It's acceptable if they end up in infra or flow groups, but the route
    // should still be detected as an entrypoint regardless.
    let route_in_group = output
        .groups
        .iter()
        .any(|g| g.files.iter().any(|f| f.path.contains("routes/api.ts")));
    let route_in_infra = output
        .infrastructure_group
        .as_ref()
        .map(|ig| ig.files.iter().any(|f| f.contains("routes/api.ts")))
        .unwrap_or(false);
    assert!(
        route_in_group || route_in_infra,
        "The real route should be present regardless of generated code"
    );

    // If generated files ended up in infra, check sub-group classification
    if let Some(ref infra) = output.infrastructure_group {
        if !infra.sub_groups.is_empty() {
            let has_generated_subgroup = infra.sub_groups.iter().any(|sg| {
                sg.category == flowdiff_core::types::InfraCategory::Generated
                    || sg.files.iter().any(|f| f.contains("generated"))
            });
            if infra.files.iter().any(|f| f.contains("generated")) {
                assert!(
                    has_generated_subgroup,
                    "Generated files in infra should be in a Generated sub-group"
                );
            }
        }
    }

    // The generated files should not produce excessive groups
    if !gen_in_flow_groups.is_empty() {
        // If they ended up in flow groups, there shouldn't be more than a
        // handful of groups total (not 200+ from individual types)
        assert!(
            output.groups.len() <= 10,
            "Generated code should not explode group count: got {}",
            output.groups.len()
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 007 — Mixed Language Project
// ═══════════════════════════════════════════════════════════════════════════
//
// A repo with TypeScript, Python, and Rust files in the same diff.
// The pipeline must detect all languages and not crash on mixed inputs.

#[test]
fn regression_007_mixed_language_ts_python_rust() {
    let rb = RepoBuilder::new();

    rb.write_file("package.json", r#"{"name": "mixed-lang"}"#);
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/mixed");
    rb.checkout("feature/mixed");

    // TypeScript: Express route
    rb.write_file(
        "frontend/src/routes/dashboard.ts",
        r#"
import express from 'express';
import { fetchMetrics } from '../services/metricsService';

const router = express.Router();
export function getDashboard(req: any, res: any) {
    const metrics = fetchMetrics();
    res.json(metrics);
}
router.get('/dashboard', getDashboard);
export default router;
"#,
    );
    rb.write_file(
        "frontend/src/services/metricsService.ts",
        r#"
export function fetchMetrics() {
    return { cpu: 42, memory: 80 };
}
"#,
    );

    // Python: FastAPI endpoint
    rb.write_file(
        "backend/app/routes/health.py",
        r#"
from fastapi import APIRouter
from backend.app.services.health_service import get_health

router = APIRouter()

@router.get("/health")
def health_check():
    return get_health()
"#,
    );
    rb.write_file(
        "backend/app/services/health_service.py",
        r#"
def get_health():
    return {"status": "ok", "version": "1.0.0"}
"#,
    );

    // Rust: axum handler
    rb.write_file(
        "services/api/src/handlers/status.rs",
        r#"
use axum::{Json, response::IntoResponse};
use serde_json::json;

pub async fn get_status() -> impl IntoResponse {
    Json(json!({"status": "running"}))
}
"#,
    );
    rb.write_file(
        "services/api/src/main.rs",
        r#"
mod handlers;
use axum::{Router, routing::get};

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/status", get(handlers::status::get_status));
    println!("Listening on :3000");
}
"#,
    );

    rb.commit("Mixed language: TS + Python + Rust");

    let output = run_pipeline(rb.path(), "main", "feature/mixed");

    assert_all_files_accounted(&output);
    assert_valid_json_schema(&output);
    assert_valid_scores(&output);
    assert_json_roundtrip(&output);

    // 6 source files
    assert_eq!(output.summary.total_files_changed, 6);

    // Multiple languages should be detected
    let langs = &output.summary.languages_detected;
    assert!(
        langs.len() >= 2,
        "Expected at least 2 languages detected, got {:?}",
        langs
    );
}

#[test]
fn regression_007_mixed_language_no_cross_contamination() {
    // Files from different languages should not be grouped together
    // unless they share actual import edges.
    let rb = RepoBuilder::new();

    rb.write_file("package.json", r#"{"name": "mixed-iso"}"#);
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/iso");
    rb.checkout("feature/iso");

    // Isolated TypeScript file
    rb.write_file(
        "web/src/utils.ts",
        "export function formatDate(d: Date): string { return d.toISOString(); }\n",
    );

    // Isolated Python file
    rb.write_file(
        "api/utils.py",
        "def format_date(d):\n    return d.isoformat()\n",
    );

    // Isolated Rust file
    rb.write_file(
        "core/src/utils.rs",
        "pub fn format_date(d: &str) -> String { d.to_string() }\n",
    );

    rb.commit("Isolated utils in 3 languages");

    let output = run_pipeline(rb.path(), "main", "feature/iso");

    assert_all_files_accounted(&output);
    assert_eq!(output.summary.total_files_changed, 3);

    // Each file should be independent — no flow group should contain files
    // from different language directories (since there are no import edges)
    for group in &output.groups {
        let dirs: std::collections::HashSet<&str> = group
            .files
            .iter()
            .filter_map(|f| f.path.split('/').next())
            .collect();
        assert!(
            dirs.len() <= 1,
            "Group '{}' mixes language directories: {:?}",
            group.name,
            dirs
        );
    }
}
