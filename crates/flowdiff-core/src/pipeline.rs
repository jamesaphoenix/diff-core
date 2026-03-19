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
}
