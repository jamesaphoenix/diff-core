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
use once_cell::sync::OnceCell;
use std::cell::RefCell;
use std::collections::HashMap;
use thiserror::Error;
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

// ---------------------------------------------------------------------------
// Thread-local parser pool
// ---------------------------------------------------------------------------
//
// `tree_sitter::Parser` is `Send` but `!Sync`, so we cannot store it inside
// `QueryEngine` (which is shared via `&QueryEngine` across rayon threads).
// Instead, each thread maintains its own set of parsers keyed by `Language`.
// This avoids allocating a new `Parser` on every `parse_tree()` call while
// remaining safe for rayon's work-stealing parallelism.

thread_local! {
    static THREAD_PARSERS: RefCell<HashMap<Language, Parser>> = RefCell::new(HashMap::new());
}

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
    pub mod go {
        pub const IMPORTS: &str = include_str!("../queries/go/imports.scm");
        pub const DEFINITIONS: &str = include_str!("../queries/go/definitions.scm");
        pub const CALLS: &str = include_str!("../queries/go/calls.scm");
        pub const ASSIGNMENTS: &str = include_str!("../queries/go/assignments.scm");
    }
    pub mod rust {
        pub const IMPORTS: &str = include_str!("../queries/rust/imports.scm");
        pub const DEFINITIONS: &str = include_str!("../queries/rust/definitions.scm");
        pub const CALLS: &str = include_str!("../queries/rust/calls.scm");
        pub const ASSIGNMENTS: &str = include_str!("../queries/rust/assignments.scm");
    }
    pub mod java {
        pub const IMPORTS: &str = include_str!("../queries/java/imports.scm");
        pub const DEFINITIONS: &str = include_str!("../queries/java/definitions.scm");
        pub const CALLS: &str = include_str!("../queries/java/calls.scm");
        pub const ASSIGNMENTS: &str = include_str!("../queries/java/assignments.scm");
    }
    pub mod csharp {
        pub const IMPORTS: &str = include_str!("../queries/csharp/imports.scm");
        pub const DEFINITIONS: &str = include_str!("../queries/csharp/definitions.scm");
        pub const CALLS: &str = include_str!("../queries/csharp/calls.scm");
        pub const ASSIGNMENTS: &str = include_str!("../queries/csharp/assignments.scm");
    }
    pub mod php {
        pub const IMPORTS: &str = include_str!("../queries/php/imports.scm");
        pub const DEFINITIONS: &str = include_str!("../queries/php/definitions.scm");
        pub const CALLS: &str = include_str!("../queries/php/calls.scm");
        pub const ASSIGNMENTS: &str = include_str!("../queries/php/assignments.scm");
    }
    pub mod ruby {
        pub const IMPORTS: &str = include_str!("../queries/ruby/imports.scm");
        pub const DEFINITIONS: &str = include_str!("../queries/ruby/definitions.scm");
        pub const CALLS: &str = include_str!("../queries/ruby/calls.scm");
        pub const ASSIGNMENTS: &str = include_str!("../queries/ruby/assignments.scm");
    }
    pub mod kotlin {
        pub const IMPORTS: &str = include_str!("../queries/kotlin/imports.scm");
        pub const DEFINITIONS: &str = include_str!("../queries/kotlin/definitions.scm");
        pub const CALLS: &str = include_str!("../queries/kotlin/calls.scm");
        pub const ASSIGNMENTS: &str = include_str!("../queries/kotlin/assignments.scm");
    }
    pub mod swift {
        pub const IMPORTS: &str = include_str!("../queries/swift/imports.scm");
        pub const DEFINITIONS: &str = include_str!("../queries/swift/definitions.scm");
        pub const CALLS: &str = include_str!("../queries/swift/calls.scm");
        pub const ASSIGNMENTS: &str = include_str!("../queries/swift/assignments.scm");
    }
    pub mod c {
        pub const IMPORTS: &str = include_str!("../queries/c/imports.scm");
        pub const DEFINITIONS: &str = include_str!("../queries/c/definitions.scm");
        pub const CALLS: &str = include_str!("../queries/c/calls.scm");
        pub const ASSIGNMENTS: &str = include_str!("../queries/c/assignments.scm");
    }
    pub mod cpp {
        pub const IMPORTS: &str = include_str!("../queries/cpp/imports.scm");
        pub const DEFINITIONS: &str = include_str!("../queries/cpp/definitions.scm");
        pub const CALLS: &str = include_str!("../queries/cpp/calls.scm");
        pub const ASSIGNMENTS: &str = include_str!("../queries/cpp/assignments.scm");
    }
    pub mod scala {
        pub const IMPORTS: &str = include_str!("../queries/scala/imports.scm");
        pub const DEFINITIONS: &str = include_str!("../queries/scala/definitions.scm");
        pub const CALLS: &str = include_str!("../queries/scala/calls.scm");
        pub const ASSIGNMENTS: &str = include_str!("../queries/scala/assignments.scm");
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
    captures: Vec<(u32, Node<'tree>)>,
}

impl<'tree> CollectedMatch<'tree> {
    /// Check whether this match contains a capture with the given index.
    fn has_capture(&self, idx: Option<u32>) -> bool {
        match idx {
            Some(i) => self.captures.iter().any(|&(ci, _)| ci == i),
            None => false,
        }
    }

    /// Get the first node captured at the given index.
    fn get_capture(&self, idx: Option<u32>) -> Option<Node<'tree>> {
        idx.and_then(|i| {
            self.captures
                .iter()
                .find(|&&(ci, _)| ci == i)
                .map(|&(_, n)| n)
        })
    }
}

/// Collect all matches from a streaming iterator into owned data.
fn collect_matches<'tree>(
    cursor: &mut QueryCursor,
    query: &Query,
    root: Node<'tree>,
    source: &'tree [u8],
) -> Vec<CollectedMatch<'tree>> {
    let mut result = Vec::new();
    let mut matches = cursor.matches(query, root, source);
    loop {
        matches.advance();
        match matches.get() {
            Some(m) => {
                let caps: Vec<(u32, Node)> =
                    m.captures.iter().map(|c| (c.index, c.node)).collect();
                result.push(CollectedMatch { captures: caps });
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
/// Query compilation is lazy: `.scm` query files are compiled on first use per
/// language, not upfront. This means `QueryEngine::new()` is near-instant and
/// only the languages actually encountered in a diff pay the compilation cost.
pub struct QueryEngine {
    ts_queries: OnceCell<LanguageQueries>,
    py_queries: OnceCell<LanguageQueries>,
    go_queries: OnceCell<LanguageQueries>,
    rust_queries: OnceCell<LanguageQueries>,
    java_queries: OnceCell<LanguageQueries>,
    csharp_queries: OnceCell<LanguageQueries>,
    php_queries: OnceCell<LanguageQueries>,
    ruby_queries: OnceCell<LanguageQueries>,
    kotlin_queries: OnceCell<LanguageQueries>,
    swift_queries: OnceCell<LanguageQueries>,
    c_queries: OnceCell<LanguageQueries>,
    cpp_queries: OnceCell<LanguageQueries>,
    scala_queries: OnceCell<LanguageQueries>,
}

/// Compile all `.scm` queries for a single language into a [`LanguageQueries`].
fn compile_queries(
    ts_lang: tree_sitter::Language,
    lang_name: &str,
    imports_src: &str,
    exports_src: Option<&str>,
    definitions_src: &str,
    calls_src: &str,
    assignments_src: &str,
) -> Result<LanguageQueries, QueryEngineError> {
    Ok(LanguageQueries {
        language: ts_lang.clone(),
        imports: QueryWithCaptures::new(&ts_lang, imports_src, lang_name, "imports")?,
        exports: exports_src
            .map(|src| QueryWithCaptures::new(&ts_lang, src, lang_name, "exports"))
            .transpose()?,
        definitions: QueryWithCaptures::new(&ts_lang, definitions_src, lang_name, "definitions")?,
        calls: QueryWithCaptures::new(&ts_lang, calls_src, lang_name, "calls")?,
        assignments: QueryWithCaptures::new(&ts_lang, assignments_src, lang_name, "assignments")?,
    })
}

impl QueryEngine {
    /// Create a new query engine.
    ///
    /// Construction is near-instant: `.scm` query compilation is deferred to
    /// first use per language via [`OnceCell`].
    pub fn new() -> Result<Self, QueryEngineError> {
        Ok(Self {
            ts_queries: OnceCell::new(),
            py_queries: OnceCell::new(),
            go_queries: OnceCell::new(),
            rust_queries: OnceCell::new(),
            java_queries: OnceCell::new(),
            csharp_queries: OnceCell::new(),
            php_queries: OnceCell::new(),
            ruby_queries: OnceCell::new(),
            kotlin_queries: OnceCell::new(),
            swift_queries: OnceCell::new(),
            c_queries: OnceCell::new(),
            cpp_queries: OnceCell::new(),
            scala_queries: OnceCell::new(),
        })
    }

    /// Parse a source file and extract symbols, imports, exports, and call sites.
    ///
    /// This is the declarative equivalent of [`crate::ast::parse_file`].
    pub fn parse_file(&self, path: &str, source: &str) -> Result<ParsedFile, QueryEngineError> {
        let language = Language::from_path(path);
        let Some(lq) = self.get_lang_queries(language)? else {
            return Ok(ParsedFile {
                path: path.to_string(),
                language: Language::Unknown,
                definitions: vec![],
                imports: vec![],
                exports: vec![],
                call_sites: vec![],
            });
        };
        self.parse_with_queries(path, source, language, lq)
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
        let Some(lq) = self.get_lang_queries(language)? else {
            return Ok(DataFlowInfo {
                assignments: vec![],
                calls_with_args: vec![],
            });
        };

        let tree = self.parse_tree(source, language, &lq.language)?;
        let root = tree.root_node();
        let src = source.as_bytes();

        let assignments =
            self.extract_assignments(&root, src, &lq.assignments, language)?;
        let calls_with_args =
            self.extract_calls_with_args(&root, src, &lq.calls, language)?;

        Ok(DataFlowInfo {
            assignments,
            calls_with_args,
        })
    }

    // -----------------------------------------------------------------------
    // Language query resolution
    // -----------------------------------------------------------------------

    /// Resolve a [`Language`] to its compiled query set, compiling on first use.
    /// Returns `Ok(None)` for [`Language::Unknown`].
    fn get_lang_queries(
        &self,
        language: Language,
    ) -> Result<Option<&LanguageQueries>, QueryEngineError> {
        match language {
            Language::TypeScript | Language::JavaScript => self
                .ts_queries
                .get_or_try_init(|| {
                    compile_queries(
                        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                        "typescript",
                        queries::typescript::IMPORTS,
                        Some(queries::typescript::EXPORTS),
                        queries::typescript::DEFINITIONS,
                        queries::typescript::CALLS,
                        queries::typescript::ASSIGNMENTS,
                    )
                })
                .map(Some),
            Language::Python => self
                .py_queries
                .get_or_try_init(|| {
                    compile_queries(
                        tree_sitter_python::LANGUAGE.into(),
                        "python",
                        queries::python::IMPORTS,
                        None,
                        queries::python::DEFINITIONS,
                        queries::python::CALLS,
                        queries::python::ASSIGNMENTS,
                    )
                })
                .map(Some),
            Language::Go => self
                .go_queries
                .get_or_try_init(|| {
                    compile_queries(
                        tree_sitter_go::LANGUAGE.into(),
                        "go",
                        queries::go::IMPORTS,
                        None,
                        queries::go::DEFINITIONS,
                        queries::go::CALLS,
                        queries::go::ASSIGNMENTS,
                    )
                })
                .map(Some),
            Language::Rust => self
                .rust_queries
                .get_or_try_init(|| {
                    compile_queries(
                        tree_sitter_rust::LANGUAGE.into(),
                        "rust",
                        queries::rust::IMPORTS,
                        None,
                        queries::rust::DEFINITIONS,
                        queries::rust::CALLS,
                        queries::rust::ASSIGNMENTS,
                    )
                })
                .map(Some),
            Language::Java => self
                .java_queries
                .get_or_try_init(|| {
                    compile_queries(
                        tree_sitter_java::LANGUAGE.into(),
                        "java",
                        queries::java::IMPORTS,
                        None,
                        queries::java::DEFINITIONS,
                        queries::java::CALLS,
                        queries::java::ASSIGNMENTS,
                    )
                })
                .map(Some),
            Language::CSharp => self
                .csharp_queries
                .get_or_try_init(|| {
                    compile_queries(
                        tree_sitter_c_sharp::LANGUAGE.into(),
                        "csharp",
                        queries::csharp::IMPORTS,
                        None,
                        queries::csharp::DEFINITIONS,
                        queries::csharp::CALLS,
                        queries::csharp::ASSIGNMENTS,
                    )
                })
                .map(Some),
            Language::Php => self
                .php_queries
                .get_or_try_init(|| {
                    compile_queries(
                        tree_sitter_php::LANGUAGE_PHP.into(),
                        "php",
                        queries::php::IMPORTS,
                        None,
                        queries::php::DEFINITIONS,
                        queries::php::CALLS,
                        queries::php::ASSIGNMENTS,
                    )
                })
                .map(Some),
            Language::Ruby => self
                .ruby_queries
                .get_or_try_init(|| {
                    compile_queries(
                        tree_sitter_ruby::LANGUAGE.into(),
                        "ruby",
                        queries::ruby::IMPORTS,
                        None,
                        queries::ruby::DEFINITIONS,
                        queries::ruby::CALLS,
                        queries::ruby::ASSIGNMENTS,
                    )
                })
                .map(Some),
            Language::Kotlin => self
                .kotlin_queries
                .get_or_try_init(|| {
                    compile_queries(
                        tree_sitter_kotlin_ng::LANGUAGE.into(),
                        "kotlin",
                        queries::kotlin::IMPORTS,
                        None,
                        queries::kotlin::DEFINITIONS,
                        queries::kotlin::CALLS,
                        queries::kotlin::ASSIGNMENTS,
                    )
                })
                .map(Some),
            Language::Swift => self
                .swift_queries
                .get_or_try_init(|| {
                    compile_queries(
                        tree_sitter_swift::LANGUAGE.into(),
                        "swift",
                        queries::swift::IMPORTS,
                        None,
                        queries::swift::DEFINITIONS,
                        queries::swift::CALLS,
                        queries::swift::ASSIGNMENTS,
                    )
                })
                .map(Some),
            Language::C => self
                .c_queries
                .get_or_try_init(|| {
                    compile_queries(
                        tree_sitter_c::LANGUAGE.into(),
                        "c",
                        queries::c::IMPORTS,
                        None,
                        queries::c::DEFINITIONS,
                        queries::c::CALLS,
                        queries::c::ASSIGNMENTS,
                    )
                })
                .map(Some),
            Language::Cpp => self
                .cpp_queries
                .get_or_try_init(|| {
                    compile_queries(
                        tree_sitter_cpp::LANGUAGE.into(),
                        "cpp",
                        queries::cpp::IMPORTS,
                        None,
                        queries::cpp::DEFINITIONS,
                        queries::cpp::CALLS,
                        queries::cpp::ASSIGNMENTS,
                    )
                })
                .map(Some),
            Language::Scala => self
                .scala_queries
                .get_or_try_init(|| {
                    compile_queries(
                        tree_sitter_scala::LANGUAGE.into(),
                        "scala",
                        queries::scala::IMPORTS,
                        None,
                        queries::scala::DEFINITIONS,
                        queries::scala::CALLS,
                        queries::scala::ASSIGNMENTS,
                    )
                })
                .map(Some),
            Language::Unknown => Ok(None),
        }
    }

    // -----------------------------------------------------------------------
    // Tree parsing
    // -----------------------------------------------------------------------

    fn parse_tree(
        &self,
        source: &str,
        language: Language,
        ts_lang: &tree_sitter::Language,
    ) -> Result<tree_sitter::Tree, QueryEngineError> {
        THREAD_PARSERS.with(|parsers| {
            let mut parsers = parsers.borrow_mut();
            if !parsers.contains_key(&language) {
                let mut p = Parser::new();
                p.set_language(ts_lang)
                    .map_err(|e| QueryEngineError::LanguageError(e.to_string()))?;
                parsers.insert(language, p);
            }
            let parser = parsers.get_mut(&language).unwrap();
            parser
                .parse(source, None)
                .ok_or_else(|| {
                    QueryEngineError::ParseError("tree-sitter failed to parse".into())
                })
        })
    }

    /// Parse source into a tree-sitter `Tree` for the given file path.
    ///
    /// Returns the tree and detected language. For unknown languages, returns
    /// `Ok(None)` — callers should produce empty results.
    pub fn parse_tree_for_path(
        &self,
        path: &str,
        source: &str,
    ) -> Result<Option<(tree_sitter::Tree, Language)>, QueryEngineError> {
        let language = Language::from_path(path);
        let Some(lq) = self.get_lang_queries(language)? else {
            return Ok(None);
        };
        let tree = self.parse_tree(source, language, &lq.language)?;
        Ok(Some((tree, language)))
    }

    // -----------------------------------------------------------------------
    // Parse with pre-parsed tree (avoids double parse)
    // -----------------------------------------------------------------------

    /// Like [`parse_file`](Self::parse_file) but reuses an already-parsed tree.
    pub fn parse_file_with_tree(
        &self,
        path: &str,
        source: &str,
        tree: &tree_sitter::Tree,
        language: Language,
    ) -> Result<ParsedFile, QueryEngineError> {
        let Some(lq) = self.get_lang_queries(language)? else {
            return Ok(ParsedFile {
                path: path.to_string(),
                language: Language::Unknown,
                definitions: vec![],
                imports: vec![],
                exports: vec![],
                call_sites: vec![],
            });
        };
        self.parse_with_queries_from_tree(path, source, language, lq, tree)
    }

    /// Like [`extract_data_flow`](Self::extract_data_flow) but reuses an already-parsed tree.
    pub fn extract_data_flow_with_tree(
        &self,
        _path: &str,
        source: &str,
        tree: &tree_sitter::Tree,
        language: Language,
    ) -> Result<DataFlowInfo, QueryEngineError> {
        let Some(lq) = self.get_lang_queries(language)? else {
            return Ok(DataFlowInfo {
                assignments: vec![],
                calls_with_args: vec![],
            });
        };

        let root = tree.root_node();
        let src = source.as_bytes();

        let assignments =
            self.extract_assignments(&root, src, &lq.assignments, language)?;
        let calls_with_args =
            self.extract_calls_with_args(&root, src, &lq.calls, language)?;

        Ok(DataFlowInfo {
            assignments,
            calls_with_args,
        })
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
        let tree = self.parse_tree(source, language, &lang_queries.language)?;
        self.parse_with_queries_from_tree(path, source, language, lang_queries, &tree)
    }

    /// Core extraction logic — shared by both fresh-parse and tree-reuse paths.
    fn parse_with_queries_from_tree(
        &self,
        path: &str,
        source: &str,
        language: Language,
        lang_queries: &LanguageQueries,
        tree: &tree_sitter::Tree,
    ) -> Result<ParsedFile, QueryEngineError> {
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
            Language::Go => self.extract_go_imports(root, source, qwc),
            Language::Rust => self.extract_rust_imports(root, source, qwc),
            Language::Java => self.extract_java_imports(root, source, qwc),
            Language::CSharp => self.extract_csharp_imports(root, source, qwc),
            Language::Php => self.extract_php_imports(root, source, qwc),
            Language::Ruby => self.extract_ruby_imports(root, source, qwc),
            Language::Kotlin => self.extract_kotlin_imports(root, source, qwc),
            Language::Swift => self.extract_swift_imports(root, source, qwc),
            Language::C | Language::Cpp => self.extract_c_imports(root, source, qwc),
            Language::Scala => self.extract_scala_imports(root, source, qwc),
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

            if m.has_capture(default_name_idx) {
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
            } else if m.has_capture(aliased_name_idx) {
                // Named import with alias (check before named_name since aliased also has named_name)
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
            } else if m.has_capture(ns_name_idx) {
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
            } else if m.has_capture(named_name_idx) {
                // Named import
                for &(idx, node) in &m.captures {
                    if Some(idx) == named_name_idx {
                        let name = node_text(&node, source).to_string();
                        if !entry.names.iter().any(|n| n.name == name) {
                            entry.names.push(ImportedName { name, alias: None });
                        }
                    }
                }
            }
            // else: side-effect import — source already captured, no names
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
        let wildcard_idx = qwc.capture_index("wildcard");
        let relative_source_idx = qwc.capture_index("relative_source");
        let relative_imported_name_idx = qwc.capture_index("relative_imported_name");
        let relative_aliased_name_idx = qwc.capture_index("relative_aliased_name");
        let relative_alias_idx = qwc.capture_index("relative_alias");

        let mut import_map: Vec<(usize, ImportBuilder)> = Vec::new();

        for m in &matches {
            let mut stmt_start = 0usize;
            let mut line = 0usize;

            for &(idx, node) in &m.captures {
                if Some(idx) == stmt_idx {
                    stmt_start = node.start_byte();
                    line = node.start_position().row + 1;
                }
            }

            if m.has_capture(relative_aliased_name_idx) {
                // from .models import User as U (relative import with alias)
                let mut src = String::new();
                let mut imported = String::new();
                let mut alias = String::new();
                for &(idx, node) in &m.captures {
                    if Some(idx) == relative_source_idx {
                        src = node_text(&node, source).to_string();
                    }
                    if Some(idx) == relative_aliased_name_idx {
                        imported = node_text(&node, source).to_string();
                    }
                    if Some(idx) == relative_alias_idx {
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
            } else if m.has_capture(relative_source_idx) {
                // from .bar import baz (relative import, non-aliased)
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
            } else if m.has_capture(aliased_imported_name_idx) {
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
            } else if m.has_capture(wildcard_idx) {
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
            } else if m.has_capture(imported_name_idx) {
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
            } else if m.has_capture(alias_idx) {
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
            } else if m.has_capture(module_name_idx) {
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
        }

        Ok(import_map.into_iter().map(|(_, b)| b.build()).collect())
    }

    fn extract_go_imports(
        &self,
        root: &Node,
        source: &[u8],
        qwc: &QueryWithCaptures,
    ) -> Result<Vec<ImportInfo>, QueryEngineError> {
        let mut cursor = QueryCursor::new();
        let matches = collect_matches(&mut cursor, &qwc.query, *root, source);

        let stmt_idx = qwc.capture_index("stmt");
        let source_idx = qwc.capture_index("source");
        let alias_name_idx = qwc.capture_index("alias_name");
        let dot_import_idx = qwc.capture_index("dot_import");
        let blank_import_idx = qwc.capture_index("blank_import");

        let mut imports = Vec::new();
        let mut seen: Vec<(usize, String)> = Vec::new();

        for m in &matches {
            let mut line = 0usize;
            let mut source_text = String::new();

            for &(idx, node) in &m.captures {
                if Some(idx) == stmt_idx {
                    line = node.start_position().row + 1;
                }
                if Some(idx) == source_idx {
                    // Strip quotes from Go string literal
                    let raw = node_text(&node, source);
                    source_text = raw.trim_matches('"').to_string();
                    line = node.start_position().row + 1;
                }
            }

            if source_text.is_empty() {
                continue;
            }

            // Dedup: same source at same line
            let key = (line, source_text.clone());
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);

            let pkg_name = source_text
                .rsplit('/')
                .next()
                .unwrap_or(&source_text)
                .to_string();

            if m.has_capture(dot_import_idx) {
                // import . "path" — wildcard import
                imports.push(ImportInfo {
                    source: source_text,
                    names: vec![ImportedName {
                        name: "*".to_string(),
                        alias: None,
                    }],
                    is_default: false,
                    is_namespace: false,
                    line,
                });
            } else if m.has_capture(blank_import_idx) {
                // import _ "path" — side-effect only
                imports.push(ImportInfo {
                    source: source_text,
                    names: vec![],
                    is_default: false,
                    is_namespace: false,
                    line,
                });
            } else if m.has_capture(alias_name_idx) {
                // import alias "path"
                let mut alias_text = String::new();
                for &(idx, node) in &m.captures {
                    if Some(idx) == alias_name_idx {
                        alias_text = node_text(&node, source).to_string();
                    }
                }
                imports.push(ImportInfo {
                    source: source_text,
                    names: vec![ImportedName {
                        name: pkg_name,
                        alias: Some(alias_text),
                    }],
                    is_default: false,
                    is_namespace: true,
                    line,
                });
            } else {
                // Simple import "path"
                imports.push(ImportInfo {
                    source: source_text,
                    names: vec![ImportedName {
                        name: pkg_name,
                        alias: None,
                    }],
                    is_default: false,
                    is_namespace: true,
                    line,
                });
            }
        }

        Ok(imports)
    }

    fn extract_rust_imports(
        &self,
        root: &Node,
        source: &[u8],
        qwc: &QueryWithCaptures,
    ) -> Result<Vec<ImportInfo>, QueryEngineError> {
        let mut cursor = QueryCursor::new();
        let matches = collect_matches(&mut cursor, &qwc.query, *root, source);

        let stmt_idx = qwc.capture_index("stmt");
        let source_idx = qwc.capture_index("source");
        let alias_name_idx = qwc.capture_index("alias_name");
        let named_name_idx = qwc.capture_index("named_name");
        let aliased_name_idx = qwc.capture_index("aliased_name");
        let alias_idx = qwc.capture_index("alias");

        // Use the same ordered-map/builder pattern as TS imports to aggregate
        // multiple matches for the same use statement (e.g. use list items).
        let mut import_map: Vec<(usize, ImportBuilder)> = Vec::new();

        for m in &matches {
            let mut line = 0usize;
            let mut source_text = String::new();
            let mut stmt_start = 0usize;

            for &(idx, node) in &m.captures {
                if Some(idx) == stmt_idx {
                    stmt_start = node.start_byte();
                    line = node.start_position().row + 1;
                }
                if Some(idx) == source_idx {
                    source_text = node_text(&node, source).to_string();
                    if line == 0 {
                        line = node.start_position().row + 1;
                    }
                }
            }

            if source_text.is_empty() {
                continue;
            }

            let module_path = source_text;

            if m.has_capture(aliased_name_idx) {
                // use std::io::{Read as R};
                let mut imported = String::new();
                let mut alias_val = String::new();
                for &(idx, node) in &m.captures {
                    if Some(idx) == aliased_name_idx {
                        imported = node_text(&node, source).to_string();
                    }
                    if Some(idx) == alias_idx {
                        alias_val = node_text(&node, source).to_string();
                    }
                }
                let entry = get_or_insert_import(&mut import_map, stmt_start, &module_path, line);
                entry.names.retain(|n| n.name != imported);
                entry.names.push(ImportedName {
                    name: imported,
                    alias: if alias_val.is_empty() { None } else { Some(alias_val) },
                });
            } else if m.has_capture(alias_name_idx) {
                // use std::io as stdio;
                let mut alias_text = String::new();
                for &(idx, node) in &m.captures {
                    if Some(idx) == alias_name_idx {
                        alias_text = node_text(&node, source).to_string();
                    }
                }
                let name = module_path
                    .rsplit("::")
                    .next()
                    .unwrap_or(&module_path)
                    .to_string();
                let entry = get_or_insert_import(&mut import_map, stmt_start, &module_path, line);
                entry.is_namespace = true;
                entry.names.push(ImportedName {
                    name,
                    alias: Some(alias_text),
                });
            } else if m.has_capture(named_name_idx) {
                // use std::io::{Read, Write}; — may be called multiple times per use stmt
                let entry = get_or_insert_import(&mut import_map, stmt_start, &module_path, line);
                for &(idx, node) in &m.captures {
                    if Some(idx) == named_name_idx {
                        let name = node_text(&node, source).to_string();
                        if !entry.names.iter().any(|n| n.name == name) {
                            entry.names.push(ImportedName { name, alias: None });
                        }
                    }
                }
            } else {
                // Simple use: use std::io; or use serde;
                // Also handles glob: use std::io::*; (source is "std::io")
                let name = module_path
                    .rsplit("::")
                    .next()
                    .unwrap_or(&module_path)
                    .to_string();
                let entry = get_or_insert_import(&mut import_map, stmt_start, &module_path, line);
                entry.is_namespace = true;
                if !entry.names.iter().any(|n| n.name == name) {
                    entry.names.push(ImportedName { name, alias: None });
                }
            }
        }

        Ok(import_map.into_iter().map(|(_, b)| b.build()).collect())
    }

    fn extract_java_imports(
        &self,
        root: &Node,
        source: &[u8],
        qwc: &QueryWithCaptures,
    ) -> Result<Vec<ImportInfo>, QueryEngineError> {
        let mut cursor = QueryCursor::new();
        let matches = collect_matches(&mut cursor, &qwc.query, *root, source);

        let stmt_idx = qwc.capture_index("stmt");
        let source_idx = qwc.capture_index("source");
        let static_source_idx = qwc.capture_index("static_source");
        let wildcard_idx = qwc.capture_index("wildcard");
        let wildcard_source_idx = qwc.capture_index("wildcard_source");

        let mut imports = Vec::new();
        let mut seen: Vec<(usize, String)> = Vec::new();

        // First pass: collect lines that have wildcard imports so we skip
        // the regular source match for those lines.
        let mut wildcard_lines: Vec<usize> = Vec::new();
        for m in &matches {
            if m.has_capture(wildcard_idx) {
                let line = m
                    .get_capture(stmt_idx)
                    .map(|n| n.start_position().row + 1)
                    .unwrap_or(0);
                wildcard_lines.push(line);
            }
        }

        for m in &matches {
            let mut line = 0usize;

            for &(idx, node) in &m.captures {
                if Some(idx) == stmt_idx {
                    line = node.start_position().row + 1;
                }
            }

            if m.has_capture(wildcard_idx) {
                // import com.example.*; — wildcard import
                let pkg = m
                    .get_capture(wildcard_source_idx)
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                if !pkg.is_empty() {
                    let key = (line, pkg.clone());
                    if !seen.contains(&key) {
                        seen.push(key);
                        imports.push(ImportInfo {
                            source: pkg,
                            names: vec![ImportedName {
                                name: "*".to_string(),
                                alias: None,
                            }],
                            is_default: false,
                            is_namespace: false,
                            line,
                        });
                    }
                }
            } else if m.has_capture(static_source_idx) {
                // import static com.example.Foo.bar;
                let source_text = m
                    .get_capture(static_source_idx)
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                if !source_text.is_empty() {
                    // Split into package and member: com.example.Foo.bar -> source=com.example.Foo, name=bar
                    let (pkg, member) = if let Some(dot_pos) = source_text.rfind('.') {
                        (
                            source_text[..dot_pos].to_string(),
                            source_text[dot_pos + 1..].to_string(),
                        )
                    } else {
                        (source_text.clone(), source_text.clone())
                    };
                    let key = (line, source_text.clone());
                    if !seen.contains(&key) {
                        seen.push(key);
                        imports.push(ImportInfo {
                            source: pkg,
                            names: vec![ImportedName {
                                name: member,
                                alias: None,
                            }],
                            is_default: false,
                            is_namespace: false,
                            line,
                        });
                    }
                }
            } else if m.has_capture(source_idx) && !wildcard_lines.contains(&line) {
                // import com.example.Foo; — skip if this line is a wildcard import
                let source_text = m
                    .get_capture(source_idx)
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                if !source_text.is_empty() {
                    // Extract class name from fully qualified path
                    let class_name = source_text
                        .rsplit('.')
                        .next()
                        .unwrap_or(&source_text)
                        .to_string();
                    // Use the package (everything before the last dot) as source
                    let pkg = if let Some(dot_pos) = source_text.rfind('.') {
                        source_text[..dot_pos].to_string()
                    } else {
                        source_text.clone()
                    };
                    let key = (line, source_text.clone());
                    if !seen.contains(&key) {
                        seen.push(key);
                        imports.push(ImportInfo {
                            source: pkg,
                            names: vec![ImportedName {
                                name: class_name,
                                alias: None,
                            }],
                            is_default: false,
                            is_namespace: false,
                            line,
                        });
                    }
                }
            }
        }

        Ok(imports)
    }

    fn extract_csharp_imports(
        &self,
        root: &Node,
        source: &[u8],
        qwc: &QueryWithCaptures,
    ) -> Result<Vec<ImportInfo>, QueryEngineError> {
        let mut cursor = QueryCursor::new();
        let matches = collect_matches(&mut cursor, &qwc.query, *root, source);

        let stmt_idx = qwc.capture_index("stmt");
        let source_idx = qwc.capture_index("source");

        let mut imports = Vec::new();
        let mut seen: Vec<(usize, String)> = Vec::new();

        for m in &matches {
            let mut line = 0usize;

            for &(idx, node) in &m.captures {
                if Some(idx) == stmt_idx {
                    line = node.start_position().row + 1;
                }
            }

            if m.has_capture(source_idx) {
                let source_text = m
                    .get_capture(source_idx)
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                if source_text.is_empty() {
                    continue;
                }

                // Skip alias using directives — if the using_directive has a `name:`
                // field, it's `using Alias = Type;`, the captured source is the alias
                // identifier, not the namespace. We detect this by checking if the
                // stmt node text contains '='.
                let stmt_text = m
                    .get_capture(stmt_idx)
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                if stmt_text.contains('=') {
                    continue;
                }

                let key = (line, source_text.clone());
                if seen.contains(&key) {
                    continue;
                }
                seen.push(key);

                // C# `using` imports an entire namespace, not a single class.
                // Keep the full namespace as source for framework detection.
                // Use "*" as the name to indicate namespace import (like Java wildcard).
                imports.push(ImportInfo {
                    source: source_text,
                    names: vec![ImportedName {
                        name: "*".to_string(),
                        alias: None,
                    }],
                    is_default: false,
                    is_namespace: true,
                    line,
                });
            }
        }

        Ok(imports)
    }

    fn extract_php_imports(
        &self,
        root: &Node,
        source: &[u8],
        qwc: &QueryWithCaptures,
    ) -> Result<Vec<ImportInfo>, QueryEngineError> {
        let mut cursor = QueryCursor::new();
        let matches = collect_matches(&mut cursor, &qwc.query, *root, source);

        let stmt_idx = qwc.capture_index("stmt");
        let source_idx = qwc.capture_index("source");
        let alias_idx = qwc.capture_index("alias");
        let include_source_idx = qwc.capture_index("include_source");

        let mut imports: Vec<ImportInfo> = Vec::new();
        // Track seen imports by (line, source_text). If we see a later match
        // with an alias on the same line+source, update the existing import.
        let mut seen: Vec<(usize, String)> = Vec::new();

        for m in &matches {
            let mut line = 0usize;

            for &(idx, node) in &m.captures {
                if Some(idx) == stmt_idx {
                    line = node.start_position().row + 1;
                }
            }

            // Handle `use` imports (namespace imports)
            if m.has_capture(source_idx) {
                let source_text = m
                    .get_capture(source_idx)
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                if source_text.is_empty() {
                    continue;
                }

                let alias = m
                    .get_capture(alias_idx)
                    .map(|n| node_text(&n, source).to_string());

                let key = (line, source_text.clone());
                if let Some(pos) = seen.iter().position(|k| *k == key) {
                    // If this match has an alias and the existing one doesn't, update it
                    if alias.is_some() && imports[pos].names[0].alias.is_none() {
                        imports[pos].names[0].alias = alias;
                    }
                    continue;
                }
                seen.push(key);

                // Extract the class name (last segment) as the imported name
                let class_name = source_text
                    .rsplit('\\')
                    .next()
                    .unwrap_or(&source_text)
                    .to_string();

                // Use backslash-separated namespace path as source
                imports.push(ImportInfo {
                    source: source_text,
                    names: vec![ImportedName {
                        name: class_name,
                        alias,
                    }],
                    is_default: false,
                    is_namespace: false,
                    line,
                });
            }

            // Handle require/include (file inclusion)
            if m.has_capture(include_source_idx) {
                let path_text = m
                    .get_capture(include_source_idx)
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                if path_text.is_empty() {
                    continue;
                }

                let key = (line, path_text.clone());
                if seen.contains(&key) {
                    continue;
                }
                seen.push(key);

                imports.push(ImportInfo {
                    source: path_text,
                    names: vec![],
                    is_default: false,
                    is_namespace: false,
                    line,
                });
            }
        }

        Ok(imports)
    }

    fn extract_ruby_imports(
        &self,
        root: &Node,
        source: &[u8],
        qwc: &QueryWithCaptures,
    ) -> Result<Vec<ImportInfo>, QueryEngineError> {
        let mut cursor = QueryCursor::new();
        let matches = collect_matches(&mut cursor, &qwc.query, *root, source);

        let stmt_idx = qwc.capture_index("stmt");
        let source_idx = qwc.capture_index("source");
        let require_relative_idx = qwc.capture_index("require_relative_source");
        let include_name_idx = qwc.capture_index("include_name");

        let mut imports: Vec<ImportInfo> = Vec::new();
        let mut seen: Vec<(usize, String)> = Vec::new();

        for m in &matches {
            let mut line = 0usize;

            for &(idx, node) in &m.captures {
                if Some(idx) == stmt_idx {
                    line = node.start_position().row + 1;
                }
            }

            // Handle `require 'gem_name'`
            if m.has_capture(source_idx) {
                let source_text = m
                    .get_capture(source_idx)
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                if source_text.is_empty() {
                    continue;
                }

                let key = (line, source_text.clone());
                if seen.contains(&key) {
                    continue;
                }
                seen.push(key);

                imports.push(ImportInfo {
                    source: source_text,
                    names: vec![],
                    is_default: false,
                    is_namespace: false,
                    line,
                });
            }

            // Handle `require_relative '../models/user'`
            if m.has_capture(require_relative_idx) {
                let path_text = m
                    .get_capture(require_relative_idx)
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                if path_text.is_empty() {
                    continue;
                }

                let key = (line, path_text.clone());
                if seen.contains(&key) {
                    continue;
                }
                seen.push(key);

                imports.push(ImportInfo {
                    source: path_text,
                    names: vec![],
                    is_default: false,
                    is_namespace: false,
                    line,
                });
            }

            // Handle `include ModuleName` / `extend ModuleName`
            if m.has_capture(include_name_idx) {
                let mod_name = m
                    .get_capture(include_name_idx)
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                if mod_name.is_empty() {
                    continue;
                }

                let key = (line, mod_name.clone());
                if seen.contains(&key) {
                    continue;
                }
                seen.push(key);

                imports.push(ImportInfo {
                    source: mod_name.clone(),
                    names: vec![ImportedName {
                        name: mod_name,
                        alias: None,
                    }],
                    is_default: false,
                    is_namespace: false,
                    line,
                });
            }
        }

        Ok(imports)
    }

    fn extract_kotlin_imports(
        &self,
        root: &Node,
        source: &[u8],
        qwc: &QueryWithCaptures,
    ) -> Result<Vec<ImportInfo>, QueryEngineError> {
        let mut cursor = QueryCursor::new();
        let matches = collect_matches(&mut cursor, &qwc.query, *root, source);

        let stmt_idx = qwc.capture_index("stmt");
        let source_idx = qwc.capture_index("source");
        let alias_source_idx = qwc.capture_index("alias_source");
        let alias_name_idx = qwc.capture_index("alias_name");
        let wildcard_idx = qwc.capture_index("wildcard");
        let wildcard_source_idx = qwc.capture_index("wildcard_source");

        let mut imports = Vec::new();
        let mut seen: Vec<(usize, String)> = Vec::new();

        // First pass: collect lines that have wildcard or aliased imports
        let mut wildcard_lines: Vec<usize> = Vec::new();
        let mut alias_lines: Vec<usize> = Vec::new();
        for m in &matches {
            let line = m
                .get_capture(stmt_idx)
                .map(|n| n.start_position().row + 1)
                .unwrap_or(0);
            if m.has_capture(wildcard_idx) {
                wildcard_lines.push(line);
            }
            if m.has_capture(alias_name_idx) {
                alias_lines.push(line);
            }
        }

        for m in &matches {
            let line = m
                .get_capture(stmt_idx)
                .map(|n| n.start_position().row + 1)
                .unwrap_or(0);

            if m.has_capture(wildcard_idx) {
                // import com.example.*
                let pkg = m
                    .get_capture(wildcard_source_idx)
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                if !pkg.is_empty() {
                    let key = (line, pkg.clone());
                    if !seen.contains(&key) {
                        seen.push(key);
                        imports.push(ImportInfo {
                            source: pkg,
                            names: vec![ImportedName {
                                name: "*".to_string(),
                                alias: None,
                            }],
                            is_default: false,
                            is_namespace: false,
                            line,
                        });
                    }
                }
            } else if m.has_capture(alias_name_idx) {
                // import com.example.Foo as Bar
                let source_text = m
                    .get_capture(alias_source_idx)
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                let alias_text = m
                    .get_capture(alias_name_idx)
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                if !source_text.is_empty() {
                    let class_name = source_text
                        .rsplit('.')
                        .next()
                        .unwrap_or(&source_text)
                        .to_string();
                    let pkg = if let Some(dot_pos) = source_text.rfind('.') {
                        source_text[..dot_pos].to_string()
                    } else {
                        source_text.clone()
                    };
                    let key = (line, source_text.clone());
                    if !seen.contains(&key) {
                        seen.push(key);
                        imports.push(ImportInfo {
                            source: pkg,
                            names: vec![ImportedName {
                                name: class_name,
                                alias: if alias_text.is_empty() {
                                    None
                                } else {
                                    Some(alias_text)
                                },
                            }],
                            is_default: false,
                            is_namespace: false,
                            line,
                        });
                    }
                }
            } else if m.has_capture(source_idx)
                && !wildcard_lines.contains(&line)
                && !alias_lines.contains(&line)
            {
                // import com.example.Foo
                let source_text = m
                    .get_capture(source_idx)
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                if !source_text.is_empty() {
                    let class_name = source_text
                        .rsplit('.')
                        .next()
                        .unwrap_or(&source_text)
                        .to_string();
                    let pkg = if let Some(dot_pos) = source_text.rfind('.') {
                        source_text[..dot_pos].to_string()
                    } else {
                        source_text.clone()
                    };
                    let key = (line, source_text.clone());
                    if !seen.contains(&key) {
                        seen.push(key);
                        imports.push(ImportInfo {
                            source: pkg,
                            names: vec![ImportedName {
                                name: class_name,
                                alias: None,
                            }],
                            is_default: false,
                            is_namespace: false,
                            line,
                        });
                    }
                }
            }
        }

        Ok(imports)
    }

    fn extract_swift_imports(
        &self,
        root: &Node,
        source: &[u8],
        qwc: &QueryWithCaptures,
    ) -> Result<Vec<ImportInfo>, QueryEngineError> {
        let mut cursor = QueryCursor::new();
        let matches = collect_matches(&mut cursor, &qwc.query, *root, source);

        let stmt_idx = qwc.capture_index("stmt");
        let source_idx = qwc.capture_index("source");

        let mut imports = Vec::new();
        let mut seen: Vec<(usize, String)> = Vec::new();

        for m in &matches {
            let line = m
                .get_capture(stmt_idx)
                .map(|n| n.start_position().row + 1)
                .unwrap_or(0);

            if m.has_capture(source_idx) {
                // import Foundation / import Vapor
                // The identifier node contains the full module path as text
                let source_text = m
                    .get_capture(source_idx)
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                if !source_text.is_empty() {
                    let key = (line, source_text.clone());
                    if !seen.contains(&key) {
                        seen.push(key);
                        // Swift imports are module-level: `import Foundation`
                        // The module name is the source, imported name is the module itself
                        let module_name = source_text
                            .rsplit('.')
                            .next()
                            .unwrap_or(&source_text)
                            .to_string();
                        imports.push(ImportInfo {
                            source: source_text,
                            names: vec![ImportedName {
                                name: module_name,
                                alias: None,
                            }],
                            is_default: false,
                            is_namespace: true, // Swift imports bring the whole module into scope
                            line,
                        });
                    }
                }
            }
        }

        Ok(imports)
    }

    fn extract_c_imports(
        &self,
        root: &Node,
        source: &[u8],
        qwc: &QueryWithCaptures,
    ) -> Result<Vec<ImportInfo>, QueryEngineError> {
        let mut cursor = QueryCursor::new();
        let matches = collect_matches(&mut cursor, &qwc.query, *root, source);

        let stmt_idx = qwc.capture_index("stmt");
        let source_idx = qwc.capture_index("source");

        let mut imports = Vec::new();
        let mut seen: Vec<(usize, String)> = Vec::new();

        for m in &matches {
            let line = m
                .get_capture(stmt_idx)
                .map(|n| n.start_position().row + 1)
                .unwrap_or(0);

            if m.has_capture(source_idx) {
                let raw = m
                    .get_capture(source_idx)
                    .map(|n| node_text(&n, source).to_string())
                    .unwrap_or_default();
                // Strip quotes and angle brackets: "foo.h" -> foo.h, <stdio.h> -> stdio.h
                let source_text = raw
                    .trim_start_matches('"')
                    .trim_end_matches('"')
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_string();
                if !source_text.is_empty() {
                    let key = (line, source_text.clone());
                    if !seen.contains(&key) {
                        seen.push(key);
                        // Extract the header name without path for the imported name
                        let header_name = source_text
                            .rsplit('/')
                            .next()
                            .unwrap_or(&source_text)
                            .trim_end_matches(".h")
                            .trim_end_matches(".hpp")
                            .trim_end_matches(".hxx")
                            .to_string();
                        imports.push(ImportInfo {
                            source: source_text,
                            names: vec![ImportedName {
                                name: header_name,
                                alias: None,
                            }],
                            is_default: false,
                            is_namespace: true, // #include brings everything into scope
                            line,
                        });
                    }
                }
            }
        }

        Ok(imports)
    }

    fn extract_scala_imports(
        &self,
        root: &Node,
        source: &[u8],
        qwc: &QueryWithCaptures,
    ) -> Result<Vec<ImportInfo>, QueryEngineError> {
        let mut cursor = QueryCursor::new();
        let matches = collect_matches(&mut cursor, &qwc.query, *root, source);

        let stmt_idx = qwc.capture_index("stmt");

        let mut imports = Vec::new();

        for m in &matches {
            let stmt_node = match m.get_capture(stmt_idx) {
                Some(n) => n,
                None => continue,
            };
            let line = stmt_node.start_position().row + 1;

            // Walk import_declaration children to reconstruct the dotted path.
            // Children: `import`, identifier, `.`, identifier, `.`, ..., [namespace_selectors | namespace_wildcard]
            let mut path_parts: Vec<String> = Vec::new();
            let mut named_imports: Vec<String> = Vec::new();
            let mut is_wildcard = false;

            let mut child_cursor = stmt_node.walk();
            for child in stmt_node.children(&mut child_cursor) {
                match child.kind() {
                    "identifier" => {
                        path_parts.push(node_text(&child, source).to_string());
                    }
                    "namespace_selectors" => {
                        // { A, B } — collect named imports from inside selectors
                        let mut sel_cursor = child.walk();
                        for sel_child in child.children(&mut sel_cursor) {
                            if sel_child.kind() == "identifier" {
                                named_imports
                                    .push(node_text(&sel_child, source).to_string());
                            }
                        }
                    }
                    "namespace_wildcard" => {
                        is_wildcard = true;
                    }
                    _ => {} // skip `import`, `.`, etc.
                }
            }

            if path_parts.is_empty() {
                continue;
            }

            if is_wildcard {
                // import com.example._ → source = "com.example", namespace import
                let pkg = path_parts.join(".");
                imports.push(ImportInfo {
                    source: pkg,
                    names: vec![ImportedName {
                        name: "*".to_string(),
                        alias: None,
                    }],
                    is_default: false,
                    is_namespace: true,
                    line,
                });
            } else if !named_imports.is_empty() {
                // import akka.actor.{ActorSystem, Props} → source = "akka.actor"
                let pkg = path_parts.join(".");
                let names = named_imports
                    .into_iter()
                    .map(|n| ImportedName {
                        name: n,
                        alias: None,
                    })
                    .collect();
                imports.push(ImportInfo {
                    source: pkg,
                    names,
                    is_default: false,
                    is_namespace: false,
                    line,
                });
            } else {
                // import com.example.Foo → source = "com.example", name = "Foo"
                let class_name = path_parts.last().cloned().unwrap_or_default();
                let pkg = if path_parts.len() > 1 {
                    path_parts[..path_parts.len() - 1].join(".")
                } else {
                    class_name.clone()
                };
                imports.push(ImportInfo {
                    source: pkg,
                    names: vec![ImportedName {
                        name: class_name,
                        alias: None,
                    }],
                    is_default: false,
                    is_namespace: false,
                    line,
                });
            }
        }

        Ok(imports)
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

        // Exported declaration name captures — order doesn't matter,
        // we dispatch by which capture is present.
        let decl_name_captures: &[(Option<u32>, SymbolKind)] = &[
            (decl_fn_name_idx, SymbolKind::Function),
            (decl_gen_name_idx, SymbolKind::Function),
            (decl_class_name_idx, SymbolKind::Class),
            (decl_abstract_name_idx, SymbolKind::Class),
            (decl_iface_name_idx, SymbolKind::Interface),
            (decl_type_name_idx, SymbolKind::TypeAlias),
            (decl_var_name_idx, SymbolKind::Constant),
        ];

        for m in &matches {
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

            if m.has_capture(reexport_name_idx) {
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
            } else if m.has_capture(export_name_idx) {
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
            } else if m.has_capture(wildcard_source_idx) {
                // export * from './other'
                // Only treat as wildcard if there's no export_clause child
                // (re-export pattern already handles those).
                let has_export_clause = stmt_node
                    .map(|n| {
                        let mut c = n.walk();
                        let result = n
                            .named_children(&mut c)
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
            } else {
                // Exported declarations: function, generator, class, abstract, interface, type, variable
                for &(name_cap_idx, _) in decl_name_captures {
                    if m.has_capture(name_cap_idx) {
                        for &(idx, node) in &m.captures {
                            if Some(idx) == name_cap_idx {
                                exports.push(ExportInfo {
                                    name: node_text(&node, source).to_string(),
                                    is_default,
                                    is_reexport: false,
                                    source: None,
                                    line,
                                });
                            }
                        }
                        break;
                    }
                }
            }
        }

        dedup_exports(&mut exports);

        Ok(exports)
    }

    /// Extract definitions from exported declarations in exports.scm.
    /// Only matches that have a `decl_*_name` capture are declarations.
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

        let decl_name_captures: &[(Option<u32>, SymbolKind)] = &[
            (decl_fn_name_idx, SymbolKind::Function),
            (decl_gen_name_idx, SymbolKind::Function),
            (decl_class_name_idx, SymbolKind::Class),
            (decl_abstract_name_idx, SymbolKind::Class),
            (decl_iface_name_idx, SymbolKind::Interface),
            (decl_type_name_idx, SymbolKind::TypeAlias),
            (decl_var_name_idx, SymbolKind::Constant),
        ];

        let mut definitions = Vec::new();

        for m in &matches {
            let mut decl_node: Option<Node> = None;

            for &(idx, node) in &m.captures {
                if Some(idx) == stmt_idx {
                    if let Some(decl) = node.child_by_field_name("declaration") {
                        decl_node = Some(decl);
                    }
                }
            }

            // Find which declaration capture is present
            let mut found = false;
            for &(name_cap_idx, kind) in decl_name_captures {
                if m.has_capture(name_cap_idx) {
                    let mut name_text = String::new();
                    for &(idx, node) in &m.captures {
                        if Some(idx) == name_cap_idx {
                            name_text = node_text(&node, source).to_string();
                        }
                    }
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
                    found = true;
                    break;
                }
            }
            let _ = found; // suppress unused warning
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

        let mut definitions = Vec::new();
        let mut seen_nodes: Vec<(usize, usize)> = Vec::new();

        match language {
            Language::TypeScript | Language::JavaScript => {
                // TS/JS: each definition kind has a distinct capture name pair
                let fn_name_idx = qwc.capture_index("fn_name");
                let fn_node_idx = qwc.capture_index("fn_node");
                let gen_name_idx = qwc.capture_index("gen_name");
                let gen_node_idx = qwc.capture_index("gen_node");
                let class_name_idx = qwc.capture_index("class_name");
                let class_node_idx = qwc.capture_index("class_node");
                let abstract_name_idx = qwc.capture_index("abstract_name");
                let abstract_node_idx = qwc.capture_index("abstract_node");
                let iface_name_idx = qwc.capture_index("iface_name");
                let iface_node_idx = qwc.capture_index("iface_node");
                let type_name_idx = qwc.capture_index("type_name");
                let type_node_idx = qwc.capture_index("type_node");
                let arrow_name_idx = qwc.capture_index("arrow_name");
                let arrow_node_idx = qwc.capture_index("arrow_node");
                let fn_expr_name_idx = qwc.capture_index("fn_expr_name");
                let fn_expr_node_idx = qwc.capture_index("fn_expr_node");
                let const_name_idx = qwc.capture_index("const_name");
                let const_value_idx = qwc.capture_index("const_value");
                let const_node_idx = qwc.capture_index("const_node");
                let method_name_idx = qwc.capture_index("method_name");
                let method_node_idx = qwc.capture_index("method_node");

                // Ordered list: (name_capture, node_capture, kind).
                // const_name/const_value is special-cased below.
                let ts_def_captures: &[(Option<u32>, Option<u32>, SymbolKind)] = &[
                    (fn_name_idx, fn_node_idx, SymbolKind::Function),
                    (gen_name_idx, gen_node_idx, SymbolKind::Function),
                    (class_name_idx, class_node_idx, SymbolKind::Class),
                    (abstract_name_idx, abstract_node_idx, SymbolKind::Class),
                    (iface_name_idx, iface_node_idx, SymbolKind::Interface),
                    (type_name_idx, type_node_idx, SymbolKind::TypeAlias),
                    (arrow_name_idx, arrow_node_idx, SymbolKind::Function),
                    (fn_expr_name_idx, fn_expr_node_idx, SymbolKind::Function),
                    (method_name_idx, method_node_idx, SymbolKind::Function),
                ];

                for m in &matches {
                    // Skip the const_name pattern if the value is an arrow/function
                    // (those are already captured by arrow_name/fn_expr_name patterns)
                    if m.has_capture(const_name_idx) {
                        let has_fn_value = m
                            .get_capture(const_value_idx)
                            .map(|n| {
                                let k = n.kind();
                                k == "arrow_function" || k == "function" || k == "function_expression"
                            })
                            .unwrap_or(false);
                        if has_fn_value {
                            continue;
                        }
                        // Non-function constant
                        let name_text = m
                            .get_capture(const_name_idx)
                            .map(|n| node_text(&n, source).to_string())
                            .unwrap_or_default();
                        let (start_line, end_line, node_start) =
                            node_span(m, const_node_idx);
                        if !name_text.is_empty() {
                            let key = (node_start, hash_str(&name_text));
                            if !seen_nodes.contains(&key) {
                                seen_nodes.push(key);
                                definitions.push(Definition {
                                    name: name_text,
                                    kind: SymbolKind::Constant,
                                    start_line,
                                    end_line,
                                });
                            }
                        }
                        continue;
                    }

                    // Check each distinct definition capture
                    for &(name_cap, node_cap, kind) in ts_def_captures {
                        if m.has_capture(name_cap) {
                            let name_text = m
                                .get_capture(name_cap)
                                .map(|n| node_text(&n, source).to_string())
                                .unwrap_or_default();
                            let (start_line, end_line, node_start) =
                                node_span(m, node_cap);
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
                            break;
                        }
                    }
                }
            }
            Language::Python => {
                // Python: each definition kind has a distinct capture name pair
                let fn_name_idx = qwc.capture_index("fn_name");
                let fn_node_idx = qwc.capture_index("fn_node");
                let class_name_idx = qwc.capture_index("class_name");
                let class_node_idx = qwc.capture_index("class_node");
                let decorated_fn_name_idx = qwc.capture_index("decorated_fn_name");
                let decorated_fn_node_idx = qwc.capture_index("decorated_fn_node");
                let decorated_class_name_idx = qwc.capture_index("decorated_class_name");
                let decorated_class_node_idx = qwc.capture_index("decorated_class_node");
                let method_name_idx = qwc.capture_index("method_name");
                let method_node_idx = qwc.capture_index("method_node");
                let decorated_method_name_idx = qwc.capture_index("decorated_method_name");
                let decorated_method_node_idx = qwc.capture_index("decorated_method_node");

                let py_def_captures: &[(Option<u32>, Option<u32>, SymbolKind)] = &[
                    (fn_name_idx, fn_node_idx, SymbolKind::Function),
                    (class_name_idx, class_node_idx, SymbolKind::Class),
                    (decorated_fn_name_idx, decorated_fn_node_idx, SymbolKind::Function),
                    (decorated_class_name_idx, decorated_class_node_idx, SymbolKind::Class),
                    (method_name_idx, method_node_idx, SymbolKind::Function),
                    (
                        decorated_method_name_idx,
                        decorated_method_node_idx,
                        SymbolKind::Function,
                    ),
                ];

                for m in &matches {
                    for &(name_cap, node_cap, kind) in py_def_captures {
                        if m.has_capture(name_cap) {
                            let name_node = m.get_capture(name_cap);
                            let name_text = name_node
                                .map(|n| node_text(&n, source).to_string())
                                .unwrap_or_default();
                            let (start_line, end_line, _node_start) =
                                node_span(m, node_cap);
                            if !name_text.is_empty() {
                                // Dedup by name node start byte (not outer node) to
                                // prevent decorated functions/classes from being counted
                                // twice — the bare pattern and decorated pattern share
                                // the same inner name identifier node.
                                let name_start = name_node
                                    .map(|n| n.start_byte())
                                    .unwrap_or(0);
                                let key = (name_start, hash_str(&name_text));
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
                            break;
                        }
                    }
                }
            }
            Language::Go => {
                let fn_name_idx = qwc.capture_index("fn_name");
                let fn_node_idx = qwc.capture_index("fn_node");
                let method_name_idx = qwc.capture_index("method_name");
                let method_node_idx = qwc.capture_index("method_node");
                let struct_name_idx = qwc.capture_index("struct_name");
                let struct_node_idx = qwc.capture_index("struct_node");
                let iface_name_idx = qwc.capture_index("iface_name");
                let iface_node_idx = qwc.capture_index("iface_node");
                let type_name_idx = qwc.capture_index("type_name");
                let type_node_idx = qwc.capture_index("type_node");
                let const_name_idx = qwc.capture_index("const_name");
                let const_node_idx = qwc.capture_index("const_node");
                let var_decl_name_idx = qwc.capture_index("var_decl_name");
                let var_decl_node_idx = qwc.capture_index("var_decl_node");

                let go_def_captures: &[(Option<u32>, Option<u32>, SymbolKind)] = &[
                    (fn_name_idx, fn_node_idx, SymbolKind::Function),
                    (method_name_idx, method_node_idx, SymbolKind::Function),
                    (struct_name_idx, struct_node_idx, SymbolKind::Class),
                    (iface_name_idx, iface_node_idx, SymbolKind::Interface),
                    (type_name_idx, type_node_idx, SymbolKind::TypeAlias),
                    (const_name_idx, const_node_idx, SymbolKind::Constant),
                    (var_decl_name_idx, var_decl_node_idx, SymbolKind::Constant),
                ];

                for m in &matches {
                    for &(name_cap, node_cap, kind) in go_def_captures {
                        if m.has_capture(name_cap) {
                            let name_text = m
                                .get_capture(name_cap)
                                .map(|n| node_text(&n, source).to_string())
                                .unwrap_or_default();
                            let (start_line, end_line, node_start) =
                                node_span(m, node_cap);
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
                            break;
                        }
                    }
                }
            }
            Language::Java => {
                let method_name_idx = qwc.capture_index("method_name");
                let method_node_idx = qwc.capture_index("method_node");
                let ctor_name_idx = qwc.capture_index("ctor_name");
                let ctor_node_idx = qwc.capture_index("ctor_node");
                let class_name_idx = qwc.capture_index("class_name");
                let class_node_idx = qwc.capture_index("class_node");
                let iface_name_idx = qwc.capture_index("iface_name");
                let iface_node_idx = qwc.capture_index("iface_node");
                let enum_name_idx = qwc.capture_index("enum_name");
                let enum_node_idx = qwc.capture_index("enum_node");
                let annotation_name_idx = qwc.capture_index("annotation_name");
                let annotation_node_idx = qwc.capture_index("annotation_node");
                let field_name_idx = qwc.capture_index("field_name");
                let field_node_idx = qwc.capture_index("field_node");

                let java_def_captures: &[(Option<u32>, Option<u32>, SymbolKind)] = &[
                    (method_name_idx, method_node_idx, SymbolKind::Function),
                    (ctor_name_idx, ctor_node_idx, SymbolKind::Function),
                    (class_name_idx, class_node_idx, SymbolKind::Class),
                    (iface_name_idx, iface_node_idx, SymbolKind::Interface),
                    (enum_name_idx, enum_node_idx, SymbolKind::Class),
                    (annotation_name_idx, annotation_node_idx, SymbolKind::Interface),
                    (field_name_idx, field_node_idx, SymbolKind::Constant),
                ];

                for m in &matches {
                    for &(name_cap, node_cap, kind) in java_def_captures {
                        if m.has_capture(name_cap) {
                            let name_text = m
                                .get_capture(name_cap)
                                .map(|n| node_text(&n, source).to_string())
                                .unwrap_or_default();
                            let (start_line, end_line, node_start) =
                                node_span(m, node_cap);
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
                            break;
                        }
                    }
                }
            }
            Language::Rust => {
                let fn_name_idx = qwc.capture_index("fn_name");
                let fn_node_idx = qwc.capture_index("fn_node");
                let struct_name_idx = qwc.capture_index("struct_name");
                let struct_node_idx = qwc.capture_index("struct_node");
                let enum_name_idx = qwc.capture_index("enum_name");
                let enum_node_idx = qwc.capture_index("enum_node");
                let trait_name_idx = qwc.capture_index("trait_name");
                let trait_node_idx = qwc.capture_index("trait_node");
                let type_name_idx = qwc.capture_index("type_name");
                let type_node_idx = qwc.capture_index("type_node");
                let const_name_idx = qwc.capture_index("const_name");
                let const_node_idx = qwc.capture_index("const_node");
                let static_name_idx = qwc.capture_index("static_name");
                let static_node_idx = qwc.capture_index("static_node");
                let method_name_idx = qwc.capture_index("method_name");
                let method_node_idx = qwc.capture_index("method_node");
                let macro_name_idx = qwc.capture_index("macro_name");
                let macro_node_idx = qwc.capture_index("macro_node");

                let rust_def_captures: &[(Option<u32>, Option<u32>, SymbolKind)] = &[
                    (fn_name_idx, fn_node_idx, SymbolKind::Function),
                    (method_name_idx, method_node_idx, SymbolKind::Function),
                    (struct_name_idx, struct_node_idx, SymbolKind::Class),
                    (enum_name_idx, enum_node_idx, SymbolKind::Class),
                    (trait_name_idx, trait_node_idx, SymbolKind::Interface),
                    (type_name_idx, type_node_idx, SymbolKind::TypeAlias),
                    (const_name_idx, const_node_idx, SymbolKind::Constant),
                    (static_name_idx, static_node_idx, SymbolKind::Constant),
                    (macro_name_idx, macro_node_idx, SymbolKind::Function),
                ];

                for m in &matches {
                    for &(name_cap, node_cap, kind) in rust_def_captures {
                        if m.has_capture(name_cap) {
                            let name_text = m
                                .get_capture(name_cap)
                                .map(|n| node_text(&n, source).to_string())
                                .unwrap_or_default();
                            let (start_line, end_line, node_start) =
                                node_span(m, node_cap);
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
                            break;
                        }
                    }
                }
            }
            Language::CSharp => {
                let method_name_idx = qwc.capture_index("method_name");
                let method_node_idx = qwc.capture_index("method_node");
                let ctor_name_idx = qwc.capture_index("ctor_name");
                let ctor_node_idx = qwc.capture_index("ctor_node");
                let class_name_idx = qwc.capture_index("class_name");
                let class_node_idx = qwc.capture_index("class_node");
                let struct_name_idx = qwc.capture_index("struct_name");
                let struct_node_idx = qwc.capture_index("struct_node");
                let iface_name_idx = qwc.capture_index("iface_name");
                let iface_node_idx = qwc.capture_index("iface_node");
                let enum_name_idx = qwc.capture_index("enum_name");
                let enum_node_idx = qwc.capture_index("enum_node");
                let record_name_idx = qwc.capture_index("record_name");
                let record_node_idx = qwc.capture_index("record_node");
                let prop_name_idx = qwc.capture_index("prop_name");
                let prop_node_idx = qwc.capture_index("prop_node");
                let field_name_idx = qwc.capture_index("field_name");
                let field_node_idx = qwc.capture_index("field_node");
                let delegate_name_idx = qwc.capture_index("delegate_name");
                let delegate_node_idx = qwc.capture_index("delegate_node");

                let csharp_def_captures: &[(Option<u32>, Option<u32>, SymbolKind)] = &[
                    (method_name_idx, method_node_idx, SymbolKind::Function),
                    (ctor_name_idx, ctor_node_idx, SymbolKind::Function),
                    (class_name_idx, class_node_idx, SymbolKind::Class),
                    (struct_name_idx, struct_node_idx, SymbolKind::Class),
                    (iface_name_idx, iface_node_idx, SymbolKind::Interface),
                    (enum_name_idx, enum_node_idx, SymbolKind::Class),
                    (record_name_idx, record_node_idx, SymbolKind::Class),
                    (prop_name_idx, prop_node_idx, SymbolKind::Constant),
                    (field_name_idx, field_node_idx, SymbolKind::Constant),
                    (delegate_name_idx, delegate_node_idx, SymbolKind::Interface),
                ];

                for m in &matches {
                    for &(name_cap, node_cap, kind) in csharp_def_captures {
                        if m.has_capture(name_cap) {
                            let name_text = m
                                .get_capture(name_cap)
                                .map(|n| node_text(&n, source).to_string())
                                .unwrap_or_default();
                            let (start_line, end_line, node_start) =
                                node_span(m, node_cap);
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
                            break;
                        }
                    }
                }
            }
            Language::Php => {
                let method_name_idx = qwc.capture_index("method_name");
                let method_node_idx = qwc.capture_index("method_node");
                let func_name_idx = qwc.capture_index("func_name");
                let func_node_idx = qwc.capture_index("func_node");
                let class_name_idx = qwc.capture_index("class_name");
                let class_node_idx = qwc.capture_index("class_node");
                let iface_name_idx = qwc.capture_index("iface_name");
                let iface_node_idx = qwc.capture_index("iface_node");
                let trait_name_idx = qwc.capture_index("trait_name");
                let trait_node_idx = qwc.capture_index("trait_node");
                let enum_name_idx = qwc.capture_index("enum_name");
                let enum_node_idx = qwc.capture_index("enum_node");
                let const_name_idx = qwc.capture_index("const_name");
                let const_node_idx = qwc.capture_index("const_node");

                let php_def_captures: &[(Option<u32>, Option<u32>, SymbolKind)] = &[
                    (method_name_idx, method_node_idx, SymbolKind::Function),
                    (func_name_idx, func_node_idx, SymbolKind::Function),
                    (class_name_idx, class_node_idx, SymbolKind::Class),
                    (iface_name_idx, iface_node_idx, SymbolKind::Interface),
                    (trait_name_idx, trait_node_idx, SymbolKind::Interface),
                    (enum_name_idx, enum_node_idx, SymbolKind::TypeAlias),
                    (const_name_idx, const_node_idx, SymbolKind::Constant),
                ];

                for m in &matches {
                    for &(name_cap, node_cap, kind) in php_def_captures {
                        if m.has_capture(name_cap) {
                            let name_node = m.get_capture(name_cap);
                            let name_text = name_node
                                .map(|n| node_text(&n, source).to_string())
                                .unwrap_or_default();
                            let (start_line, end_line, _node_start) =
                                node_span(m, node_cap);
                            if !name_text.is_empty() {
                                let name_start = name_node
                                    .map(|n| n.start_byte())
                                    .unwrap_or(0);
                                let key = (name_start, hash_str(&name_text));
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
                            break;
                        }
                    }
                }
            }
            Language::Ruby => {
                let method_name_idx = qwc.capture_index("method_name");
                let method_node_idx = qwc.capture_index("method_node");
                let singleton_method_name_idx = qwc.capture_index("singleton_method_name");
                let singleton_method_node_idx = qwc.capture_index("singleton_method_node");
                let class_name_idx = qwc.capture_index("class_name");
                let class_node_idx = qwc.capture_index("class_node");
                let module_name_idx = qwc.capture_index("module_name");
                let module_node_idx = qwc.capture_index("module_node");
                let const_name_idx = qwc.capture_index("const_name");
                let const_node_idx = qwc.capture_index("const_node");

                let ruby_def_captures: &[(Option<u32>, Option<u32>, SymbolKind)] = &[
                    (method_name_idx, method_node_idx, SymbolKind::Function),
                    (singleton_method_name_idx, singleton_method_node_idx, SymbolKind::Function),
                    (class_name_idx, class_node_idx, SymbolKind::Class),
                    (module_name_idx, module_node_idx, SymbolKind::Module),
                    (const_name_idx, const_node_idx, SymbolKind::Constant),
                ];

                for m in &matches {
                    for &(name_cap, node_cap, kind) in ruby_def_captures {
                        if m.has_capture(name_cap) {
                            let name_node = m.get_capture(name_cap);
                            let name_text = name_node
                                .map(|n| node_text(&n, source).to_string())
                                .unwrap_or_default();
                            let (start_line, end_line, _node_start) =
                                node_span(m, node_cap);
                            if !name_text.is_empty() {
                                let name_start = name_node
                                    .map(|n| n.start_byte())
                                    .unwrap_or(0);
                                let key = (name_start, hash_str(&name_text));
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
                            break;
                        }
                    }
                }
            }
            Language::Kotlin => {
                let func_name_idx = qwc.capture_index("func_name");
                let func_node_idx = qwc.capture_index("func_node");
                let class_name_idx = qwc.capture_index("class_name");
                let class_node_idx = qwc.capture_index("class_node");
                let object_name_idx = qwc.capture_index("object_name");
                let object_node_idx = qwc.capture_index("object_node");
                let prop_name_idx = qwc.capture_index("prop_name");
                let prop_node_idx = qwc.capture_index("prop_node");
                let typealias_name_idx = qwc.capture_index("typealias_name");
                let typealias_node_idx = qwc.capture_index("typealias_node");

                let kotlin_def_captures: &[(Option<u32>, Option<u32>, SymbolKind)] = &[
                    (func_name_idx, func_node_idx, SymbolKind::Function),
                    (class_name_idx, class_node_idx, SymbolKind::Class),
                    (object_name_idx, object_node_idx, SymbolKind::Class),
                    (prop_name_idx, prop_node_idx, SymbolKind::Constant),
                    (typealias_name_idx, typealias_node_idx, SymbolKind::TypeAlias),
                ];

                for m in &matches {
                    for &(name_cap, node_cap, kind) in kotlin_def_captures {
                        if m.has_capture(name_cap) {
                            let name_node = m.get_capture(name_cap);
                            let name_text = name_node
                                .map(|n| node_text(&n, source).to_string())
                                .unwrap_or_default();
                            let (start_line, end_line, _node_start) =
                                node_span(m, node_cap);
                            if !name_text.is_empty() {
                                let name_start = name_node
                                    .map(|n| n.start_byte())
                                    .unwrap_or(0);
                                let key = (name_start, hash_str(&name_text));
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
                            break;
                        }
                    }
                }
            }
            Language::Swift => {
                let func_name_idx = qwc.capture_index("func_name");
                let func_node_idx = qwc.capture_index("func_node");
                let class_name_idx = qwc.capture_index("class_name");
                let class_node_idx = qwc.capture_index("class_node");
                let protocol_name_idx = qwc.capture_index("protocol_name");
                let protocol_node_idx = qwc.capture_index("protocol_node");
                let proto_func_name_idx = qwc.capture_index("proto_func_name");
                let proto_func_node_idx = qwc.capture_index("proto_func_node");
                let prop_name_idx = qwc.capture_index("prop_name");
                let prop_node_idx = qwc.capture_index("prop_node");
                let typealias_name_idx = qwc.capture_index("typealias_name");
                let typealias_node_idx = qwc.capture_index("typealias_node");

                let swift_def_captures: &[(Option<u32>, Option<u32>, SymbolKind)] = &[
                    (func_name_idx, func_node_idx, SymbolKind::Function),
                    (class_name_idx, class_node_idx, SymbolKind::Class),
                    (protocol_name_idx, protocol_node_idx, SymbolKind::Interface),
                    (proto_func_name_idx, proto_func_node_idx, SymbolKind::Function),
                    (prop_name_idx, prop_node_idx, SymbolKind::Constant),
                    (typealias_name_idx, typealias_node_idx, SymbolKind::TypeAlias),
                ];

                for m in &matches {
                    for &(name_cap, node_cap, kind) in swift_def_captures {
                        if m.has_capture(name_cap) {
                            let name_node = m.get_capture(name_cap);
                            let name_text = name_node
                                .map(|n| node_text(&n, source).to_string())
                                .unwrap_or_default();
                            let (start_line, end_line, _node_start) =
                                node_span(m, node_cap);
                            if !name_text.is_empty() {
                                let name_start = name_node
                                    .map(|n| n.start_byte())
                                    .unwrap_or(0);
                                let key = (name_start, hash_str(&name_text));
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
                            break;
                        }
                    }
                }
            }
            Language::C => {
                let func_name_idx = qwc.capture_index("func_name");
                let func_node_idx = qwc.capture_index("func_node");
                let struct_name_idx = qwc.capture_index("struct_name");
                let struct_node_idx = qwc.capture_index("struct_node");
                let enum_name_idx = qwc.capture_index("enum_name");
                let enum_node_idx = qwc.capture_index("enum_node");
                let union_name_idx = qwc.capture_index("union_name");
                let union_node_idx = qwc.capture_index("union_node");
                let typedef_name_idx = qwc.capture_index("typedef_name");
                let typedef_node_idx = qwc.capture_index("typedef_node");
                let global_name_idx = qwc.capture_index("global_name");
                let global_node_idx = qwc.capture_index("global_node");

                let c_def_captures: &[(Option<u32>, Option<u32>, SymbolKind)] = &[
                    (func_name_idx, func_node_idx, SymbolKind::Function),
                    (struct_name_idx, struct_node_idx, SymbolKind::Class),
                    (enum_name_idx, enum_node_idx, SymbolKind::Class),
                    (union_name_idx, union_node_idx, SymbolKind::Class),
                    (typedef_name_idx, typedef_node_idx, SymbolKind::TypeAlias),
                    (global_name_idx, global_node_idx, SymbolKind::Constant),
                ];

                for m in &matches {
                    for &(name_cap, node_cap, kind) in c_def_captures {
                        if m.has_capture(name_cap) {
                            let name_node = m.get_capture(name_cap);
                            let name_text = name_node
                                .map(|n| node_text(&n, source).to_string())
                                .unwrap_or_default();
                            let (start_line, end_line, _node_start) =
                                node_span(m, node_cap);
                            if !name_text.is_empty() {
                                let name_start = name_node
                                    .map(|n| n.start_byte())
                                    .unwrap_or(0);
                                let key = (name_start, hash_str(&name_text));
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
                            break;
                        }
                    }
                }
            }
            Language::Cpp => {
                let func_name_idx = qwc.capture_index("func_name");
                let func_node_idx = qwc.capture_index("func_node");
                let method_name_idx = qwc.capture_index("method_name");
                let method_node_idx = qwc.capture_index("method_node");
                let class_name_idx = qwc.capture_index("class_name");
                let class_node_idx = qwc.capture_index("class_node");
                let struct_name_idx = qwc.capture_index("struct_name");
                let struct_node_idx = qwc.capture_index("struct_node");
                let enum_name_idx = qwc.capture_index("enum_name");
                let enum_node_idx = qwc.capture_index("enum_node");
                let namespace_name_idx = qwc.capture_index("namespace_name");
                let namespace_node_idx = qwc.capture_index("namespace_node");
                let alias_name_idx = qwc.capture_index("alias_name");
                let alias_node_idx = qwc.capture_index("alias_node");
                let template_func_name_idx = qwc.capture_index("template_func_name");
                let template_func_node_idx = qwc.capture_index("template_func_node");
                let template_class_name_idx = qwc.capture_index("template_class_name");
                let template_class_node_idx = qwc.capture_index("template_class_node");

                let cpp_def_captures: &[(Option<u32>, Option<u32>, SymbolKind)] = &[
                    (func_name_idx, func_node_idx, SymbolKind::Function),
                    (method_name_idx, method_node_idx, SymbolKind::Function),
                    (class_name_idx, class_node_idx, SymbolKind::Class),
                    (struct_name_idx, struct_node_idx, SymbolKind::Class),
                    (enum_name_idx, enum_node_idx, SymbolKind::Class),
                    (namespace_name_idx, namespace_node_idx, SymbolKind::Module),
                    (alias_name_idx, alias_node_idx, SymbolKind::TypeAlias),
                    (template_func_name_idx, template_func_node_idx, SymbolKind::Function),
                    (template_class_name_idx, template_class_node_idx, SymbolKind::Class),
                ];

                for m in &matches {
                    for &(name_cap, node_cap, kind) in cpp_def_captures {
                        if m.has_capture(name_cap) {
                            let name_node = m.get_capture(name_cap);
                            let name_text = name_node
                                .map(|n| node_text(&n, source).to_string())
                                .unwrap_or_default();
                            let (start_line, end_line, _node_start) =
                                node_span(m, node_cap);
                            if !name_text.is_empty() {
                                let name_start = name_node
                                    .map(|n| n.start_byte())
                                    .unwrap_or(0);
                                let key = (name_start, hash_str(&name_text));
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
                            break;
                        }
                    }
                }
            }
            Language::Scala => {
                let func_name_idx = qwc.capture_index("func_name");
                let func_node_idx = qwc.capture_index("func_node");
                let class_name_idx = qwc.capture_index("class_name");
                let class_node_idx = qwc.capture_index("class_node");
                let trait_name_idx = qwc.capture_index("trait_name");
                let trait_node_idx = qwc.capture_index("trait_node");
                let object_name_idx = qwc.capture_index("object_name");
                let object_node_idx = qwc.capture_index("object_node");
                let prop_name_idx = qwc.capture_index("prop_name");
                let prop_node_idx = qwc.capture_index("prop_node");
                let typealias_name_idx = qwc.capture_index("typealias_name");
                let typealias_node_idx = qwc.capture_index("typealias_node");

                let scala_def_captures: &[(Option<u32>, Option<u32>, SymbolKind)] = &[
                    (func_name_idx, func_node_idx, SymbolKind::Function),
                    (class_name_idx, class_node_idx, SymbolKind::Class),
                    (trait_name_idx, trait_node_idx, SymbolKind::Interface),
                    (object_name_idx, object_node_idx, SymbolKind::Class),
                    (prop_name_idx, prop_node_idx, SymbolKind::Constant),
                    (typealias_name_idx, typealias_node_idx, SymbolKind::TypeAlias),
                ];

                for m in &matches {
                    for &(name_cap, node_cap, kind) in scala_def_captures {
                        if m.has_capture(name_cap) {
                            let name_node = m.get_capture(name_cap);
                            let name_text = name_node
                                .map(|n| node_text(&n, source).to_string())
                                .unwrap_or_default();
                            let (start_line, end_line, _node_start) =
                                node_span(m, node_cap);
                            if !name_text.is_empty() {
                                let name_start = name_node
                                    .map(|n| n.start_byte())
                                    .unwrap_or(0);
                                let key = (name_start, hash_str(&name_text));
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
                            break;
                        }
                    }
                }
            }
            _ => {}
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

/// Extract (start_line, end_line, start_byte) from a node capture.
fn node_span(m: &CollectedMatch, node_cap: Option<u32>) -> (usize, usize, usize) {
    m.get_capture(node_cap)
        .map(|n| {
            (
                n.start_position().row + 1,
                n.end_position().row + 1,
                n.start_byte(),
            )
        })
        .unwrap_or((0, 0, 0))
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
        Language::Go => &["function_declaration", "method_declaration"],
        Language::Rust => &["function_item"],
        Language::Java => &["method_declaration", "constructor_declaration"],
        Language::CSharp => &["method_declaration", "constructor_declaration"],
        Language::Php => &["method_declaration", "function_definition"],
        Language::Ruby => &["method", "singleton_method"],
        Language::Kotlin => &["function_declaration"],
        Language::Swift => &["function_declaration"],
        Language::C => &["function_definition"],
        Language::Cpp => &["function_definition"],
        Language::Scala => &["function_definition", "function_declaration"],
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
                .map(|v| {
                    v.kind() == "arrow_function"
                        || v.kind() == "function"
                        || v.kind() == "function_expression"
                })
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
// Shared test QueryEngine (file-level, reused by all test modules)
// ---------------------------------------------------------------------------

/// Single shared `QueryEngine` across all test modules in this file.
/// Lazy query compilation per language happens once and is reused by every test.
#[cfg(test)]
fn shared_test_engine() -> &'static QueryEngine {
    use std::sync::OnceLock;
    static ENGINE: OnceLock<QueryEngine> = OnceLock::new();
    ENGINE.get_or_init(|| QueryEngine::new().expect("shared test QueryEngine init"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod tests {
    use super::*;

    fn engine() -> &'static QueryEngine {
        shared_test_engine()
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
// Lazy initialization tests (Phase 12.3)
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod lazy_init_tests {
    use super::*;

    #[test]
    fn new_is_instant_no_queries_compiled() {
        // Construction should succeed without compiling any queries.
        let engine = QueryEngine::new().unwrap();
        // All OnceCells should be uninitialized.
        assert!(engine.ts_queries.get().is_none());
        assert!(engine.py_queries.get().is_none());
        assert!(engine.go_queries.get().is_none());
        assert!(engine.rust_queries.get().is_none());
        assert!(engine.java_queries.get().is_none());
        assert!(engine.csharp_queries.get().is_none());
        assert!(engine.php_queries.get().is_none());
        assert!(engine.ruby_queries.get().is_none());
        assert!(engine.kotlin_queries.get().is_none());
        assert!(engine.swift_queries.get().is_none());
        assert!(engine.c_queries.get().is_none());
        assert!(engine.cpp_queries.get().is_none());
        assert!(engine.scala_queries.get().is_none());
    }

    #[test]
    fn first_ts_parse_compiles_only_typescript() {
        let engine = QueryEngine::new().unwrap();
        let _ = engine.parse_file("app.ts", "import x from 'y';").unwrap();
        // Only TypeScript queries should be compiled.
        assert!(engine.ts_queries.get().is_some());
        assert!(engine.py_queries.get().is_none());
        assert!(engine.go_queries.get().is_none());
        assert!(engine.rust_queries.get().is_none());
    }

    #[test]
    fn first_py_parse_compiles_only_python() {
        let engine = QueryEngine::new().unwrap();
        let _ = engine.parse_file("app.py", "import os").unwrap();
        assert!(engine.ts_queries.get().is_none());
        assert!(engine.py_queries.get().is_some());
        assert!(engine.go_queries.get().is_none());
    }

    #[test]
    fn js_and_ts_share_same_queries() {
        let engine = QueryEngine::new().unwrap();
        // Parse a JS file — should compile the ts_queries (shared for TS/JS).
        let _ = engine.parse_file("app.js", "import x from 'y';").unwrap();
        assert!(engine.ts_queries.get().is_some());
        // Parse a TS file — should not trigger a new compilation.
        let _ = engine.parse_file("app.ts", "import x from 'y';").unwrap();
        // Still the same compiled queries.
        assert!(engine.ts_queries.get().is_some());
    }

    #[test]
    fn multiple_languages_compile_independently() {
        let engine = QueryEngine::new().unwrap();
        let _ = engine.parse_file("a.ts", "const x = 1;").unwrap();
        let _ = engine.parse_file("b.py", "x = 1").unwrap();
        let _ = engine.parse_file("c.go", "package main").unwrap();
        assert!(engine.ts_queries.get().is_some());
        assert!(engine.py_queries.get().is_some());
        assert!(engine.go_queries.get().is_some());
        // Unused languages stay uncompiled.
        assert!(engine.rust_queries.get().is_none());
        assert!(engine.java_queries.get().is_none());
        assert!(engine.scala_queries.get().is_none());
    }

    #[test]
    fn unknown_language_does_not_compile_anything() {
        let engine = QueryEngine::new().unwrap();
        let result = engine.parse_file("readme.txt", "hello world").unwrap();
        assert_eq!(result.language, Language::Unknown);
        assert!(result.definitions.is_empty());
        // No queries should be compiled.
        assert!(engine.ts_queries.get().is_none());
        assert!(engine.py_queries.get().is_none());
    }

    #[test]
    fn extract_data_flow_triggers_lazy_init() {
        let engine = QueryEngine::new().unwrap();
        assert!(engine.py_queries.get().is_none());
        let _ = engine
            .extract_data_flow("main.py", "x = foo(1)")
            .unwrap();
        assert!(engine.py_queries.get().is_some());
    }

    #[test]
    fn parse_tree_for_path_triggers_lazy_init() {
        let engine = QueryEngine::new().unwrap();
        assert!(engine.go_queries.get().is_none());
        let _ = engine
            .parse_tree_for_path("main.go", "package main")
            .unwrap();
        assert!(engine.go_queries.get().is_some());
    }

    #[test]
    fn cached_results_identical_to_fresh_compilation() {
        let engine = QueryEngine::new().unwrap();
        let src = "import { Foo } from './foo';\nfunction bar() { return Foo(); }";

        // First parse: triggers compilation.
        let r1 = engine.parse_file("a.ts", src).unwrap();
        // Second parse: uses cached queries.
        let r2 = engine.parse_file("b.ts", src).unwrap();

        // Results should be structurally identical (paths differ by design).
        assert_eq!(r1.imports.len(), r2.imports.len());
        assert_eq!(r1.definitions.len(), r2.definitions.len());
        assert_eq!(r1.call_sites.len(), r2.call_sites.len());
        for (i1, i2) in r1.imports.iter().zip(r2.imports.iter()) {
            assert_eq!(i1.source, i2.source);
            assert_eq!(i1.names.len(), i2.names.len());
        }
    }
}

// ---------------------------------------------------------------------------
// Thread-local parser reuse tests (Phase 12.7)
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod thread_local_parser_tests {
    use super::*;

    #[test]
    fn parser_reused_across_multiple_parses_same_language() {
        let engine = QueryEngine::new().unwrap();
        let src1 = "function foo() { return 1; }";
        let src2 = "function bar() { return 2; }";

        let r1 = engine.parse_file("a.ts", src1).unwrap();
        let r2 = engine.parse_file("b.ts", src2).unwrap();

        // Both parse successfully, demonstrating parser reuse works.
        assert_eq!(r1.definitions.len(), 1);
        assert_eq!(r2.definitions.len(), 1);
        assert_eq!(r1.definitions[0].name, "foo");
        assert_eq!(r2.definitions[0].name, "bar");
    }

    #[test]
    fn parser_reused_across_different_languages() {
        let engine = QueryEngine::new().unwrap();

        let ts_result = engine
            .parse_file("app.ts", "function hello() {}")
            .unwrap();
        let py_result = engine
            .parse_file("main.py", "def hello():\n    pass")
            .unwrap();

        assert_eq!(ts_result.definitions.len(), 1);
        assert_eq!(py_result.definitions.len(), 1);
    }

    #[test]
    fn thread_local_parsers_work_with_rayon() {
        use rayon::prelude::*;

        let engine = QueryEngine::new().unwrap();
        let files: Vec<(&str, &str)> = vec![
            ("a.ts", "function a() { return 1; }"),
            ("b.ts", "function b() { return 2; }"),
            ("c.py", "def c():\n    pass"),
            ("d.py", "def d():\n    return 1"),
            ("e.ts", "const e = () => 42;"),
            ("f.go", "package main\nfunc f() {}"),
        ];

        let results: Vec<ParsedFile> = files
            .par_iter()
            .map(|&(path, src)| engine.parse_file(path, src).unwrap())
            .collect();

        assert_eq!(results.len(), 6);
        // All files parsed successfully in parallel with thread-local parsers.
        for r in &results {
            assert!(!r.definitions.is_empty() || r.language == Language::Unknown);
        }
    }

    #[test]
    fn parse_tree_for_path_reuses_parser() {
        let engine = QueryEngine::new().unwrap();

        // First call creates the parser for TypeScript.
        let r1 = engine
            .parse_tree_for_path("a.ts", "const x = 1;")
            .unwrap();
        assert!(r1.is_some());

        // Second call reuses the same thread-local parser.
        let r2 = engine
            .parse_tree_for_path("b.ts", "const y = 2;")
            .unwrap();
        assert!(r2.is_some());
    }

    #[test]
    fn extract_data_flow_reuses_parser() {
        let engine = QueryEngine::new().unwrap();

        let df1 = engine
            .extract_data_flow("a.ts", "const x = foo(1);")
            .unwrap();
        let df2 = engine
            .extract_data_flow("b.ts", "const y = bar(2);")
            .unwrap();

        // Both extractions succeed with parser reuse.
        assert!(!df1.assignments.is_empty());
        assert!(!df2.assignments.is_empty());
    }

    #[test]
    fn results_identical_with_reused_vs_fresh_parser() {
        // Parse with a fresh engine (parsers created fresh for first file).
        let engine1 = QueryEngine::new().unwrap();
        let src = "import { Foo } from './foo';\nfunction bar() { return Foo(); }";
        let r_fresh = engine1.parse_file("test.ts", src).unwrap();

        // Parse again on the same engine (parser reused from thread-local).
        let r_reused = engine1.parse_file("test2.ts", src).unwrap();

        // Results must be structurally identical.
        assert_eq!(r_fresh.imports.len(), r_reused.imports.len());
        assert_eq!(r_fresh.definitions.len(), r_reused.definitions.len());
        assert_eq!(r_fresh.call_sites.len(), r_reused.call_sites.len());
        assert_eq!(r_fresh.exports.len(), r_reused.exports.len());

        for (a, b) in r_fresh.imports.iter().zip(r_reused.imports.iter()) {
            assert_eq!(a.source, b.source);
            assert_eq!(a.names.len(), b.names.len());
        }
        for (a, b) in r_fresh
            .definitions
            .iter()
            .zip(r_reused.definitions.iter())
        {
            assert_eq!(a.name, b.name);
            assert_eq!(a.kind, b.kind);
        }
    }
}

// ---------------------------------------------------------------------------
// Property-based tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod prop_tests {
    use super::*;
    use proptest::prelude::*;

    fn engine() -> &'static QueryEngine {
        shared_test_engine()
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

// ---------------------------------------------------------------------------
// Phase 8 audit: edge case tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod audit_tests {
    use super::*;

    fn engine() -> &'static QueryEngine {
        shared_test_engine()
    }

    #[test]
    fn test_ts_abstract_class() {
        let e = engine();
        let source = "abstract class Base { abstract process(): void; helper() {} }";
        let result = e.parse_file("base.ts", source).unwrap();
        assert!(
            result.definitions.iter().any(|d| d.name == "Base" && d.kind == SymbolKind::Class),
            "abstract classes should be captured"
        );
        assert!(result.definitions.iter().any(|d| d.name == "helper"));
    }

    #[test]
    fn test_ts_generator_function() {
        let e = engine();
        let result = e
            .parse_file("gen.ts", "function* gen() { yield 1; }")
            .unwrap();
        assert!(result.definitions.iter().any(|d| d.name == "gen" && d.kind == SymbolKind::Function));
    }

    #[test]
    fn test_ts_export_default_expression() {
        // Known limitation: the query engine's exports.scm captures exported
        // declarations (export function/class/const/etc.) but not bare
        // `export default <expression>`. The imperative ast.rs parser handles
        // this via the `value` field fallback. Document the gap here.
        let e = engine();
        let source = "const app = createApp();\nexport default app;\n";
        let _result = e.parse_file("app.ts", source).unwrap();
        // The imperative parser captures this; query engine does not (known gap)
        let ast_result = crate::ast::parse_file("app.ts", source).unwrap();
        assert!(
            ast_result.exports.iter().any(|e| e.is_default),
            "ast.rs should capture export default expression"
        );
        // Query engine may or may not capture it — this is a known gap
        // (exports.scm needs a pattern for `export default <expression>`)
    }

    #[test]
    fn test_ts_multiple_exports_same_line() {
        let e = engine();
        let source = "export { a, b, c };";
        let result = e.parse_file("mod.ts", source).unwrap();
        let names: Vec<&str> = result.exports.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
        assert!(names.contains(&"c"));
    }

    #[test]
    fn test_ts_reexport_with_rename() {
        let e = engine();
        let source = "export { foo as bar } from './mod';";
        let result = e.parse_file("index.ts", source).unwrap();
        // The query engine should capture the re-export
        assert!(!result.exports.is_empty());
        // Should have the re-export source
        assert!(result.exports.iter().any(|e| e.is_reexport));
    }

    #[test]
    fn test_python_from_relative_import() {
        let e = engine();
        let result = e.parse_file("pkg/sub.py", "from . import utils").unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, ".");
        assert!(result.imports[0].names.iter().any(|n| n.name == "utils"));
    }

    #[test]
    fn test_python_from_import_aliased() {
        let e = engine();
        let result = e
            .parse_file("app.py", "from models import User as U")
            .unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "models");
        assert_eq!(result.imports[0].names[0].name, "User");
        assert_eq!(result.imports[0].names[0].alias, Some("U".to_string()));
    }

    #[test]
    fn test_ts_unicode_identifiers() {
        let e = engine();
        let source = "function grüßen() {}\nconst αβγ = 42;\n";
        let result = e.parse_file("unicode.ts", source).unwrap();
        assert!(result.definitions.iter().any(|d| d.name == "grüßen"));
        assert!(result.definitions.iter().any(|d| d.name == "αβγ"));
    }

    #[test]
    fn test_ts_syntax_error_partial_results() {
        let e = engine();
        let source = "function valid() {}\nconst x = {{{;\nfunction alsoValid() {}";
        let result = e.parse_file("broken.ts", source).unwrap();
        // tree-sitter does partial parsing — the definition before the error survives
        assert!(result.definitions.iter().any(|d| d.name == "valid"));
        // Recovery after errors is best-effort — later defs may or may not be captured
        assert_eq!(result.language, Language::TypeScript);
    }

    #[test]
    fn test_ts_let_and_var_declarations() {
        let e = engine();
        let source = "let x = 1;\nvar y = 2;\n";
        let result = e.parse_file("vars.ts", source).unwrap();
        // let and var are variable_declarations, should be captured as constants
        assert!(result.definitions.iter().any(|d| d.name == "x"));
        assert!(result.definitions.iter().any(|d| d.name == "y"));
    }

    #[test]
    fn test_python_decorated_class() {
        let e = engine();
        let source = "@dataclass\nclass User:\n    name: str\n    def greet(self):\n        pass";
        let result = e.parse_file("models.py", source).unwrap();
        assert!(result.definitions.iter().any(|d| d.name == "User" && d.kind == SymbolKind::Class));
        assert!(result.definitions.iter().any(|d| d.name == "greet" && d.kind == SymbolKind::Function));
    }

    #[test]
    fn test_hash_str_no_collision_for_common_names() {
        // Verify the hash function gives different results for common definition names
        let names = [
            "foo", "bar", "baz", "get", "set", "create", "update", "delete",
            "handler", "process", "validate", "transform", "save", "load",
        ];
        let hashes: Vec<usize> = names.iter().map(|n| hash_str(n)).collect();
        for i in 0..hashes.len() {
            for j in (i + 1)..hashes.len() {
                assert_ne!(
                    hashes[i], hashes[j],
                    "hash collision between '{}' and '{}'",
                    names[i], names[j]
                );
            }
        }
    }

    #[test]
    fn test_ts_deeply_nested_call_containing_function() {
        let e = engine();
        let source = r#"
function outer() {
    function inner() {
        deepCall();
    }
    outerCall();
}
"#;
        let result = e.parse_file("nested.ts", source).unwrap();
        let deep = result.call_sites.iter().find(|c| c.callee == "deepCall").unwrap();
        assert_eq!(deep.containing_function, Some("inner".to_string()));

        let outer_call = result.call_sites.iter().find(|c| c.callee == "outerCall").unwrap();
        assert_eq!(outer_call.containing_function, Some("outer".to_string()));
    }

    #[test]
    fn test_ts_arrow_function_containing() {
        let e = engine();
        let source = "const handler = () => { innerCall(); };";
        let result = e.parse_file("fn.ts", source).unwrap();
        let call = result.call_sites.iter().find(|c| c.callee == "innerCall").unwrap();
        assert_eq!(call.containing_function, Some("handler".to_string()));
    }

    #[test]
    fn test_ts_comments_only_empty_result() {
        let e = engine();
        let source = "// comment\n/* block */\n/** jsdoc */\n";
        let result = e.parse_file("comments.ts", source).unwrap();
        assert!(result.definitions.is_empty());
        assert!(result.imports.is_empty());
        assert!(result.exports.is_empty());
        assert!(result.call_sites.is_empty());
    }

    #[test]
    fn test_ts_export_multiple_const() {
        let e = engine();
        let source = "export const A = 1, B = 2;";
        let result = e.parse_file("consts.ts", source).unwrap();
        assert!(result.exports.iter().any(|ex| ex.name == "A"));
        assert!(result.exports.iter().any(|ex| ex.name == "B"));
    }

    #[test]
    fn test_ts_class_with_multiple_methods() {
        let e = engine();
        let source = "class Router {\n    get() {}\n    post() {}\n    delete() {}\n}";
        let result = e.parse_file("router.ts", source).unwrap();
        assert!(result.definitions.iter().any(|d| d.name == "Router" && d.kind == SymbolKind::Class));
        assert!(result.definitions.iter().any(|d| d.name == "get"));
        assert!(result.definitions.iter().any(|d| d.name == "post"));
        assert!(result.definitions.iter().any(|d| d.name == "delete"));
    }

    #[test]
    fn test_python_method_call_with_args() {
        let e = engine();
        let df = e
            .extract_data_flow("app.py", "db.save(user, commit=True)")
            .unwrap();
        assert_eq!(df.calls_with_args.len(), 1);
        assert_eq!(df.calls_with_args[0].callee, "db.save");
        // keyword arg value should be captured
        assert!(df.calls_with_args[0].arguments.contains(&"user".to_string()));
        assert!(df.calls_with_args[0].arguments.contains(&"True".to_string()));
    }

    #[test]
    fn test_ts_data_flow_method_chain_assignment() {
        let e = engine();
        let df = e
            .extract_data_flow("app.ts", "const result = db.query.findMany();")
            .unwrap();
        assert_eq!(df.assignments.len(), 1);
        assert_eq!(df.assignments[0].variable, "result");
        assert_eq!(df.assignments[0].callee, "db.query.findMany");
    }

    #[test]
    fn test_parity_ts_exports_match_ast() {
        let e = engine();
        let source = r#"
export function fn1() {}
export default function fn2() {}
export { a, b };
export { c } from './mod';
export * from './all';
export const VAL = 1;
export class Cls {}
export interface IFace {}
export type TAlias = number;
"#;
        let qe = e.parse_file("parity.ts", source).unwrap();
        let ast = crate::ast::parse_file("parity.ts", source).unwrap();

        assert_eq!(
            qe.exports.len(),
            ast.exports.len(),
            "export count mismatch: qe={} vs ast={}",
            qe.exports.len(),
            ast.exports.len()
        );

        // Every export name from ast should be in qe
        for ast_exp in &ast.exports {
            assert!(
                qe.exports.iter().any(|e| e.name == ast_exp.name),
                "missing export '{}' in query engine",
                ast_exp.name
            );
        }
    }

    #[test]
    fn test_parity_ts_data_flow_match_ast() {
        let e = engine();
        let source = r#"
function handler(req: any) {
    const data = parseBody(req);
    const user = await fetchUser(data.id);
    return respond(user);
}
"#;
        let qe_df = e.extract_data_flow("handler.ts", source).unwrap();
        let ast_df = crate::ast::extract_data_flow_info("handler.ts", source).unwrap();

        assert_eq!(
            qe_df.assignments.len(),
            ast_df.assignments.len(),
            "assignment count mismatch"
        );

        for ast_a in &ast_df.assignments {
            assert!(
                qe_df.assignments.iter().any(|a| a.variable == ast_a.variable && a.callee == ast_a.callee),
                "missing assignment {}.{} in query engine",
                ast_a.variable,
                ast_a.callee
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 8 audit: .scm query file coverage tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod scm_audit_tests {
    use super::*;

    fn engine() -> &'static QueryEngine {
        shared_test_engine()
    }

    // === TS enum declarations ===

    #[test]
    fn test_ts_enum_not_captured_known_gap() {
        // Known gap: TS enum declarations (enum_declaration) are not in definitions.scm
        let e = engine();
        let source = "enum Direction { Up, Down, Left, Right }";
        let result = e.parse_file("dir.ts", source).unwrap();
        // Document current behavior: enums are NOT captured
        let has_enum = result.definitions.iter().any(|d| d.name == "Direction");
        assert!(!has_enum, "Enums are not captured (known gap) — if this starts passing, update the .scm file");
    }

    #[test]
    fn test_ts_const_enum_not_captured_known_gap() {
        let e = engine();
        let source = "const enum Status { Active, Inactive }";
        let result = e.parse_file("status.ts", source).unwrap();
        let has_enum = result.definitions.iter().any(|d| d.name == "Status");
        assert!(!has_enum, "Const enums are not captured (known gap)");
    }

    // === TS export default expression ===

    #[test]
    fn test_ts_export_default_identifier_gap() {
        // Known gap: `export default foo` (bare identifier) not captured
        let e = engine();
        let source = "const app = {};\nexport default app;";
        let result = e.parse_file("app.ts", source).unwrap();
        // Query engine cannot capture bare export default expressions
        let has_default = result.exports.iter().any(|e| e.is_default);
        assert!(!has_default, "Export default identifier is not captured (known gap)");
    }

    #[test]
    fn test_ts_export_default_class_expression_gap() {
        // export default class { } (anonymous class) — another known gap
        let e = engine();
        let source = "export default class { method() {} }";
        let _result = e.parse_file("anon.ts", source).unwrap();
        // Anonymous class export — no name to capture
        // This is acceptable: we can't meaningfully track anonymous exports
    }

    // === TS enum in exports ===

    #[test]
    fn test_ts_export_enum_not_captured_known_gap() {
        let e = engine();
        let source = "export enum Color { Red, Green, Blue }";
        let result = e.parse_file("colors.ts", source).unwrap();
        let has_enum_export = result.exports.iter().any(|e| e.name == "Color");
        assert!(!has_enum_export, "Export enum is not captured (known gap)");
    }

    // === TS import type ===

    #[test]
    fn test_ts_import_type_captured() {
        // `import type { Foo } from ...` is parsed as a regular import_statement
        // by tree-sitter TS, so our import patterns capture it. This is fine for
        // flow analysis — type imports still create dependency edges.
        let e = engine();
        let source = "import type { User } from './models';";
        let result = e.parse_file("app.ts", source).unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "./models");
        // The "type" keyword gets captured as the first identifier by the default_name pattern
        // This is slightly incorrect but the import source is correct, which is what matters
        // for building dependency edges
    }

    // === Python walrus operator ===

    #[test]
    fn test_python_walrus_operator_not_in_assignments() {
        // := (walrus operator / named expression) is not an assignment statement,
        // it's a named_expression. Our assignments.scm only captures assignment
        // statements. This is acceptable because walrus operators are typically
        // used inline (if/while conditions) and rarely represent data flow.
        let e = engine();
        let df = e.extract_data_flow("app.py", "if (x := compute()):\n    pass").unwrap();
        assert!(df.assignments.is_empty(), "walrus operator not captured (acceptable gap)");
    }

    // === TS destructuring assignments ===

    #[test]
    fn test_ts_destructuring_assignment_not_in_assignments() {
        // Destructuring assignments like `const { a, b } = foo()` are not captured
        // by assignments.scm because the LHS is not an `identifier` but an
        // `object_pattern`. The IR layer handles destructuring via IrPattern.
        let e = engine();
        let df = e.extract_data_flow("app.ts", "const { a, b } = getData();").unwrap();
        assert!(df.assignments.is_empty(), "destructuring assignments not in .scm query (handled by IR layer)");
    }

    #[test]
    fn test_ts_array_destructuring_not_in_assignments() {
        let e = engine();
        let df = e.extract_data_flow("app.ts", "const [first, ...rest] = getList();").unwrap();
        assert!(df.assignments.is_empty(), "array destructuring not in .scm query (handled by IR layer)");
    }

    // === Python tuple unpacking ===

    #[test]
    fn test_python_tuple_unpacking_not_in_assignments() {
        // `a, b = foo()` has LHS as `pattern_list`, not `identifier`
        let e = engine();
        let df = e.extract_data_flow("app.py", "a, b = compute()").unwrap();
        assert!(df.assignments.is_empty(), "tuple unpacking not in .scm query (handled by IR layer)");
    }

    // === Python relative import with alias ===

    #[test]
    fn test_python_relative_import_with_alias() {
        // `from .models import User as U` — Pattern 6 in python/imports.scm
        let e = engine();
        let result = e.parse_file("pkg/app.py", "from .models import User as U").unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, ".models");
        assert_eq!(result.imports[0].names.len(), 1);
        assert_eq!(result.imports[0].names[0].name, "User");
        assert_eq!(result.imports[0].names[0].alias, Some("U".to_string()));
    }

    // === Python async def ===

    #[test]
    fn test_python_async_def_captured() {
        // async def should be captured — tree-sitter-python uses `function_definition`
        // for both sync and async functions
        let e = engine();
        let source = "async def fetch_data():\n    pass";
        let result = e.parse_file("api.py", source).unwrap();
        assert!(
            result.definitions.iter().any(|d| d.name == "fetch_data" && d.kind == SymbolKind::Function),
            "async functions should be captured"
        );
    }

    // === TS `as const` / `satisfies` ===

    #[test]
    fn test_ts_as_const_assignment() {
        let e = engine();
        let source = "const config = { port: 3000 } as const;";
        let result = e.parse_file("config.ts", source).unwrap();
        assert!(result.definitions.iter().any(|d| d.name == "config" && d.kind == SymbolKind::Constant));
    }

    #[test]
    fn test_ts_satisfies_expression() {
        let e = engine();
        let source = "const config = { port: 3000 } satisfies Config;";
        let result = e.parse_file("config.ts", source).unwrap();
        assert!(result.definitions.iter().any(|d| d.name == "config" && d.kind == SymbolKind::Constant));
    }

    // === TS `export default function` with no name ===

    #[test]
    fn test_ts_export_default_anonymous_function() {
        let e = engine();
        let source = "export default function() { return 42; }";
        let _result = e.parse_file("anon.ts", source).unwrap();
        // Anonymous function — no name to capture. This is acceptable.
        // The export statement itself may or may not match.
    }

    // === TS template literal type ===

    #[test]
    fn test_ts_complex_type_alias() {
        let e = engine();
        let source = "type EventName = `on${string}`;";
        let result = e.parse_file("events.ts", source).unwrap();
        assert!(result.definitions.iter().any(|d| d.name == "EventName" && d.kind == SymbolKind::TypeAlias));
    }

    // === TS namespace/module declarations ===

    #[test]
    fn test_ts_namespace_not_captured_known_gap() {
        let e = engine();
        let source = "namespace MyApp { export function init() {} }";
        let result = e.parse_file("app.ts", source).unwrap();
        // namespace declarations use `module` node type in tree-sitter TS
        // Our definitions.scm doesn't capture them — acceptable for diff analysis
        let _has_namespace = result.definitions.iter().any(|d| d.name == "MyApp");
        // init() inside should still be captured
        assert!(result.definitions.iter().any(|d| d.name == "init"));
    }

    // === TS `export =` (CommonJS-style) ===

    #[test]
    fn test_ts_export_assignment_not_captured() {
        let e = engine();
        let source = "class Foo {}\nexport = Foo;";
        let _result = e.parse_file("mod.ts", source).unwrap();
        // export = Foo is TypeScript-specific CommonJS compat, rarely used
        // Not captured — acceptable gap
    }

    // === Python __all__ ===

    #[test]
    fn test_python_dunder_all_not_captured() {
        // Python uses __all__ for explicit exports, but it's just an assignment
        // statement (not a function/class definition), so it's not captured as
        // a definition. This is correct — Python assignments are not definitions.
        let e = engine();
        let source = "__all__ = ['foo', 'bar']";
        let result = e.parse_file("mod.py", source).unwrap();
        // Python has no const/let/var, so assignments are not captured as definitions
        assert!(result.definitions.is_empty(), "__all__ is an assignment, not a definition");
    }

    // === TS `require()` calls (CJS imports) ===

    #[test]
    fn test_ts_require_captured_as_call() {
        // require() is a call expression, not an import statement
        // It should appear in call_sites, which is correct for CJS detection
        let e = engine();
        let source = "const fs = require('fs');";
        let result = e.parse_file("app.ts", source).unwrap();
        assert!(result.call_sites.iter().any(|c| c.callee == "require"));
    }

    // === TS dynamic import ===

    #[test]
    fn test_ts_dynamic_import_not_in_imports() {
        // import('module') is a call_expression, not an import_statement
        let e = engine();
        let source = "const mod = await import('./lazy');";
        let result = e.parse_file("app.ts", source).unwrap();
        // Should NOT be in static imports
        assert!(result.imports.is_empty());
        // But should be captured by data flow as an assignment from a call
        let df = e.extract_data_flow("app.ts", source).unwrap();
        assert_eq!(df.assignments.len(), 1);
        assert_eq!(df.assignments[0].variable, "mod");
    }

    // === Python decorated method inside decorated class ===

    #[test]
    fn test_python_decorated_method_in_decorated_class() {
        let e = engine();
        let source = "@dataclass\nclass Service:\n    @staticmethod\n    def create():\n        pass\n    @classmethod\n    def from_config(cls):\n        pass";
        let result = e.parse_file("svc.py", source).unwrap();
        assert!(result.definitions.iter().any(|d| d.name == "Service" && d.kind == SymbolKind::Class));
        assert!(result.definitions.iter().any(|d| d.name == "create"));
        assert!(result.definitions.iter().any(|d| d.name == "from_config"));
    }

    // === TS exported arrow function ===

    #[test]
    fn test_ts_export_arrow_function() {
        let e = engine();
        let source = "export const handler = async (req: Request) => { return new Response(); };";
        let result = e.parse_file("handler.ts", source).unwrap();
        assert!(result.exports.iter().any(|e| e.name == "handler"));
        assert!(result.definitions.iter().any(|d| d.name == "handler" && d.kind == SymbolKind::Function));
    }

    // === Python multiline import ===

    #[test]
    fn test_python_multiline_import() {
        let e = engine();
        let source = "from models import (\n    User,\n    Post,\n    Comment\n)";
        let result = e.parse_file("app.py", source).unwrap();
        assert_eq!(result.imports.len(), 1);
        let names: Vec<&str> = result.imports[0].names.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"User"));
        assert!(names.contains(&"Post"));
        assert!(names.contains(&"Comment"));
    }

    // === Python multiple import on same line ===

    #[test]
    fn test_python_multiple_import_same_statement() {
        let e = engine();
        let source = "import os, sys, json";
        let result = e.parse_file("app.py", source).unwrap();
        // tree-sitter-python may parse this as separate import nodes
        // or as one import_statement with multiple dotted_names
        assert!(!result.imports.is_empty());
    }

    // === TS re-export namespace ===

    #[test]
    fn test_ts_namespace_reexport() {
        let e = engine();
        let source = "export * as utils from './utils';";
        let result = e.parse_file("index.ts", source).unwrap();
        // namespace re-export should be captured by the wildcard pattern
        assert!(!result.exports.is_empty());
    }

    // === find_containing_function for function_expression ===

    #[test]
    fn test_ts_function_expression_containing() {
        let e = engine();
        let source = "const handler = function named() { innerCall(); };";
        let result = e.parse_file("fn.ts", source).unwrap();
        let call = result.call_sites.iter().find(|c| c.callee == "innerCall").unwrap();
        // function_expression assigned to variable — should find "handler" or "named"
        assert!(
            call.containing_function.is_some(),
            "function expression should have a containing function"
        );
    }

    // === TS computed property method ===

    #[test]
    fn test_ts_computed_property_method() {
        let e = engine();
        let source = "class Router { [Symbol.iterator]() {} }";
        let result = e.parse_file("router.ts", source).unwrap();
        // Computed property methods have non-identifier names — class should still be captured
        assert!(result.definitions.iter().any(|d| d.name == "Router"));
        // The computed method name is captured as the full expression text
        // e.g. "Symbol.iterator" via the (_) capture in method_definition
    }

    // === Python nested class ===

    #[test]
    fn test_python_nested_class() {
        let e = engine();
        let source = "class Outer:\n    class Inner:\n        def method(self):\n            pass";
        let result = e.parse_file("nested.py", source).unwrap();
        assert!(result.definitions.iter().any(|d| d.name == "Outer"));
        // Inner class may or may not be captured depending on query depth
    }

    // === TS class with static methods ===

    #[test]
    fn test_ts_static_method() {
        let e = engine();
        let source = "class Factory { static create() {} }";
        let result = e.parse_file("factory.ts", source).unwrap();
        assert!(result.definitions.iter().any(|d| d.name == "Factory"));
        assert!(result.definitions.iter().any(|d| d.name == "create"));
    }

    // === TS getter/setter ===

    #[test]
    fn test_ts_getter_setter() {
        let e = engine();
        let source = "class User { get name() { return ''; } set name(v: string) {} }";
        let result = e.parse_file("user.ts", source).unwrap();
        assert!(result.definitions.iter().any(|d| d.name == "User"));
        // getter/setter are method_definitions — should be captured
        assert!(result.definitions.iter().any(|d| d.name == "name"));
    }

    // === Verify .scm pattern ordering doesn't matter ===

    #[test]
    fn test_capture_name_dispatch_order_independent() {
        // Verify that the engine dispatches by capture name, not pattern index.
        // This is the key architectural property of the capture-name refactor.
        let e = engine();
        let source = r#"
import React from 'react';
import { useState } from 'react';
import * as path from 'path';
import './polyfill';
"#;
        let result = e.parse_file("app.ts", source).unwrap();
        assert_eq!(result.imports.len(), 4);
        // Verify each import type was correctly dispatched
        assert!(result.imports.iter().any(|i| i.is_default)); // default
        assert!(result.imports.iter().any(|i| !i.is_default && !i.is_namespace && !i.names.is_empty())); // named
        assert!(result.imports.iter().any(|i| i.is_namespace)); // namespace
        assert!(result.imports.iter().any(|i| i.names.is_empty())); // side-effect
    }

    // === Verify all export types dispatch correctly ===

    #[test]
    fn test_all_export_declaration_types() {
        let e = engine();
        let source = r#"
export function fn() {}
export function* gen() {}
export class Cls {}
export abstract class ACls {}
export interface IFace {}
export type TAlias = string;
export const VAL = 1;
"#;
        let result = e.parse_file("all.ts", source).unwrap();
        assert!(result.exports.iter().any(|e| e.name == "fn"), "export function");
        assert!(result.exports.iter().any(|e| e.name == "gen"), "export generator");
        assert!(result.exports.iter().any(|e| e.name == "Cls"), "export class");
        assert!(result.exports.iter().any(|e| e.name == "ACls"), "export abstract class");
        assert!(result.exports.iter().any(|e| e.name == "IFace"), "export interface");
        assert!(result.exports.iter().any(|e| e.name == "TAlias"), "export type alias");
        assert!(result.exports.iter().any(|e| e.name == "VAL"), "export const");
    }

    // === Verify definition extraction finds all kinds ===

    #[test]
    fn test_all_definition_kinds_ts() {
        let e = engine();
        let source = r#"
function fn() {}
function* gen() {}
class Cls {}
abstract class ACls {}
interface IFace {}
type TAlias = string;
const handler = () => {};
const fnExpr = function() {};
const VAL = 42;
class WithMethod { method() {} }
"#;
        let result = e.parse_file("all.ts", source).unwrap();
        assert!(result.definitions.iter().any(|d| d.name == "fn" && d.kind == SymbolKind::Function));
        assert!(result.definitions.iter().any(|d| d.name == "gen" && d.kind == SymbolKind::Function));
        assert!(result.definitions.iter().any(|d| d.name == "Cls" && d.kind == SymbolKind::Class));
        assert!(result.definitions.iter().any(|d| d.name == "ACls" && d.kind == SymbolKind::Class));
        assert!(result.definitions.iter().any(|d| d.name == "IFace" && d.kind == SymbolKind::Interface));
        assert!(result.definitions.iter().any(|d| d.name == "TAlias" && d.kind == SymbolKind::TypeAlias));
        assert!(result.definitions.iter().any(|d| d.name == "handler" && d.kind == SymbolKind::Function));
        assert!(result.definitions.iter().any(|d| d.name == "fnExpr" && d.kind == SymbolKind::Function));
        assert!(result.definitions.iter().any(|d| d.name == "VAL" && d.kind == SymbolKind::Constant));
        assert!(result.definitions.iter().any(|d| d.name == "method" && d.kind == SymbolKind::Function));
    }

    #[test]
    fn test_all_definition_kinds_python() {
        let e = engine();
        let source = "def fn():\n    pass\n\nclass Cls:\n    def method(self):\n        pass\n\n@deco\ndef decorated():\n    pass\n\n@deco\nclass DCls:\n    @deco\n    def dmethod(self):\n        pass";
        let result = e.parse_file("all.py", source).unwrap();
        assert!(result.definitions.iter().any(|d| d.name == "fn" && d.kind == SymbolKind::Function));
        assert!(result.definitions.iter().any(|d| d.name == "Cls" && d.kind == SymbolKind::Class));
        assert!(result.definitions.iter().any(|d| d.name == "method" && d.kind == SymbolKind::Function));
        assert!(result.definitions.iter().any(|d| d.name == "decorated" && d.kind == SymbolKind::Function));
        assert!(result.definitions.iter().any(|d| d.name == "DCls" && d.kind == SymbolKind::Class));
        assert!(result.definitions.iter().any(|d| d.name == "dmethod" && d.kind == SymbolKind::Function));
    }

    // === Verify Python import capture coverage ===

    #[test]
    fn test_all_python_import_variants() {
        let e = engine();
        let source = r#"import os
import numpy as np
from os.path import join
from typing import List
from models import User as U
from os.path import *
from . import utils
"#;
        let result = e.parse_file("all.py", source).unwrap();
        // Should capture all 7 imports (though some may be combined)
        assert!(result.imports.len() >= 6, "expected at least 6 imports, got {}", result.imports.len());
    }

    // === Agent audit Issue 3: Decorated Python function double-counting ===

    #[test]
    fn test_python_decorated_function_not_double_counted() {
        // A decorated function fires both the bare `function_definition` pattern
        // and the `decorated_definition > function_definition` pattern.
        // Verify they are deduplicated (same name should appear only once).
        let e = engine();
        let source = "@app.route('/hello')\ndef hello():\n    pass\n\n@cache\ndef cached_func():\n    pass";
        let result = e.parse_file("app.py", source).unwrap();
        let hello_count = result.definitions.iter().filter(|d| d.name == "hello").count();
        let cached_count = result.definitions.iter().filter(|d| d.name == "cached_func").count();
        assert_eq!(hello_count, 1, "decorated function 'hello' should appear exactly once, got {}", hello_count);
        assert_eq!(cached_count, 1, "decorated function 'cached_func' should appear exactly once, got {}", cached_count);
    }

    #[test]
    fn test_python_decorated_class_not_double_counted() {
        let e = engine();
        let source = "@dataclass\nclass User:\n    name: str";
        let result = e.parse_file("models.py", source).unwrap();
        let user_count = result.definitions.iter().filter(|d| d.name == "User").count();
        assert_eq!(user_count, 1, "decorated class 'User' should appear exactly once, got {}", user_count);
    }

    // === Agent audit Issue 4: "function" kind string in const skip logic ===

    #[test]
    fn test_ts_const_assigned_function_expression_not_double_def() {
        // const fnExpr = function() {} should be captured as Function (via fn_expr_name),
        // NOT additionally as a Constant (via const_name). The skip logic checks
        // the value node kind for "arrow_function", "function", and "function_expression".
        let e = engine();
        let source = "const handler = function() { return 42; };";
        let result = e.parse_file("fn.ts", source).unwrap();
        let handler_defs: Vec<_> = result.definitions.iter().filter(|d| d.name == "handler").collect();
        assert_eq!(handler_defs.len(), 1, "function expression should produce exactly 1 definition, got {}", handler_defs.len());
        assert_eq!(handler_defs[0].kind, SymbolKind::Function, "should be Function, not Constant");
    }

    #[test]
    fn test_ts_const_assigned_named_function_expression_not_double_def() {
        // const handler = function named() {} — named function expression
        let e = engine();
        let source = "const handler = function named() { return 42; };";
        let result = e.parse_file("fn.ts", source).unwrap();
        // Should have "handler" as Function, not double-counted
        let handler_defs: Vec<_> = result.definitions.iter().filter(|d| d.name == "handler").collect();
        assert_eq!(handler_defs.len(), 1, "named function expression should produce exactly 1 definition for 'handler'");
        assert_eq!(handler_defs[0].kind, SymbolKind::Function);
    }

    // === Agent audit Issue 1: TS new_expression not in calls.scm ===

    #[test]
    fn test_ts_new_expression_not_captured_known_gap() {
        // `new Foo()` uses `new_expression` in tree-sitter TS, not `call_expression`.
        // calls.scm only matches call_expression, so class instantiations are missed.
        // This affects `instantiates` edge construction.
        let e = engine();
        let source = "const user = new User('Alice');";
        let result = e.parse_file("app.ts", source).unwrap();
        let has_user_call = result.call_sites.iter().any(|c| c.callee == "User");
        assert!(!has_user_call, "new_expression is not captured as a call site (known gap)");
    }

    // =====================================================================
    // Rust language tests
    // =====================================================================

    #[test]
    fn test_rust_language_detection() {
        assert_eq!(Language::from_path("main.rs"), Language::Rust);
        assert_eq!(Language::from_path("src/lib.rs"), Language::Rust);
        assert_eq!(Language::from_path("src/handlers/auth.rs"), Language::Rust);
    }

    #[test]
    fn test_rust_simple_use() {
        let e = engine();
        let result = e.parse_file("main.rs", "use std::io;").unwrap();
        assert_eq!(result.language, Language::Rust);
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "std::io");
    }

    #[test]
    fn test_rust_use_list() {
        let e = engine();
        let source = "use std::collections::{HashMap, BTreeMap};";
        let result = e.parse_file("main.rs", source).unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "std::collections");
        let names: Vec<&str> = result.imports[0].names.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"HashMap"));
        assert!(names.contains(&"BTreeMap"));
    }

    #[test]
    fn test_rust_use_alias() {
        let e = engine();
        let source = "use std::io as stdio;";
        let result = e.parse_file("main.rs", source).unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "std::io");
        assert_eq!(result.imports[0].names[0].alias.as_deref(), Some("stdio"));
    }

    #[test]
    fn test_rust_use_crate() {
        let e = engine();
        let source = "use crate::handlers::auth;";
        let result = e.parse_file("main.rs", source).unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "crate::handlers::auth");
    }

    #[test]
    fn test_rust_use_self() {
        let e = engine();
        let source = "use self::models::User;";
        let result = e.parse_file("main.rs", source).unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "self::models::User");
    }

    #[test]
    fn test_rust_use_glob() {
        let e = engine();
        let source = "use std::io::*;";
        let result = e.parse_file("main.rs", source).unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "std::io");
    }

    #[test]
    fn test_rust_multiple_uses() {
        let e = engine();
        let source = r#"
use std::io;
use serde::{Serialize, Deserialize};
use tokio;
"#;
        let result = e.parse_file("main.rs", source).unwrap();
        assert!(result.imports.len() >= 3);
    }

    #[test]
    fn test_rust_function_definitions() {
        let e = engine();
        let source = r#"
fn hello() {
    println!("hello");
}

pub fn greet(name: &str) -> String {
    format!("Hello, {}", name)
}

async fn fetch_data() -> Result<(), Error> {
    Ok(())
}
"#;
        let result = e.parse_file("main.rs", source).unwrap();
        let fn_names: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Function)
            .map(|d| d.name.as_str())
            .collect();
        assert!(fn_names.contains(&"hello"));
        assert!(fn_names.contains(&"greet"));
        assert!(fn_names.contains(&"fetch_data"));
    }

    #[test]
    fn test_rust_struct_definitions() {
        let e = engine();
        let source = r#"
pub struct User {
    pub name: String,
    pub email: String,
}

struct Config {
    port: u16,
}
"#;
        let result = e.parse_file("main.rs", source).unwrap();
        let struct_names: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Class)
            .map(|d| d.name.as_str())
            .collect();
        assert!(struct_names.contains(&"User"));
        assert!(struct_names.contains(&"Config"));
    }

    #[test]
    fn test_rust_enum_definitions() {
        let e = engine();
        let source = r#"
pub enum Status {
    Active,
    Inactive,
    Suspended,
}
"#;
        let result = e.parse_file("main.rs", source).unwrap();
        let enum_names: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Class)
            .map(|d| d.name.as_str())
            .collect();
        assert!(enum_names.contains(&"Status"));
    }

    #[test]
    fn test_rust_trait_definitions() {
        let e = engine();
        let source = r#"
pub trait Repository {
    fn find_by_id(&self, id: u64) -> Option<User>;
    fn save(&self, user: &User) -> Result<(), Error>;
}
"#;
        let result = e.parse_file("main.rs", source).unwrap();
        let trait_names: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Interface)
            .map(|d| d.name.as_str())
            .collect();
        assert!(trait_names.contains(&"Repository"));
    }

    #[test]
    fn test_rust_type_alias() {
        let e = engine();
        let source = r#"
type Result<T> = std::result::Result<T, AppError>;
type UserId = u64;
"#;
        let result = e.parse_file("main.rs", source).unwrap();
        let type_names: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::TypeAlias)
            .map(|d| d.name.as_str())
            .collect();
        assert!(type_names.contains(&"Result"));
        assert!(type_names.contains(&"UserId"));
    }

    #[test]
    fn test_rust_const_and_static() {
        let e = engine();
        let source = r#"
const MAX_RETRIES: u32 = 3;
static DB_URL: &str = "postgres://localhost/mydb";
"#;
        let result = e.parse_file("main.rs", source).unwrap();
        let const_names: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Constant)
            .map(|d| d.name.as_str())
            .collect();
        assert!(const_names.contains(&"MAX_RETRIES"));
        assert!(const_names.contains(&"DB_URL"));
    }

    #[test]
    fn test_rust_impl_methods() {
        let e = engine();
        let source = r#"
struct UserService;

impl UserService {
    fn new() -> Self {
        UserService
    }

    pub fn find_user(&self, id: u64) -> Option<User> {
        None
    }
}
"#;
        let result = e.parse_file("main.rs", source).unwrap();
        let fn_names: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Function)
            .map(|d| d.name.as_str())
            .collect();
        assert!(fn_names.contains(&"new"));
        assert!(fn_names.contains(&"find_user"));
    }

    #[test]
    fn test_rust_call_sites() {
        let e = engine();
        let source = r#"
fn main() {
    let service = UserService::new();
    let user = service.find_user(42);
    println!("User: {:?}", user);
}
"#;
        let result = e.parse_file("main.rs", source).unwrap();
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"UserService::new"));
        assert!(callees.contains(&"service.find_user"));
    }

    #[test]
    fn test_rust_call_sites_containing_function() {
        let e = engine();
        let source = r#"
fn process() {
    let data = fetch_data();
    save(data);
}
"#;
        let result = e.parse_file("main.rs", source).unwrap();
        for call in &result.call_sites {
            assert_eq!(call.containing_function.as_deref(), Some("process"));
        }
    }

    #[test]
    fn test_rust_data_flow_let_binding() {
        let e = engine();
        let source = r#"
fn handler() {
    let user = find_user(42);
    let result = save_user(user);
}
"#;
        let result = e.extract_data_flow("main.rs", source).unwrap();
        let vars: Vec<&str> = result.assignments.iter().map(|a| a.variable.as_str()).collect();
        assert!(vars.contains(&"user"));
        assert!(vars.contains(&"result"));
    }

    #[test]
    fn test_rust_data_flow_call_with_args() {
        let e = engine();
        let source = r#"
fn handler(req: Request) {
    let body = parse_body(req);
    let user = create_user(body);
}
"#;
        let result = e.extract_data_flow("main.rs", source).unwrap();
        let call = result.calls_with_args.iter().find(|c| c.callee == "parse_body");
        assert!(call.is_some());
        assert!(call.unwrap().arguments.contains(&"req".to_string()));
    }

    #[test]
    fn test_rust_macro_definition() {
        let e = engine();
        let source = r#"
macro_rules! my_macro {
    ($x:expr) => {
        println!("{}", $x);
    };
}
"#;
        let result = e.parse_file("main.rs", source).unwrap();
        let macro_names: Vec<&str> = result.definitions.iter()
            .filter(|d| d.name == "my_macro")
            .map(|d| d.name.as_str())
            .collect();
        assert!(macro_names.contains(&"my_macro"));
    }

    #[test]
    fn test_rust_empty_source() {
        let e = engine();
        let result = e.parse_file("main.rs", "").unwrap();
        assert_eq!(result.language, Language::Rust);
        assert!(result.definitions.is_empty());
        assert!(result.imports.is_empty());
    }

    #[test]
    fn test_rust_complex_use_patterns() {
        let e = engine();
        let source = r#"
use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use crate::models::User;
use super::config::Config;
"#;
        let result = e.parse_file("main.rs", source).unwrap();
        assert!(result.imports.len() >= 4);
        // Verify serde has named imports
        let serde_import = result.imports.iter().find(|i| i.source == "serde");
        assert!(serde_import.is_some());
        let serde_names: Vec<&str> = serde_import.unwrap().names.iter().map(|n| n.name.as_str()).collect();
        assert!(serde_names.contains(&"Serialize"));
        assert!(serde_names.contains(&"Deserialize"));
    }

    #[test]
    fn test_rust_use_from_crate_root() {
        // use axum::{Router, routing::get};  — source should be "axum"
        let e = engine();
        let source = "use axum::{Router, routing};";
        let result = e.parse_file("main.rs", source).unwrap();
        assert!(!result.imports.is_empty(), "should have at least one import");
        // The source should be "axum" (the crate root)
        assert!(result.imports.iter().any(|i| i.source == "axum"),
            "should detect axum import; got: {:?}", result.imports.iter().map(|i| &i.source).collect::<Vec<_>>());
    }

    #[test]
    fn test_rust_async_main_detection() {
        let e = engine();
        let source = r#"
use axum::{Router, routing::get};

#[tokio::main]
async fn main() {
    let app = Router::new();
    axum::serve(app).await.unwrap();
}
"#;
        let result = e.parse_file("src/main.rs", source).unwrap();
        let fn_names: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Function)
            .map(|d| d.name.as_str())
            .collect();
        assert!(fn_names.contains(&"main"), "should detect async fn main(); got defs: {:?}", fn_names);
    }

    #[test]
    fn test_rust_full_module() {
        // Test a realistic Rust file with mixed definitions
        let e = engine();
        let source = r#"
use std::sync::Arc;
use serde::{Serialize, Deserialize};

const DEFAULT_PORT: u16 = 8080;

#[derive(Debug, Serialize, Deserialize)]
pub struct AppConfig {
    pub port: u16,
    pub host: String,
}

pub trait Service {
    fn start(&self) -> Result<(), Box<dyn std::error::Error>>;
}

impl AppConfig {
    pub fn new() -> Self {
        AppConfig {
            port: DEFAULT_PORT,
            host: "localhost".to_string(),
        }
    }
}

pub fn run_server(config: AppConfig) {
    let addr = format!("{}:{}", config.host, config.port);
    start_listener(addr);
}

fn start_listener(addr: String) {
    println!("Listening on {}", addr);
}
"#;
        let result = e.parse_file("lib.rs", source).unwrap();
        assert_eq!(result.language, Language::Rust);

        // Check imports
        assert!(result.imports.len() >= 2);

        // Check definitions
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"DEFAULT_PORT"));
        assert!(def_names.contains(&"AppConfig"));
        assert!(def_names.contains(&"Service"));
        assert!(def_names.contains(&"new"));
        assert!(def_names.contains(&"run_server"));
        assert!(def_names.contains(&"start_listener"));

        // Check call sites (format! is a macro, not a call_expression — won't appear)
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"start_listener"));
    }

    // === Java language detection ===

    #[test]
    fn test_java_language_detection() {
        assert_eq!(Language::from_path("Main.java"), Language::Java);
        assert_eq!(Language::from_path("src/com/example/App.java"), Language::Java);
        assert_eq!(Language::from_path("src/main/java/UserController.java"), Language::Java);
    }

    // === Java imports ===

    #[test]
    fn test_java_simple_import() {
        let e = engine();
        let result = e.parse_file("App.java", "import java.util.List;").unwrap();
        assert_eq!(result.language, Language::Java);
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "java.util");
        assert_eq!(result.imports[0].names[0].name, "List");
    }

    #[test]
    fn test_java_wildcard_import() {
        let e = engine();
        let result = e.parse_file("App.java", "import java.util.*;").unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "java.util");
        assert_eq!(result.imports[0].names[0].name, "*");
    }

    #[test]
    fn test_java_static_import() {
        let e = engine();
        let result = e.parse_file("App.java", "import static org.junit.Assert.assertEquals;").unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "org.junit.Assert");
        assert_eq!(result.imports[0].names[0].name, "assertEquals");
    }

    #[test]
    fn test_java_multiple_imports() {
        let e = engine();
        let source = r#"
import java.util.List;
import java.util.Map;
import org.springframework.web.bind.annotation.RestController;
"#;
        let result = e.parse_file("App.java", source).unwrap();
        assert_eq!(result.imports.len(), 3);
    }

    // === Java definitions ===

    #[test]
    fn test_java_class_definition() {
        let e = engine();
        let source = r#"
public class UserController {
}
"#;
        let result = e.parse_file("UserController.java", source).unwrap();
        let class_defs: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Class)
            .map(|d| d.name.as_str())
            .collect();
        assert!(class_defs.contains(&"UserController"));
    }

    #[test]
    fn test_java_interface_definition() {
        let e = engine();
        let source = r#"
public interface UserRepository {
    User findById(Long id);
}
"#;
        let result = e.parse_file("UserRepository.java", source).unwrap();
        let iface_defs: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Interface)
            .map(|d| d.name.as_str())
            .collect();
        assert!(iface_defs.contains(&"UserRepository"));
    }

    #[test]
    fn test_java_method_definitions() {
        let e = engine();
        let source = r#"
public class UserService {
    public User findUser(Long id) {
        return null;
    }

    public void createUser(String name) {
    }
}
"#;
        let result = e.parse_file("UserService.java", source).unwrap();
        let fn_defs: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Function)
            .map(|d| d.name.as_str())
            .collect();
        assert!(fn_defs.contains(&"findUser"));
        assert!(fn_defs.contains(&"createUser"));
    }

    #[test]
    fn test_java_enum_definition() {
        let e = engine();
        let source = r#"
public enum Status {
    ACTIVE,
    INACTIVE
}
"#;
        let result = e.parse_file("Status.java", source).unwrap();
        let class_defs: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Class)
            .map(|d| d.name.as_str())
            .collect();
        assert!(class_defs.contains(&"Status"));
    }

    #[test]
    fn test_java_constructor_definition() {
        let e = engine();
        let source = r#"
public class UserService {
    public UserService(UserRepository repo) {
    }
}
"#;
        let result = e.parse_file("UserService.java", source).unwrap();
        let fn_defs: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Function)
            .map(|d| d.name.as_str())
            .collect();
        assert!(fn_defs.contains(&"UserService"));
    }

    #[test]
    fn test_java_field_definition() {
        let e = engine();
        let source = r#"
public class Config {
    private String apiKey;
    public static final int MAX_RETRIES = 3;
}
"#;
        let result = e.parse_file("Config.java", source).unwrap();
        let field_defs: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Constant)
            .map(|d| d.name.as_str())
            .collect();
        assert!(field_defs.contains(&"apiKey"));
        assert!(field_defs.contains(&"MAX_RETRIES"));
    }

    // === Java call sites ===

    #[test]
    fn test_java_method_call() {
        let e = engine();
        let source = r#"
public class App {
    public void run() {
        userService.findUser(42);
    }
}
"#;
        let result = e.parse_file("App.java", source).unwrap();
        let callees: Vec<&str> = result.call_sites.iter()
            .map(|c| c.callee.as_str())
            .collect();
        assert!(callees.contains(&"findUser"));
    }

    #[test]
    fn test_java_constructor_call() {
        let e = engine();
        let source = r#"
public class App {
    public void run() {
        User user = new User("Alice");
    }
}
"#;
        let result = e.parse_file("App.java", source).unwrap();
        let callees: Vec<&str> = result.call_sites.iter()
            .map(|c| c.callee.as_str())
            .collect();
        assert!(callees.contains(&"User"));
    }

    #[test]
    fn test_java_call_containing_function() {
        let e = engine();
        let source = r#"
public class App {
    public void run() {
        doSomething();
    }
}
"#;
        let result = e.parse_file("App.java", source).unwrap();
        let call = result.call_sites.iter().find(|c| c.callee == "doSomething").unwrap();
        assert_eq!(call.containing_function.as_deref(), Some("run"));
    }

    // === Java data flow ===

    #[test]
    fn test_java_data_flow_assignment() {
        let e = engine();
        let source = r#"
public class App {
    public void run() {
        User user = findUser(1);
    }
}
"#;
        let df = e.extract_data_flow("App.java", source).unwrap();
        assert!(!df.assignments.is_empty());
        assert_eq!(df.assignments[0].variable, "user");
        assert_eq!(df.assignments[0].callee, "findUser");
    }

    #[test]
    fn test_java_data_flow_constructor_assignment() {
        let e = engine();
        let source = r#"
public class App {
    public void run() {
        UserService svc = new UserService();
    }
}
"#;
        let df = e.extract_data_flow("App.java", source).unwrap();
        assert!(!df.assignments.is_empty());
        assert_eq!(df.assignments[0].variable, "svc");
        assert_eq!(df.assignments[0].callee, "UserService");
    }

    // === Java empty / edge cases ===

    #[test]
    fn test_java_empty_source() {
        let e = engine();
        let result = e.parse_file("App.java", "").unwrap();
        assert_eq!(result.language, Language::Java);
        assert!(result.definitions.is_empty());
        assert!(result.imports.is_empty());
    }

    // === Java full module ===

    #[test]
    fn test_java_full_spring_controller() {
        let e = engine();
        let source = r#"
package com.example.demo;

import java.util.List;
import org.springframework.web.bind.annotation.RestController;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;

@RestController
public class UserController {

    private final UserService userService;

    public UserController(UserService userService) {
        this.userService = userService;
    }

    @GetMapping("/users")
    public List<User> getUsers() {
        return userService.findAll();
    }

    @PostMapping("/users")
    public User createUser(@RequestBody User user) {
        return userService.save(user);
    }
}
"#;
        let result = e.parse_file("UserController.java", source).unwrap();

        // Imports
        assert!(result.imports.len() >= 4);
        let import_sources: Vec<&str> = result.imports.iter().map(|i| i.source.as_str()).collect();
        assert!(import_sources.iter().any(|s| s.contains("java.util")));
        assert!(import_sources.iter().any(|s| s.contains("org.springframework")));

        // Definitions
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"UserController"));
        assert!(def_names.contains(&"getUsers"));
        assert!(def_names.contains(&"createUser"));

        // Call sites
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"findAll"));
        assert!(callees.contains(&"save"));
    }

    // ===================================================================
    // C# tests
    // ===================================================================

    #[test]
    fn test_csharp_language_detection() {
        assert_eq!(Language::from_path("Program.cs"), Language::CSharp);
        assert_eq!(Language::from_path("src/Controllers/UserController.cs"), Language::CSharp);
        assert_eq!(Language::from_path("Models/User.cs"), Language::CSharp);
    }

    // === C# imports ===

    #[test]
    fn test_csharp_simple_using() {
        let e = engine();
        let result = e.parse_file("App.cs", "using System;").unwrap();
        assert_eq!(result.language, Language::CSharp);
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "System");
    }

    #[test]
    fn test_csharp_qualified_using() {
        let e = engine();
        let result = e.parse_file("App.cs", "using System.Collections.Generic;").unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "System.Collections.Generic");
        assert_eq!(result.imports[0].names[0].name, "*");
        assert!(result.imports[0].is_namespace);
    }

    #[test]
    fn test_csharp_multiple_usings() {
        let e = engine();
        let source = r#"
using System;
using System.Linq;
using Microsoft.AspNetCore.Mvc;
using Microsoft.EntityFrameworkCore;
"#;
        let result = e.parse_file("Controller.cs", source).unwrap();
        assert_eq!(result.imports.len(), 4);
        let sources: Vec<&str> = result.imports.iter().map(|i| i.source.as_str()).collect();
        assert!(sources.contains(&"System"));
        assert!(sources.contains(&"System.Linq"));
        assert!(sources.contains(&"Microsoft.AspNetCore.Mvc"));
        assert!(sources.contains(&"Microsoft.EntityFrameworkCore"));
    }

    #[test]
    fn test_csharp_aspnet_using() {
        let e = engine();
        let result = e.parse_file("App.cs", "using Microsoft.AspNetCore.Mvc;").unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "Microsoft.AspNetCore.Mvc");
        assert_eq!(result.imports[0].names[0].name, "*");
    }

    // === C# definitions ===

    #[test]
    fn test_csharp_class_definition() {
        let e = engine();
        let source = r#"
public class UserService
{
}
"#;
        let result = e.parse_file("UserService.cs", source).unwrap();
        let class_defs: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Class)
            .map(|d| d.name.as_str())
            .collect();
        assert!(class_defs.contains(&"UserService"));
    }

    #[test]
    fn test_csharp_interface_definition() {
        let e = engine();
        let source = r#"
public interface IUserRepository
{
    User FindById(int id);
}
"#;
        let result = e.parse_file("IUserRepository.cs", source).unwrap();
        let iface_defs: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Interface)
            .map(|d| d.name.as_str())
            .collect();
        assert!(iface_defs.contains(&"IUserRepository"));
    }

    #[test]
    fn test_csharp_struct_definition() {
        let e = engine();
        let source = r#"
public struct Point
{
    public int X;
    public int Y;
}
"#;
        let result = e.parse_file("Point.cs", source).unwrap();
        let struct_defs: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Class)
            .map(|d| d.name.as_str())
            .collect();
        assert!(struct_defs.contains(&"Point"));
    }

    #[test]
    fn test_csharp_record_definition() {
        let e = engine();
        let source = r#"
public record UserDto(string Name, string Email);
"#;
        let result = e.parse_file("UserDto.cs", source).unwrap();
        let record_defs: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Class)
            .map(|d| d.name.as_str())
            .collect();
        assert!(record_defs.contains(&"UserDto"));
    }

    #[test]
    fn test_csharp_method_definitions() {
        let e = engine();
        let source = r#"
public class UserService
{
    public User FindUser(int id)
    {
        return null;
    }

    public void CreateUser(string name)
    {
    }
}
"#;
        let result = e.parse_file("UserService.cs", source).unwrap();
        let fn_defs: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Function)
            .map(|d| d.name.as_str())
            .collect();
        assert!(fn_defs.contains(&"FindUser"));
        assert!(fn_defs.contains(&"CreateUser"));
    }

    #[test]
    fn test_csharp_enum_definition() {
        let e = engine();
        let source = r#"
public enum Status
{
    Active,
    Inactive
}
"#;
        let result = e.parse_file("Status.cs", source).unwrap();
        let class_defs: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Class)
            .map(|d| d.name.as_str())
            .collect();
        assert!(class_defs.contains(&"Status"));
    }

    #[test]
    fn test_csharp_constructor_definition() {
        let e = engine();
        let source = r#"
public class UserService
{
    public UserService(IUserRepository repo)
    {
    }
}
"#;
        let result = e.parse_file("UserService.cs", source).unwrap();
        let fn_defs: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Function)
            .map(|d| d.name.as_str())
            .collect();
        assert!(fn_defs.contains(&"UserService"));
    }

    #[test]
    fn test_csharp_property_definition() {
        let e = engine();
        let source = r#"
public class Config
{
    public string ApiKey { get; set; }
    public int MaxRetries { get; set; } = 3;
}
"#;
        let result = e.parse_file("Config.cs", source).unwrap();
        let prop_defs: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Constant)
            .map(|d| d.name.as_str())
            .collect();
        assert!(prop_defs.contains(&"ApiKey"));
        assert!(prop_defs.contains(&"MaxRetries"));
    }

    #[test]
    fn test_csharp_field_definition() {
        let e = engine();
        let source = r#"
public class Config
{
    private readonly string _connectionString;
    public static readonly int MaxRetries = 3;
}
"#;
        let result = e.parse_file("Config.cs", source).unwrap();
        let field_defs: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Constant)
            .map(|d| d.name.as_str())
            .collect();
        assert!(field_defs.contains(&"_connectionString"));
        assert!(field_defs.contains(&"MaxRetries"));
    }

    #[test]
    fn test_csharp_delegate_definition() {
        let e = engine();
        let source = r#"
public delegate void EventHandler(object sender, EventArgs e);
"#;
        let result = e.parse_file("EventHandler.cs", source).unwrap();
        let delegate_defs: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Interface)
            .map(|d| d.name.as_str())
            .collect();
        assert!(delegate_defs.contains(&"EventHandler"));
    }

    // === C# call sites ===

    #[test]
    fn test_csharp_method_call() {
        let e = engine();
        let source = r#"
public class App
{
    public void Run()
    {
        _userService.FindUser(42);
    }
}
"#;
        let result = e.parse_file("App.cs", source).unwrap();
        let callees: Vec<&str> = result.call_sites.iter()
            .map(|c| c.callee.as_str())
            .collect();
        assert!(callees.contains(&"FindUser"));
    }

    #[test]
    fn test_csharp_direct_call() {
        let e = engine();
        let source = r#"
public class App
{
    public void Run()
    {
        DoSomething();
    }
}
"#;
        let result = e.parse_file("App.cs", source).unwrap();
        let callees: Vec<&str> = result.call_sites.iter()
            .map(|c| c.callee.as_str())
            .collect();
        assert!(callees.contains(&"DoSomething"));
    }

    #[test]
    fn test_csharp_constructor_call() {
        let e = engine();
        let source = r#"
public class App
{
    public void Run()
    {
        var user = new User("Alice");
    }
}
"#;
        let result = e.parse_file("App.cs", source).unwrap();
        let callees: Vec<&str> = result.call_sites.iter()
            .map(|c| c.callee.as_str())
            .collect();
        assert!(callees.contains(&"User"));
    }

    #[test]
    fn test_csharp_call_containing_function() {
        let e = engine();
        let source = r#"
public class App
{
    public void Run()
    {
        DoSomething();
    }
}
"#;
        let result = e.parse_file("App.cs", source).unwrap();
        let call = result.call_sites.iter().find(|c| c.callee == "DoSomething").unwrap();
        assert_eq!(call.containing_function.as_deref(), Some("Run"));
    }

    // === C# data flow ===

    #[test]
    fn test_csharp_data_flow_assignment() {
        let e = engine();
        let source = r#"
public class App
{
    public void Run()
    {
        var user = FindUser(1);
    }
}
"#;
        let df = e.extract_data_flow("App.cs", source).unwrap();
        assert!(!df.assignments.is_empty());
        assert_eq!(df.assignments[0].variable, "user");
        assert_eq!(df.assignments[0].callee, "FindUser");
    }

    #[test]
    fn test_csharp_data_flow_member_assignment() {
        let e = engine();
        let source = r#"
public class App
{
    public void Run()
    {
        var users = _repository.GetAll();
    }
}
"#;
        let df = e.extract_data_flow("App.cs", source).unwrap();
        assert!(!df.assignments.is_empty());
        assert_eq!(df.assignments[0].variable, "users");
        assert_eq!(df.assignments[0].callee, "GetAll");
    }

    #[test]
    fn test_csharp_data_flow_constructor_assignment() {
        let e = engine();
        let source = r#"
public class App
{
    public void Run()
    {
        var svc = new UserService();
    }
}
"#;
        let df = e.extract_data_flow("App.cs", source).unwrap();
        assert!(!df.assignments.is_empty());
        assert_eq!(df.assignments[0].variable, "svc");
        assert_eq!(df.assignments[0].callee, "UserService");
    }

    // === C# empty / edge cases ===

    #[test]
    fn test_csharp_empty_source() {
        let e = engine();
        let result = e.parse_file("App.cs", "").unwrap();
        assert_eq!(result.language, Language::CSharp);
        assert!(result.definitions.is_empty());
        assert!(result.imports.is_empty());
    }

    // === C# full module ===

    #[test]
    fn test_csharp_full_aspnet_controller() {
        let e = engine();
        let source = r#"
using System.Collections.Generic;
using Microsoft.AspNetCore.Mvc;

namespace MyApp.Controllers
{
    [ApiController]
    [Route("api/[controller]")]
    public class UsersController : ControllerBase
    {
        private readonly IUserService _userService;

        public UsersController(IUserService userService)
        {
            _userService = userService;
        }

        [HttpGet]
        public ActionResult<IEnumerable<User>> GetUsers()
        {
            return Ok(_userService.FindAll());
        }

        [HttpPost]
        public ActionResult<User> CreateUser(User user)
        {
            return Ok(_userService.Save(user));
        }
    }
}
"#;
        let result = e.parse_file("UsersController.cs", source).unwrap();

        // Imports
        assert!(result.imports.len() >= 2);
        let import_sources: Vec<&str> = result.imports.iter().map(|i| i.source.as_str()).collect();
        assert!(import_sources.contains(&"System.Collections.Generic"));
        assert!(import_sources.contains(&"Microsoft.AspNetCore.Mvc"));

        // Definitions
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"UsersController"));
        assert!(def_names.contains(&"GetUsers"));
        assert!(def_names.contains(&"CreateUser"));

        // Call sites
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"FindAll"));
        assert!(callees.contains(&"Save"));
        assert!(callees.contains(&"Ok"));
    }

    // ===================================================================
    // PHP Tests
    // ===================================================================

    #[test]
    fn test_php_language_detection() {
        assert_eq!(Language::from_path("app/Http/Controllers/UserController.php"), Language::Php);
        assert_eq!(Language::from_path("src/Services/UserService.php"), Language::Php);
        assert_eq!(Language::from_path("tests/Unit/UserTest.php"), Language::Php);
    }

    #[test]
    fn test_php_use_import() {
        let e = engine();
        let result = e
            .parse_file(
                "app/Models/User.php",
                "<?php\n\nnamespace App\\Models;\n\nuse Illuminate\\Database\\Eloquent\\Model;\n\nclass User extends Model {}\n",
            )
            .unwrap();
        assert_eq!(result.language, Language::Php);
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "Illuminate\\Database\\Eloquent\\Model");
        assert_eq!(result.imports[0].names[0].name, "Model");
    }

    #[test]
    fn test_php_use_import_with_alias() {
        let e = engine();
        let result = e
            .parse_file(
                "app/Services/Service.php",
                "<?php\n\nuse App\\Services\\UserService as US;\n",
            )
            .unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "App\\Services\\UserService");
        assert_eq!(result.imports[0].names[0].name, "UserService");
        assert_eq!(result.imports[0].names[0].alias, Some("US".to_string()));
    }

    #[test]
    fn test_php_multiple_imports() {
        let e = engine();
        let result = e
            .parse_file(
                "app/Http/Controllers/UserController.php",
                "<?php\n\nuse Illuminate\\Http\\Request;\nuse App\\Models\\User;\nuse App\\Services\\UserService;\n",
            )
            .unwrap();
        assert_eq!(result.imports.len(), 3);
        let sources: Vec<&str> = result.imports.iter().map(|i| i.source.as_str()).collect();
        assert!(sources.contains(&"Illuminate\\Http\\Request"));
        assert!(sources.contains(&"App\\Models\\User"));
        assert!(sources.contains(&"App\\Services\\UserService"));
    }

    #[test]
    fn test_php_require_include() {
        let e = engine();
        let result = e
            .parse_file(
                "index.php",
                "<?php\n\nrequire 'config.php';\nrequire_once 'vendor/autoload.php';\ninclude 'helpers.php';\ninclude_once 'utils.php';\n",
            )
            .unwrap();
        assert_eq!(result.imports.len(), 4);
        let sources: Vec<&str> = result.imports.iter().map(|i| i.source.as_str()).collect();
        assert!(sources.contains(&"config.php"));
        assert!(sources.contains(&"vendor/autoload.php"));
        assert!(sources.contains(&"helpers.php"));
        assert!(sources.contains(&"utils.php"));
    }

    #[test]
    fn test_php_function_definition() {
        let e = engine();
        let result = e
            .parse_file(
                "helpers.php",
                "<?php\n\nfunction greet($name) {\n    return \"Hello, \" . $name;\n}\n\nfunction farewell($name) {\n    return \"Goodbye, \" . $name;\n}\n",
            )
            .unwrap();
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"greet"));
        assert!(def_names.contains(&"farewell"));
        assert_eq!(result.definitions.len(), 2);
        // Both should be Function kind
        assert!(result.definitions.iter().all(|d| d.kind == SymbolKind::Function));
    }

    #[test]
    fn test_php_class_definition() {
        let e = engine();
        let result = e
            .parse_file(
                "app/Models/User.php",
                "<?php\n\nclass User {\n    public function getName() {\n        return $this->name;\n    }\n}\n",
            )
            .unwrap();
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"User"));
        assert!(def_names.contains(&"getName"));
        let class_def = result.definitions.iter().find(|d| d.name == "User").unwrap();
        assert_eq!(class_def.kind, SymbolKind::Class);
    }

    #[test]
    fn test_php_interface_definition() {
        let e = engine();
        let result = e
            .parse_file(
                "app/Contracts/Greetable.php",
                "<?php\n\ninterface Greetable {\n    public function greet();\n}\n",
            )
            .unwrap();
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"Greetable"));
        let iface_def = result.definitions.iter().find(|d| d.name == "Greetable").unwrap();
        assert_eq!(iface_def.kind, SymbolKind::Interface);
    }

    #[test]
    fn test_php_trait_definition() {
        let e = engine();
        let result = e
            .parse_file(
                "app/Traits/Loggable.php",
                "<?php\n\ntrait Loggable {\n    public function log($msg) {\n        echo $msg;\n    }\n}\n",
            )
            .unwrap();
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"Loggable"));
        assert!(def_names.contains(&"log"));
    }

    #[test]
    fn test_php_enum_definition() {
        let e = engine();
        let result = e
            .parse_file(
                "app/Enums/Color.php",
                "<?php\n\nenum Color {\n    case Red;\n    case Blue;\n}\n",
            )
            .unwrap();
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"Color"));
    }

    #[test]
    fn test_php_const_definition() {
        let e = engine();
        let result = e
            .parse_file(
                "config.php",
                "<?php\n\nconst MAX_SIZE = 100;\nconst APP_NAME = 'flowdiff';\n",
            )
            .unwrap();
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"MAX_SIZE"));
        assert!(def_names.contains(&"APP_NAME"));
        assert!(result.definitions.iter().all(|d| d.kind == SymbolKind::Constant));
    }

    #[test]
    fn test_php_method_definitions() {
        let e = engine();
        let result = e
            .parse_file(
                "app/Services/UserService.php",
                "<?php\n\nclass UserService {\n    public function findAll() {\n        return [];\n    }\n\n    public function findById($id) {\n        return null;\n    }\n\n    public function create($data) {\n        return $data;\n    }\n}\n",
            )
            .unwrap();
        let method_names: Vec<&str> = result.definitions.iter()
            .filter(|d| d.kind == SymbolKind::Function)
            .map(|d| d.name.as_str())
            .collect();
        assert!(method_names.contains(&"findAll"));
        assert!(method_names.contains(&"findById"));
        assert!(method_names.contains(&"create"));
    }

    #[test]
    fn test_php_function_call() {
        let e = engine();
        let result = e
            .parse_file(
                "index.php",
                "<?php\n\n$result = greet('World');\necho strlen($result);\n",
            )
            .unwrap();
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"greet"));
        assert!(callees.contains(&"strlen"));
    }

    #[test]
    fn test_php_method_call() {
        let e = engine();
        let result = e
            .parse_file(
                "app/Http/Controllers/UserController.php",
                "<?php\n\nclass UserController {\n    public function index() {\n        $users = $this->service->findAll();\n        return response()->json($users);\n    }\n}\n",
            )
            .unwrap();
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"findAll"));
        assert!(callees.contains(&"response"));
        assert!(callees.contains(&"json"));
    }

    #[test]
    fn test_php_static_call() {
        let e = engine();
        let result = e
            .parse_file(
                "app/Http/Controllers/UserController.php",
                "<?php\n\nclass UserController {\n    public function index() {\n        $users = User::all();\n        $user = User::find(1);\n        return $users;\n    }\n}\n",
            )
            .unwrap();
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"all"));
        assert!(callees.contains(&"find"));
    }

    #[test]
    fn test_php_object_creation() {
        let e = engine();
        let result = e
            .parse_file(
                "index.php",
                "<?php\n\n$service = new UserService();\n$response = new JsonResponse($data);\n",
            )
            .unwrap();
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"UserService"));
        assert!(callees.contains(&"JsonResponse"));
    }

    #[test]
    fn test_php_call_containing_function() {
        let e = engine();
        let result = e
            .parse_file(
                "app/Services/UserService.php",
                "<?php\n\nclass UserService {\n    public function create($data) {\n        $user = User::create($data);\n        return $user;\n    }\n}\n",
            )
            .unwrap();
        let create_call = result.call_sites.iter().find(|c| c.callee == "create").unwrap();
        // The containing function should be the method "create"
        assert!(create_call.containing_function.is_some());
        assert_eq!(create_call.containing_function.as_deref(), Some("create"));
    }

    #[test]
    fn test_php_data_flow_function_assignment() {
        let e = engine();
        let result = e
            .extract_data_flow(
                "index.php",
                "<?php\n\n$result = greet('World');\n",
            )
            .unwrap();
        assert!(!result.assignments.is_empty());
        let a = &result.assignments[0];
        assert_eq!(a.variable, "result");
        assert_eq!(a.callee, "greet");
    }

    #[test]
    fn test_php_data_flow_method_assignment() {
        let e = engine();
        let result = e
            .extract_data_flow(
                "app/Controllers/UserController.php",
                "<?php\n\nclass UserController {\n    public function index() {\n        $users = $this->service->findAll();\n    }\n}\n",
            )
            .unwrap();
        let assignment = result.assignments.iter().find(|a| a.variable == "users");
        assert!(assignment.is_some(), "should find $users assignment");
        assert_eq!(assignment.unwrap().callee, "findAll");
    }

    #[test]
    fn test_php_data_flow_static_assignment() {
        let e = engine();
        let result = e
            .extract_data_flow(
                "app/Controllers/UserController.php",
                "<?php\n\nclass UserController {\n    public function index() {\n        $users = User::all();\n    }\n}\n",
            )
            .unwrap();
        let assignment = result.assignments.iter().find(|a| a.variable == "users");
        assert!(assignment.is_some(), "should find $users assignment");
        assert_eq!(assignment.unwrap().callee, "all");
    }

    #[test]
    fn test_php_data_flow_constructor_assignment() {
        let e = engine();
        let result = e
            .extract_data_flow(
                "index.php",
                "<?php\n\n$service = new UserService();\n",
            )
            .unwrap();
        let assignment = result.assignments.iter().find(|a| a.variable == "service");
        assert!(assignment.is_some(), "should find $service assignment");
        assert_eq!(assignment.unwrap().callee, "UserService");
    }

    #[test]
    fn test_php_empty_source() {
        let e = engine();
        let result = e.parse_file("empty.php", "<?php\n").unwrap();
        assert_eq!(result.language, Language::Php);
        assert!(result.definitions.is_empty());
        assert!(result.imports.is_empty());
        assert!(result.call_sites.is_empty());
    }

    #[test]
    fn test_php_full_laravel_controller() {
        let e = engine();
        let source = "<?php\n\nnamespace App\\Http\\Controllers;\n\nuse Illuminate\\Http\\Request;\nuse App\\Models\\User;\nuse App\\Services\\UserService;\n\nclass UserController extends Controller\n{\n    private $service;\n\n    public function __construct(UserService $service)\n    {\n        $this->service = $service;\n    }\n\n    public function index()\n    {\n        $users = User::all();\n        return response()->json($users);\n    }\n\n    public function store(Request $request)\n    {\n        $data = $request->validated();\n        $user = User::create($data);\n        return response()->json($user, 201);\n    }\n\n    public function show(User $user)\n    {\n        return response()->json($user);\n    }\n\n    public function destroy(User $user)\n    {\n        $user->delete();\n        return response()->json(null, 204);\n    }\n}\n";
        let result = e.parse_file("app/Http/Controllers/UserController.php", source).unwrap();
        assert_eq!(result.language, Language::Php);

        // Imports
        let import_sources: Vec<&str> = result.imports.iter().map(|i| i.source.as_str()).collect();
        assert!(import_sources.contains(&"Illuminate\\Http\\Request"));
        assert!(import_sources.contains(&"App\\Models\\User"));
        assert!(import_sources.contains(&"App\\Services\\UserService"));

        // Definitions
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"UserController"));
        assert!(def_names.contains(&"__construct"));
        assert!(def_names.contains(&"index"));
        assert!(def_names.contains(&"store"));
        assert!(def_names.contains(&"show"));
        assert!(def_names.contains(&"destroy"));

        // Call sites
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"all"));
        assert!(callees.contains(&"response"));
        assert!(callees.contains(&"json"));
        assert!(callees.contains(&"validated"));
        assert!(callees.contains(&"create"));
        assert!(callees.contains(&"delete"));
    }

    // -----------------------------------------------------------------------
    // Ruby tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_ruby_language_detection() {
        assert_eq!(Language::from_path("app/controllers/users_controller.rb"), Language::Ruby);
        assert_eq!(Language::from_path("app/models/user.rb"), Language::Ruby);
        assert_eq!(Language::from_path("spec/models/user_spec.rb"), Language::Ruby);
    }

    #[test]
    fn test_ruby_require_import() {
        let e = engine();
        let result = e
            .parse_file(
                "app/models/user.rb",
                "require 'json'\nrequire 'active_record'\n",
            )
            .unwrap();
        assert_eq!(result.language, Language::Ruby);
        assert_eq!(result.imports.len(), 2);
        let sources: Vec<&str> = result.imports.iter().map(|i| i.source.as_str()).collect();
        assert!(sources.contains(&"json"));
        assert!(sources.contains(&"active_record"));
    }

    #[test]
    fn test_ruby_require_relative_import() {
        let e = engine();
        let result = e
            .parse_file(
                "app/services/user_service.rb",
                "require_relative '../models/user'\nrequire_relative '../repositories/user_repository'\n",
            )
            .unwrap();
        assert_eq!(result.imports.len(), 2);
        let sources: Vec<&str> = result.imports.iter().map(|i| i.source.as_str()).collect();
        assert!(sources.contains(&"../models/user"));
        assert!(sources.contains(&"../repositories/user_repository"));
    }

    #[test]
    fn test_ruby_include_extend_imports() {
        let e = engine();
        let result = e
            .parse_file(
                "app/models/user.rb",
                "class User\n  include Logging\n  extend ClassMethods\nend\n",
            )
            .unwrap();
        let import_sources: Vec<&str> = result.imports.iter().map(|i| i.source.as_str()).collect();
        assert!(import_sources.contains(&"Logging"));
        assert!(import_sources.contains(&"ClassMethods"));
        // include/extend imports should have names
        let logging_import = result.imports.iter().find(|i| i.source == "Logging").unwrap();
        assert_eq!(logging_import.names[0].name, "Logging");
    }

    #[test]
    fn test_ruby_method_definition() {
        let e = engine();
        let result = e
            .parse_file(
                "app/services/user_service.rb",
                "class UserService\n  def create(attrs)\n    User.new(attrs)\n  end\n\n  def find(id)\n    User.find(id)\n  end\nend\n",
            )
            .unwrap();
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"create"));
        assert!(def_names.contains(&"find"));
        assert!(def_names.contains(&"UserService"));
        assert!(result.definitions.iter().filter(|d| d.kind == SymbolKind::Function).count() >= 2);
    }

    #[test]
    fn test_ruby_singleton_method() {
        let e = engine();
        let result = e
            .parse_file(
                "app/services/user_service.rb",
                "class UserService\n  def self.instance\n    @inst ||= new\n  end\nend\n",
            )
            .unwrap();
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"instance"));
        assert!(def_names.contains(&"UserService"));
    }

    #[test]
    fn test_ruby_class_definition() {
        let e = engine();
        let result = e
            .parse_file(
                "app/models/user.rb",
                "class User < ActiveRecord::Base\n  def name\n    @name\n  end\nend\n",
            )
            .unwrap();
        let class_def = result.definitions.iter().find(|d| d.name == "User").unwrap();
        assert_eq!(class_def.kind, SymbolKind::Class);
    }

    #[test]
    fn test_ruby_module_definition() {
        let e = engine();
        let result = e
            .parse_file(
                "app/concerns/logging.rb",
                "module Logging\n  def log(msg)\n    puts msg\n  end\nend\n",
            )
            .unwrap();
        let mod_def = result.definitions.iter().find(|d| d.name == "Logging").unwrap();
        assert_eq!(mod_def.kind, SymbolKind::Module);
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"log"));
    }

    #[test]
    fn test_ruby_constant_assignment() {
        let e = engine();
        let result = e
            .parse_file(
                "config/constants.rb",
                "MAX_RETRIES = 3\nDEFAULT_TIMEOUT = 30\n",
            )
            .unwrap();
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"MAX_RETRIES"));
        assert!(def_names.contains(&"DEFAULT_TIMEOUT"));
        assert!(result.definitions.iter().all(|d| d.kind == SymbolKind::Constant));
    }

    #[test]
    fn test_ruby_method_call() {
        let e = engine();
        let result = e
            .parse_file(
                "app/services/user_service.rb",
                "class UserService\n  def create(attrs)\n    user = User.new(attrs)\n    user.save()\n    EventBus.publish('user.created', user)\n  end\nend\n",
            )
            .unwrap();
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"new"));
        assert!(callees.contains(&"save"));
        assert!(callees.contains(&"publish"));
    }

    #[test]
    fn test_ruby_call_containing_function() {
        let e = engine();
        let result = e
            .parse_file(
                "app/services/user_service.rb",
                "class UserService\n  def create(attrs)\n    User.new(attrs)\n  end\n\n  def find(id)\n    User.find(id)\n  end\nend\n",
            )
            .unwrap();
        let create_call = result.call_sites.iter().find(|c| c.callee == "new").unwrap();
        assert_eq!(create_call.containing_function, Some("create".to_string()));
        let find_call = result.call_sites.iter().find(|c| c.callee == "find").unwrap();
        assert_eq!(find_call.containing_function, Some("find".to_string()));
    }

    #[test]
    fn test_ruby_data_flow_assignment() {
        let e = engine();
        let result = e
            .extract_data_flow("app/services/user_service.rb", "class UserService\n  def create(attrs)\n    user = User.new(attrs)\n    result = user.save()\n  end\nend\n")
            .unwrap();
        let var_names: Vec<&str> = result.assignments.iter().map(|a| a.variable.as_str()).collect();
        assert!(var_names.contains(&"user"));
        assert!(var_names.contains(&"result"));
    }

    #[test]
    fn test_ruby_data_flow_instance_var_assignment() {
        let e = engine();
        let result = e
            .extract_data_flow("app/controllers/users_controller.rb", "class UsersController\n  def index\n    @users = User.all()\n  end\nend\n")
            .unwrap();
        let var_names: Vec<&str> = result.assignments.iter().map(|a| a.variable.as_str()).collect();
        assert!(var_names.contains(&"@users"));
    }

    #[test]
    fn test_ruby_empty_source() {
        let e = engine();
        let result = e.parse_file("empty.rb", "").unwrap();
        assert_eq!(result.language, Language::Ruby);
        assert!(result.definitions.is_empty());
        assert!(result.imports.is_empty());
        assert!(result.call_sites.is_empty());
    }

    #[test]
    fn test_ruby_multiple_imports() {
        let e = engine();
        let result = e
            .parse_file(
                "app/services/user_service.rb",
                "require 'json'\nrequire 'logger'\nrequire_relative '../models/user'\nrequire_relative '../repositories/user_repo'\n",
            )
            .unwrap();
        assert_eq!(result.imports.len(), 4);
    }

    #[test]
    fn test_ruby_full_rails_controller() {
        let e = engine();
        let source = "require 'action_controller'\nrequire_relative '../models/user'\nrequire_relative '../services/user_service'\n\nclass UsersController < ApplicationController\n  include Authentication\n\n  def index\n    @users = User.all()\n    respond_to()\n  end\n\n  def show\n    @user = User.find(params())\n  end\n\n  def create\n    @user = User.new(user_params())\n    @user.save()\n    redirect_to(@user)\n  end\n\n  private\n\n  def user_params\n    params().require().permit()\n  end\nend\n";
        let result = e.parse_file("app/controllers/users_controller.rb", source).unwrap();
        assert_eq!(result.language, Language::Ruby);

        // Imports
        let import_sources: Vec<&str> = result.imports.iter().map(|i| i.source.as_str()).collect();
        assert!(import_sources.contains(&"action_controller"));
        assert!(import_sources.contains(&"../models/user"));
        assert!(import_sources.contains(&"../services/user_service"));
        assert!(import_sources.contains(&"Authentication"));

        // Definitions
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"UsersController"));
        assert!(def_names.contains(&"index"));
        assert!(def_names.contains(&"show"));
        assert!(def_names.contains(&"create"));
        assert!(def_names.contains(&"user_params"));

        // Call sites
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"all"));
        assert!(callees.contains(&"respond_to"));
        assert!(callees.contains(&"find"));
        assert!(callees.contains(&"new"));
        assert!(callees.contains(&"save"));
        assert!(callees.contains(&"redirect_to"));
    }

    // Kotlin tests

    #[test]
    fn test_kotlin_language_detection() {
        assert_eq!(Language::from_path("src/main/kotlin/App.kt"), Language::Kotlin);
        assert_eq!(Language::from_path("build.gradle.kts"), Language::Kotlin);
        assert_eq!(Language::from_path("src/test/kotlin/AppTest.kt"), Language::Kotlin);
    }

    #[test]
    fn test_kotlin_imports() {
        let e = engine();
        let result = e
            .parse_file(
                "src/main/kotlin/App.kt",
                r#"import io.ktor.server.routing.get
import com.example.services.UserService
import com.example.models.*
import org.jetbrains.exposed.sql.Database as DB

fun main() {
    val service = UserService()
}
"#,
            )
            .unwrap();
        assert_eq!(result.language, Language::Kotlin);

        let import_sources: Vec<&str> = result.imports.iter().map(|i| i.source.as_str()).collect();
        assert!(
            import_sources.contains(&"io.ktor.server.routing"),
            "should extract Ktor import package; got: {:?}",
            import_sources
        );
        assert!(
            import_sources.contains(&"com.example.services"),
            "should extract service import package; got: {:?}",
            import_sources
        );

        // Wildcard import
        let wildcard_import = result.imports.iter().find(|i| {
            i.names.iter().any(|n| n.name == "*")
        });
        assert!(
            wildcard_import.is_some(),
            "should detect wildcard import; imports: {:?}",
            result.imports
        );

        // Aliased import
        let aliased_import = result.imports.iter().find(|i| {
            i.names.iter().any(|n| n.alias.as_deref() == Some("DB"))
        });
        assert!(
            aliased_import.is_some(),
            "should detect aliased import (Database as DB); imports: {:?}",
            result.imports
        );
    }

    #[test]
    fn test_kotlin_definitions() {
        let e = engine();
        let result = e
            .parse_file(
                "src/main/kotlin/models/User.kt",
                r#"import kotlinx.serialization.Serializable

data class User(
    val id: String,
    val name: String
)

object UserFactory {
    fun create(name: String): User {
        return User("1", name)
    }
}

fun greet(user: User): String {
    return "Hello, ${user.name}"
}

val DEFAULT_NAME = "World"

typealias UserId = String
"#,
            )
            .unwrap();
        assert_eq!(result.language, Language::Kotlin);

        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"User"), "should detect data class; got: {:?}", def_names);
        assert!(def_names.contains(&"UserFactory"), "should detect object; got: {:?}", def_names);
        assert!(def_names.contains(&"create"), "should detect function in object; got: {:?}", def_names);
        assert!(def_names.contains(&"greet"), "should detect top-level function; got: {:?}", def_names);
        assert!(def_names.contains(&"DEFAULT_NAME"), "should detect val property; got: {:?}", def_names);
        assert!(def_names.contains(&"UserId"), "should detect typealias; got: {:?}", def_names);
    }

    #[test]
    fn test_kotlin_call_sites() {
        let e = engine();
        let result = e
            .parse_file(
                "src/main/kotlin/services/UserService.kt",
                r#"import com.example.repositories.UserRepository

class UserService(private val repo: UserRepository) {
    fun findAll(): List<String> {
        val users = repo.findAll()
        return users.map { it.toString() }
    }

    fun create(name: String): String {
        val result = repo.save(name)
        println("Created: $result")
        return result
    }
}
"#,
            )
            .unwrap();
        assert_eq!(result.language, Language::Kotlin);

        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"findAll"), "should detect method call; got: {:?}", callees);
        assert!(callees.contains(&"save"), "should detect repo.save call; got: {:?}", callees);
        assert!(callees.contains(&"println"), "should detect println call; got: {:?}", callees);
    }

    #[test]
    fn test_kotlin_data_flow() {
        let e = engine();
        let dfi = e
            .extract_data_flow(
                "src/main/kotlin/services/UserService.kt",
                r#"import com.example.repositories.UserRepository

class UserService(private val repo: UserRepository) {
    fun findAll(): List<String> {
        val users = repo.findAll()
        return users
    }
}
"#,
            )
            .unwrap();

        // Should detect val users = repo.findAll() as an assignment
        let assign_vars: Vec<&str> = dfi.assignments.iter().map(|a| a.variable.as_str()).collect();
        assert!(
            assign_vars.contains(&"users"),
            "should detect val assignment from call; got: {:?}",
            assign_vars
        );
    }

    #[test]
    fn test_kotlin_full_parsing() {
        let e = engine();
        let source = r#"import io.ktor.server.routing.get
import io.ktor.server.routing.post
import io.ktor.server.response.respond
import com.example.services.UserService

fun Route.userRoutes(userService: UserService) {
    get("/users") {
        val users = userService.findAll()
        call.respond(users)
    }

    post("/users") {
        val user = userService.create(call)
        call.respond(user)
    }
}
"#;
        let result = e.parse_file("src/main/kotlin/routes/UserRoutes.kt", source).unwrap();
        assert_eq!(result.language, Language::Kotlin);

        // Imports
        let import_sources: Vec<&str> = result.imports.iter().map(|i| i.source.as_str()).collect();
        assert!(import_sources.contains(&"io.ktor.server.routing"));
        assert!(import_sources.contains(&"io.ktor.server.response"));
        assert!(import_sources.contains(&"com.example.services"));

        // Definitions
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"userRoutes"), "should detect extension function; got: {:?}", def_names);

        // Call sites
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"get"), "should detect get route; got: {:?}", callees);
        assert!(callees.contains(&"post"), "should detect post route; got: {:?}", callees);
        assert!(callees.contains(&"findAll"), "should detect service call; got: {:?}", callees);
        assert!(callees.contains(&"respond"), "should detect respond call; got: {:?}", callees);
    }

    // Swift tests

    #[test]
    fn test_swift_language_detection() {
        assert_eq!(Language::from_path("Sources/App/main.swift"), Language::Swift);
        assert_eq!(Language::from_path("Tests/AppTests/AppTests.swift"), Language::Swift);
        assert_eq!(Language::from_path("Package.swift"), Language::Swift);
    }

    #[test]
    fn test_swift_imports() {
        let e = engine();
        let result = e
            .parse_file(
                "Sources/App/Controllers/UserController.swift",
                r#"import Foundation
import Vapor

class UserController {
    func index(req: Request) throws -> EventLoopFuture<[User]> {
        return User.query(on: req.db).all()
    }
}
"#,
            )
            .unwrap();
        assert_eq!(result.language, Language::Swift);

        let import_sources: Vec<&str> = result.imports.iter().map(|i| i.source.as_str()).collect();
        assert!(
            import_sources.contains(&"Foundation"),
            "should detect Foundation import; got: {:?}",
            import_sources
        );
        assert!(
            import_sources.contains(&"Vapor"),
            "should detect Vapor import; got: {:?}",
            import_sources
        );
    }

    #[test]
    fn test_swift_definitions() {
        let e = engine();
        let result = e
            .parse_file(
                "Sources/App/Models/User.swift",
                r#"import Foundation

struct User: Content {
    var id: UUID?
    var name: String
}

class UserService {
    func findAll() -> [User] {
        return []
    }
}

protocol UserRepository {
    func findById(id: UUID) -> User?
}

enum UserRole {
    case admin
    case regular
}

let defaultName = "World"

typealias UserID = UUID

func greet(name: String) -> String {
    return "Hello, \(name)!"
}
"#,
            )
            .unwrap();
        assert_eq!(result.language, Language::Swift);

        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"User"), "should detect struct; got: {:?}", def_names);
        assert!(def_names.contains(&"UserService"), "should detect class; got: {:?}", def_names);
        assert!(def_names.contains(&"findAll"), "should detect method; got: {:?}", def_names);
        assert!(def_names.contains(&"UserRepository"), "should detect protocol; got: {:?}", def_names);
        assert!(def_names.contains(&"findById"), "should detect protocol function; got: {:?}", def_names);
        assert!(def_names.contains(&"UserRole"), "should detect enum; got: {:?}", def_names);
        assert!(def_names.contains(&"defaultName"), "should detect let property; got: {:?}", def_names);
        assert!(def_names.contains(&"UserID"), "should detect typealias; got: {:?}", def_names);
        assert!(def_names.contains(&"greet"), "should detect top-level function; got: {:?}", def_names);
    }

    #[test]
    fn test_swift_call_sites() {
        let e = engine();
        let result = e
            .parse_file(
                "Sources/App/Controllers/UserController.swift",
                r#"import Vapor

class UserController {
    func index(req: Request) throws -> [User] {
        let users = service.findAll()
        return users.map { $0.toDTO() }
    }

    func create(req: Request) throws -> User {
        let dto = try req.content.decode(UserDTO.self)
        let user = User(name: dto.name)
        try user.save(on: req.db)
        return user
    }
}
"#,
            )
            .unwrap();
        assert_eq!(result.language, Language::Swift);

        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"findAll"), "should detect method call; got: {:?}", callees);
        assert!(callees.contains(&"decode"), "should detect decode call; got: {:?}", callees);
        assert!(callees.contains(&"save"), "should detect save call; got: {:?}", callees);
    }

    #[test]
    fn test_swift_data_flow() {
        let e = engine();
        let dfi = e
            .extract_data_flow(
                "Sources/App/Controllers/UserController.swift",
                r#"import Vapor

func index(req: Request) throws -> [User] {
    let users = service.findAll()
    let result = transform(users)
    return result
}
"#,
            )
            .unwrap();

        let assign_vars: Vec<&str> = dfi.assignments.iter().map(|a| a.variable.as_str()).collect();
        assert!(
            assign_vars.contains(&"users"),
            "should detect let assignment from method call; got: {:?}",
            assign_vars
        );
        assert!(
            assign_vars.contains(&"result"),
            "should detect let assignment from function call; got: {:?}",
            assign_vars
        );
    }

    #[test]
    fn test_swift_full_parsing() {
        let e = engine();
        let source = r#"import Foundation
import Vapor
import Fluent

struct UserController: RouteCollection {
    func boot(routes: RoutesBuilder) throws {
        let users = routes.grouped("users")
        users.get(use: index)
        users.post(use: create)
    }

    func index(req: Request) throws -> EventLoopFuture<[User]> {
        return User.query(on: req.db).all()
    }

    func create(req: Request) throws -> EventLoopFuture<User> {
        let user = try req.content.decode(User.self)
        return user.save(on: req.db).map { user }
    }
}

typealias UserID = UUID
"#;
        let result = e.parse_file("Sources/App/Controllers/UserController.swift", source).unwrap();
        assert_eq!(result.language, Language::Swift);

        // Imports
        let import_sources: Vec<&str> = result.imports.iter().map(|i| i.source.as_str()).collect();
        assert!(import_sources.contains(&"Foundation"));
        assert!(import_sources.contains(&"Vapor"));
        assert!(import_sources.contains(&"Fluent"));

        // Definitions
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"UserController"), "should detect struct; got: {:?}", def_names);
        assert!(def_names.contains(&"boot"), "should detect boot function; got: {:?}", def_names);
        assert!(def_names.contains(&"index"), "should detect index function; got: {:?}", def_names);
        assert!(def_names.contains(&"create"), "should detect create function; got: {:?}", def_names);
        assert!(def_names.contains(&"UserID"), "should detect typealias; got: {:?}", def_names);

        // Call sites
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"grouped"), "should detect grouped call; got: {:?}", callees);
        assert!(callees.contains(&"get"), "should detect get route; got: {:?}", callees);
        assert!(callees.contains(&"post"), "should detect post route; got: {:?}", callees);
        assert!(callees.contains(&"all"), "should detect query().all(); got: {:?}", callees);
    }
}

// ===========================================================================
// C language tests
// ===========================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod c_tests {
    use super::*;

    fn engine() -> &'static QueryEngine {
        shared_test_engine()
    }

    // === C imports (#include) ===

    #[test]
    fn test_c_system_include() {
        let e = engine();
        let result = e
            .parse_file("main.c", "#include <stdio.h>\n#include <stdlib.h>\n")
            .unwrap();
        assert_eq!(result.imports.len(), 2);
        assert_eq!(result.imports[0].source, "stdio.h");
        assert_eq!(result.imports[0].names[0].name, "stdio");
        assert_eq!(result.imports[1].source, "stdlib.h");
        assert_eq!(result.imports[1].names[0].name, "stdlib");
    }

    #[test]
    fn test_c_local_include() {
        let e = engine();
        let result = e
            .parse_file("main.c", "#include \"myheader.h\"\n")
            .unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "myheader.h");
        assert_eq!(result.imports[0].names[0].name, "myheader");
    }

    #[test]
    fn test_c_include_with_path() {
        let e = engine();
        let result = e
            .parse_file("main.c", "#include <curl/curl.h>\n")
            .unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "curl/curl.h");
        assert_eq!(result.imports[0].names[0].name, "curl");
    }

    // === C function definitions ===

    #[test]
    fn test_c_function_definition() {
        let e = engine();
        let source = r#"
int add(int a, int b) {
    return a + b;
}

void print_hello() {
    printf("Hello\n");
}
"#;
        let result = e.parse_file("math.c", source).unwrap();
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"add"), "should detect add function; got: {:?}", def_names);
        assert!(def_names.contains(&"print_hello"), "should detect print_hello function; got: {:?}", def_names);
    }

    #[test]
    fn test_c_struct_definition() {
        let e = engine();
        let source = r#"
struct Point {
    int x;
    int y;
};
"#;
        let result = e.parse_file("types.c", source).unwrap();
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"Point"), "should detect struct Point; got: {:?}", def_names);
    }

    #[test]
    fn test_c_enum_definition() {
        let e = engine();
        let source = r#"
enum Color {
    RED,
    GREEN,
    BLUE
};
"#;
        let result = e.parse_file("color.c", source).unwrap();
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"Color"), "should detect enum Color; got: {:?}", def_names);
    }

    #[test]
    fn test_c_typedef() {
        let e = engine();
        // typedef struct X NewName; — struct typedef has type_identifier as declarator
        let source = "typedef struct Config AppConfig;\n";
        let result = e.parse_file("types.c", source).unwrap();
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"AppConfig"), "should detect typedef; got: {:?}", def_names);
    }

    // === C call sites ===

    #[test]
    fn test_c_simple_call() {
        let e = engine();
        let source = r#"
void foo() {
    int x = bar(42);
    printf("result: %d\n", x);
}
"#;
        let result = e.parse_file("main.c", source).unwrap();
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"bar"), "should detect bar call; got: {:?}", callees);
        assert!(callees.contains(&"printf"), "should detect printf call; got: {:?}", callees);
    }

    #[test]
    fn test_c_member_call() {
        let e = engine();
        let source = r#"
void process(struct Obj *obj) {
    obj->init();
}
"#;
        let result = e.parse_file("main.c", source).unwrap();
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"init"), "should detect member call via ->; got: {:?}", callees);
    }

    // === C data flow ===

    #[test]
    fn test_c_assignment_from_call() {
        let e = engine();
        let source = r#"
void foo() {
    int result = process_data();
}
"#;
        let df = e.extract_data_flow("main.c", source).unwrap();
        assert!(!df.assignments.is_empty(), "should detect assignment from call");
        assert_eq!(df.assignments[0].variable, "result");
        assert_eq!(df.assignments[0].callee, "process_data");
    }

    // === C language detection ===

    #[test]
    fn test_c_language_detection() {
        assert_eq!(Language::from_path("main.c"), Language::C);
        assert_eq!(Language::from_path("header.h"), Language::C);
    }

    // === C full integration ===

    #[test]
    fn test_c_full_file() {
        let e = engine();
        let source = r#"
#include <stdio.h>
#include "service.h"

struct Config {
    int port;
    char* host;
};

typedef struct Config AppConfig;

int process_request(int fd) {
    char* data = read_data(fd);
    int result = handle(data);
    printf("Done: %d\n", result);
    return result;
}

int main() {
    int server = init_server();
    process_request(server);
    return 0;
}
"#;
        let result = e.parse_file("server.c", source).unwrap();

        // Imports
        assert_eq!(result.imports.len(), 2);

        // Definitions
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"Config"), "struct Config; got: {:?}", def_names);
        assert!(def_names.contains(&"AppConfig"), "typedef AppConfig; got: {:?}", def_names);
        assert!(def_names.contains(&"process_request"), "process_request fn; got: {:?}", def_names);
        assert!(def_names.contains(&"main"), "main fn; got: {:?}", def_names);

        // Call sites
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"read_data"), "read_data call; got: {:?}", callees);
        assert!(callees.contains(&"handle"), "handle call; got: {:?}", callees);
        assert!(callees.contains(&"printf"), "printf call; got: {:?}", callees);
        assert!(callees.contains(&"init_server"), "init_server call; got: {:?}", callees);
        assert!(callees.contains(&"process_request"), "process_request call; got: {:?}", callees);
    }
}

// ===========================================================================
// C++ language tests
// ===========================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod cpp_tests {
    use super::*;

    fn engine() -> &'static QueryEngine {
        shared_test_engine()
    }

    // === C++ imports (#include) ===

    #[test]
    fn test_cpp_system_include() {
        let e = engine();
        let result = e
            .parse_file("main.cpp", "#include <iostream>\n#include <vector>\n")
            .unwrap();
        assert_eq!(result.imports.len(), 2);
        assert_eq!(result.imports[0].source, "iostream");
        assert_eq!(result.imports[1].source, "vector");
    }

    #[test]
    fn test_cpp_local_include() {
        let e = engine();
        let result = e
            .parse_file("main.cpp", "#include \"service.hpp\"\n")
            .unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "service.hpp");
        assert_eq!(result.imports[0].names[0].name, "service");
    }

    // === C++ definitions ===

    #[test]
    fn test_cpp_function_definition() {
        let e = engine();
        let source = r#"
int add(int a, int b) {
    return a + b;
}

std::string greet(const std::string& name) {
    return "Hello, " + name;
}
"#;
        let result = e.parse_file("math.cpp", source).unwrap();
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"add"), "should detect add function; got: {:?}", def_names);
        assert!(def_names.contains(&"greet"), "should detect greet function; got: {:?}", def_names);
    }

    #[test]
    fn test_cpp_class_definition() {
        let e = engine();
        let source = r#"
class UserService {
public:
    void create_user(const std::string& name);
    bool delete_user(int id);
private:
    int count_;
};
"#;
        let result = e.parse_file("service.cpp", source).unwrap();
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"UserService"), "should detect class UserService; got: {:?}", def_names);
    }

    #[test]
    fn test_cpp_struct_definition() {
        let e = engine();
        let source = r#"
struct Point {
    double x;
    double y;
};
"#;
        let result = e.parse_file("types.cpp", source).unwrap();
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"Point"), "should detect struct Point; got: {:?}", def_names);
    }

    #[test]
    fn test_cpp_namespace_definition() {
        let e = engine();
        let source = r#"
namespace myapp {
    void init();
}
"#;
        let result = e.parse_file("app.cpp", source).unwrap();
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"myapp"), "should detect namespace myapp; got: {:?}", def_names);
    }

    #[test]
    fn test_cpp_enum_definition() {
        let e = engine();
        let source = r#"
enum class Status {
    Active,
    Inactive,
    Deleted
};
"#;
        let result = e.parse_file("status.cpp", source).unwrap();
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"Status"), "should detect enum class Status; got: {:?}", def_names);
    }

    #[test]
    fn test_cpp_using_alias() {
        let e = engine();
        let source = "using StringVec = std::vector<std::string>;\n";
        let result = e.parse_file("types.cpp", source).unwrap();
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"StringVec"), "should detect using alias; got: {:?}", def_names);
    }

    // === C++ call sites ===

    #[test]
    fn test_cpp_simple_call() {
        let e = engine();
        let source = r#"
void foo() {
    auto result = process();
    std::cout << result << std::endl;
}
"#;
        let result = e.parse_file("main.cpp", source).unwrap();
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"process"), "should detect process call; got: {:?}", callees);
    }

    #[test]
    fn test_cpp_method_call() {
        let e = engine();
        let source = r#"
void bar(UserService& svc) {
    svc.create_user("Alice");
    svc.delete_user(42);
}
"#;
        let result = e.parse_file("main.cpp", source).unwrap();
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"create_user"), "should detect method call; got: {:?}", callees);
        assert!(callees.contains(&"delete_user"), "should detect method call; got: {:?}", callees);
    }

    #[test]
    fn test_cpp_qualified_call() {
        let e = engine();
        let source = r#"
void foo() {
    std::sort(v.begin(), v.end());
    std::transform(a.begin(), a.end(), b.begin(), op);
}
"#;
        let result = e.parse_file("algo.cpp", source).unwrap();
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"sort"), "should detect std::sort; got: {:?}", callees);
        assert!(callees.contains(&"transform"), "should detect std::transform; got: {:?}", callees);
    }

    // === C++ data flow ===

    #[test]
    fn test_cpp_assignment_from_call() {
        let e = engine();
        let source = r#"
void foo() {
    auto result = compute();
}
"#;
        let df = e.extract_data_flow("main.cpp", source).unwrap();
        assert!(!df.assignments.is_empty(), "should detect assignment from call");
        assert_eq!(df.assignments[0].variable, "result");
        assert_eq!(df.assignments[0].callee, "compute");
    }

    // === C++ language detection ===

    #[test]
    fn test_cpp_language_detection() {
        assert_eq!(Language::from_path("main.cpp"), Language::Cpp);
        assert_eq!(Language::from_path("main.cc"), Language::Cpp);
        assert_eq!(Language::from_path("main.cxx"), Language::Cpp);
        assert_eq!(Language::from_path("header.hpp"), Language::Cpp);
        assert_eq!(Language::from_path("header.hxx"), Language::Cpp);
        assert_eq!(Language::from_path("header.hh"), Language::Cpp);
        assert_eq!(Language::from_path("header.h++"), Language::Cpp);
    }

    // === C++ full integration ===

    #[test]
    fn test_cpp_full_file() {
        let e = engine();
        let source = r#"
#include <iostream>
#include <vector>
#include "repository.hpp"

namespace api {

class UserService {
public:
    std::vector<User> list_users() {
        return repo_.find_all();
    }

    void create_user(const std::string& name) {
        repo_.insert(name);
    }
private:
    UserRepository repo_;
};

using UserList = std::vector<User>;

}
"#;
        let result = e.parse_file("service.cpp", source).unwrap();

        // Imports
        assert_eq!(result.imports.len(), 3);

        // Definitions
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"api"), "namespace api; got: {:?}", def_names);
        assert!(def_names.contains(&"UserService"), "class UserService; got: {:?}", def_names);
        assert!(def_names.contains(&"UserList"), "using alias UserList; got: {:?}", def_names);

        // Call sites
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"find_all"), "find_all method call; got: {:?}", callees);
        assert!(callees.contains(&"insert"), "insert method call; got: {:?}", callees);
    }

    // Scala tests

    #[test]
    fn test_scala_language_detection() {
        assert_eq!(Language::from_path("src/main/scala/App.scala"), Language::Scala);
        assert_eq!(Language::from_path("build.sc"), Language::Scala);
        assert_eq!(Language::from_path("src/test/scala/AppTest.scala"), Language::Scala);
    }

    #[test]
    fn test_scala_imports() {
        let e = engine();
        let result = e
            .parse_file(
                "src/main/scala/App.scala",
                r#"import scala.collection.mutable
import akka.actor.{ActorSystem, Props}
import play.api.mvc._
import com.example.UserService

object Main {
  val system = ActorSystem("test")
}
"#,
            )
            .unwrap();
        assert_eq!(result.language, Language::Scala);

        let import_sources: Vec<&str> = result.imports.iter().map(|i| i.source.as_str()).collect();
        assert!(
            import_sources.contains(&"scala.collection"),
            "should extract scala.collection import; got: {:?}",
            import_sources
        );
        assert!(
            import_sources.contains(&"akka.actor"),
            "should extract akka.actor import; got: {:?}",
            import_sources
        );

        // Named imports {ActorSystem, Props}
        let akka_import = result.imports.iter().find(|i| i.source == "akka.actor").unwrap();
        let names: Vec<&str> = akka_import.names.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"ActorSystem"), "should have ActorSystem; got: {:?}", names);
        assert!(names.contains(&"Props"), "should have Props; got: {:?}", names);

        // Wildcard import
        let wildcard_import = result.imports.iter().find(|i| i.source == "play.api.mvc");
        assert!(
            wildcard_import.is_some(),
            "should detect play.api.mvc wildcard import; imports: {:?}",
            result.imports
        );
        assert!(wildcard_import.unwrap().is_namespace);

        // Regular import
        assert!(
            import_sources.contains(&"com.example"),
            "should extract com.example import; got: {:?}",
            import_sources
        );
    }

    #[test]
    fn test_scala_definitions() {
        let e = engine();
        let result = e
            .parse_file(
                "src/main/scala/models/User.scala",
                r#"package com.example

class UserService {
  def getUser(id: String): User = ???
  val maxRetries: Int = 3
}

trait Repository[T] {
  def findById(id: String): Option[T]
}

object UserService {
  def apply(): UserService = new UserService()
}

case class User(name: String, email: String)

sealed trait Shape

type UserId = String
"#,
            )
            .unwrap();
        assert_eq!(result.language, Language::Scala);

        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"UserService"), "should detect class; got: {:?}", def_names);
        assert!(def_names.contains(&"getUser"), "should detect def; got: {:?}", def_names);
        assert!(def_names.contains(&"maxRetries"), "should detect val; got: {:?}", def_names);
        assert!(def_names.contains(&"Repository"), "should detect trait; got: {:?}", def_names);
        assert!(def_names.contains(&"findById"), "should detect abstract def; got: {:?}", def_names);
        assert!(def_names.contains(&"apply"), "should detect object method; got: {:?}", def_names);
        assert!(def_names.contains(&"User"), "should detect case class; got: {:?}", def_names);
        assert!(def_names.contains(&"Shape"), "should detect sealed trait; got: {:?}", def_names);
        assert!(def_names.contains(&"UserId"), "should detect type alias; got: {:?}", def_names);

        // Verify kinds
        let user_service = result.definitions.iter().find(|d| d.name == "UserService").unwrap();
        assert_eq!(user_service.kind, SymbolKind::Class);

        let repository = result.definitions.iter().find(|d| d.name == "Repository").unwrap();
        assert_eq!(repository.kind, SymbolKind::Interface);

        let user_id = result.definitions.iter().find(|d| d.name == "UserId").unwrap();
        assert_eq!(user_id.kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn test_scala_call_sites() {
        let e = engine();
        let result = e
            .parse_file(
                "src/main/scala/services/UserService.scala",
                r#"import com.example.repositories.UserRepository

class UserService(repo: UserRepository) {
  def findAll(): List[User] = {
    val users = repo.findAll()
    users.map(_.toString())
  }

  def create(name: String): User = {
    val result = repo.save(name)
    println("Created")
    result
  }
}
"#,
            )
            .unwrap();
        assert_eq!(result.language, Language::Scala);

        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"findAll"), "should detect method call; got: {:?}", callees);
        assert!(callees.contains(&"save"), "should detect repo.save call; got: {:?}", callees);
        assert!(callees.contains(&"println"), "should detect println call; got: {:?}", callees);
    }

    #[test]
    fn test_scala_data_flow() {
        let e = engine();
        let dfi = e
            .extract_data_flow(
                "src/main/scala/services/UserService.scala",
                r#"class UserService(repo: UserRepository) {
  def findAll(): List[User] = {
    val users = repo.findAll()
    users
  }
}
"#,
            )
            .unwrap();

        assert!(
            !dfi.assignments.is_empty(),
            "should detect val assignment from method call; got: {:?}",
            dfi.assignments
        );
        let assign = dfi.assignments.iter().find(|a| a.variable == "users");
        assert!(
            assign.is_some(),
            "should find 'users' assignment; got: {:?}",
            dfi.assignments
        );
    }

    #[test]
    fn test_scala_full_file() {
        let e = engine();
        let source = r#"
import akka.actor.{ActorSystem, Props}
import akka.http.scaladsl.Http
import com.example.services.UserService
import com.example.repositories.UserRepository

object Main {
  def main(args: Array[String]): Unit = {
    val system = ActorSystem("test")
    val repo = UserRepository()
    val service = UserService(repo)
    val result = service.findAll()
    println(result)
    Http().newServerAt("0.0.0.0", 8080).bind(routes)
  }

  def routes(): Route = {
    get(complete("ok"))
  }
}

case class User(id: String, name: String)

type UserId = String
"#;
        let result = e.parse_file("src/main/scala/Main.scala", source).unwrap();

        // Imports
        assert_eq!(result.imports.len(), 4);
        let import_sources: Vec<&str> = result.imports.iter().map(|i| i.source.as_str()).collect();
        assert!(import_sources.contains(&"akka.actor"));
        assert!(import_sources.contains(&"akka.http.scaladsl"));
        assert!(import_sources.contains(&"com.example.services"));
        assert!(import_sources.contains(&"com.example.repositories"));

        // Definitions
        let def_names: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(def_names.contains(&"Main"), "object Main; got: {:?}", def_names);
        assert!(def_names.contains(&"main"), "def main; got: {:?}", def_names);
        assert!(def_names.contains(&"routes"), "def routes; got: {:?}", def_names);
        assert!(def_names.contains(&"User"), "case class User; got: {:?}", def_names);
        assert!(def_names.contains(&"UserId"), "type alias UserId; got: {:?}", def_names);

        // Call sites
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"ActorSystem"), "ActorSystem call; got: {:?}", callees);
        assert!(callees.contains(&"UserRepository"), "UserRepository call; got: {:?}", callees);
        assert!(callees.contains(&"UserService"), "UserService call; got: {:?}", callees);
        assert!(callees.contains(&"findAll"), "findAll call; got: {:?}", callees);
        assert!(callees.contains(&"println"), "println call; got: {:?}", callees);
    }
}
