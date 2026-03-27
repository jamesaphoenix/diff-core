#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
//! Cross-layer integration audit — runs the full pipeline on adversarial repos.
//!
//! Tests the complete path: git diff → AST parse → IR → graph → entrypoints →
//! flow analysis → cluster → rank → JSON output, on repos designed to expose
//! edge cases: empty repos, binary-only, deeply nested circular imports,
//! non-UTF8 paths, symlinks, large-scale monorepos, etc.
//!
//! Run with:
//!   cargo test --test cross_layer_audit

mod helpers;

use flowdiff_core::ast;
use flowdiff_core::cluster;
use flowdiff_core::entrypoint;
use flowdiff_core::flow::{self, FlowConfig};
use flowdiff_core::git;
use flowdiff_core::graph::SymbolGraph;
use flowdiff_core::output::{self, build_analysis_output};
use flowdiff_core::pipeline;
use flowdiff_core::rank;
use flowdiff_core::types::{AnalysisOutput, GroupRankInput, RankWeights};
use helpers::graph_assertions::{
    assert_all_files_accounted, assert_json_roundtrip, assert_valid_json_schema,
    assert_valid_scores,
};
use helpers::repo_builder::RepoBuilder;

// ═══════════════════════════════════════════════════════════════════════════
// Helper: run full pipeline on a repo (same as eval fixtures but inline)
// ═══════════════════════════════════════════════════════════════════════════

fn run_full_pipeline(rb: &RepoBuilder, base: &str, head: &str) -> AnalysisOutput {
    let repo = git2::Repository::open(rb.path()).expect("failed to open repo");
    let diff_result = git::diff_refs(&repo, base, head).expect("diff_refs failed");

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
                risk: rank::compute_risk_score(
                    risk_flags.has_schema_change,
                    risk_flags.has_api_change,
                    risk_flags.has_auth_change,
                    false,
                ),
                centrality: 0.5,
                surface_area: rank::compute_surface_area(total_add, total_del, 1000),
                uncertainty: if risk_flags.has_test_only { 0.1 } else { 0.5 },
            }
        })
        .collect();

    let ranked = rank::rank_groups(&rank_inputs, &weights);

    let diff_source = output::diff_source_branch(
        base,
        head,
        diff_result.base_sha.as_deref(),
        diff_result.head_sha.as_deref(),
    );

    build_analysis_output(
        &diff_result,
        diff_source,
        &parsed_files,
        &cluster_result,
        &ranked,
    )
}

/// Same as run_full_pipeline but using the IR pipeline path (QueryEngine + IrFile)
fn run_ir_pipeline(rb: &RepoBuilder, base: &str, head: &str) -> AnalysisOutput {
    let repo = git2::Repository::open(rb.path()).expect("failed to open repo");
    let diff_result = git::diff_refs(&repo, base, head).expect("diff_refs failed");

    use std::sync::OnceLock;
    static ENGINE: OnceLock<flowdiff_core::query_engine::QueryEngine> = OnceLock::new();
    let engine = ENGINE.get_or_init(|| {
        flowdiff_core::query_engine::QueryEngine::new().expect("shared QueryEngine init")
    });

    let file_inputs: Vec<(&str, &str)> = diff_result
        .files
        .iter()
        .filter_map(|file_diff| {
            let content = file_diff
                .new_content
                .as_deref()
                .or(file_diff.old_content.as_deref())?;
            Some((file_diff.path(), content))
        })
        .collect();

    let (ir_files, _errors) = pipeline::parse_all_to_ir(&engine, &file_inputs, None);

    // Build graph from IR
    let mut graph = SymbolGraph::build_from_ir(&ir_files);
    let entrypoints = entrypoint::detect_entrypoints_ir(&ir_files);
    let flow_analysis = flow::analyze_data_flow_ir(&ir_files, &FlowConfig::default());
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
                risk: rank::compute_risk_score(
                    risk_flags.has_schema_change,
                    risk_flags.has_api_change,
                    risk_flags.has_auth_change,
                    false,
                ),
                centrality: 0.5,
                surface_area: rank::compute_surface_area(total_add, total_del, 1000),
                uncertainty: if risk_flags.has_test_only { 0.1 } else { 0.5 },
            }
        })
        .collect();

    let ranked = rank::rank_groups(&rank_inputs, &weights);

    // We need ParsedFile for build_analysis_output, so re-parse via ast path
    let mut parsed_files = Vec::new();
    for file_diff in &diff_result.files {
        if let Some(ref content) = file_diff.new_content {
            if let Ok(parsed) = ast::parse_file(file_diff.path(), content) {
                parsed_files.push(parsed);
            }
        }
    }

    let diff_source = output::diff_source_branch(
        base,
        head,
        diff_result.base_sha.as_deref(),
        diff_result.head_sha.as_deref(),
    );

    build_analysis_output(
        &diff_result,
        diff_source,
        &parsed_files,
        &cluster_result,
        &ranked,
    )
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. Empty repo — no commits at all
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_empty_repo_no_commits() {
    let rb = RepoBuilder::new();
    // No commits, no branches — diff_refs should fail gracefully
    let repo = git2::Repository::open(rb.path()).expect("open");
    let result = git::diff_refs(&repo, "main", "HEAD");
    assert!(result.is_err(), "diff_refs on empty repo should error");
    let err = result.unwrap_err();
    let msg = format!("{}", err);
    assert!(
        msg.contains("not found") || msg.contains("empty") || msg.contains("ref"),
        "error should mention missing ref: {}",
        msg
    );
}

#[test]
fn adversarial_empty_repo_staged_diff() {
    let rb = RepoBuilder::new();
    let repo = git2::Repository::open(rb.path()).expect("open");
    let result = git::diff_staged(&repo);
    assert!(
        result.is_err(),
        "staged diff on empty repo (no HEAD) should error"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. Repo with only binary files
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_binary_only_repo() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/binary");
    rb.checkout("feature/binary");

    // Write binary-like files (null bytes signal binary to git)
    let binary_content = {
        let mut v = Vec::new();
        for i in 0..256u16 {
            v.push((i % 256) as u8);
        }
        v
    };
    let full_path = rb.path().join("image.png");
    std::fs::write(&full_path, &binary_content).unwrap();
    let full_path2 = rb.path().join("data.bin");
    std::fs::write(&full_path2, &binary_content).unwrap();

    // Use git2 to stage and commit
    let repo = git2::Repository::open(rb.path()).unwrap();
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    let sig = git2::Signature::now("Test", "test@test.com").unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        "add binary files",
        &tree,
        &[&parent],
    )
    .unwrap();

    let output = run_full_pipeline(&rb, "main", "feature/binary");

    // Binary files should be filtered out — no files to analyze
    assert_eq!(
        output.summary.total_files_changed, 0,
        "binary files should be skipped, but got {} files",
        output.summary.total_files_changed
    );
    assert!(output.groups.is_empty());

    // JSON should still be valid
    assert_json_roundtrip(&output);
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. Repo with deeply nested circular imports
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_circular_imports_deep() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/circular");
    rb.checkout("feature/circular");

    // Create a cycle: a → b → c → d → e → a
    rb.write_file(
        "src/a.ts",
        "import { b } from './b';\nexport function a() { return b(); }\n",
    );
    rb.write_file(
        "src/b.ts",
        "import { c } from './c';\nexport function b() { return c(); }\n",
    );
    rb.write_file(
        "src/c.ts",
        "import { d } from './d';\nexport function c() { return d(); }\n",
    );
    rb.write_file(
        "src/d.ts",
        "import { e } from './e';\nexport function d() { return e(); }\n",
    );
    rb.write_file(
        "src/e.ts",
        "import { a } from './a';\nexport function e() { return a(); }\n",
    );
    rb.commit("circular imports");

    let output = run_full_pipeline(&rb, "main", "feature/circular");

    // Pipeline should complete without infinite loops
    assert_eq!(output.summary.total_files_changed, 5);
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);
    assert_valid_scores(&output);
}

#[test]
fn adversarial_self_importing_file() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/self-import");
    rb.checkout("feature/self-import");

    // File imports itself (pathological edge case)
    rb.write_file(
        "src/recursive.ts",
        "import { recursive } from './recursive';\nexport function recursive() { return recursive(); }\n",
    );
    rb.commit("self-importing file");

    let output = run_full_pipeline(&rb, "main", "feature/self-import");

    assert_eq!(output.summary.total_files_changed, 1);
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);
}

#[test]
fn adversarial_mutual_circular_imports() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/mutual");
    rb.checkout("feature/mutual");

    // Two files import each other (common real-world pattern)
    rb.write_file(
        "src/model.ts",
        r#"
import { validate } from './validator';
export interface User { name: string; }
export function createUser(data: any): User {
    validate(data);
    return { name: data.name };
}
"#,
    );
    rb.write_file(
        "src/validator.ts",
        r#"
import { User } from './model';
export function validate(data: any): boolean {
    return typeof data.name === 'string';
}
export function validateUser(user: User): boolean {
    return validate(user);
}
"#,
    );
    rb.commit("mutual imports");

    let output = run_full_pipeline(&rb, "main", "feature/mutual");

    assert_eq!(output.summary.total_files_changed, 2);
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. Large-scale repo (100+ files)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_100_file_diff() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/big");
    rb.checkout("feature/big");

    // Create 100 files with import chains forming a tree structure
    for i in 0..100 {
        let content = if i == 0 {
            format!(
                "import express from 'express';\nconst router = express.Router();\nexport function handler_{}(req: any, res: any) {{ res.json({{}}); }}\nrouter.get('/route_{}', handler_{});\n",
                i, i, i
            )
        } else {
            format!(
                "import {{ handler_{} }} from './file_{}';\nexport function handler_{}() {{ return handler_{}(); }}\n",
                i / 2, i / 2, i, i / 2
            )
        };
        rb.write_file(&format!("src/file_{}.ts", i), &content);
    }
    rb.commit("100 files");

    let start = std::time::Instant::now();
    let output = run_full_pipeline(&rb, "main", "feature/big");
    let elapsed = start.elapsed();

    assert_eq!(output.summary.total_files_changed, 100);
    assert_all_files_accounted(&output);
    assert_valid_scores(&output);
    assert_json_roundtrip(&output);

    assert!(
        elapsed.as_secs() < 60,
        "100-file analysis should complete in <60s, took {:?}",
        elapsed
    );

    // JSON should be valid and well-formed
    let json = output::to_json(&output).unwrap();
    let _: serde_json::Value = serde_json::from_str(&json).unwrap();
}

// ═══════════════════════════════════════════════════════════════════════════
// 5. Non-UTF8 file content (embedded null bytes, binary content that
//    slipped through the filter)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_files_with_mixed_binary_text() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/mixed");
    rb.checkout("feature/mixed");

    // A valid TS file alongside a problematic one
    rb.write_file(
        "src/good.ts",
        "export function good() { return 'hello'; }\n",
    );
    // "Source" file that contains some code but also some garbage
    rb.write_file(
        "src/weird.ts",
        "function broken() {\n  let x = 42;\n  return x;\n}\n\x00\x01\x02\x03",
    );
    rb.commit("mixed content");

    let output = run_full_pipeline(&rb, "main", "feature/mixed");

    // Should complete without panicking; the good file should definitely be present
    assert!(output.summary.total_files_changed >= 1);
    assert_json_roundtrip(&output);
}

// ═══════════════════════════════════════════════════════════════════════════
// 6. Symlinks in repo
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_repo_with_symlinks() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/symlinks");
    rb.checkout("feature/symlinks");

    // Create a real file
    rb.write_file("src/real.ts", "export function real() { return 42; }\n");

    // Create a symlink to it
    let target = rb.path().join("src/real.ts");
    let link = rb.path().join("src/link.ts");
    // symlinks may not work on all platforms but should not crash
    let symlink_result = std::os::unix::fs::symlink(&target, &link);

    if symlink_result.is_ok() {
        // Also create a symlink to a non-existent file (dangling symlink)
        let dangling = rb.path().join("src/dangling.ts");
        let _ = std::os::unix::fs::symlink("/nonexistent/path.ts", &dangling);

        rb.commit("symlinks");

        let output = run_full_pipeline(&rb, "main", "feature/symlinks");

        // Should not crash. At minimum the real file should be present.
        assert!(output.summary.total_files_changed >= 1);
        assert_json_roundtrip(&output);
    }
    // If symlinks aren't supported, test is a no-op
}

// ═══════════════════════════════════════════════════════════════════════════
// 7. Monorepo with 50+ packages
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_monorepo_50_packages() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/monorepo");
    rb.checkout("feature/monorepo");

    // Create 50 packages, each with a main file and a test file
    for i in 0..50 {
        let pkg = format!("packages/pkg-{:02}", i);
        rb.write_file(
            &format!("{}/src/index.ts", pkg),
            &format!("export function pkg{}() {{ return 'package {}'; }}\n", i, i),
        );
        rb.write_file(
            &format!("{}/src/utils.ts", pkg),
            &format!(
                "import {{ pkg{i} }} from './index';\nexport function util{i}() {{ return pkg{i}(); }}\n",
                i = i
            ),
        );
        rb.write_file(
            &format!("{}/package.json", pkg),
            &format!(r#"{{"name": "@mono/pkg-{:02}", "version": "1.0.0"}}"#, i),
        );
    }

    // Cross-package imports (a few packages depend on earlier ones)
    for i in (10..50).step_by(5) {
        rb.write_file(
            &format!("packages/pkg-{:02}/src/cross.ts", i),
            &format!(
                "import {{ pkg{dep} }} from '../../pkg-{dep:02}/src/index';\nexport function cross{i}() {{ return pkg{dep}(); }}\n",
                i = i,
                dep = i - 5
            ),
        );
    }

    rb.commit("50 packages");

    let start = std::time::Instant::now();
    let output = run_full_pipeline(&rb, "main", "feature/monorepo");
    let elapsed = start.elapsed();

    // Should have 100+ changed files (2 per package + 8 cross files + 50 package.json)
    assert!(
        output.summary.total_files_changed >= 100,
        "expected 100+ changed files, got {}",
        output.summary.total_files_changed
    );
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);
    assert_valid_scores(&output);

    assert!(
        elapsed.as_secs() < 120,
        "50-package monorepo should complete in <120s, took {:?}",
        elapsed
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 8. Repo with only deleted files
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_all_files_deleted() {
    let rb = RepoBuilder::new();
    rb.write_file(
        "src/handler.ts",
        "export function handler() { return 42; }\n",
    );
    rb.write_file(
        "src/service.ts",
        "import { handler } from './handler';\nexport function svc() { return handler(); }\n",
    );
    rb.write_file(
        "src/utils.ts",
        "export function util() { return 'hello'; }\n",
    );
    rb.commit("initial files");
    rb.create_branch("main");

    rb.create_branch("feature/delete-all");
    rb.checkout("feature/delete-all");

    // Delete all source files
    std::fs::remove_file(rb.path().join("src/handler.ts")).unwrap();
    std::fs::remove_file(rb.path().join("src/service.ts")).unwrap();
    std::fs::remove_file(rb.path().join("src/utils.ts")).unwrap();
    rb.commit("delete all");

    let output = run_full_pipeline(&rb, "main", "feature/delete-all");

    // Deleted files have no new_content → nothing to parse → no groups
    // But file accounting should still work (they appear in infra or are simply counted)
    assert_eq!(output.summary.total_files_changed, 3);
    // With no new_content, there are no parsed files for analysis,
    // so all files should be in infrastructure group
    assert_json_roundtrip(&output);
}

// ═══════════════════════════════════════════════════════════════════════════
// 9. Repo with deeply nested directory structure (10+ levels)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_deeply_nested_dirs() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/deep");
    rb.checkout("feature/deep");

    // Create a 15-level deep directory structure
    let mut path = String::from("src");
    for i in 0..15 {
        path = format!("{}/level_{}", path, i);
    }

    rb.write_file(
        &format!("{}/handler.ts", path),
        r#"
import express from 'express';
export function deepHandler(req: any, res: any) {
    res.json({ deep: true });
}
const app = express();
app.get('/deep', deepHandler);
"#,
    );

    // Another file at the root that imports the deep one
    rb.write_file(
        "src/index.ts",
        &format!(
            "import {{ deepHandler }} from './{}/handler';\nexport {{ deepHandler }};\n",
            path.strip_prefix("src/").unwrap_or(&path)
        ),
    );
    rb.commit("deeply nested");

    let output = run_full_pipeline(&rb, "main", "feature/deep");

    assert_eq!(output.summary.total_files_changed, 2);
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);
}

// ═══════════════════════════════════════════════════════════════════════════
// 10. Repo with file renames
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_file_renames_with_content_change() {
    let rb = RepoBuilder::new();
    rb.write_file(
        "src/old_name.ts",
        "export function handler() { return 'v1'; }\n",
    );
    rb.write_file(
        "src/caller.ts",
        "import { handler } from './old_name';\nexport function call() { return handler(); }\n",
    );
    rb.commit("initial");
    rb.create_branch("main");

    rb.create_branch("feature/rename");
    rb.checkout("feature/rename");

    // Rename + modify
    std::fs::rename(
        rb.path().join("src/old_name.ts"),
        rb.path().join("src/new_name.ts"),
    )
    .unwrap();
    rb.write_file(
        "src/new_name.ts",
        "export function handler() { return 'v2 - renamed!'; }\n",
    );
    rb.write_file(
        "src/caller.ts",
        "import { handler } from './new_name';\nexport function call() { return handler(); }\n",
    );
    rb.commit("rename + modify");

    let output = run_full_pipeline(&rb, "main", "feature/rename");

    // Should have at least 2 changed files (the rename + the caller update)
    assert!(output.summary.total_files_changed >= 2);
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);
}

// ═══════════════════════════════════════════════════════════════════════════
// 11. Repo with unicode file paths and content
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_unicode_paths_and_content() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/unicode");
    rb.checkout("feature/unicode");

    // Unicode in paths
    rb.write_file(
        "src/日本語/handler.ts",
        r#"
export function こんにちは() {
    return 'Hello from 日本語 handler 🎉';
}
"#,
    );
    rb.write_file(
        "src/café/utils.ts",
        r#"
export function résumé() {
    return 'Héllo wörld 🌍';
}
"#,
    );
    // File with emoji in path
    rb.write_file(
        "src/🚀/launch.ts",
        "export function launch() { return '🚀 launching!'; }\n",
    );
    rb.commit("unicode paths and content");

    let output = run_full_pipeline(&rb, "main", "feature/unicode");

    assert!(output.summary.total_files_changed >= 3);
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);
}

// ═══════════════════════════════════════════════════════════════════════════
// 12. Repo with very large single file
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_single_very_large_file() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/large-file");
    rb.checkout("feature/large-file");

    // Generate a large TypeScript file with 2000 functions
    let mut source = String::with_capacity(200_000);
    source.push_str("import express from 'express';\nconst app = express();\n\n");
    for i in 0..2000 {
        source.push_str(&format!(
            "export function handler_{}(req: any, res: any) {{ res.json({{ id: {} }}); }}\n",
            i, i
        ));
        if i < 100 {
            source.push_str(&format!("app.get('/route_{}', handler_{});\n", i, i));
        }
    }
    rb.write_file("src/mega.ts", &source);
    rb.commit("large file");

    let start = std::time::Instant::now();
    let output = run_full_pipeline(&rb, "main", "feature/large-file");
    let elapsed = start.elapsed();

    assert_eq!(output.summary.total_files_changed, 1);
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);

    assert!(
        elapsed.as_secs() < 30,
        "single large file should complete in <30s, took {:?}",
        elapsed
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 13. IR pipeline parity on adversarial inputs
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_ir_pipeline_circular_imports() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/ir-circular");
    rb.checkout("feature/ir-circular");

    rb.write_file(
        "src/a.ts",
        "import { b } from './b';\nexport function a() { return b(); }\n",
    );
    rb.write_file(
        "src/b.ts",
        "import { a } from './a';\nexport function b() { return a(); }\n",
    );
    rb.commit("circular");

    // Both pipeline paths should complete without infinite loops
    let output_ast = run_full_pipeline(&rb, "main", "feature/ir-circular");
    let output_ir = run_ir_pipeline(&rb, "main", "feature/ir-circular");

    assert_eq!(
        output_ast.summary.total_files_changed,
        output_ir.summary.total_files_changed
    );
    assert_json_roundtrip(&output_ir);
}

#[test]
fn adversarial_ir_pipeline_monorepo_scale() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/ir-scale");
    rb.checkout("feature/ir-scale");

    // 30 files with mixed languages
    for i in 0..15 {
        rb.write_file(
            &format!("src/ts/mod_{}.ts", i),
            &format!("export function ts_fn_{}() {{ return {}; }}\n", i, i),
        );
        rb.write_file(
            &format!("src/py/mod_{}.py", i),
            &format!("def py_fn_{}():\n    return {}\n", i, i),
        );
    }
    rb.commit("mixed lang scale");

    let output_ir = run_ir_pipeline(&rb, "main", "feature/ir-scale");

    assert_eq!(output_ir.summary.total_files_changed, 30);
    assert_all_files_accounted(&output_ir);
    assert_json_roundtrip(&output_ir);
}

// ═══════════════════════════════════════════════════════════════════════════
// 14. Empty diff — identical branches
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_identical_branches() {
    let rb = RepoBuilder::new();
    rb.write_file("src/app.ts", "export function app() {}\n");
    rb.commit("init");
    rb.create_branch("main");
    rb.create_branch("feature/noop");
    rb.checkout("feature/noop");
    // No changes on the feature branch
    rb.commit("empty commit");

    // The branches have no diff between them
    let output = run_full_pipeline(&rb, "main", "feature/noop");

    assert_eq!(output.summary.total_files_changed, 0);
    assert_eq!(output.summary.total_groups, 0);
    assert!(output.groups.is_empty());
    assert!(output.infrastructure_group.is_none());
    assert_json_roundtrip(&output);
}

// ═══════════════════════════════════════════════════════════════════════════
// 15. Repo with only config/non-code files
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_config_only_changes() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/config");
    rb.checkout("feature/config");

    rb.write_file("tsconfig.json", r#"{"compilerOptions": {"strict": true}}"#);
    rb.write_file("package.json", r#"{"name": "test", "version": "2.0.0"}"#);
    rb.write_file(".eslintrc.json", r#"{"rules": {"no-console": "error"}}"#);
    rb.write_file(".gitignore", "node_modules/\ndist/\n");
    rb.write_file("Makefile", "build:\n\tnpm run build\n");
    rb.write_file("Dockerfile", "FROM node:18\nCOPY . .\n");
    rb.commit("config changes only");

    let output = run_full_pipeline(&rb, "main", "feature/config");

    // All files should be in infrastructure group (no entrypoints detected)
    assert!(output.summary.total_files_changed >= 6);
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);

    // No TypeScript/Python entrypoints → all files in infra
    if output.groups.is_empty() {
        assert!(
            output.infrastructure_group.is_some(),
            "with no entrypoints, files should be in infrastructure group"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 16. Repo with mixed Python + TypeScript circular dependency
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_cross_language_dependency_chain() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/cross-lang");
    rb.checkout("feature/cross-lang");

    // TypeScript frontend calling Python backend (realistic microservice pattern)
    rb.write_file(
        "frontend/src/api.ts",
        r#"
export async function fetchUsers() {
    const response = await fetch('/api/users');
    return response.json();
}
export async function createUser(name: string) {
    const response = await fetch('/api/users', {
        method: 'POST',
        body: JSON.stringify({ name }),
    });
    return response.json();
}
"#,
    );
    rb.write_file(
        "frontend/src/UserPage.tsx",
        r#"
import { fetchUsers, createUser } from './api';
export default function UserPage() {
    const users = fetchUsers();
    return users;
}
"#,
    );
    rb.write_file(
        "backend/app/routes.py",
        r#"
from fastapi import APIRouter
from backend.app.services import user_service

router = APIRouter()

@router.get("/api/users")
def list_users():
    return user_service.get_all()

@router.post("/api/users")
def create_user(data: dict):
    return user_service.create(data)
"#,
    );
    rb.write_file(
        "backend/app/services.py",
        r#"
from backend.app.db import db

class UserService:
    def get_all(self):
        return db.query("SELECT * FROM users")

    def create(self, data):
        return db.insert("users", data)

user_service = UserService()
"#,
    );
    rb.write_file(
        "backend/app/db.py",
        r#"
class Database:
    def query(self, sql):
        return []
    def insert(self, table, data):
        return data

db = Database()
"#,
    );
    rb.commit("cross-language microservice");

    let output = run_full_pipeline(&rb, "main", "feature/cross-lang");

    assert_eq!(output.summary.total_files_changed, 5);
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);
    assert_valid_scores(&output);

    // Should detect both languages
    let langs = &output.summary.languages_detected;
    assert!(
        langs.contains(&"typescript".to_string()),
        "should detect TypeScript, got {:?}",
        langs
    );
    assert!(
        langs.contains(&"python".to_string()),
        "should detect Python, got {:?}",
        langs
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 17. Many entrypoints in one file (stress test for entrypoint detection)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_many_entrypoints_single_file() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/many-eps");
    rb.checkout("feature/many-eps");

    // A single file with 50 route handlers
    let mut source = String::from("import express from 'express';\nconst app = express();\n\n");
    for i in 0..50 {
        source.push_str(&format!(
            "app.{}('/route_{}', (req: any, res: any) => {{ res.json({{ id: {} }}); }});\n",
            if i % 4 == 0 {
                "get"
            } else if i % 4 == 1 {
                "post"
            } else if i % 4 == 2 {
                "put"
            } else {
                "delete"
            },
            i,
            i
        ));
    }
    rb.write_file("src/routes.ts", &source);
    rb.commit("many routes");

    let output = run_full_pipeline(&rb, "main", "feature/many-eps");

    assert_eq!(output.summary.total_files_changed, 1);
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);
}

// ═══════════════════════════════════════════════════════════════════════════
// 18. Empty files and whitespace-only files
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_empty_and_whitespace_files() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/empty");
    rb.checkout("feature/empty");

    rb.write_file("src/empty.ts", "");
    rb.write_file("src/whitespace.ts", "   \n\n\t\t\n   \n");
    rb.write_file(
        "src/comments_only.ts",
        "// just comments\n/* block comment */\n",
    );
    rb.write_file("src/real.ts", "export function real() { return 42; }\n");
    rb.commit("empty and whitespace files");

    let output = run_full_pipeline(&rb, "main", "feature/empty");

    assert!(output.summary.total_files_changed >= 4);
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);
}

// ═══════════════════════════════════════════════════════════════════════════
// 19. JSON output roundtrip stability on all adversarial outputs
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_json_schema_validity_stress() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/schema-stress");
    rb.checkout("feature/schema-stress");

    // Mix: Express routes, Python FastAPI, test files, config, deep nesting
    rb.write_file(
        "src/routes/api.ts",
        r#"
import express from 'express';
import { processOrder } from '../services/orderService';
const router = express.Router();
router.post('/orders', (req: any, res: any) => {
    const result = processOrder(req.body);
    res.json(result);
});
export default router;
"#,
    );
    rb.write_file(
        "src/services/orderService.ts",
        "export function processOrder(data: any) { return { ...data, id: 1 }; }\n",
    );
    rb.write_file(
        "backend/views.py",
        r#"
from fastapi import APIRouter
router = APIRouter()

@router.get("/health")
def health():
    return {"ok": True}
"#,
    );
    rb.write_file(
        "src/__tests__/order.test.ts",
        r#"
import { processOrder } from '../services/orderService';
describe('processOrder', () => {
    it('should create order', () => {
        expect(processOrder({name: 'test'})).toBeDefined();
    });
});
"#,
    );
    rb.write_file("tsconfig.json", r#"{"compilerOptions": {"strict": true}}"#);
    rb.write_file(
        "deep/a/b/c/d/e/f/leaf.ts",
        "export const DEEP_VALUE = 42;\n",
    );
    rb.commit("schema stress test");

    let output = run_full_pipeline(&rb, "main", "feature/schema-stress");

    assert_valid_json_schema(&output);
    assert_json_roundtrip(&output);
    assert_all_files_accounted(&output);
    assert_valid_scores(&output);

    // Double-check JSON is parseable
    let json = output::to_json(&output).unwrap();
    let parsed: AnalysisOutput = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.version, "1.0.0");
    assert_eq!(
        parsed.summary.total_files_changed,
        output.summary.total_files_changed
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 20. Determinism across both pipeline paths
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_determinism_across_runs() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/det");
    rb.checkout("feature/det");

    for i in 0..20 {
        rb.write_file(
            &format!("src/mod_{}.ts", i),
            &format!("export function fn_{}() {{ return {}; }}\n", i, i),
        );
    }
    // Add some imports to create graph edges
    rb.write_file(
        "src/index.ts",
        &(0..20)
            .map(|i| format!("import {{ fn_{i} }} from './mod_{i}';\n", i = i))
            .chain(std::iter::once(
                "export function main() { return [".to_string()
                    + &(0..20)
                        .map(|i| format!("fn_{}()", i))
                        .collect::<Vec<_>>()
                        .join(", ")
                    + "]; }\n",
            ))
            .collect::<String>(),
    );
    rb.commit("determinism test");

    let output1 = run_full_pipeline(&rb, "main", "feature/det");
    let output2 = run_full_pipeline(&rb, "main", "feature/det");

    let json1 = output::to_json(&output1).unwrap();
    let json2 = output::to_json(&output2).unwrap();
    assert_eq!(json1, json2, "two runs should produce identical JSON");
}

// ═══════════════════════════════════════════════════════════════════════════
// 21. Merge-base diff on adversarial repo topology
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_merge_base_diverged_branches() {
    let rb = RepoBuilder::new();
    rb.write_file("src/shared.ts", "export const V = 1;\n");
    rb.commit("init");
    rb.create_branch("main");

    // Feature branch diverges from main
    rb.create_branch("feature/diverge");
    rb.checkout("feature/diverge");
    rb.write_file("src/feature.ts", "export function feat() { return 42; }\n");
    rb.commit("feature work");

    // Meanwhile, main also advances
    rb.checkout("main");
    rb.write_file("src/shared.ts", "export const V = 2;\n");
    rb.write_file("src/mainonly.ts", "export function mainFn() {}\n");
    rb.commit("main advances");

    // Now test merge-base diff
    let repo = git2::Repository::open(rb.path()).unwrap();
    let merge_base_result = git::diff_merge_base(&repo, "main", "feature/diverge");
    assert!(
        merge_base_result.is_ok(),
        "merge-base diff should work on diverged branches"
    );
    let diff_result = merge_base_result.unwrap();
    // Only the feature branch changes should appear (not main's changes)
    let paths: Vec<&str> = diff_result.files.iter().map(|f| f.path()).collect();
    assert!(
        paths.contains(&"src/feature.ts"),
        "merge-base diff should include feature file"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 22. Repo with special characters in file paths
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_special_chars_in_paths() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/special-paths");
    rb.checkout("feature/special-paths");

    rb.write_file(
        "src/file with spaces.ts",
        "export function spaced() { return 1; }\n",
    );
    rb.write_file(
        "src/file-with-dashes.ts",
        "export function dashed() { return 2; }\n",
    );
    rb.write_file(
        "src/file_with_underscores.ts",
        "export function underscored() { return 3; }\n",
    );
    rb.write_file(
        "src/file.multiple.dots.ts",
        "export function dotted() { return 4; }\n",
    );
    rb.write_file(
        "src/(parentheses).ts",
        "export function parens() { return 5; }\n",
    );
    rb.write_file(
        "src/[brackets].ts",
        "export function brackets() { return 6; }\n",
    );
    rb.commit("special chars in paths");

    let output = run_full_pipeline(&rb, "main", "feature/special-paths");

    assert!(output.summary.total_files_changed >= 6);
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);
}

// ═══════════════════════════════════════════════════════════════════════════
// 23. Star/hub topology — one file imports everything
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_star_topology() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/star");
    rb.checkout("feature/star");

    // 30 leaf files
    for i in 0..30 {
        rb.write_file(
            &format!("src/leaf_{}.ts", i),
            &format!("export function leaf_{}() {{ return {}; }}\n", i, i),
        );
    }

    // One hub file that imports all leaves
    let hub_source: String = (0..30)
        .map(|i| format!("import {{ leaf_{i} }} from './leaf_{i}';\n", i = i))
        .chain(std::iter::once(format!(
            "export function hub() {{ return [{}]; }}\n",
            (0..30)
                .map(|i| format!("leaf_{}()", i))
                .collect::<Vec<_>>()
                .join(", ")
        )))
        .collect();
    rb.write_file("src/hub.ts", &hub_source);
    rb.commit("star topology");

    let output = run_full_pipeline(&rb, "main", "feature/star");

    assert_eq!(output.summary.total_files_changed, 31);
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);
    assert_valid_scores(&output);
}

// ═══════════════════════════════════════════════════════════════════════════
// 24. Diamond dependency — shared dependency with two paths
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_diamond_dependency() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/diamond");
    rb.checkout("feature/diamond");

    // Diamond: route → serviceA → shared, route → serviceB → shared
    rb.write_file(
        "src/route.ts",
        r#"
import express from 'express';
import { serviceA } from './serviceA';
import { serviceB } from './serviceB';

const router = express.Router();
export function handle(req: any, res: any) {
    const a = serviceA();
    const b = serviceB();
    res.json({ a, b });
}
router.get('/diamond', handle);
"#,
    );
    rb.write_file(
        "src/serviceA.ts",
        "import { shared } from './shared';\nexport function serviceA() { return shared() + 'A'; }\n",
    );
    rb.write_file(
        "src/serviceB.ts",
        "import { shared } from './shared';\nexport function serviceB() { return shared() + 'B'; }\n",
    );
    rb.write_file(
        "src/shared.ts",
        "export function shared() { return 'base'; }\n",
    );
    rb.commit("diamond dependency");

    let output = run_full_pipeline(&rb, "main", "feature/diamond");

    assert_eq!(output.summary.total_files_changed, 4);
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);
    assert_valid_scores(&output);
}

// ═══════════════════════════════════════════════════════════════════════════
// 25. Adversarial: everything at once (kitchen sink)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_kitchen_sink() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/kitchen-sink");
    rb.checkout("feature/kitchen-sink");

    // Express routes
    rb.write_file(
        "src/routes/api.ts",
        r#"
import express from 'express';
import { processOrder } from '../services/orderService';
const router = express.Router();
router.post('/orders', (req: any, res: any) => {
    const result = processOrder(req.body);
    res.json(result);
});
export default router;
"#,
    );

    // Python FastAPI in the same repo
    rb.write_file(
        "backend/views.py",
        r#"
from fastapi import APIRouter
router = APIRouter()

@router.get("/health")
def health():
    return {"ok": True}
"#,
    );

    // Circular imports
    rb.write_file(
        "src/services/orderService.ts",
        "import { validate } from './validator';\nexport function processOrder(d: any) { validate(d); return d; }\n",
    );
    rb.write_file(
        "src/services/validator.ts",
        "import { processOrder } from './orderService';\nexport function validate(d: any) { return !!d; }\n",
    );

    // Deeply nested
    rb.write_file("deep/a/b/c/d/e/leaf.ts", "export const DEEP = 42;\n");

    // Empty and whitespace files
    rb.write_file("src/empty.ts", "");
    rb.write_file("src/whitespace.ts", "   \n\n\t\n");

    // Config files
    rb.write_file("tsconfig.json", r#"{"compilerOptions": {"strict": true}}"#);
    rb.write_file("package.json", r#"{"name": "kitchen-sink"}"#);

    // Test file
    rb.write_file(
        "src/__tests__/order.test.ts",
        "import { processOrder } from '../services/orderService';\ndescribe('test', () => { it('works', () => {}); });\n",
    );

    // Unicode
    rb.write_file(
        "src/i18n/日本語.ts",
        "export const greeting = 'こんにちは';\n",
    );

    rb.commit("kitchen sink");

    let output = run_full_pipeline(&rb, "main", "feature/kitchen-sink");

    assert!(
        output.summary.total_files_changed >= 10,
        "expected 10+ files, got {}",
        output.summary.total_files_changed
    );
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);
    assert_valid_json_schema(&output);
    assert_valid_scores(&output);

    // Both languages should be detected
    let langs = &output.summary.languages_detected;
    assert!(langs.contains(&"typescript".to_string()));
    assert!(langs.contains(&"python".to_string()));

    // JSON roundtrip should be stable
    let json1 = output::to_json(&output).unwrap();
    let parsed: AnalysisOutput = serde_json::from_str(&json1).unwrap();
    let json2 = output::to_json(&parsed).unwrap();
    assert_eq!(json1, json2);
}

// ═══════════════════════════════════════════════════════════════════════════
// 26. Barrel file explosion — index.ts re-exporting 50+ modules
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_barrel_file_explosion() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/barrel");
    rb.checkout("feature/barrel");

    // 50 utility modules
    for i in 0..50 {
        rb.write_file(
            &format!("src/utils/util_{}.ts", i),
            &format!("export function util_{}() {{ return {}; }}\n", i, i),
        );
    }

    // Barrel file re-exporting all 50
    let barrel: String = (0..50)
        .map(|i| format!("export {{ util_{i} }} from './util_{i}';\n", i = i))
        .collect();
    rb.write_file("src/utils/index.ts", &barrel);

    // Entrypoint that imports from the barrel
    rb.write_file(
        "src/route.ts",
        r#"
import express from 'express';
import { util_0, util_25, util_49 } from './utils';

const router = express.Router();
export function handler(req: any, res: any) {
    res.json({ a: util_0(), b: util_25(), c: util_49() });
}
router.get('/barrel', handler);
"#,
    );
    rb.commit("barrel file explosion");

    let output = run_full_pipeline(&rb, "main", "feature/barrel");

    // 50 utils + 1 barrel + 1 route = 52 files
    assert_eq!(output.summary.total_files_changed, 52);
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);
    assert_valid_scores(&output);
    // The barrel file should not distort grouping — all files should end up
    // in groups, not be silently dropped.
    let total_in_groups: usize = output.groups.iter().map(|g| g.files.len()).sum::<usize>()
        + output
            .infrastructure_group
            .as_ref()
            .map(|ig| ig.files.len())
            .unwrap_or(0);
    assert_eq!(total_in_groups, 52);
}

// ═══════════════════════════════════════════════════════════════════════════
// 27. Re-export chains — A re-exports B which re-exports C
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_reexport_chains() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/reexport");
    rb.checkout("feature/reexport");

    // Chain: index.ts → services/index.ts → services/auth.ts → services/db.ts
    rb.write_file(
        "src/services/db.ts",
        "export function query(sql: string) { return sql; }\n",
    );
    rb.write_file(
        "src/services/auth.ts",
        "import { query } from './db';\nexport function authenticate(token: string) { return query('SELECT * FROM users WHERE token=' + token); }\nexport { query } from './db';\n",
    );
    rb.write_file(
        "src/services/index.ts",
        "export { authenticate, query } from './auth';\n",
    );
    rb.write_file(
        "src/index.ts",
        "export { authenticate, query } from './services';\n",
    );

    // Entrypoint that uses the top-level re-export
    rb.write_file(
        "src/route.ts",
        r#"
import express from 'express';
import { authenticate } from './index';

const router = express.Router();
export function handler(req: any, res: any) {
    const result = authenticate(req.headers.authorization);
    res.json({ result });
}
router.post('/auth', handler);
"#,
    );
    rb.commit("re-export chains");

    let output = run_full_pipeline(&rb, "main", "feature/reexport");

    assert_eq!(output.summary.total_files_changed, 5);
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);
    assert_valid_scores(&output);
}

// ═══════════════════════════════════════════════════════════════════════════
// 28. Deeply nested transitive deps — 10+ levels of import chains
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_deeply_nested_transitive_deps() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/deep-chain");
    rb.checkout("feature/deep-chain");

    let depth = 12;

    // Create a chain: layer_0 → layer_1 → ... → layer_11
    for i in (0..depth).rev() {
        let content = if i == depth - 1 {
            // Leaf — no imports
            format!("export function layer_{}() {{ return 'leaf'; }}\n", i)
        } else {
            // Imports the next layer
            format!(
                "import {{ layer_{next} }} from './layer_{next}';\nexport function layer_{i}() {{ return layer_{next}() + '_{i}'; }}\n",
                i = i,
                next = i + 1
            )
        };
        rb.write_file(&format!("src/layer_{}.ts", i), &content);
    }

    // Entrypoint imports layer_0
    rb.write_file(
        "src/route.ts",
        r#"
import express from 'express';
import { layer_0 } from './layer_0';

const router = express.Router();
export function handler(req: any, res: any) {
    res.json({ result: layer_0() });
}
router.get('/deep', handler);
"#,
    );
    rb.commit("deeply nested transitive deps");

    let output = run_full_pipeline(&rb, "main", "feature/deep-chain");

    // 12 layers + 1 route = 13 files
    assert_eq!(output.summary.total_files_changed, 13);
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);
    assert_valid_scores(&output);

    // All files should be in groups (reachable from the entrypoint via transitive chain)
    let grouped_count: usize = output.groups.iter().map(|g| g.files.len()).sum();
    let infra_count = output
        .infrastructure_group
        .as_ref()
        .map(|ig| ig.files.len())
        .unwrap_or(0);
    assert_eq!(grouped_count + infra_count, 13);
}

// ═══════════════════════════════════════════════════════════════════════════
// 29. Orphan clusters — connected files with no entrypoint reachability
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn adversarial_orphan_clusters() {
    let rb = RepoBuilder::new();
    rb.write_file("init.txt", "initial");
    rb.commit("init");
    rb.create_branch("main");

    rb.create_branch("feature/orphans");
    rb.checkout("feature/orphans");

    // Cluster 1: connected internally but no entrypoint
    rb.write_file(
        "src/helpers/format.ts",
        "import { validate } from './validate';\nexport function format(s: string) { return validate(s) ? s.trim() : s; }\n",
    );
    rb.write_file(
        "src/helpers/validate.ts",
        "export function validate(s: string) { return s.length > 0; }\n",
    );

    // Cluster 2: another connected group, also no entrypoint
    rb.write_file(
        "src/lib/parser.ts",
        "import { tokenize } from './tokenizer';\nexport function parse(input: string) { return tokenize(input); }\n",
    );
    rb.write_file(
        "src/lib/tokenizer.ts",
        "export function tokenize(input: string) { return input.split(' '); }\n",
    );

    // One real entrypoint with its own chain
    rb.write_file(
        "src/route.ts",
        r#"
import express from 'express';
import { doWork } from './worker';

const router = express.Router();
export function handler(req: any, res: any) {
    res.json({ result: doWork() });
}
router.get('/work', handler);
"#,
    );
    rb.write_file(
        "src/worker.ts",
        "export function doWork() { return 'done'; }\n",
    );
    rb.commit("orphan clusters");

    let output = run_full_pipeline(&rb, "main", "feature/orphans");

    assert_eq!(output.summary.total_files_changed, 6);
    assert_all_files_accounted(&output);
    assert_json_roundtrip(&output);
    assert_valid_scores(&output);

    // The entrypoint chain (route + worker) should form a group.
    // The orphan clusters (helpers/format+validate, lib/parser+tokenizer)
    // should end up in infrastructure since they have no entrypoint.
    let infra = output.infrastructure_group.as_ref();
    assert!(
        infra.is_some(),
        "Orphan clusters should form an infrastructure group"
    );
    let infra_files = &infra.unwrap().files;
    // At minimum, the orphan files should be in infrastructure
    assert!(
        infra_files.len() >= 2,
        "Expected at least 2 orphan files in infrastructure, got {}",
        infra_files.len()
    );
}
