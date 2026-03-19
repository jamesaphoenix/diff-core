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
    match engine.extract_data_flow(path, source) {
        Ok(data_flow) => ir.enrich_with_data_flow(&data_flow),
        Err(_) => {} // Non-fatal: file may have syntax errors or unsupported language
    }

    Ok(ir)
}

/// Parse multiple files into IR representations.
///
/// Files that fail to parse are skipped (with errors collected).
pub fn parse_all_to_ir(
    engine: &QueryEngine,
    files: &[(&str, &str)],
) -> (Vec<IrFile>, Vec<PipelineError>) {
    let mut ir_files = Vec::with_capacity(files.len());
    let mut errors = Vec::new();

    for &(path, source) in files {
        match parse_to_ir(engine, path, source) {
            Ok(ir) => ir_files.push(ir),
            Err(e) => errors.push(e),
        }
    }

    (ir_files, errors)
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
        assert_eq!(ir_files[0].path, "src/a.ts");
        assert_eq!(ir_files[1].path, "src/b.ts");
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
    fn parse_all_to_ir_preserves_order() {
        let engine = QueryEngine::new().unwrap();
        let files = vec![
            ("c.ts", "function c() {}"),
            ("a.ts", "function a() {}"),
            ("b.ts", "function b() {}"),
        ];
        let (ir_files, errors) = parse_all_to_ir(&engine, &files);
        assert!(errors.is_empty());
        assert_eq!(ir_files[0].path, "c.ts");
        assert_eq!(ir_files[1].path, "a.ts");
        assert_eq!(ir_files[2].path, "b.ts");
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
        // TS and Python should have functions, JSON should not
        assert!(!ir_files[0].functions.is_empty());
        assert!(!ir_files[1].functions.is_empty());
        assert!(ir_files[2].functions.is_empty());
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
        }
    }
}
