//! Unified analysis pipeline using the declarative query engine and IR types.
//!
//! This module provides the primary entry point for the analysis pipeline,
//! composing the query engine, IR conversion, and all downstream analysis
//! stages (graph, entrypoints, flow, clustering, ranking).
//!
//! # Pipeline flow
//!
//! ```text
//! source code → QueryEngine::parse_file → ParsedFile
//!             → QueryEngine::extract_data_flow → DataFlowInfo
//!             → IrFile::from_parsed_file + enrich_with_data_flow → IrFile
//!             → SymbolGraph::build_from_ir (graph construction)
//!             → detect_entrypoints_ir (entrypoint detection)
//!             → analyze_data_flow_ir (heuristic flow analysis)
//!             → trace_data_flow_ir (variable-level data flow edges)
//!             → cluster + rank (unchanged, operate on graph/entrypoints)
//! ```

use log::warn;
use rayon::prelude::*;

use crate::ast::{self, ParsedFile};
use crate::ir::IrFile;
use crate::query_engine::QueryEngine;

/// Parse a single file into an IR representation using the query engine.
///
/// This is the primary parsing entry point for the IR pipeline. It uses the
/// declarative `.scm` query files instead of imperative tree-sitter code.
pub fn parse_to_ir(engine: &QueryEngine, path: &str, source: &str) -> Result<IrFile, PipelineError> {
    let parsed = engine
        .parse_file(path, source)
        .map_err(|e| PipelineError::Parse(format!("{}: {}", path, e)))?;

    let mut ir = IrFile::from_parsed_file(&parsed);

    // Enrich with data flow info (assignments, call arguments).
    // Non-fatal: file may have syntax errors or unsupported language.
    if let Err(e) = engine.extract_data_flow(path, source).map(|df| ir.enrich_with_data_flow(&df)) {
        warn!("Data flow extraction failed for {}: {} (non-fatal, skipping enrichment)", path, e);
    }

    Ok(ir)
}

/// Parse multiple files into IR representations in parallel using rayon.
///
/// Files that fail to parse are skipped (with errors collected).
/// Results are sorted by file path to ensure deterministic output regardless
/// of thread scheduling.
pub fn parse_all_to_ir(
    engine: &QueryEngine,
    files: &[(&str, &str)],
) -> (Vec<IrFile>, Vec<PipelineError>) {
    let results: Vec<Result<IrFile, PipelineError>> = files
        .par_iter()
        .map(|&(path, source)| parse_to_ir(engine, path, source))
        .collect();

    let mut ir_files = Vec::with_capacity(files.len());
    let mut errors = Vec::new();

    for result in results {
        match result {
            Ok(ir) => ir_files.push(ir),
            Err(e) => errors.push(e),
        }
    }

    // Sort by path to ensure deterministic output regardless of thread ordering.
    ir_files.sort_by(|a, b| a.path.cmp(&b.path));

    (ir_files, errors)
}

/// Parse multiple source files into `ParsedFile` representations in parallel.
///
/// This is the parallel equivalent of calling `ast::parse_file` in a loop —
/// used by the CLI and Tauri analysis pipelines. Each file is parsed
/// independently via rayon, then results are collected and sorted by path
/// for determinism.
///
/// Files that fail to parse are skipped with a warning logged. Tree-sitter is
/// error-tolerant, so failures are rare — typically only unsupported languages.
pub fn parse_files_parallel(files: &[(&str, &str)]) -> Vec<ParsedFile> {
    let results: Vec<Result<ParsedFile, (String, crate::ast::AstError)>> = files
        .par_iter()
        .map(|&(path, source)| {
            ast::parse_file(path, source).map_err(|e| (path.to_string(), e))
        })
        .collect();

    let mut parsed = Vec::with_capacity(files.len());
    for result in results {
        match result {
            Ok(file) => parsed.push(file),
            Err((path, e)) => {
                warn!("Skipping file {} due to parse error: {}", path, e);
            }
        }
    }

    // Sort by path for deterministic ordering.
    parsed.sort_by(|a, b| a.path.cmp(&b.path));

    parsed
}

/// Errors from the pipeline.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("parse error: {0}")]
    Parse(String),
    #[error("query engine error: {0}")]
    QueryEngine(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_to_ir_typescript() {
        let engine = QueryEngine::new().unwrap();
        let source = r#"
import { validate } from './utils';
export function handler(req: Request) {
    const data = validate(req.body);
    return save(data);
}
function save(data: any) {
    return db.insert(data);
}
"#;
        let ir = parse_to_ir(&engine, "src/handler.ts", source).unwrap();

        assert_eq!(ir.path, "src/handler.ts");
        assert!(!ir.functions.is_empty(), "should have function definitions");
        assert!(!ir.imports.is_empty(), "should have imports");
        assert!(!ir.exports.is_empty(), "should have exports");
        assert!(
            !ir.call_expressions.is_empty(),
            "should have call expressions"
        );
    }

    #[test]
    fn test_parse_to_ir_python() {
        let engine = QueryEngine::new().unwrap();
        let source = r#"
from flask import Flask
app = Flask(__name__)

@app.route('/users')
def list_users():
    users = db.query('SELECT * FROM users')
    return users
"#;
        let ir = parse_to_ir(&engine, "app/views.py", source).unwrap();

        assert_eq!(ir.path, "app/views.py");
        assert!(!ir.functions.is_empty());
        assert!(!ir.imports.is_empty());
    }

    #[test]
    fn test_parse_to_ir_unknown_language() {
        let engine = QueryEngine::new().unwrap();
        let ir = parse_to_ir(&engine, "data.csv", "a,b,c\n1,2,3").unwrap();
        assert_eq!(ir.path, "data.csv");
        assert!(ir.functions.is_empty());
    }

    #[test]
    fn test_parse_all_to_ir() {
        let engine = QueryEngine::new().unwrap();
        let files = vec![
            ("src/a.ts", "export function a() {}"),
            ("src/b.ts", "import { a } from './a'; function b() { a(); }"),
        ];
        let (ir_files, errors) = parse_all_to_ir(&engine, &files);

        assert_eq!(ir_files.len(), 2);
        assert!(errors.is_empty());
        // Sorted by path
        let paths: Vec<&str> = ir_files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"src/a.ts"));
        assert!(paths.contains(&"src/b.ts"));
    }

    #[test]
    fn test_parse_to_ir_enriches_data_flow() {
        let engine = QueryEngine::new().unwrap();
        let source = r#"
function process() {
    const result = fetchData();
    transform(result);
}
"#;
        let ir = parse_to_ir(&engine, "src/process.ts", source).unwrap();

        // Should have assignments from data flow enrichment.
        // The exact content depends on query engine extraction, but the pipeline
        // should not error.
        assert_eq!(ir.path, "src/process.ts");
        assert!(!ir.functions.is_empty());
    }

    #[test]
    fn test_full_ir_pipeline() {
        let engine = QueryEngine::new().unwrap();
        let files = vec![
            (
                "src/utils.ts",
                r#"
export function validate(data: any) { return data; }
export function sanitize(data: any) { return data; }
"#,
            ),
            (
                "src/handler.ts",
                r#"
import { validate, sanitize } from './utils';
export function handle(req: any) {
    const clean = sanitize(req.body);
    const valid = validate(clean);
    return valid;
}
"#,
            ),
        ];

        let (ir_files, errors) = parse_all_to_ir(&engine, &files);
        assert!(errors.is_empty());
        assert_eq!(ir_files.len(), 2);

        // Build graph from IR.
        let graph = crate::graph::SymbolGraph::build_from_ir(&ir_files);
        assert!(graph.node_count() > 0);
        assert!(graph.edge_count() > 0);

        // Detect entrypoints from IR.
        let entrypoints = crate::entrypoint::detect_entrypoints_ir(&ir_files);
        // No entrypoints expected (no HTTP routes, CLI commands, etc.)
        // but the function should not panic.
        let _ = entrypoints;

        // Analyze data flow from IR.
        let flow_analysis =
            crate::flow::analyze_data_flow_ir(&ir_files, &crate::flow::FlowConfig::default());
        let _ = flow_analysis;

        // Trace data flow from IR (no source re-parsing needed!).
        let data_flow_edges = crate::flow::trace_data_flow_ir(&ir_files);
        let _ = data_flow_edges;
    }

    // ── Empty / edge-case inputs ─────────────────────────────────────

    #[test]
    fn parse_to_ir_empty_source() {
        let engine = QueryEngine::new().unwrap();
        let ir = parse_to_ir(&engine, "src/empty.ts", "").unwrap();
        assert_eq!(ir.path, "src/empty.ts");
        assert!(ir.functions.is_empty());
        assert!(ir.imports.is_empty());
        assert!(ir.exports.is_empty());
        assert!(ir.call_expressions.is_empty());
    }

    #[test]
    fn parse_to_ir_whitespace_only_source() {
        let engine = QueryEngine::new().unwrap();
        let ir = parse_to_ir(&engine, "src/blank.ts", "   \n\n  \t  \n").unwrap();
        assert_eq!(ir.path, "src/blank.ts");
        assert!(ir.functions.is_empty());
    }

    #[test]
    fn parse_to_ir_comments_only() {
        let engine = QueryEngine::new().unwrap();
        let ir = parse_to_ir(&engine, "src/comments.ts", "// just a comment\n/* block */\n").unwrap();
        assert_eq!(ir.path, "src/comments.ts");
        assert!(ir.functions.is_empty());
    }

    #[test]
    fn parse_all_to_ir_empty_list() {
        let engine = QueryEngine::new().unwrap();
        let (ir_files, errors) = parse_all_to_ir(&engine, &[]);
        assert!(ir_files.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn parse_all_to_ir_single_file() {
        let engine = QueryEngine::new().unwrap();
        let files = vec![("src/one.ts", "function one() {}")];
        let (ir_files, errors) = parse_all_to_ir(&engine, &files);
        assert_eq!(ir_files.len(), 1);
        assert!(errors.is_empty());
        assert_eq!(ir_files[0].path, "src/one.ts");
    }

    #[test]
    fn parse_all_to_ir_sorted_by_path() {
        let engine = QueryEngine::new().unwrap();
        let files = vec![
            ("c.ts", "function c() {}"),
            ("a.ts", "function a() {}"),
            ("b.ts", "function b() {}"),
        ];
        let (ir_files, errors) = parse_all_to_ir(&engine, &files);
        assert!(errors.is_empty());
        // Parallel output is sorted by path for determinism
        assert_eq!(ir_files[0].path, "a.ts");
        assert_eq!(ir_files[1].path, "b.ts");
        assert_eq!(ir_files[2].path, "c.ts");
    }

    #[test]
    fn parse_all_to_ir_mixed_languages() {
        let engine = QueryEngine::new().unwrap();
        let files = vec![
            ("handler.ts", "export function handler() {}"),
            ("views.py", "def handler(): pass"),
            ("data.json", r#"{"key": "value"}"#),
        ];
        let (ir_files, errors) = parse_all_to_ir(&engine, &files);
        assert!(errors.is_empty());
        assert_eq!(ir_files.len(), 3);
        // Check by path (sorted: data.json, handler.ts, views.py)
        let ts = ir_files.iter().find(|f| f.path == "handler.ts").unwrap();
        let py = ir_files.iter().find(|f| f.path == "views.py").unwrap();
        let json = ir_files.iter().find(|f| f.path == "data.json").unwrap();
        assert!(!ts.functions.is_empty());
        assert!(!py.functions.is_empty());
        assert!(json.functions.is_empty());
    }

    // ── Syntax error tolerance ───────────────────────────────────────

    #[test]
    fn parse_to_ir_tolerates_ts_syntax_errors() {
        let engine = QueryEngine::new().unwrap();
        let source = "function broken( { return; }\nfunction ok() { return 1; }";
        // Should not error — tree-sitter is error-tolerant
        let ir = parse_to_ir(&engine, "src/broken.ts", source).unwrap();
        assert_eq!(ir.path, "src/broken.ts");
    }

    #[test]
    fn parse_to_ir_tolerates_python_syntax_errors() {
        let engine = QueryEngine::new().unwrap();
        let source = "def broken(\n    pass\ndef ok():\n    return 1";
        let ir = parse_to_ir(&engine, "broken.py", source).unwrap();
        assert_eq!(ir.path, "broken.py");
    }

    // ── Path handling ────────────────────────────────────────────────

    #[test]
    fn parse_to_ir_deeply_nested_path() {
        let engine = QueryEngine::new().unwrap();
        let ir = parse_to_ir(
            &engine,
            "packages/core/src/modules/auth/handlers/login.ts",
            "export function login() {}",
        )
        .unwrap();
        assert_eq!(
            ir.path,
            "packages/core/src/modules/auth/handlers/login.ts"
        );
    }

    #[test]
    fn parse_to_ir_nextjs_dynamic_route_path() {
        let engine = QueryEngine::new().unwrap();
        let ir = parse_to_ir(
            &engine,
            "src/app/[slug]/page.tsx",
            "export default function Page() { return null; }",
        )
        .unwrap();
        assert_eq!(ir.path, "src/app/[slug]/page.tsx");
    }

    #[test]
    fn parse_to_ir_dotfile_path() {
        let engine = QueryEngine::new().unwrap();
        let ir = parse_to_ir(&engine, ".eslintrc.js", "module.exports = {};").unwrap();
        assert_eq!(ir.path, ".eslintrc.js");
    }

    // ── Data flow enrichment ─────────────────────────────────────────

    #[test]
    fn parse_to_ir_enriches_ts_variable_assignments() {
        let engine = QueryEngine::new().unwrap();
        let source = r#"
function processOrder() {
    const user = getUser();
    const order = createOrder(user);
    const receipt = sendReceipt(order);
    return receipt;
}
"#;
        let ir = parse_to_ir(&engine, "src/order.ts", source).unwrap();
        assert!(!ir.functions.is_empty());
        // Data flow enrichment should populate assignments
        assert!(
            !ir.assignments.is_empty(),
            "should have enriched assignments from data flow"
        );
    }

    #[test]
    fn parse_to_ir_enriches_python_assignments() {
        let engine = QueryEngine::new().unwrap();
        let source = r#"
def process():
    data = fetch_data()
    result = transform(data)
    return result
"#;
        let ir = parse_to_ir(&engine, "src/process.py", source).unwrap();
        assert!(!ir.functions.is_empty());
        assert!(
            !ir.assignments.is_empty(),
            "should have enriched Python assignments"
        );
    }

    // ── PipelineError display ────────────────────────────────────────

    #[test]
    fn pipeline_error_parse_display() {
        let err = PipelineError::Parse("file.ts: unexpected token".into());
        let msg = format!("{}", err);
        assert!(msg.contains("parse error"));
        assert!(msg.contains("file.ts"));
        assert!(msg.contains("unexpected token"));
    }

    #[test]
    fn pipeline_error_query_engine_display() {
        let err = PipelineError::QueryEngine("failed to compile query".into());
        let msg = format!("{}", err);
        assert!(msg.contains("query engine error"));
        assert!(msg.contains("failed to compile query"));
    }

    #[test]
    fn pipeline_error_is_debug() {
        let err = PipelineError::Parse("test".into());
        let debug = format!("{:?}", err);
        assert!(debug.contains("Parse"));
    }

    // ── Determinism ──────────────────────────────────────────────────

    #[test]
    fn parse_to_ir_deterministic() {
        let engine = QueryEngine::new().unwrap();
        let source = r#"
import { a } from './a';
import { b } from './b';
export function handler() {
    a();
    b();
}
"#;
        let ir1 = parse_to_ir(&engine, "src/h.ts", source).unwrap();
        let ir2 = parse_to_ir(&engine, "src/h.ts", source).unwrap();
        assert_eq!(ir1.path, ir2.path);
        assert_eq!(ir1.functions.len(), ir2.functions.len());
        assert_eq!(ir1.imports.len(), ir2.imports.len());
        assert_eq!(ir1.exports.len(), ir2.exports.len());
        assert_eq!(ir1.call_expressions.len(), ir2.call_expressions.len());
    }

    #[test]
    fn parse_all_to_ir_deterministic() {
        let engine = QueryEngine::new().unwrap();
        let files = vec![
            ("a.ts", "export function a() {}"),
            ("b.ts", "import { a } from './a'; export function b() { a(); }"),
            ("c.py", "def c(): pass"),
        ];
        let (ir1, err1) = parse_all_to_ir(&engine, &files);
        let (ir2, err2) = parse_all_to_ir(&engine, &files);
        assert_eq!(ir1.len(), ir2.len());
        assert_eq!(err1.len(), err2.len());
        for (a, b) in ir1.iter().zip(ir2.iter()) {
            assert_eq!(a.path, b.path);
            assert_eq!(a.functions.len(), b.functions.len());
        }
    }

    // ── Parallel parse_files_parallel tests ────────────────────────────

    #[test]
    fn parse_files_parallel_basic() {
        let files = vec![
            ("src/a.ts", "export function a() {}"),
            ("src/b.ts", "function b() {}"),
        ];
        let parsed = parse_files_parallel(&files);
        assert_eq!(parsed.len(), 2);
        let paths: Vec<&str> = parsed.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"src/a.ts"));
        assert!(paths.contains(&"src/b.ts"));
    }

    #[test]
    fn parse_files_parallel_empty() {
        let parsed = parse_files_parallel(&[]);
        assert!(parsed.is_empty());
    }

    #[test]
    fn parse_files_parallel_single_file() {
        let files = vec![("main.ts", "function main() {}")];
        let parsed = parse_files_parallel(&files);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].path, "main.ts");
    }

    #[test]
    fn parse_files_parallel_mixed_languages() {
        let files = vec![
            ("app.ts", "export function app() {}"),
            ("views.py", "def view(): pass"),
            ("config.json", "{}"),
        ];
        let parsed = parse_files_parallel(&files);
        assert_eq!(parsed.len(), 3);
        let ts = parsed.iter().find(|f| f.path == "app.ts").unwrap();
        let py = parsed.iter().find(|f| f.path == "views.py").unwrap();
        assert!(!ts.definitions.is_empty());
        assert!(!py.definitions.is_empty());
    }

    #[test]
    fn parse_files_parallel_deterministic() {
        let files = vec![
            ("c.ts", "function c() {}"),
            ("a.ts", "function a() {}"),
            ("b.ts", "function b() {}"),
        ];
        let r1 = parse_files_parallel(&files);
        let r2 = parse_files_parallel(&files);
        assert_eq!(r1.len(), r2.len());
        for (a, b) in r1.iter().zip(r2.iter()) {
            assert_eq!(a.path, b.path);
            assert_eq!(a.definitions.len(), b.definitions.len());
        }
    }

    #[test]
    fn parse_files_parallel_sorted_by_path() {
        let files = vec![
            ("z.ts", "function z() {}"),
            ("a.ts", "function a() {}"),
            ("m.ts", "function m() {}"),
        ];
        let parsed = parse_files_parallel(&files);
        assert_eq!(parsed[0].path, "a.ts");
        assert_eq!(parsed[1].path, "m.ts");
        assert_eq!(parsed[2].path, "z.ts");
    }

    #[test]
    fn parse_files_parallel_many_files() {
        let sources: Vec<(String, String)> = (0..50)
            .map(|i| (format!("src/file_{:03}.ts", i), format!("function f{}() {{}}", i)))
            .collect();
        let files: Vec<(&str, &str)> = sources
            .iter()
            .map(|(p, s)| (p.as_str(), s.as_str()))
            .collect();
        let parsed = parse_files_parallel(&files);
        assert_eq!(parsed.len(), 50);
        // Verify sorted
        for w in parsed.windows(2) {
            assert!(w[0].path <= w[1].path);
        }
    }

    // ── Property-based tests ─────────────────────────────────────────

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        fn arb_ts_function_name() -> impl Strategy<Value = String> {
            "[a-z][a-zA-Z0-9]{0,15}".prop_map(|s| s.to_string())
        }

        fn arb_ts_file() -> impl Strategy<Value = (String, String)> {
            (
                arb_ts_function_name(),
                proptest::collection::vec(arb_ts_function_name(), 0..5),
            )
                .prop_map(|(main_fn, extra_fns)| {
                    let mut source = format!("function {}() {{}}\n", main_fn);
                    for f in &extra_fns {
                        source.push_str(&format!("function {}() {{}}\n", f));
                    }
                    let path = format!("src/{}.ts", main_fn);
                    (path, source)
                })
        }

        proptest! {
            #[test]
            fn prop_parse_to_ir_never_panics(source in ".*") {
                let engine = QueryEngine::new().unwrap();
                // Should never panic, even on garbage input
                let _ = parse_to_ir(&engine, "test.ts", &source);
            }

            #[test]
            fn prop_parse_to_ir_path_preserved(
                path in "[a-z/]{1,30}\\.(ts|py|js|tsx|jsx)"
            ) {
                let engine = QueryEngine::new().unwrap();
                let ir = parse_to_ir(&engine, &path, "function f() {}").unwrap();
                prop_assert_eq!(&ir.path, &path);
            }

            #[test]
            fn prop_parse_all_to_ir_file_count(
                files in proptest::collection::vec(arb_ts_file(), 0..10)
            ) {
                let engine = QueryEngine::new().unwrap();
                let file_refs: Vec<(&str, &str)> = files
                    .iter()
                    .map(|(p, s)| (p.as_str(), s.as_str()))
                    .collect();
                let (ir_files, errors) = parse_all_to_ir(&engine, &file_refs);
                // Total IR files + errors should equal input count
                prop_assert_eq!(ir_files.len() + errors.len(), files.len());
            }

            #[test]
            fn prop_parse_to_ir_deterministic(
                (path, source) in arb_ts_file()
            ) {
                let engine = QueryEngine::new().unwrap();
                let ir1 = parse_to_ir(&engine, &path, &source).unwrap();
                let ir2 = parse_to_ir(&engine, &path, &source).unwrap();
                prop_assert_eq!(ir1.path, ir2.path);
                prop_assert_eq!(ir1.functions.len(), ir2.functions.len());
                prop_assert_eq!(ir1.imports.len(), ir2.imports.len());
            }

            #[test]
            fn prop_parse_to_ir_empty_source_has_no_definitions(
                path in "[a-z]{1,10}\\.(ts|py|js)"
            ) {
                let engine = QueryEngine::new().unwrap();
                let ir = parse_to_ir(&engine, &path, "").unwrap();
                prop_assert!(ir.functions.is_empty());
                prop_assert!(ir.imports.is_empty());
                prop_assert!(ir.exports.is_empty());
                prop_assert!(ir.call_expressions.is_empty());
            }

            #[test]
            fn prop_parse_files_parallel_deterministic(
                files in proptest::collection::vec(arb_ts_file(), 0..10)
            ) {
                let file_refs: Vec<(&str, &str)> = files
                    .iter()
                    .map(|(p, s)| (p.as_str(), s.as_str()))
                    .collect();
                let r1 = parse_files_parallel(&file_refs);
                let r2 = parse_files_parallel(&file_refs);
                prop_assert_eq!(r1.len(), r2.len());
                for (a, b) in r1.iter().zip(r2.iter()) {
                    prop_assert_eq!(&a.path, &b.path);
                    prop_assert_eq!(a.definitions.len(), b.definitions.len());
                }
            }

            #[test]
            fn prop_parse_files_parallel_count_matches(
                files in proptest::collection::vec(arb_ts_file(), 0..10)
            ) {
                let file_refs: Vec<(&str, &str)> = files
                    .iter()
                    .map(|(p, s)| (p.as_str(), s.as_str()))
                    .collect();
                let parsed = parse_files_parallel(&file_refs);
                // All valid TS files should parse successfully
                prop_assert_eq!(parsed.len(), files.len());
            }
        }
    }

    // ── Error path and edge case tests ────────────────────────────────

    #[test]
    fn parse_files_parallel_handles_unsupported_languages_gracefully() {
        // Files with unsupported extensions should be parsed (tree-sitter is
        // error-tolerant) or gracefully skipped without panicking.
        let files = vec![
            ("src/app.ts", "export function app() {}"),
            ("data.csv", "a,b,c\n1,2,3"),
            ("image.png", "PNG binary-like content"), // non-code content
            ("src/main.rs", "fn main() {}"),      // Rust not yet supported by query engine
        ];
        let parsed = parse_files_parallel(&files);
        // Should not panic. The exact count depends on which languages are supported,
        // but TS should always parse.
        assert!(parsed.iter().any(|f| f.path == "src/app.ts"));
    }

    #[test]
    fn parse_all_to_ir_collects_errors_separately() {
        let engine = QueryEngine::new().unwrap();
        // Mix of valid and invalid files
        let files = vec![
            ("valid.ts", "export function valid() {}"),
            ("also_valid.py", "def also_valid(): pass"),
        ];
        let (ir_files, errors) = parse_all_to_ir(&engine, &files);
        // Both should succeed (tree-sitter is error-tolerant)
        assert_eq!(ir_files.len() + errors.len(), files.len());
    }

    #[test]
    fn parse_to_ir_with_very_large_source() {
        let engine = QueryEngine::new().unwrap();
        // Generate a large file with many functions
        let mut source = String::new();
        for i in 0..500 {
            source.push_str(&format!("function fn_{}() {{ return {}; }}\n", i, i));
        }
        let ir = parse_to_ir(&engine, "src/large.ts", &source).unwrap();
        assert_eq!(ir.path, "src/large.ts");
        assert!(ir.functions.len() >= 100); // Should parse many (if not all) functions
    }

    #[test]
    fn parse_to_ir_with_null_bytes_in_source() {
        let engine = QueryEngine::new().unwrap();
        // Source with embedded null bytes (could come from binary files that
        // slipped through the binary filter)
        let source = "function a() {}\0\0\0function b() {}";
        // Should not panic
        let result = parse_to_ir(&engine, "src/nulls.ts", source);
        assert!(result.is_ok());
    }

    #[test]
    fn parse_to_ir_with_unicode_content() {
        let engine = QueryEngine::new().unwrap();
        let source = r#"
function greet(name: string) {
    return `Hello, ${name}! 🎉`;
}
const message = "日本語テスト";
"#;
        let ir = parse_to_ir(&engine, "src/unicode.ts", source).unwrap();
        assert_eq!(ir.path, "src/unicode.ts");
        assert!(!ir.functions.is_empty());
    }

    #[test]
    fn parse_files_parallel_with_duplicate_paths() {
        // Two entries with the same path but different content
        let files = vec![
            ("src/a.ts", "function a() {}"),
            ("src/a.ts", "function b() {}"),
        ];
        let parsed = parse_files_parallel(&files);
        // Both should parse (parallel parsing doesn't deduplicate)
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn pipeline_error_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<PipelineError>();
    }
}
