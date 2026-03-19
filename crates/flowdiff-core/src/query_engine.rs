//! Declarative tree-sitter query engine.
//!
//! Instead of imperative per-language Rust code, this module uses `.scm` query
//! files (tree-sitter's native query language) to declaratively capture AST
//! patterns. A single generic engine maps query captures to [`ParsedFile`] and
//! [`DataFlowInfo`] types.
//!
//! Adding a new language requires only writing `.scm` query files — zero Rust
//! code changes.

use crate::ast::{
    CallSite, CallWithArgs, DataFlowInfo, Definition, ExportInfo, ImportInfo, ImportedName,
    Language, ParsedFile, VarCallAssignment,
};
use crate::types::SymbolKind;
use std::collections::HashMap;
use thiserror::Error;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

// ---------------------------------------------------------------------------
// Embedded query files (compiled into the binary)
// ---------------------------------------------------------------------------

mod queries {
    pub mod typescript {
        pub const IMPORTS: &str = include_str!("../queries/typescript/imports.scm");
        pub const EXPORTS: &str = include_str!("../queries/typescript/exports.scm");
        pub const DEFINITIONS: &str = include_str!("../queries/typescript/definitions.scm");
        pub const CALLS: &str = include_str!("../queries/typescript/calls.scm");
        pub const ASSIGNMENTS: &str = include_str!("../queries/typescript/assignments.scm");
    }
    pub mod python {
        pub const IMPORTS: &str = include_str!("../queries/python/imports.scm");
        pub const DEFINITIONS: &str = include_str!("../queries/python/definitions.scm");
        pub const CALLS: &str = include_str!("../queries/python/calls.scm");
        pub const ASSIGNMENTS: &str = include_str!("../queries/python/assignments.scm");
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum QueryEngineError {
    #[error("failed to compile query for {language}/{category}: {detail}")]
    QueryCompilation {
        language: String,
        category: String,
        detail: String,
    },
    #[error("failed to set parser language: {0}")]
    LanguageError(String),
    #[error("failed to parse source: {0}")]
    ParseError(String),
}

// ---------------------------------------------------------------------------
// Per-language compiled queries
// ---------------------------------------------------------------------------

struct LanguageQueries {
    language: tree_sitter::Language,
    imports: QueryWithCaptures,
    exports: Option<QueryWithCaptures>,
    definitions: QueryWithCaptures,
    calls: QueryWithCaptures,
    assignments: QueryWithCaptures,
}

/// A compiled query bundled with its capture name -> index mapping.
struct QueryWithCaptures {
    query: Query,
    capture_names: HashMap<String, u32>,
}

impl QueryWithCaptures {
    fn new(
        lang: &tree_sitter::Language,
        source: &str,
        lang_name: &str,
        category: &str,
    ) -> Result<Self, QueryEngineError> {
        let query =
            Query::new(lang, source).map_err(|e| QueryEngineError::QueryCompilation {
                language: lang_name.to_string(),
                category: category.to_string(),
                detail: e.to_string(),
            })?;
        let mut capture_names = HashMap::new();
        for (i, name) in query.capture_names().iter().enumerate() {
            capture_names.insert(name.to_string(), i as u32);
        }
        Ok(Self {
            query,
            capture_names,
        })
    }

    fn capture_index(&self, name: &str) -> Option<u32> {
        self.capture_names.get(name).copied()
    }
}

// ---------------------------------------------------------------------------
// Collected match data (owns all data extracted from streaming iterator)
// ---------------------------------------------------------------------------

/// Owned representation of a single query match.
/// Extracted from the streaming iterator so we can process after iteration.
struct CollectedMatch<'tree> {
    pattern_index: usize,
    captures: Vec<(u32, Node<'tree>)>,
}

/// Collect all matches from a streaming iterator into owned data.
fn collect_matches<'tree>(
    cursor: &mut QueryCursor,
    query: &Query,
    root: Node<'tree>,
    source: &'tree [u8],
) -> Vec<CollectedMatch<'tree>> {
    let mut result = Vec::new();
    let mut matches = cursor.matches(&query, root, source);
    loop {
        matches.advance();
        match matches.get() {
            Some(m) => {
                let caps: Vec<(u32, Node)> =
                    m.captures.iter().map(|c| (c.index, c.node)).collect();
                result.push(CollectedMatch {
                    pattern_index: m.pattern_index,
                    captures: caps,
                });
            }
            None => break,
        }
    }
    result
}

// ---------------------------------------------------------------------------
// QueryEngine
// ---------------------------------------------------------------------------

/// Declarative tree-sitter query engine.
///
/// Compiles `.scm` query files once at construction, then efficiently runs them
/// against parsed source trees to produce [`ParsedFile`] and [`DataFlowInfo`].
pub struct QueryEngine {
    ts_queries: LanguageQueries,
    py_queries: LanguageQueries,
}

impl QueryEngine {
    /// Create a new query engine, compiling all embedded `.scm` query files.
    pub fn new() -> Result<Self, QueryEngineError> {
        let ts_lang: tree_sitter::Language = tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into();
        let py_lang: tree_sitter::Language = tree_sitter_python::LANGUAGE.into();

        let ts_queries = LanguageQueries {
            language: ts_lang.clone(),
            imports: QueryWithCaptures::new(
                &ts_lang,
                queries::typescript::IMPORTS,
                "typescript",
                "imports",
            )?,
            exports: Some(QueryWithCaptures::new(
                &ts_lang,
                queries::typescript::EXPORTS,
                "typescript",
                "exports",
            )?),
            definitions: QueryWithCaptures::new(
                &ts_lang,
                queries::typescript::DEFINITIONS,
                "typescript",
                "definitions",
            )?,
            calls: QueryWithCaptures::new(
                &ts_lang,
                queries::typescript::CALLS,
                "typescript",
                "calls",
            )?,
            assignments: QueryWithCaptures::new(
                &ts_lang,
                queries::typescript::ASSIGNMENTS,
                "typescript",
                "assignments",
            )?,
        };

        let py_queries = LanguageQueries {
            language: py_lang.clone(),
            imports: QueryWithCaptures::new(
                &py_lang,
                queries::python::IMPORTS,
                "python",
                "imports",
            )?,
            exports: None, // Python has no explicit export syntax
            definitions: QueryWithCaptures::new(
                &py_lang,
                queries::python::DEFINITIONS,
                "python",
                "definitions",
            )?,
            calls: QueryWithCaptures::new(
                &py_lang,
                queries::python::CALLS,
                "python",
                "calls",
            )?,
            assignments: QueryWithCaptures::new(
                &py_lang,
                queries::python::ASSIGNMENTS,
                "python",
                "assignments",
            )?,
        };

        Ok(Self {
            ts_queries,
            py_queries,
        })
    }

    /// Parse a source file and extract symbols, imports, exports, and call sites.
    ///
    /// This is the declarative equivalent of [`crate::ast::parse_file`].
    pub fn parse_file(&self, path: &str, source: &str) -> Result<ParsedFile, QueryEngineError> {
        let language = Language::from_path(path);
        match language {
            Language::TypeScript | Language::JavaScript => {
                self.parse_with_queries(path, source, language, &self.ts_queries)
            }
            Language::Python => {
                self.parse_with_queries(path, source, language, &self.py_queries)
            }
            Language::Unknown => Ok(ParsedFile {
                path: path.to_string(),
                language: Language::Unknown,
                definitions: vec![],
                imports: vec![],
                exports: vec![],
                call_sites: vec![],
            }),
        }
    }

    /// Extract data flow information (variable assignments from calls, calls with args).
    ///
    /// This is the declarative equivalent of [`crate::ast::extract_data_flow_info`].
    pub fn extract_data_flow(
        &self,
        path: &str,
        source: &str,
    ) -> Result<DataFlowInfo, QueryEngineError> {
        let language = Language::from_path(path);
        let lang_queries = match language {
            Language::TypeScript | Language::JavaScript => &self.ts_queries,
            Language::Python => &self.py_queries,
            Language::Unknown => {
                return Ok(DataFlowInfo {
                    assignments: vec![],
                    calls_with_args: vec![],
                })
            }
        };

        let tree = self.parse_tree(source, &lang_queries.language)?;
        let root = tree.root_node();
        let src = source.as_bytes();

        let assignments =
            self.extract_assignments(&root, src, &lang_queries.assignments, language)?;
        let calls_with_args =
            self.extract_calls_with_args(&root, src, &lang_queries.calls, language)?;

        Ok(DataFlowInfo {
            assignments,
            calls_with_args,
        })
    }

    // -----------------------------------------------------------------------
    // Internal: parse tree
    // -----------------------------------------------------------------------

    fn parse_tree(
        &self,
        source: &str,
        lang: &tree_sitter::Language,
    ) -> Result<tree_sitter::Tree, QueryEngineError> {
        let mut parser = Parser::new();
        parser
            .set_language(lang)
            .map_err(|e| QueryEngineError::LanguageError(e.to_string()))?;
        parser
            .parse(source, None)
            .ok_or_else(|| QueryEngineError::ParseError("tree-sitter failed to parse".into()))
    }

    // -----------------------------------------------------------------------
    // Internal: generic parse
    // -----------------------------------------------------------------------

    fn parse_with_queries(
        &self,
        path: &str,
        source: &str,
        language: Language,
        lang_queries: &LanguageQueries,
    ) -> Result<ParsedFile, QueryEngineError> {
        let tree = self.parse_tree(source, &lang_queries.language)?;
        let root = tree.root_node();
        let src = source.as_bytes();

        let imports = self.extract_imports(&root, src, &lang_queries.imports, language)?;
        let exports = if let Some(ref eq) = lang_queries.exports {
            self.extract_exports(&root, src, eq)?
        } else {
            vec![]
        };
        let mut definitions =
            self.extract_definitions(&root, src, &lang_queries.definitions, language)?;

        // For exports that also introduce definitions (exported declarations),
        // merge any definitions extracted from the exports query.
        if let Some(ref eq) = lang_queries.exports {
            let export_defs = self.extract_export_definitions(&root, src, eq)?;
            for def in export_defs {
                if !definitions
                    .iter()
                    .any(|d| d.name == def.name && d.kind == def.kind)
                {
                    definitions.push(def);
                }
            }
        }

        let call_sites = self.extract_call_sites(&root, src, &lang_queries.calls, language)?;

        Ok(ParsedFile {
            path: path.to_string(),
            language,
            definitions,
            imports,
            exports,
            call_sites,
        })
    }

    // -----------------------------------------------------------------------
    // Import extraction
    // -----------------------------------------------------------------------

    fn extract_imports(
        &self,
        root: &Node,
        source: &[u8],
        qwc: &QueryWithCaptures,
        language: Language,
    ) -> Result<Vec<ImportInfo>, QueryEngineError> {
        match language {
            Language::TypeScript | Language::JavaScript => {
                self.extract_ts_imports(root, source, qwc)
            }
            Language::Python => self.extract_python_imports(root, source, qwc),
            _ => Ok(vec![]),
        }
    }

    fn extract_ts_imports(
        &self,
        root: &Node,
        source: &[u8],
        qwc: &QueryWithCaptures,
    ) -> Result<Vec<ImportInfo>, QueryEngineError> {
        let mut cursor = QueryCursor::new();
        let matches = collect_matches(&mut cursor, &qwc.query, *root, source);

        let stmt_idx = qwc.capture_index("stmt");
        let source_idx = qwc.capture_index("source");
        let default_name_idx = qwc.capture_index("default_name");
        let named_name_idx = qwc.capture_index("named_name");
        let aliased_name_idx = qwc.capture_index("aliased_name");
        let alias_idx = qwc.capture_index("alias");
        let ns_name_idx = qwc.capture_index("ns_name");

        // Ordered map to preserve source order.
        let mut import_map: Vec<(usize, ImportBuilder)> = Vec::new();

        for m in &matches {
            let pattern = m.pattern_index;
            let mut stmt_start = 0usize;
            let mut source_text = String::new();
            let mut line = 0usize;

            for &(idx, node) in &m.captures {
                if Some(idx) == stmt_idx {
                    stmt_start = node.start_byte();
                    line = node.start_position().row + 1;
                }
                if Some(idx) == source_idx {
                    source_text = node_text(&node, source).to_string();
                }
            }

            let entry = get_or_insert_import(&mut import_map, stmt_start, &source_text, line);

            match pattern {
                0 => {
                    // Default import
                    for &(idx, node) in &m.captures {
                        if Some(idx) == default_name_idx {
                            entry.is_default = true;
                            entry.names.push(ImportedName {
                                name: node_text(&node, source).to_string(),
                                alias: None,
                            });
                        }
                    }
                }
                1 => {
                    // Named import (may also match aliased — engine deduplicates)
                    for &(idx, node) in &m.captures {
                        if Some(idx) == named_name_idx {
                            let name = node_text(&node, source).to_string();
                            if !entry.names.iter().any(|n| n.name == name) {
                                entry.names.push(ImportedName { name, alias: None });
                            }
                        }
                    }
                }
                2 => {
                    // Named import with alias
                    let mut name = String::new();
                    let mut alias = String::new();
                    for &(idx, node) in &m.captures {
                        if Some(idx) == aliased_name_idx {
                            name = node_text(&node, source).to_string();
                        }
                        if Some(idx) == alias_idx {
                            alias = node_text(&node, source).to_string();
                        }
                    }
                    if !name.is_empty() {
                        entry.names.retain(|n| n.name != name);
                        entry.names.push(ImportedName {
                            name,
                            alias: Some(alias),
                        });
                    }
                }
                3 => {
                    // Namespace import
                    for &(idx, node) in &m.captures {
                        if Some(idx) == ns_name_idx {
                            entry.is_namespace = true;
                            entry.names.push(ImportedName {
                                name: node_text(&node, source).to_string(),
                                alias: None,
                            });
                        }
                    }
                }
                4 => {
                    // Side-effect import — source already captured, no names
                }
                _ => {}
            }
        }

        Ok(import_map.into_iter().map(|(_, b)| b.build()).collect())
    }

    fn extract_python_imports(
        &self,
        root: &Node,
        source: &[u8],
        qwc: &QueryWithCaptures,
    ) -> Result<Vec<ImportInfo>, QueryEngineError> {
        let mut cursor = QueryCursor::new();
        let matches = collect_matches(&mut cursor, &qwc.query, *root, source);

        let stmt_idx = qwc.capture_index("stmt");
        let module_name_idx = qwc.capture_index("module_name");
        let alias_idx = qwc.capture_index("alias");
        let source_cap_idx = qwc.capture_index("source");
        let imported_name_idx = qwc.capture_index("imported_name");
        let aliased_imported_name_idx = qwc.capture_index("aliased_imported_name");
        let imported_alias_idx = qwc.capture_index("imported_alias");
        let _wildcard_idx = qwc.capture_index("wildcard");
        let relative_source_idx = qwc.capture_index("relative_source");
        let relative_imported_name_idx = qwc.capture_index("relative_imported_name");

        let mut import_map: Vec<(usize, ImportBuilder)> = Vec::new();

        for m in &matches {
            let pattern = m.pattern_index;

            let mut stmt_start = 0usize;
            let mut line = 0usize;

            for &(idx, node) in &m.captures {
                if Some(idx) == stmt_idx {
                    stmt_start = node.start_byte();
                    line = node.start_position().row + 1;
                }
            }

            match pattern {
                0 => {
                    // import foo
                    for &(idx, node) in &m.captures {
                        if Some(idx) == module_name_idx {
                            let name = node_text(&node, source).to_string();
                            let entry =
                                get_or_insert_import(&mut import_map, stmt_start, &name, line);
                            entry.is_namespace = true;
                            entry.names.push(ImportedName {
                                name: name.clone(),
                                alias: None,
                            });
                        }
                    }
                }
                1 => {
                    // import foo as bar
                    let mut name = String::new();
                    let mut alias = String::new();
                    for &(idx, node) in &m.captures {
                        if Some(idx) == module_name_idx {
                            name = node_text(&node, source).to_string();
                        }
                        if Some(idx) == alias_idx {
                            alias = node_text(&node, source).to_string();
                        }
                    }
                    if !name.is_empty() {
                        let entry =
                            get_or_insert_import(&mut import_map, stmt_start, &name, line);
                        entry.is_namespace = true;
                        entry.names.push(ImportedName {
                            name,
                            alias: if alias.is_empty() { None } else { Some(alias) },
                        });
                    }
                }
                2 => {
                    // from foo import bar
                    let mut src = String::new();
                    let mut imported = String::new();
                    for &(idx, node) in &m.captures {
                        if Some(idx) == source_cap_idx {
                            src = node_text(&node, source).to_string();
                        }
                        if Some(idx) == imported_name_idx {
                            imported = node_text(&node, source).to_string();
                        }
                    }
                    if !src.is_empty() {
                        let entry =
                            get_or_insert_import(&mut import_map, stmt_start, &src, line);
                        if !imported.is_empty()
                            && !entry.names.iter().any(|n| n.name == imported)
                        {
                            entry.names.push(ImportedName {
                                name: imported,
                                alias: None,
                            });
                        }
                    }
                }
                3 => {
                    // from foo import bar as baz
                    let mut src = String::new();
                    let mut imported = String::new();
                    let mut alias = String::new();
                    for &(idx, node) in &m.captures {
                        if Some(idx) == source_cap_idx {
                            src = node_text(&node, source).to_string();
                        }
                        if Some(idx) == aliased_imported_name_idx {
                            imported = node_text(&node, source).to_string();
                        }
                        if Some(idx) == imported_alias_idx {
                            alias = node_text(&node, source).to_string();
                        }
                    }
                    if !src.is_empty() && !imported.is_empty() {
                        let entry =
                            get_or_insert_import(&mut import_map, stmt_start, &src, line);
                        entry.names.retain(|n| n.name != imported);
                        entry.names.push(ImportedName {
                            name: imported,
                            alias: if alias.is_empty() { None } else { Some(alias) },
                        });
                    }
                }
                4 => {
                    // from foo import *
                    let mut src = String::new();
                    for &(idx, node) in &m.captures {
                        if Some(idx) == source_cap_idx {
                            src = node_text(&node, source).to_string();
                        }
                    }
                    if !src.is_empty() {
                        let entry =
                            get_or_insert_import(&mut import_map, stmt_start, &src, line);
                        entry.names.push(ImportedName {
                            name: "*".to_string(),
                            alias: None,
                        });
                    }
                }
                5 => {
                    // from .bar import baz (relative import)
                    let mut src = String::new();
                    let mut imported = String::new();
                    for &(idx, node) in &m.captures {
                        if Some(idx) == relative_source_idx {
                            src = node_text(&node, source).to_string();
                        }
                        if Some(idx) == relative_imported_name_idx {
                            imported = node_text(&node, source).to_string();
                        }
                    }
                    if !src.is_empty() {
                        let entry =
                            get_or_insert_import(&mut import_map, stmt_start, &src, line);
                        if !imported.is_empty() {
                            entry.names.push(ImportedName {
                                name: imported,
                                alias: None,
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(import_map.into_iter().map(|(_, b)| b.build()).collect())
    }

    // -----------------------------------------------------------------------
    // Export extraction (TypeScript only)
    // -----------------------------------------------------------------------

    fn extract_exports(
        &self,
        root: &Node,
        source: &[u8],
        qwc: &QueryWithCaptures,
    ) -> Result<Vec<ExportInfo>, QueryEngineError> {
        let mut cursor = QueryCursor::new();
        let matches = collect_matches(&mut cursor, &qwc.query, *root, source);

        let stmt_idx = qwc.capture_index("stmt");
        let export_name_idx = qwc.capture_index("export_name");
        let reexport_name_idx = qwc.capture_index("reexport_name");
        let reexport_source_idx = qwc.capture_index("reexport_source");
        let wildcard_source_idx = qwc.capture_index("wildcard_source");
        let decl_fn_name_idx = qwc.capture_index("decl_fn_name");
        let decl_gen_name_idx = qwc.capture_index("decl_gen_name");
        let decl_class_name_idx = qwc.capture_index("decl_class_name");
        let decl_abstract_name_idx = qwc.capture_index("decl_abstract_name");
        let decl_iface_name_idx = qwc.capture_index("decl_iface_name");
        let decl_type_name_idx = qwc.capture_index("decl_type_name");
        let decl_var_name_idx = qwc.capture_index("decl_var_name");

        let mut exports = Vec::new();

        for m in &matches {
            let pattern = m.pattern_index;
            let mut line = 0usize;
            let mut stmt_node: Option<Node> = None;

            for &(idx, node) in &m.captures {
                if Some(idx) == stmt_idx {
                    line = node.start_position().row + 1;
                    stmt_node = Some(node);
                }
            }

            let is_default = stmt_node
                .map(|n| has_default_keyword(&n))
                .unwrap_or(false);

            match pattern {
                0 => {
                    // export { foo, bar }
                    for &(idx, node) in &m.captures {
                        if Some(idx) == export_name_idx {
                            exports.push(ExportInfo {
                                name: node_text(&node, source).to_string(),
                                is_default: false,
                                is_reexport: false,
                                source: None,
                                line,
                            });
                        }
                    }
                }
                1 => {
                    // export { baz } from './other'
                    let mut reexport_src = String::new();
                    for &(idx, node) in &m.captures {
                        if Some(idx) == reexport_source_idx {
                            reexport_src = node_text(&node, source).to_string();
                        }
                    }
                    for &(idx, node) in &m.captures {
                        if Some(idx) == reexport_name_idx {
                            exports.push(ExportInfo {
                                name: node_text(&node, source).to_string(),
                                is_default: false,
                                is_reexport: true,
                                source: Some(reexport_src.clone()),
                                line,
                            });
                        }
                    }
                }
                2 => {
                    // export * from './other' (or export * as ns from '...')
                    // This pattern matches any export_statement with source field.
                    // Only treat as wildcard if there's no export_clause child
                    // (pattern 1 already handles re-exports with export_clause).
                    let has_export_clause = stmt_node
                        .map(|n| {
                            let mut c = n.walk();
                            let result = n.named_children(&mut c)
                                .any(|ch| ch.kind() == "export_clause");
                            result
                        })
                        .unwrap_or(false);
                    if !has_export_clause {
                        for &(idx, node) in &m.captures {
                            if Some(idx) == wildcard_source_idx {
                                exports.push(ExportInfo {
                                    name: "*".to_string(),
                                    is_default: false,
                                    is_reexport: true,
                                    source: Some(node_text(&node, source).to_string()),
                                    line,
                                });
                            }
                        }
                    }
                }
                3 | 4 => {
                    // Exported function / generator
                    let name_idx = if pattern == 3 {
                        decl_fn_name_idx
                    } else {
                        decl_gen_name_idx
                    };
                    for &(idx, node) in &m.captures {
                        if Some(idx) == name_idx {
                            exports.push(ExportInfo {
                                name: node_text(&node, source).to_string(),
                                is_default,
                                is_reexport: false,
                                source: None,
                                line,
                            });
                        }
                    }
                }
                5 | 6 => {
                    // Exported class / abstract class
                    let name_idx = if pattern == 5 {
                        decl_class_name_idx
                    } else {
                        decl_abstract_name_idx
                    };
                    for &(idx, node) in &m.captures {
                        if Some(idx) == name_idx {
                            exports.push(ExportInfo {
                                name: node_text(&node, source).to_string(),
                                is_default,
                                is_reexport: false,
                                source: None,
                                line,
                            });
                        }
                    }
                }
                7 => {
                    // Exported interface
                    for &(idx, node) in &m.captures {
                        if Some(idx) == decl_iface_name_idx {
                            exports.push(ExportInfo {
                                name: node_text(&node, source).to_string(),
                                is_default,
                                is_reexport: false,
                                source: None,
                                line,
                            });
                        }
                    }
                }
                8 => {
                    // Exported type alias
                    for &(idx, node) in &m.captures {
                        if Some(idx) == decl_type_name_idx {
                            exports.push(ExportInfo {
                                name: node_text(&node, source).to_string(),
                                is_default,
                                is_reexport: false,
                                source: None,
                                line,
                            });
                        }
                    }
                }
                9 => {
                    // Exported variable
                    for &(idx, node) in &m.captures {
                        if Some(idx) == decl_var_name_idx {
                            exports.push(ExportInfo {
                                name: node_text(&node, source).to_string(),
                                is_default,
                                is_reexport: false,
                                source: None,
                                line,
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        dedup_exports(&mut exports);

        Ok(exports)
    }

    /// Extract definitions from exported declarations (patterns 3-9 in exports.scm).
    fn extract_export_definitions(
        &self,
        root: &Node,
        source: &[u8],
        qwc: &QueryWithCaptures,
    ) -> Result<Vec<Definition>, QueryEngineError> {
        let mut cursor = QueryCursor::new();
        let matches = collect_matches(&mut cursor, &qwc.query, *root, source);

        let decl_fn_name_idx = qwc.capture_index("decl_fn_name");
        let decl_gen_name_idx = qwc.capture_index("decl_gen_name");
        let decl_class_name_idx = qwc.capture_index("decl_class_name");
        let decl_abstract_name_idx = qwc.capture_index("decl_abstract_name");
        let decl_iface_name_idx = qwc.capture_index("decl_iface_name");
        let decl_type_name_idx = qwc.capture_index("decl_type_name");
        let decl_var_name_idx = qwc.capture_index("decl_var_name");
        let stmt_idx = qwc.capture_index("stmt");

        let mut definitions = Vec::new();

        for m in &matches {
            let pattern = m.pattern_index;
            if pattern < 3 {
                continue;
            }

            let mut decl_node: Option<Node> = None;
            let mut name_text = String::new();

            for &(idx, node) in &m.captures {
                if Some(idx) == stmt_idx {
                    if let Some(decl) = node.child_by_field_name("declaration") {
                        decl_node = Some(decl);
                    }
                }
            }

            let kind = match pattern {
                3 | 4 => {
                    let target_idx = if pattern == 3 {
                        decl_fn_name_idx
                    } else {
                        decl_gen_name_idx
                    };
                    for &(idx, node) in &m.captures {
                        if Some(idx) == target_idx {
                            name_text = node_text(&node, source).to_string();
                        }
                    }
                    SymbolKind::Function
                }
                5 => {
                    for &(idx, node) in &m.captures {
                        if Some(idx) == decl_class_name_idx {
                            name_text = node_text(&node, source).to_string();
                        }
                    }
                    SymbolKind::Class
                }
                6 => {
                    for &(idx, node) in &m.captures {
                        if Some(idx) == decl_abstract_name_idx {
                            name_text = node_text(&node, source).to_string();
                        }
                    }
                    SymbolKind::Class
                }
                7 => {
                    for &(idx, node) in &m.captures {
                        if Some(idx) == decl_iface_name_idx {
                            name_text = node_text(&node, source).to_string();
                        }
                    }
                    SymbolKind::Interface
                }
                8 => {
                    for &(idx, node) in &m.captures {
                        if Some(idx) == decl_type_name_idx {
                            name_text = node_text(&node, source).to_string();
                        }
                    }
                    SymbolKind::TypeAlias
                }
                9 => {
                    for &(idx, node) in &m.captures {
                        if Some(idx) == decl_var_name_idx {
                            name_text = node_text(&node, source).to_string();
                        }
                    }
                    SymbolKind::Constant
                }
                _ => continue,
            };

            if !name_text.is_empty() {
                let (start_line, end_line) = if let Some(dn) = decl_node {
                    (dn.start_position().row + 1, dn.end_position().row + 1)
                } else {
                    (0, 0)
                };
                definitions.push(Definition {
                    name: name_text,
                    kind,
                    start_line,
                    end_line,
                });
            }
        }

        Ok(definitions)
    }

    // -----------------------------------------------------------------------
    // Definition extraction
    // -----------------------------------------------------------------------

    fn extract_definitions(
        &self,
        root: &Node,
        source: &[u8],
        qwc: &QueryWithCaptures,
        language: Language,
    ) -> Result<Vec<Definition>, QueryEngineError> {
        let mut cursor = QueryCursor::new();
        let matches = collect_matches(&mut cursor, &qwc.query, *root, source);

        let name_idx = qwc.capture_index("name");
        let node_idx = qwc.capture_index("node");
        let value_idx = qwc.capture_index("value");

        // Python-specific captures
        let method_name_idx = qwc.capture_index("method_name");
        let method_node_idx = qwc.capture_index("method_node");
        let decorated_method_name_idx = qwc.capture_index("decorated_method_name");
        let decorated_method_node_idx = qwc.capture_index("decorated_method_node");

        let mut definitions = Vec::new();
        let mut seen_nodes: Vec<(usize, usize)> = Vec::new();

        for m in &matches {
            let pattern = m.pattern_index;

            match language {
                Language::TypeScript | Language::JavaScript => {
                    let kind = match pattern {
                        0 | 1 => SymbolKind::Function,
                        2 | 3 => SymbolKind::Class,
                        4 => SymbolKind::Interface,
                        5 => SymbolKind::TypeAlias,
                        6 | 7 => SymbolKind::Function,
                        8 => {
                            let mut is_fn = false;
                            for &(idx, node) in &m.captures {
                                if Some(idx) == value_idx {
                                    let kind = node.kind();
                                    is_fn = kind == "arrow_function" || kind == "function";
                                }
                            }
                            if is_fn {
                                SymbolKind::Function
                            } else {
                                SymbolKind::Constant
                            }
                        }
                        9 => SymbolKind::Function,
                        _ => continue,
                    };

                    // Skip pattern 8 for arrow/function values (already captured by 6/7)
                    if pattern == 8 {
                        let mut has_fn_value = false;
                        for &(idx, node) in &m.captures {
                            if Some(idx) == value_idx {
                                let k = node.kind();
                                has_fn_value = k == "arrow_function" || k == "function";
                            }
                        }
                        if has_fn_value {
                            continue;
                        }
                    }

                    let mut name_text = String::new();
                    let mut start_line = 0;
                    let mut end_line = 0;
                    let mut node_start = 0usize;

                    for &(idx, node) in &m.captures {
                        if Some(idx) == name_idx {
                            name_text = node_text(&node, source).to_string();
                        }
                        if Some(idx) == node_idx {
                            start_line = node.start_position().row + 1;
                            end_line = node.end_position().row + 1;
                            node_start = node.start_byte();
                        }
                    }

                    if !name_text.is_empty() {
                        let key = (node_start, hash_str(&name_text));
                        if !seen_nodes.contains(&key) {
                            seen_nodes.push(key);
                            definitions.push(Definition {
                                name: name_text,
                                kind,
                                start_line,
                                end_line,
                            });
                        }
                    }
                }
                Language::Python => {
                    let (target_name_idx, target_node_idx, kind) = match pattern {
                        0 | 2 => (name_idx, node_idx, SymbolKind::Function),
                        1 | 3 => (name_idx, node_idx, SymbolKind::Class),
                        4 => (method_name_idx, method_node_idx, SymbolKind::Function),
                        5 => (
                            decorated_method_name_idx,
                            decorated_method_node_idx,
                            SymbolKind::Function,
                        ),
                        _ => continue,
                    };

                    let mut name_text = String::new();
                    let mut start_line = 0;
                    let mut end_line = 0;
                    let mut node_start = 0usize;

                    for &(idx, node) in &m.captures {
                        if Some(idx) == target_name_idx {
                            name_text = node_text(&node, source).to_string();
                        }
                        if Some(idx) == target_node_idx {
                            start_line = node.start_position().row + 1;
                            end_line = node.end_position().row + 1;
                            node_start = node.start_byte();
                        }
                    }

                    if !name_text.is_empty() {
                        let key = (node_start, hash_str(&name_text));
                        if !seen_nodes.contains(&key) {
                            seen_nodes.push(key);
                            definitions.push(Definition {
                                name: name_text,
                                kind,
                                start_line,
                                end_line,
                            });
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(definitions)
    }

    // -----------------------------------------------------------------------
    // Call site extraction
    // -----------------------------------------------------------------------

    fn extract_call_sites(
        &self,
        root: &Node,
        source: &[u8],
        qwc: &QueryWithCaptures,
        language: Language,
    ) -> Result<Vec<CallSite>, QueryEngineError> {
        let mut cursor = QueryCursor::new();
        let matches = collect_matches(&mut cursor, &qwc.query, *root, source);

        let callee_idx = qwc.capture_index("callee");
        let node_idx = qwc.capture_index("node");

        let mut call_sites = Vec::new();

        for m in &matches {
            let mut callee_text = String::new();
            let mut call_line = 0;
            let mut call_node: Option<Node> = None;

            for &(idx, node) in &m.captures {
                if Some(idx) == callee_idx {
                    callee_text = node_text(&node, source).to_string();
                }
                if Some(idx) == node_idx {
                    call_line = node.start_position().row + 1;
                    call_node = Some(node);
                }
            }

            if !callee_text.is_empty() {
                let containing =
                    call_node.and_then(|n| find_containing_function(&n, source, language));
                call_sites.push(CallSite {
                    callee: callee_text,
                    line: call_line,
                    containing_function: containing,
                });
            }
        }

        Ok(call_sites)
    }

    // -----------------------------------------------------------------------
    // Assignment extraction (data flow)
    // -----------------------------------------------------------------------

    fn extract_assignments(
        &self,
        root: &Node,
        source: &[u8],
        qwc: &QueryWithCaptures,
        language: Language,
    ) -> Result<Vec<VarCallAssignment>, QueryEngineError> {
        let mut cursor = QueryCursor::new();
        let matches = collect_matches(&mut cursor, &qwc.query, *root, source);

        let var_name_idx = qwc.capture_index("var_name");
        let callee_idx = qwc.capture_index("callee");
        let node_idx = qwc.capture_index("node");

        let mut assignments = Vec::new();

        for m in &matches {
            let mut var_name = String::new();
            let mut callee_text = String::new();
            let mut line = 0;
            let mut assign_node: Option<Node> = None;

            for &(idx, node) in &m.captures {
                if Some(idx) == var_name_idx {
                    var_name = node_text(&node, source).to_string();
                }
                if Some(idx) == callee_idx {
                    callee_text = node_text(&node, source).to_string();
                }
                if Some(idx) == node_idx {
                    line = node.start_position().row + 1;
                    assign_node = Some(node);
                }
            }

            if !var_name.is_empty() && !callee_text.is_empty() {
                let containing =
                    assign_node.and_then(|n| find_containing_function(&n, source, language));
                assignments.push(VarCallAssignment {
                    variable: var_name,
                    callee: callee_text,
                    line,
                    containing_function: containing,
                });
            }
        }

        Ok(assignments)
    }

    // -----------------------------------------------------------------------
    // Calls with arguments extraction (data flow)
    // -----------------------------------------------------------------------

    fn extract_calls_with_args(
        &self,
        root: &Node,
        source: &[u8],
        qwc: &QueryWithCaptures,
        language: Language,
    ) -> Result<Vec<CallWithArgs>, QueryEngineError> {
        let mut cursor = QueryCursor::new();
        let matches = collect_matches(&mut cursor, &qwc.query, *root, source);

        let callee_idx = qwc.capture_index("callee");
        let args_idx = qwc.capture_index("args");
        let node_idx = qwc.capture_index("node");

        let mut calls = Vec::new();

        for m in &matches {
            let mut callee_text = String::new();
            let mut arguments = Vec::new();
            let mut line = 0;
            let mut call_node: Option<Node> = None;

            for &(idx, node) in &m.captures {
                if Some(idx) == callee_idx {
                    callee_text = node_text(&node, source).to_string();
                }
                if Some(idx) == args_idx {
                    arguments = extract_arg_texts(&node, source, language);
                }
                if Some(idx) == node_idx {
                    line = node.start_position().row + 1;
                    call_node = Some(node);
                }
            }

            if !callee_text.is_empty() {
                let containing =
                    call_node.and_then(|n| find_containing_function(&n, source, language));
                calls.push(CallWithArgs {
                    callee: callee_text,
                    arguments,
                    line,
                    containing_function: containing,
                });
            }
        }

        Ok(calls)
    }
}

// ---------------------------------------------------------------------------
// Helper types
// ---------------------------------------------------------------------------

struct ImportBuilder {
    source: String,
    names: Vec<ImportedName>,
    is_default: bool,
    is_namespace: bool,
    line: usize,
}

impl ImportBuilder {
    fn new(source: &str, line: usize) -> Self {
        Self {
            source: source.to_string(),
            names: Vec::new(),
            is_default: false,
            is_namespace: false,
            line,
        }
    }

    fn build(self) -> ImportInfo {
        ImportInfo {
            source: self.source,
            names: self.names,
            is_default: self.is_default,
            is_namespace: self.is_namespace,
            line: self.line,
        }
    }
}

// ---------------------------------------------------------------------------
// Free helper functions
// ---------------------------------------------------------------------------

fn node_text<'a>(node: &Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

fn get_or_insert_import<'a>(
    map: &'a mut Vec<(usize, ImportBuilder)>,
    key: usize,
    source: &str,
    line: usize,
) -> &'a mut ImportBuilder {
    if let Some(pos) = map.iter().position(|(k, _)| *k == key) {
        &mut map[pos].1
    } else {
        map.push((key, ImportBuilder::new(source, line)));
        let len = map.len();
        &mut map[len - 1].1
    }
}

/// Check if an export_statement node has the `default` keyword.
fn has_default_keyword(node: &Node) -> bool {
    let mut cursor = node.walk();
    let result = node.children(&mut cursor).any(|ch| ch.kind() == "default");
    result
}

/// Deduplicate exports: when both a plain export and a re-export match the
/// same specifier, keep only the re-export version.
fn dedup_exports(exports: &mut Vec<ExportInfo>) {
    let mut seen: HashMap<(String, usize), usize> = HashMap::new();
    let mut to_remove = Vec::new();

    for (i, exp) in exports.iter().enumerate() {
        let key = (exp.name.clone(), exp.line);
        if let Some(&prev) = seen.get(&key) {
            if exp.is_reexport && !exports[prev].is_reexport {
                to_remove.push(prev);
                seen.insert(key, i);
            } else {
                to_remove.push(i);
            }
        } else {
            seen.insert(key, i);
        }
    }

    to_remove.sort_unstable();
    to_remove.dedup();
    for i in to_remove.into_iter().rev() {
        exports.remove(i);
    }
}

/// Walk up from a node to find the nearest containing function declaration.
fn find_containing_function(node: &Node, source: &[u8], language: Language) -> Option<String> {
    let fn_kinds: &[&str] = match language {
        Language::TypeScript | Language::JavaScript => &[
            "function_declaration",
            "generator_function_declaration",
            "method_definition",
        ],
        Language::Python => &["function_definition"],
        Language::Unknown => return None,
    };

    let mut current = node.parent();
    while let Some(parent) = current {
        if fn_kinds.contains(&parent.kind()) {
            return parent
                .child_by_field_name("name")
                .map(|n| node_text(&n, source).to_string());
        }
        // Also check for arrow function / function expression assigned to a variable
        if parent.kind() == "variable_declarator" {
            let is_fn = parent
                .child_by_field_name("value")
                .map(|v| v.kind() == "arrow_function" || v.kind() == "function")
                .unwrap_or(false);
            if is_fn {
                return parent
                    .child_by_field_name("name")
                    .filter(|n| n.kind() == "identifier")
                    .map(|n| node_text(&n, source).to_string());
            }
        }
        current = parent.parent();
    }
    None
}

/// Extract argument texts from an arguments/argument_list node.
fn extract_arg_texts(args_node: &Node, source: &[u8], language: Language) -> Vec<String> {
    let mut args = Vec::new();
    let mut cursor = args_node.walk();
    for child in args_node.named_children(&mut cursor) {
        if language == Language::Python && child.kind() == "keyword_argument" {
            if let Some(val) = child.child_by_field_name("value") {
                let text = node_text(&val, source).to_string();
                if !text.is_empty() {
                    args.push(text);
                }
            }
            continue;
        }
        let text = node_text(&child, source).to_string();
        if !text.is_empty() {
            args.push(text);
        }
    }
    args
}

/// Simple string hash for deduplication keys.
fn hash_str(s: &str) -> usize {
    let mut h: usize = 0;
    for b in s.bytes() {
        h = h.wrapping_mul(31).wrapping_add(b as usize);
    }
    h
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> QueryEngine {
        QueryEngine::new().expect("query engine should compile")
    }

    // === Construction ===

    #[test]
    fn test_engine_construction() {
        let _e = engine();
    }

    // === TypeScript imports ===

    #[test]
    fn test_ts_default_import() {
        let e = engine();
        let result = e
            .parse_file("app.ts", "import React from 'react';")
            .unwrap();
        assert_eq!(result.imports.len(), 1);
        assert!(result.imports[0].is_default);
        assert_eq!(result.imports[0].source, "react");
        assert_eq!(result.imports[0].names[0].name, "React");
    }

    #[test]
    fn test_ts_named_imports() {
        let e = engine();
        let result = e
            .parse_file("app.ts", "import { useState, useEffect } from 'react';")
            .unwrap();
        assert_eq!(result.imports.len(), 1);
        assert!(!result.imports[0].is_default);
        assert_eq!(result.imports[0].source, "react");
        let names: Vec<&str> = result.imports[0]
            .names
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        assert!(names.contains(&"useState"));
        assert!(names.contains(&"useEffect"));
    }

    #[test]
    fn test_ts_namespace_import() {
        let e = engine();
        let result = e
            .parse_file("app.ts", "import * as path from 'path';")
            .unwrap();
        assert_eq!(result.imports.len(), 1);
        assert!(result.imports[0].is_namespace);
        assert_eq!(result.imports[0].source, "path");
        assert_eq!(result.imports[0].names[0].name, "path");
    }

    #[test]
    fn test_ts_aliased_import() {
        let e = engine();
        let result = e
            .parse_file("app.ts", "import { foo as bar } from './utils';")
            .unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "./utils");
        assert_eq!(result.imports[0].names[0].name, "foo");
        assert_eq!(result.imports[0].names[0].alias, Some("bar".to_string()));
    }

    #[test]
    fn test_ts_side_effect_import() {
        let e = engine();
        let result = e.parse_file("app.ts", "import './polyfill';").unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "./polyfill");
        assert!(result.imports[0].names.is_empty());
    }

    #[test]
    fn test_ts_combined_default_and_named() {
        let e = engine();
        let result = e
            .parse_file("app.ts", "import React, { useState } from 'react';")
            .unwrap();
        assert_eq!(result.imports.len(), 1);
        assert!(result.imports[0].is_default);
        assert_eq!(result.imports[0].source, "react");
        let names: Vec<&str> = result.imports[0]
            .names
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        assert!(names.contains(&"React"));
        assert!(names.contains(&"useState"));
    }

    #[test]
    fn test_ts_multiple_imports() {
        let e = engine();
        let source = r#"
import React from 'react';
import { useState } from 'react';
import * as path from 'path';
"#;
        let result = e.parse_file("app.ts", source).unwrap();
        assert_eq!(result.imports.len(), 3);
    }

    // === TypeScript exports ===

    #[test]
    fn test_ts_exported_function() {
        let e = engine();
        let result = e
            .parse_file("lib.ts", "export function greet() {}")
            .unwrap();
        assert!(result
            .exports
            .iter()
            .any(|e| e.name == "greet" && !e.is_default));
        assert!(result.definitions.iter().any(|d| d.name == "greet"));
    }

    #[test]
    fn test_ts_export_default_function() {
        let e = engine();
        let result = e
            .parse_file("lib.ts", "export default function main() {}")
            .unwrap();
        assert!(result
            .exports
            .iter()
            .any(|e| e.name == "main" && e.is_default));
    }

    #[test]
    fn test_ts_named_exports() {
        let e = engine();
        let result = e.parse_file("lib.ts", "export { foo, bar };").unwrap();
        let names: Vec<&str> = result.exports.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"foo"));
        assert!(names.contains(&"bar"));
    }

    #[test]
    fn test_ts_reexport() {
        let e = engine();
        let result = e
            .parse_file("lib.ts", "export { baz } from './other';")
            .unwrap();
        let exp = result.exports.iter().find(|e| e.name == "baz").unwrap();
        assert!(exp.is_reexport);
        assert_eq!(exp.source, Some("./other".to_string()));
    }

    #[test]
    fn test_ts_wildcard_reexport() {
        let e = engine();
        let result = e
            .parse_file("lib.ts", "export * from './other';")
            .unwrap();
        let exp = result.exports.iter().find(|e| e.name == "*").unwrap();
        assert!(exp.is_reexport);
        assert_eq!(exp.source, Some("./other".to_string()));
    }

    #[test]
    fn test_ts_export_const() {
        let e = engine();
        let result = e
            .parse_file("lib.ts", "export const VALUE = 42;")
            .unwrap();
        assert!(result.exports.iter().any(|e| e.name == "VALUE"));
        assert!(result.definitions.iter().any(|d| d.name == "VALUE"));
    }

    // === TypeScript definitions ===

    #[test]
    fn test_ts_function_definition() {
        let e = engine();
        let result = e.parse_file("lib.ts", "function greet() {}").unwrap();
        let def = result
            .definitions
            .iter()
            .find(|d| d.name == "greet")
            .unwrap();
        assert_eq!(def.kind, SymbolKind::Function);
    }

    #[test]
    fn test_ts_class_definition() {
        let e = engine();
        let result = e
            .parse_file("lib.ts", "class User { getName() {} }")
            .unwrap();
        assert!(result
            .definitions
            .iter()
            .any(|d| d.name == "User" && d.kind == SymbolKind::Class));
        assert!(result
            .definitions
            .iter()
            .any(|d| d.name == "getName" && d.kind == SymbolKind::Function));
    }

    #[test]
    fn test_ts_interface_definition() {
        let e = engine();
        let result = e
            .parse_file("lib.ts", "interface IUser { name: string; }")
            .unwrap();
        assert!(result
            .definitions
            .iter()
            .any(|d| d.name == "IUser" && d.kind == SymbolKind::Interface));
    }

    #[test]
    fn test_ts_type_alias_definition() {
        let e = engine();
        let result = e
            .parse_file("lib.ts", "type Result = { ok: boolean };")
            .unwrap();
        assert!(result
            .definitions
            .iter()
            .any(|d| d.name == "Result" && d.kind == SymbolKind::TypeAlias));
    }

    #[test]
    fn test_ts_arrow_function_def() {
        let e = engine();
        let result = e
            .parse_file("lib.ts", "const greet = () => {};")
            .unwrap();
        let def = result
            .definitions
            .iter()
            .find(|d| d.name == "greet")
            .unwrap();
        assert_eq!(def.kind, SymbolKind::Function);
    }

    #[test]
    fn test_ts_const_value_def() {
        let e = engine();
        let result = e.parse_file("lib.ts", "const MAX = 100;").unwrap();
        let def = result
            .definitions
            .iter()
            .find(|d| d.name == "MAX")
            .unwrap();
        assert_eq!(def.kind, SymbolKind::Constant);
    }

    // === TypeScript call sites ===

    #[test]
    fn test_ts_call_site() {
        let e = engine();
        let result = e
            .parse_file("lib.ts", "function main() { greet('world'); }")
            .unwrap();
        assert!(result.call_sites.iter().any(|c| c.callee == "greet"));
    }

    #[test]
    fn test_ts_method_call() {
        let e = engine();
        let result = e
            .parse_file("lib.ts", "console.log('hello');")
            .unwrap();
        assert!(result
            .call_sites
            .iter()
            .any(|c| c.callee == "console.log"));
    }

    #[test]
    fn test_ts_call_containing_function() {
        let e = engine();
        let result = e
            .parse_file("lib.ts", "function main() { greet(); }")
            .unwrap();
        let call = result
            .call_sites
            .iter()
            .find(|c| c.callee == "greet")
            .unwrap();
        assert_eq!(call.containing_function, Some("main".to_string()));
    }

    // === TypeScript data flow ===

    #[test]
    fn test_ts_assignment_from_call() {
        let e = engine();
        let df = e
            .extract_data_flow("lib.ts", "const result = fetchData();")
            .unwrap();
        assert_eq!(df.assignments.len(), 1);
        assert_eq!(df.assignments[0].variable, "result");
        assert_eq!(df.assignments[0].callee, "fetchData");
    }

    #[test]
    fn test_ts_assignment_from_await() {
        let e = engine();
        let df = e
            .extract_data_flow("lib.ts", "const data = await fetchData();")
            .unwrap();
        assert_eq!(df.assignments.len(), 1);
        assert_eq!(df.assignments[0].variable, "data");
        assert_eq!(df.assignments[0].callee, "fetchData");
    }

    #[test]
    fn test_ts_call_with_args() {
        let e = engine();
        let df = e
            .extract_data_flow("lib.ts", "processData(input, config);")
            .unwrap();
        assert_eq!(df.calls_with_args.len(), 1);
        assert_eq!(df.calls_with_args[0].callee, "processData");
        assert_eq!(
            df.calls_with_args[0].arguments,
            vec!["input", "config"]
        );
    }

    // === Python imports ===

    #[test]
    fn test_python_simple_import() {
        let e = engine();
        let result = e.parse_file("app.py", "import os").unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "os");
        assert!(result.imports[0].is_namespace);
    }

    #[test]
    fn test_python_aliased_import() {
        let e = engine();
        let result = e
            .parse_file("app.py", "import numpy as np")
            .unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "numpy");
        assert_eq!(
            result.imports[0].names[0].alias,
            Some("np".to_string())
        );
    }

    #[test]
    fn test_python_from_import() {
        let e = engine();
        let result = e
            .parse_file("app.py", "from os.path import join, exists")
            .unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "os.path");
        let names: Vec<&str> = result.imports[0]
            .names
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        assert!(names.contains(&"join"));
        assert!(names.contains(&"exists"));
    }

    #[test]
    fn test_python_wildcard_import() {
        let e = engine();
        let result = e
            .parse_file("app.py", "from os.path import *")
            .unwrap();
        assert_eq!(result.imports.len(), 1);
        assert!(result.imports[0].names.iter().any(|n| n.name == "*"));
    }

    // === Python definitions ===

    #[test]
    fn test_python_function_def() {
        let e = engine();
        let result = e
            .parse_file("app.py", "def greet(name):\n    pass")
            .unwrap();
        assert!(result
            .definitions
            .iter()
            .any(|d| d.name == "greet" && d.kind == SymbolKind::Function));
    }

    #[test]
    fn test_python_class_def() {
        let e = engine();
        let source = "class User:\n    def get_name(self):\n        pass";
        let result = e.parse_file("app.py", source).unwrap();
        assert!(result
            .definitions
            .iter()
            .any(|d| d.name == "User" && d.kind == SymbolKind::Class));
        assert!(result
            .definitions
            .iter()
            .any(|d| d.name == "get_name" && d.kind == SymbolKind::Function));
    }

    #[test]
    fn test_python_decorated_function() {
        let e = engine();
        let source = "@app.route('/hello')\ndef hello():\n    pass";
        let result = e.parse_file("app.py", source).unwrap();
        assert!(result
            .definitions
            .iter()
            .any(|d| d.name == "hello" && d.kind == SymbolKind::Function));
    }

    // === Python call sites ===

    #[test]
    fn test_python_call_site() {
        let e = engine();
        let result = e
            .parse_file("app.py", "def main():\n    greet('world')")
            .unwrap();
        assert!(result.call_sites.iter().any(|c| c.callee == "greet"));
    }

    // === Python data flow ===

    #[test]
    fn test_python_assignment_from_call() {
        let e = engine();
        let df = e
            .extract_data_flow("app.py", "result = fetch_data()")
            .unwrap();
        assert_eq!(df.assignments.len(), 1);
        assert_eq!(df.assignments[0].variable, "result");
        assert_eq!(df.assignments[0].callee, "fetch_data");
    }

    #[test]
    fn test_python_call_with_args() {
        let e = engine();
        let df = e
            .extract_data_flow("app.py", "process(data, config)")
            .unwrap();
        assert_eq!(df.calls_with_args.len(), 1);
        assert_eq!(df.calls_with_args[0].callee, "process");
        assert_eq!(
            df.calls_with_args[0].arguments,
            vec!["data", "config"]
        );
    }

    #[test]
    fn test_python_keyword_args() {
        let e = engine();
        let df = e
            .extract_data_flow("app.py", "connect(host='localhost', port=5432)")
            .unwrap();
        assert_eq!(df.calls_with_args.len(), 1);
        assert_eq!(
            df.calls_with_args[0].arguments,
            vec!["'localhost'", "5432"]
        );
    }

    // === Unknown language ===

    #[test]
    fn test_unknown_language_returns_empty() {
        let e = engine();
        let result = e.parse_file("data.csv", "a,b,c").unwrap();
        assert_eq!(result.language, Language::Unknown);
        assert!(result.definitions.is_empty());
        assert!(result.imports.is_empty());
    }

    #[test]
    fn test_unknown_language_data_flow() {
        let e = engine();
        let df = e.extract_data_flow("data.csv", "a,b,c").unwrap();
        assert!(df.assignments.is_empty());
        assert!(df.calls_with_args.is_empty());
    }

    // === Parity with ast.rs ===

    #[test]
    fn test_parity_ts_full_file() {
        let source = r#"
import React from 'react';
import { useState, useEffect } from 'react';
import * as path from 'path';
import { foo as bar } from './utils';

export function greet(name: string) {
    console.log(name);
}

export default function main() {}

export class UserService {
    getUser() {}
}

export interface IConfig {
    port: number;
}

export type Result = { ok: boolean };

export const VALUE = 42;

const handler = () => {
    fetchData();
};
"#;
        let e = engine();
        let qe_result = e.parse_file("app.ts", source).unwrap();
        let ast_result = crate::ast::parse_file("app.ts", source).unwrap();

        // Import count should match
        assert_eq!(
            qe_result.imports.len(),
            ast_result.imports.len(),
            "import count mismatch: qe={}, ast={}",
            qe_result.imports.len(),
            ast_result.imports.len()
        );

        // Export count should match
        assert_eq!(
            qe_result.exports.len(),
            ast_result.exports.len(),
            "export count mismatch: qe={}, ast={}",
            qe_result.exports.len(),
            ast_result.exports.len()
        );

        // All definition names from ast should be present in query engine results
        for ast_def in &ast_result.definitions {
            assert!(
                qe_result
                    .definitions
                    .iter()
                    .any(|d| d.name == ast_def.name && d.kind == ast_def.kind),
                "missing definition from query engine: {} ({:?})",
                ast_def.name,
                ast_def.kind,
            );
        }

        // All call site callees from ast should be present
        for ast_call in &ast_result.call_sites {
            assert!(
                qe_result
                    .call_sites
                    .iter()
                    .any(|c| c.callee == ast_call.callee),
                "missing call site from query engine: {}",
                ast_call.callee,
            );
        }
    }

    #[test]
    fn test_parity_python_full_file() {
        let source = r#"
import os
import numpy as np
from os.path import join, exists
from typing import List

def greet(name):
    print(name)

class UserService:
    def get_user(self, user_id):
        return self.db.find(user_id)

@app.route('/hello')
def hello():
    data = fetch_data()
    return data
"#;
        let e = engine();
        let qe_result = e.parse_file("app.py", source).unwrap();
        let ast_result = crate::ast::parse_file("app.py", source).unwrap();

        // Import count should match
        assert_eq!(
            qe_result.imports.len(),
            ast_result.imports.len(),
            "import count mismatch: qe={}, ast={}",
            qe_result.imports.len(),
            ast_result.imports.len()
        );

        // All definition names from ast should be present
        for ast_def in &ast_result.definitions {
            assert!(
                qe_result.definitions.iter().any(|d| d.name == ast_def.name),
                "missing definition from query engine: {}",
                ast_def.name,
            );
        }

        // All call site callees from ast should be present
        for ast_call in &ast_result.call_sites {
            assert!(
                qe_result
                    .call_sites
                    .iter()
                    .any(|c| c.callee == ast_call.callee),
                "missing call site from query engine: {}",
                ast_call.callee,
            );
        }
    }

    // === Determinism ===

    #[test]
    fn test_deterministic_output() {
        let source = r#"
import { a, b, c } from './mod';
export function process(data: string) {
    const result = transform(data);
    return save(result);
}
"#;
        let e = engine();
        let r1 = e.parse_file("app.ts", source).unwrap();
        let r2 = e.parse_file("app.ts", source).unwrap();
        assert_eq!(r1, r2);
    }

    // === Edge cases ===

    #[test]
    fn test_empty_source() {
        let e = engine();
        let result = e.parse_file("app.ts", "").unwrap();
        assert!(result.definitions.is_empty());
        assert!(result.imports.is_empty());
        assert!(result.exports.is_empty());
        assert!(result.call_sites.is_empty());
    }

    #[test]
    fn test_syntax_error_still_parses() {
        let e = engine();
        let result = e.parse_file("app.ts", "import { from 'broken;");
        assert!(result.is_ok());
    }
}

// ---------------------------------------------------------------------------
// Property-based tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

    fn engine() -> QueryEngine {
        QueryEngine::new().unwrap()
    }

    fn ts_import_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("import React from 'react';".to_string()),
            Just("import { useState } from 'react';".to_string()),
            Just("import * as path from 'path';".to_string()),
            Just("import { foo as bar } from './utils';".to_string()),
            Just("import './polyfill';".to_string()),
            Just("import React, { useState } from 'react';".to_string()),
        ]
    }

    fn ts_definition_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("function greet() {}".to_string()),
            Just("class User {}".to_string()),
            Just("interface IUser {}".to_string()),
            Just("type Result = number;".to_string()),
            Just("const handler = () => {};".to_string()),
            Just("const MAX = 100;".to_string()),
        ]
    }

    fn python_source_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            Just("import os".to_string()),
            Just("from os.path import join".to_string()),
            Just("def greet():\n    pass".to_string()),
            Just("class User:\n    pass".to_string()),
            Just("result = fetch()".to_string()),
        ]
    }

    proptest! {
        #[test]
        fn prop_parse_never_panics(source in "[a-zA-Z0-9 (){};='\n./,*_:@#$%^&\\[\\]-]+") {
            let e = engine();
            let _ = e.parse_file("test.ts", &source);
            let _ = e.parse_file("test.py", &source);
            let _ = e.parse_file("test.csv", &source);
        }

        #[test]
        fn prop_data_flow_never_panics(source in "[a-zA-Z0-9 (){};='\n./,*_:@#$%^&\\[\\]-]+") {
            let e = engine();
            let _ = e.extract_data_flow("test.ts", &source);
            let _ = e.extract_data_flow("test.py", &source);
        }

        #[test]
        fn prop_ts_import_always_has_source(import_line in ts_import_strategy()) {
            let e = engine();
            let result = e.parse_file("test.ts", &import_line).unwrap();
            for imp in &result.imports {
                prop_assert!(!imp.source.is_empty(), "import source should not be empty");
            }
        }

        #[test]
        fn prop_ts_definition_always_has_name(def_line in ts_definition_strategy()) {
            let e = engine();
            let result = e.parse_file("test.ts", &def_line).unwrap();
            for def in &result.definitions {
                prop_assert!(!def.name.is_empty(), "definition name should not be empty");
                prop_assert!(def.start_line > 0, "start_line should be > 0");
            }
        }

        #[test]
        fn prop_python_source_has_valid_output(src in python_source_strategy()) {
            let e = engine();
            let result = e.parse_file("test.py", &src).unwrap();
            prop_assert_eq!(result.language, Language::Python);
            for imp in &result.imports {
                prop_assert!(!imp.source.is_empty());
            }
            for def in &result.definitions {
                prop_assert!(!def.name.is_empty());
            }
        }

        #[test]
        fn prop_deterministic(source in "[a-zA-Z0-9 (){};='./,*_\n]+") {
            let e = engine();
            let r1 = e.parse_file("test.ts", &source);
            let r2 = e.parse_file("test.ts", &source);
            match (r1, r2) {
                (Ok(a), Ok(b)) => prop_assert_eq!(a, b),
                (Err(_), Err(_)) => {}
                _ => prop_assert!(false, "inconsistent results"),
            }
        }

        #[test]
        fn prop_unknown_language_always_empty(source in ".*") {
            let e = engine();
            let result = e.parse_file("data.csv", &source).unwrap();
            prop_assert!(result.definitions.is_empty());
            prop_assert!(result.imports.is_empty());
            prop_assert!(result.exports.is_empty());
            prop_assert!(result.call_sites.is_empty());
        }

        #[test]
        fn prop_call_sites_have_callee(source in "[a-zA-Z_][a-zA-Z0-9_]*\\([a-zA-Z0-9_, ]*\\);?") {
            let e = engine();
            let result = e.parse_file("test.ts", &source);
            if let Ok(r) = result {
                for call in &r.call_sites {
                    prop_assert!(!call.callee.is_empty(), "call site callee should not be empty");
                    prop_assert!(call.line > 0, "call site line should be > 0");
                }
            }
        }

        #[test]
        fn prop_export_names_not_empty(idx in 0usize..5) {
            let sources = [
                "export function foo() {}",
                "export class Bar {}",
                "export { baz };",
                "export const X = 1;",
                "export type T = number;",
            ];
            let e = engine();
            let result = e.parse_file("test.ts", sources[idx]).unwrap();
            for exp in &result.exports {
                prop_assert!(!exp.name.is_empty());
            }
        }
    }
}
