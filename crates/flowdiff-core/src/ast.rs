//! AST parsing module using tree-sitter for extracting symbols, imports, exports, and call sites.

use crate::types::SymbolKind;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tree_sitter::{Node, Parser};

/// Errors from AST parsing operations.
#[derive(Debug, Error)]
pub enum AstError {
    #[error("failed to set parser language: {0}")]
    LanguageError(String),
    #[error("failed to parse source: {0}")]
    ParseError(String),
}

/// Detected programming language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    TypeScript,
    JavaScript,
    Python,
    Go,
    Rust,
    Java,
    CSharp,
    Php,
    Ruby,
    Unknown,
}

impl Language {
    /// Detect language from file path extension.
    pub fn from_path(path: &str) -> Self {
        match path.rsplit('.').next().unwrap_or("") {
            "ts" | "tsx" => Language::TypeScript,
            "js" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
            "py" | "pyi" => Language::Python,
            "go" => Language::Go,
            "rs" => Language::Rust,
            "java" => Language::Java,
            "cs" => Language::CSharp,
            "php" => Language::Php,
            "rb" => Language::Ruby,
            _ => Language::Unknown,
        }
    }
}

/// A symbol definition extracted from source code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Definition {
    pub name: String,
    pub kind: SymbolKind,
    pub start_line: usize,
    pub end_line: usize,
}

/// An import statement extracted from source code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportInfo {
    pub source: String,
    pub names: Vec<ImportedName>,
    pub is_default: bool,
    pub is_namespace: bool,
    pub line: usize,
}

/// A single imported name, optionally aliased.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedName {
    pub name: String,
    pub alias: Option<String>,
}

/// An export declaration extracted from source code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExportInfo {
    pub name: String,
    pub is_default: bool,
    pub is_reexport: bool,
    pub source: Option<String>,
    pub line: usize,
}

/// A function call site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallSite {
    pub callee: String,
    pub line: usize,
    pub containing_function: Option<String>,
}

/// Result of parsing a single source file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedFile {
    pub path: String,
    pub language: Language,
    pub definitions: Vec<Definition>,
    pub imports: Vec<ImportInfo>,
    pub exports: Vec<ExportInfo>,
    pub call_sites: Vec<CallSite>,
}

/// Represents a change to a symbol between old and new versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SymbolChange {
    Added(Definition),
    Removed(Definition),
    Modified { old: Definition, new: Definition },
}

/// A local variable assigned from a function call return value.
/// Captures patterns like `const x = funcA()` or `x = func_a()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VarCallAssignment {
    /// Name of the variable being assigned.
    pub variable: String,
    /// The callee expression (function being called).
    pub callee: String,
    /// Line number of the assignment.
    pub line: usize,
    /// Function containing this assignment.
    pub containing_function: Option<String>,
}

/// A function call with its resolved argument expressions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallWithArgs {
    /// The callee expression.
    pub callee: String,
    /// Argument expression texts (variable names, literals, nested calls, etc.).
    pub arguments: Vec<String>,
    /// Line number.
    pub line: usize,
    /// Function containing this call.
    pub containing_function: Option<String>,
}

/// Data flow information extracted from a source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataFlowInfo {
    /// Variables assigned from function call return values.
    pub assignments: Vec<VarCallAssignment>,
    /// Function calls with their argument expressions.
    pub calls_with_args: Vec<CallWithArgs>,
}

/// Parse a source file and extract symbols, imports, exports, and call sites.
pub fn parse_file(path: &str, source: &str) -> Result<ParsedFile, AstError> {
    let language = Language::from_path(path);
    match language {
        Language::TypeScript | Language::JavaScript => parse_typescript(path, source, language),
        Language::Python => parse_python(path, source),
        Language::Go => parse_go(path, source),
        Language::Rust | Language::Java | Language::CSharp | Language::Php | Language::Ruby | Language::Unknown => Ok(ParsedFile {
            path: path.to_string(),
            language,
            definitions: vec![],
            imports: vec![],
            exports: vec![],
            call_sites: vec![],
        }),
    }
}

/// Detect which symbols were added, removed, or modified between old and new versions.
pub fn detect_changed_symbols(old: &ParsedFile, new: &ParsedFile) -> Vec<SymbolChange> {
    let mut changes = Vec::new();

    // Find removed and modified.
    // Compare span (body size) rather than absolute line positions,
    // so that symbols merely relocated by surrounding edits are not flagged.
    for old_def in &old.definitions {
        match new
            .definitions
            .iter()
            .find(|d| d.name == old_def.name && d.kind == old_def.kind)
        {
            None => changes.push(SymbolChange::Removed(old_def.clone())),
            Some(new_def) => {
                let old_span = old_def.end_line.saturating_sub(old_def.start_line);
                let new_span = new_def.end_line.saturating_sub(new_def.start_line);
                if old_span != new_span {
                    changes.push(SymbolChange::Modified {
                        old: old_def.clone(),
                        new: new_def.clone(),
                    });
                }
            }
        }
    }

    // Find added
    for new_def in &new.definitions {
        if !old
            .definitions
            .iter()
            .any(|d| d.name == new_def.name && d.kind == new_def.kind)
        {
            changes.push(SymbolChange::Added(new_def.clone()));
        }
    }

    changes
}

/// Extract base class names from a Python class definition.
pub fn get_python_class_bases(source: &str, class_name: &str) -> Result<Vec<String>, AstError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .map_err(|e| AstError::LanguageError(e.to_string()))?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| AstError::ParseError("failed to parse".into()))?;

    let root = tree.root_node();
    let src = source.as_bytes();
    let mut bases = Vec::new();
    find_class_bases(&root, src, class_name, &mut bases);
    Ok(bases)
}

/// Extract data flow information from source code for tracking how data moves
/// through variable assignments and function calls within function bodies.
pub fn extract_data_flow_info(path: &str, source: &str) -> Result<DataFlowInfo, AstError> {
    let language = Language::from_path(path);
    match language {
        Language::TypeScript | Language::JavaScript => extract_ts_data_flow(source),
        Language::Python => extract_python_data_flow(source),
        Language::Go => extract_go_data_flow(source),
        Language::Rust | Language::Java | Language::CSharp | Language::Php | Language::Ruby | Language::Unknown => Ok(DataFlowInfo {
            assignments: vec![],
            calls_with_args: vec![],
        }),
    }
}

// ---------------------------------------------------------------------------
// Helper: node text
// ---------------------------------------------------------------------------

fn node_text<'a>(node: &Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

fn extract_string_content(node: &Node, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "string_fragment" {
            return Some(node_text(&child, source).to_string());
        }
    }
    // Fallback: strip quotes
    let text = node_text(node, source);
    if (text.starts_with('"') && text.ends_with('"'))
        || (text.starts_with('\'') && text.ends_with('\''))
    {
        Some(text[1..text.len() - 1].to_string())
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// TypeScript / JavaScript parsing
// ---------------------------------------------------------------------------

fn parse_typescript(path: &str, source: &str, lang: Language) -> Result<ParsedFile, AstError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
        .map_err(|e| AstError::LanguageError(e.to_string()))?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| AstError::ParseError("tree-sitter failed to parse".into()))?;

    let root = tree.root_node();
    let src = source.as_bytes();
    let mut definitions = Vec::new();
    let mut imports = Vec::new();
    let mut exports = Vec::new();
    let mut call_sites = Vec::new();

    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        match child.kind() {
            "import_statement" => {
                if let Some(imp) = extract_ts_import(&child, src) {
                    imports.push(imp);
                }
            }
            "export_statement" => {
                extract_ts_export(&child, src, &mut exports, &mut definitions);
            }
            "function_declaration" | "generator_function_declaration" => {
                if let Some(def) =
                    extract_definition_with_name(&child, src, SymbolKind::Function)
                {
                    definitions.push(def);
                }
            }
            "class_declaration" | "abstract_class_declaration" => {
                if let Some(def) =
                    extract_definition_with_name(&child, src, SymbolKind::Class)
                {
                    definitions.push(def);
                }
                extract_ts_methods(&child, src, &mut definitions);
            }
            "interface_declaration" => {
                if let Some(def) =
                    extract_definition_with_name(&child, src, SymbolKind::Interface)
                {
                    definitions.push(def);
                }
            }
            "type_alias_declaration" => {
                if let Some(def) =
                    extract_definition_with_name(&child, src, SymbolKind::TypeAlias)
                {
                    definitions.push(def);
                }
            }
            "lexical_declaration" | "variable_declaration" => {
                extract_ts_variable_defs(&child, src, &mut definitions);
            }
            _ => {}
        }
    }

    collect_call_sites(&root, src, &mut call_sites, &None, "call_expression");

    Ok(ParsedFile {
        path: path.to_string(),
        language: lang,
        definitions,
        imports,
        exports,
        call_sites,
    })
}

fn extract_ts_import(node: &Node, source: &[u8]) -> Option<ImportInfo> {
    // Get source module path — try `source` field first, then any string child
    let source_str = node
        .child_by_field_name("source")
        .or_else(|| {
            let mut c = node.walk();
            let found = node
                .named_children(&mut c)
                .find(|ch| ch.kind() == "string");
            found
        })
        .and_then(|s| extract_string_content(&s, source))?;

    let line = node.start_position().row + 1;
    let mut names = Vec::new();
    let mut is_default = false;
    let mut is_namespace = false;

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "import_clause" {
            let mut clause_cursor = child.walk();
            for clause_child in child.named_children(&mut clause_cursor) {
                match clause_child.kind() {
                    "identifier" => {
                        is_default = true;
                        names.push(ImportedName {
                            name: node_text(&clause_child, source).to_string(),
                            alias: None,
                        });
                    }
                    "named_imports" => {
                        let mut named_cursor = clause_child.walk();
                        for spec in clause_child.named_children(&mut named_cursor) {
                            if spec.kind() == "import_specifier" {
                                if let Some(name_node) = spec.child_by_field_name("name") {
                                    let alias = spec
                                        .child_by_field_name("alias")
                                        .map(|a| node_text(&a, source).to_string());
                                    names.push(ImportedName {
                                        name: node_text(&name_node, source).to_string(),
                                        alias,
                                    });
                                }
                            }
                        }
                    }
                    "namespace_import" => {
                        is_namespace = true;
                        let mut ns_cursor = clause_child.walk();
                        for ns_child in clause_child.named_children(&mut ns_cursor) {
                            if ns_child.kind() == "identifier" {
                                names.push(ImportedName {
                                    name: node_text(&ns_child, source).to_string(),
                                    alias: None,
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Some(ImportInfo {
        source: source_str,
        names,
        is_default,
        is_namespace,
        line,
    })
}

fn extract_ts_export(
    node: &Node,
    source: &[u8],
    exports: &mut Vec<ExportInfo>,
    definitions: &mut Vec<Definition>,
) {
    let line = node.start_position().row + 1;

    // Check for "default" keyword among all children (including anonymous)
    let is_default = {
        let mut c = node.walk();
        let result = node.children(&mut c).any(|ch| ch.kind() == "default");
        result
    };

    // Check for re-export source
    let reexport_source = node
        .child_by_field_name("source")
        .and_then(|s| extract_string_content(&s, source));
    let is_reexport = reexport_source.is_some();

    // export { a, b } or export { a } from 'mod' or export * from 'mod'
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "export_clause" {
            let mut spec_cursor = child.walk();
            for spec in child.named_children(&mut spec_cursor) {
                if spec.kind() == "export_specifier" {
                    if let Some(name_node) = spec.child_by_field_name("name") {
                        exports.push(ExportInfo {
                            name: node_text(&name_node, source).to_string(),
                            is_default: false,
                            is_reexport,
                            source: reexport_source.clone(),
                            line,
                        });
                    }
                }
            }
            return;
        }
        if child.kind() == "namespace_export" {
            exports.push(ExportInfo {
                name: "*".to_string(),
                is_default: false,
                is_reexport: true,
                source: reexport_source.clone(),
                line,
            });
            return;
        }
    }

    // Wildcard re-export without namespace_export node: export * from 'mod'
    {
        let mut c2 = node.walk();
        let has_star = node.children(&mut c2).any(|ch| ch.kind() == "*");
        if has_star && is_reexport {
            exports.push(ExportInfo {
                name: "*".to_string(),
                is_default: false,
                is_reexport: true,
                source: reexport_source.clone(),
                line,
            });
            return;
        }
    }

    // Exported declaration: export function foo, export class Bar, etc.
    if let Some(decl) = node.child_by_field_name("declaration") {
        match decl.kind() {
            "function_declaration" | "generator_function_declaration" => {
                if let Some(def) =
                    extract_definition_with_name(&decl, source, SymbolKind::Function)
                {
                    let name = def.name.clone();
                    definitions.push(def);
                    exports.push(ExportInfo {
                        name,
                        is_default,
                        is_reexport: false,
                        source: None,
                        line,
                    });
                }
            }
            "class_declaration" | "abstract_class_declaration" => {
                if let Some(def) =
                    extract_definition_with_name(&decl, source, SymbolKind::Class)
                {
                    let name = def.name.clone();
                    definitions.push(def);
                    extract_ts_methods(&decl, source, definitions);
                    exports.push(ExportInfo {
                        name,
                        is_default,
                        is_reexport: false,
                        source: None,
                        line,
                    });
                }
            }
            "interface_declaration" => {
                if let Some(def) =
                    extract_definition_with_name(&decl, source, SymbolKind::Interface)
                {
                    let name = def.name.clone();
                    definitions.push(def);
                    exports.push(ExportInfo {
                        name,
                        is_default,
                        is_reexport: false,
                        source: None,
                        line,
                    });
                }
            }
            "type_alias_declaration" => {
                if let Some(def) =
                    extract_definition_with_name(&decl, source, SymbolKind::TypeAlias)
                {
                    let name = def.name.clone();
                    definitions.push(def);
                    exports.push(ExportInfo {
                        name,
                        is_default,
                        is_reexport: false,
                        source: None,
                        line,
                    });
                }
            }
            "lexical_declaration" | "variable_declaration" => {
                let start_idx = definitions.len();
                extract_ts_variable_defs(&decl, source, definitions);
                for def in &definitions[start_idx..] {
                    exports.push(ExportInfo {
                        name: def.name.clone(),
                        is_default,
                        is_reexport: false,
                        source: None,
                        line,
                    });
                }
            }
            _ => {}
        }
        return;
    }

    // export default <expression>
    if is_default {
        if let Some(val) = node.child_by_field_name("value") {
            let name = if val.kind() == "identifier" {
                node_text(&val, source).to_string()
            } else {
                "default".to_string()
            };
            exports.push(ExportInfo {
                name,
                is_default: true,
                is_reexport: false,
                source: None,
                line,
            });
        }
    }
}

fn extract_definition_with_name(
    node: &Node,
    source: &[u8],
    kind: SymbolKind,
) -> Option<Definition> {
    let name_node = node.child_by_field_name("name")?;
    Some(Definition {
        name: node_text(&name_node, source).to_string(),
        kind,
        start_line: node.start_position().row + 1,
        end_line: node.end_position().row + 1,
    })
}

fn extract_ts_methods(class_node: &Node, source: &[u8], definitions: &mut Vec<Definition>) {
    let body = match class_node.child_by_field_name("body") {
        Some(b) => b,
        None => return,
    };
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        if child.kind() == "method_definition" {
            if let Some(def) =
                extract_definition_with_name(&child, source, SymbolKind::Function)
            {
                definitions.push(def);
            }
        }
    }
}

fn extract_ts_variable_defs(
    node: &Node,
    source: &[u8],
    definitions: &mut Vec<Definition>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            let name_node = match child.child_by_field_name("name") {
                Some(n) if n.kind() == "identifier" => n,
                _ => continue,
            };
            let value = child.child_by_field_name("value");
            let kind = match value.as_ref().map(|v| v.kind()) {
                Some("arrow_function") | Some("function") => SymbolKind::Function,
                _ => SymbolKind::Constant,
            };
            definitions.push(Definition {
                name: node_text(&name_node, source).to_string(),
                kind,
                start_line: child.start_position().row + 1,
                end_line: child.end_position().row + 1,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Python parsing
// ---------------------------------------------------------------------------

fn parse_python(path: &str, source: &str) -> Result<ParsedFile, AstError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .map_err(|e| AstError::LanguageError(e.to_string()))?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| AstError::ParseError("tree-sitter failed to parse".into()))?;

    let root = tree.root_node();
    let src = source.as_bytes();
    let mut definitions = Vec::new();
    let mut imports = Vec::new();
    let mut call_sites = Vec::new();

    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        match child.kind() {
            "import_statement" => {
                if let Some(imp) = extract_python_import(&child, src) {
                    imports.push(imp);
                }
            }
            "import_from_statement" => {
                if let Some(imp) = extract_python_import_from(&child, src) {
                    imports.push(imp);
                }
            }
            "function_definition" => {
                if let Some(def) =
                    extract_definition_with_name(&child, src, SymbolKind::Function)
                {
                    definitions.push(def);
                }
            }
            "class_definition" => {
                if let Some(def) =
                    extract_definition_with_name(&child, src, SymbolKind::Class)
                {
                    definitions.push(def);
                }
                extract_python_methods(&child, src, &mut definitions);
            }
            "decorated_definition" => {
                if let Some(inner) = child.child_by_field_name("definition") {
                    match inner.kind() {
                        "function_definition" => {
                            if let Some(def) = extract_definition_with_name(
                                &inner,
                                src,
                                SymbolKind::Function,
                            ) {
                                definitions.push(def);
                            }
                        }
                        "class_definition" => {
                            if let Some(def) = extract_definition_with_name(
                                &inner,
                                src,
                                SymbolKind::Class,
                            ) {
                                definitions.push(def);
                            }
                            extract_python_methods(&inner, src, &mut definitions);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    collect_call_sites(&root, src, &mut call_sites, &None, "call");

    Ok(ParsedFile {
        path: path.to_string(),
        language: Language::Python,
        definitions,
        imports,
        exports: vec![], // Python has no explicit export syntax
        call_sites,
    })
}

fn extract_python_import(node: &Node, source: &[u8]) -> Option<ImportInfo> {
    let line = node.start_position().row + 1;
    let mut names = Vec::new();

    let mut cursor = node.walk();
    for child in node.children_by_field_name("name", &mut cursor) {
        match child.kind() {
            "dotted_name" => {
                names.push(ImportedName {
                    name: node_text(&child, source).to_string(),
                    alias: None,
                });
            }
            "aliased_import" => {
                let name_node = child.child_by_field_name("name");
                let alias_node = child.child_by_field_name("alias");
                if let Some(n) = name_node {
                    names.push(ImportedName {
                        name: node_text(&n, source).to_string(),
                        alias: alias_node.map(|a| node_text(&a, source).to_string()),
                    });
                }
            }
            _ => {}
        }
    }

    if names.is_empty() {
        return None;
    }

    let source_str = names.first().map(|n| n.name.clone()).unwrap_or_default();

    Some(ImportInfo {
        source: source_str,
        names,
        is_default: false,
        is_namespace: true, // `import x` imports the whole module
        line,
    })
}

fn extract_python_import_from(node: &Node, source: &[u8]) -> Option<ImportInfo> {
    let line = node.start_position().row + 1;
    let module_node = node.child_by_field_name("module_name")?;
    let source_str = node_text(&module_node, source).to_string();

    let mut names = Vec::new();

    // Check for wildcard import
    let mut wc_cursor = node.walk();
    let has_wildcard = node
        .named_children(&mut wc_cursor)
        .any(|c| c.kind() == "wildcard_import");

    if has_wildcard {
        names.push(ImportedName {
            name: "*".to_string(),
            alias: None,
        });
    } else {
        let mut name_cursor = node.walk();
        for child in node.children_by_field_name("name", &mut name_cursor) {
            match child.kind() {
                "dotted_name" => {
                    names.push(ImportedName {
                        name: node_text(&child, source).to_string(),
                        alias: None,
                    });
                }
                "aliased_import" => {
                    let name_n = child.child_by_field_name("name");
                    let alias_n = child.child_by_field_name("alias");
                    if let Some(n) = name_n {
                        names.push(ImportedName {
                            name: node_text(&n, source).to_string(),
                            alias: alias_n.map(|a| node_text(&a, source).to_string()),
                        });
                    }
                }
                _ => {}
            }
        }
    }

    Some(ImportInfo {
        source: source_str,
        names,
        is_default: false,
        is_namespace: false,
        line,
    })
}

fn extract_python_methods(
    class_node: &Node,
    source: &[u8],
    definitions: &mut Vec<Definition>,
) {
    let body = match class_node.child_by_field_name("body") {
        Some(b) => b,
        None => return,
    };
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        match child.kind() {
            "function_definition" => {
                if let Some(def) =
                    extract_definition_with_name(&child, source, SymbolKind::Function)
                {
                    definitions.push(def);
                }
            }
            "decorated_definition" => {
                if let Some(inner) = child.child_by_field_name("definition") {
                    if inner.kind() == "function_definition" {
                        if let Some(def) =
                            extract_definition_with_name(&inner, source, SymbolKind::Function)
                        {
                            definitions.push(def);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn find_class_bases(node: &Node, source: &[u8], target_class: &str, bases: &mut Vec<String>) {
    match node.kind() {
        "class_definition" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                if node_text(&name_node, source) == target_class {
                    if let Some(supers) = node.child_by_field_name("superclasses") {
                        let mut cursor = supers.walk();
                        for child in supers.named_children(&mut cursor) {
                            match child.kind() {
                                "identifier" | "attribute" => {
                                    bases.push(node_text(&child, source).to_string());
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
        "decorated_definition" => {
            if let Some(inner) = node.child_by_field_name("definition") {
                find_class_bases(&inner, source, target_class, bases);
            }
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                find_class_bases(&child, source, target_class, bases);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Common: call site extraction (recursive tree walk)
// ---------------------------------------------------------------------------

fn collect_call_sites(
    node: &Node,
    source: &[u8],
    calls: &mut Vec<CallSite>,
    containing: &Option<String>,
    call_kind: &str,
) {
    // Update containing function context
    let new_containing = match node.kind() {
        "function_declaration"
        | "generator_function_declaration"
        | "function_definition"
        | "method_definition"
        | "method_declaration" => node
            .child_by_field_name("name")
            .map(|n| node_text(&n, source).to_string()),
        "variable_declarator" => {
            let is_fn = node
                .child_by_field_name("value")
                .map(|v| v.kind() == "arrow_function" || v.kind() == "function")
                .unwrap_or(false);
            if is_fn {
                node.child_by_field_name("name")
                    .filter(|n| n.kind() == "identifier")
                    .map(|n| node_text(&n, source).to_string())
            } else {
                None
            }
        }
        _ => None,
    };

    let effective = if new_containing.is_some() {
        &new_containing
    } else {
        containing
    };

    if node.kind() == call_kind {
        if let Some(callee) = extract_callee(node, source) {
            calls.push(CallSite {
                callee,
                line: node.start_position().row + 1,
                containing_function: effective.clone(),
            });
        }
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_call_sites(&child, source, calls, effective, call_kind);
    }
}

fn extract_callee(node: &Node, source: &[u8]) -> Option<String> {
    let func = node.child_by_field_name("function")?;
    Some(node_text(&func, source).to_string())
}

// ---------------------------------------------------------------------------
// Data flow extraction: TypeScript / JavaScript
// ---------------------------------------------------------------------------

fn extract_ts_data_flow(source: &str) -> Result<DataFlowInfo, AstError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
        .map_err(|e| AstError::LanguageError(e.to_string()))?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| AstError::ParseError("tree-sitter failed to parse".into()))?;

    let root = tree.root_node();
    let src = source.as_bytes();
    let mut assignments = Vec::new();
    let mut calls_with_args = Vec::new();

    collect_ts_data_flow(&root, src, &mut assignments, &mut calls_with_args, &None);

    Ok(DataFlowInfo {
        assignments,
        calls_with_args,
    })
}

fn collect_ts_data_flow(
    node: &Node,
    source: &[u8],
    assignments: &mut Vec<VarCallAssignment>,
    calls: &mut Vec<CallWithArgs>,
    containing: &Option<String>,
) {
    // Update containing function context (same logic as collect_call_sites).
    let new_containing = match node.kind() {
        "function_declaration"
        | "generator_function_declaration"
        | "function_definition"
        | "method_definition"
        | "method_declaration" => node
            .child_by_field_name("name")
            .map(|n| node_text(&n, source).to_string()),
        "variable_declarator" => {
            let is_fn = node
                .child_by_field_name("value")
                .map(|v| v.kind() == "arrow_function" || v.kind() == "function")
                .unwrap_or(false);
            if is_fn {
                node.child_by_field_name("name")
                    .filter(|n| n.kind() == "identifier")
                    .map(|n| node_text(&n, source).to_string())
            } else {
                None
            }
        }
        _ => None,
    };

    let effective = if new_containing.is_some() {
        &new_containing
    } else {
        containing
    };

    // Detect variable assignment from a call: `const x = funcA()` or `const x = await funcA()`
    if node.kind() == "variable_declarator" {
        if let (Some(name_node), Some(value_node)) = (
            node.child_by_field_name("name"),
            node.child_by_field_name("value"),
        ) {
            if name_node.kind() == "identifier" {
                if let Some(callee) = extract_call_from_value(&value_node, source) {
                    assignments.push(VarCallAssignment {
                        variable: node_text(&name_node, source).to_string(),
                        callee,
                        line: node.start_position().row + 1,
                        containing_function: effective.clone(),
                    });
                }
            }
        }
    }

    // Detect call expression with arguments.
    if node.kind() == "call_expression" {
        if let Some(callee) = extract_callee(node, source) {
            let arguments = extract_argument_texts(node, source);
            calls.push(CallWithArgs {
                callee,
                arguments,
                line: node.start_position().row + 1,
                containing_function: effective.clone(),
            });
        }
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_ts_data_flow(&child, source, assignments, calls, effective);
    }
}

/// Extract the callee from a value that might be a call or an await wrapping a call.
fn extract_call_from_value(node: &Node, source: &[u8]) -> Option<String> {
    match node.kind() {
        "call_expression" => extract_callee(node, source),
        "await_expression" => {
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                if child.kind() == "call_expression" {
                    return extract_callee(&child, source);
                }
            }
            None
        }
        _ => None,
    }
}

/// Extract the text of each argument in a call expression's argument list.
fn extract_argument_texts(call_node: &Node, source: &[u8]) -> Vec<String> {
    let args_node = match call_node.child_by_field_name("arguments") {
        Some(n) => n,
        None => return vec![],
    };

    let mut args = Vec::new();
    let mut cursor = args_node.walk();
    for child in args_node.named_children(&mut cursor) {
        let text = node_text(&child, source).to_string();
        if !text.is_empty() {
            args.push(text);
        }
    }
    args
}

// ---------------------------------------------------------------------------
// Data flow extraction: Python
// ---------------------------------------------------------------------------

fn extract_python_data_flow(source: &str) -> Result<DataFlowInfo, AstError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .map_err(|e| AstError::LanguageError(e.to_string()))?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| AstError::ParseError("tree-sitter failed to parse".into()))?;

    let root = tree.root_node();
    let src = source.as_bytes();
    let mut assignments = Vec::new();
    let mut calls_with_args = Vec::new();

    collect_python_data_flow(&root, src, &mut assignments, &mut calls_with_args, &None);

    Ok(DataFlowInfo {
        assignments,
        calls_with_args,
    })
}

fn collect_python_data_flow(
    node: &Node,
    source: &[u8],
    assignments: &mut Vec<VarCallAssignment>,
    calls: &mut Vec<CallWithArgs>,
    containing: &Option<String>,
) {
    // Update containing function context.
    let new_containing = match node.kind() {
        "function_definition" => node
            .child_by_field_name("name")
            .map(|n| node_text(&n, source).to_string()),
        _ => None,
    };

    let effective = if new_containing.is_some() {
        &new_containing
    } else {
        containing
    };

    // Detect assignment from a call: `x = func_a()` or `x = await func_a()`
    if node.kind() == "assignment" {
        if let (Some(left), Some(right)) = (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ) {
            if left.kind() == "identifier" {
                let callee = match right.kind() {
                    "call" => extract_callee(&right, source),
                    "await" => {
                        let mut c = right.walk();
                        let call_node = right
                            .named_children(&mut c)
                            .find(|ch| ch.kind() == "call");
                        call_node.and_then(|call| extract_callee(&call, source))
                    }
                    _ => None,
                };
                if let Some(callee) = callee {
                    assignments.push(VarCallAssignment {
                        variable: node_text(&left, source).to_string(),
                        callee,
                        line: node.start_position().row + 1,
                        containing_function: effective.clone(),
                    });
                }
            }
        }
    }

    // Detect call with arguments.
    if node.kind() == "call" {
        if let Some(callee) = extract_callee(node, source) {
            let arguments = extract_python_argument_texts(node, source);
            calls.push(CallWithArgs {
                callee,
                arguments,
                line: node.start_position().row + 1,
                containing_function: effective.clone(),
            });
        }
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_python_data_flow(&child, source, assignments, calls, effective);
    }
}

/// Extract argument texts from a Python call's argument_list.
fn extract_python_argument_texts(call_node: &Node, source: &[u8]) -> Vec<String> {
    let args_node = match call_node.child_by_field_name("arguments") {
        Some(n) => n,
        None => return vec![],
    };

    let mut args = Vec::new();
    let mut cursor = args_node.walk();
    for child in args_node.named_children(&mut cursor) {
        // Skip keyword argument names (only get the value)
        if child.kind() == "keyword_argument" {
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

// ---------------------------------------------------------------------------
// Go parsing
// ---------------------------------------------------------------------------

fn parse_go(path: &str, source: &str) -> Result<ParsedFile, AstError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .map_err(|e| AstError::LanguageError(e.to_string()))?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| AstError::ParseError("tree-sitter failed to parse".into()))?;

    let root = tree.root_node();
    let src = source.as_bytes();
    let mut definitions = Vec::new();
    let mut imports = Vec::new();
    let mut call_sites = Vec::new();
    let mut exports = Vec::new();

    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        match child.kind() {
            "function_declaration" => {
                if let Some(def) =
                    extract_definition_with_name(&child, src, SymbolKind::Function)
                {
                    if def.name.chars().next().map_or(false, |c| c.is_uppercase()) {
                        exports.push(ExportInfo {
                            name: def.name.clone(),
                            is_default: false,
                            is_reexport: false,
                            source: None,
                            line: def.start_line,
                        });
                    }
                    definitions.push(def);
                }
            }
            "method_declaration" => {
                if let Some(name_node) = child.child_by_field_name("name") {
                    let name = node_text(&name_node, src).to_string();
                    let def = Definition {
                        name: name.clone(),
                        kind: SymbolKind::Function,
                        start_line: child.start_position().row + 1,
                        end_line: child.end_position().row + 1,
                    };
                    if name.chars().next().map_or(false, |c| c.is_uppercase()) {
                        exports.push(ExportInfo {
                            name: name.clone(),
                            is_default: false,
                            is_reexport: false,
                            source: None,
                            line: def.start_line,
                        });
                    }
                    definitions.push(def);
                }
            }
            "type_declaration" => {
                extract_go_type_defs(&child, src, &mut definitions, &mut exports);
            }
            "const_declaration" => {
                extract_go_const_defs(&child, src, &mut definitions, &mut exports);
            }
            "var_declaration" => {
                extract_go_var_defs(&child, src, &mut definitions);
            }
            "import_declaration" => {
                extract_go_imports(&child, src, &mut imports);
            }
            _ => {}
        }
    }

    collect_call_sites(&root, src, &mut call_sites, &None, "call_expression");

    Ok(ParsedFile {
        path: path.to_string(),
        language: Language::Go,
        definitions,
        imports,
        exports,
        call_sites,
    })
}

fn extract_go_imports(node: &Node, source: &[u8], imports: &mut Vec<ImportInfo>) {
    let line = node.start_position().row + 1;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "import_spec" => {
                if let Some(imp) = extract_single_go_import(&child, source, line) {
                    imports.push(imp);
                }
            }
            "import_spec_list" => {
                let mut list_cursor = child.walk();
                for spec in child.named_children(&mut list_cursor) {
                    if spec.kind() == "import_spec" {
                        let spec_line = spec.start_position().row + 1;
                        if let Some(imp) = extract_single_go_import(&spec, source, spec_line) {
                            imports.push(imp);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn extract_single_go_import(node: &Node, source: &[u8], line: usize) -> Option<ImportInfo> {
    let path_node = node.child_by_field_name("path")?;
    let raw = node_text(&path_node, source);
    // Strip quotes from Go string literal: "fmt" → fmt
    let source_str = raw.trim_matches('"').to_string();

    let alias_node = node.child_by_field_name("name");
    let mut names = Vec::new();
    let mut is_namespace = false;

    match alias_node.map(|n| n.kind()) {
        Some("package_identifier") => {
            // Aliased import: import alias "path"
            let alias_text = node_text(&alias_node.unwrap(), source).to_string();
            names.push(ImportedName {
                name: go_package_name(&source_str),
                alias: Some(alias_text),
            });
            is_namespace = true;
        }
        Some("dot") => {
            // Dot import: import . "path" — imports all exported names
            names.push(ImportedName {
                name: "*".to_string(),
                alias: None,
            });
        }
        Some("blank_identifier") => {
            // Blank import: import _ "path" — side-effect only
            // No names imported
        }
        _ => {
            // Simple import: import "path" — imports the package name
            let pkg = go_package_name(&source_str);
            names.push(ImportedName {
                name: pkg,
                alias: None,
            });
            is_namespace = true;
        }
    }

    Some(ImportInfo {
        source: source_str,
        names,
        is_default: false,
        is_namespace,
        line,
    })
}

/// Extract Go package name from import path (last path segment).
/// e.g., "net/http" → "http", "github.com/gin-gonic/gin" → "gin"
fn go_package_name(import_path: &str) -> String {
    import_path
        .rsplit('/')
        .next()
        .unwrap_or(import_path)
        .to_string()
}

fn extract_go_type_defs(
    node: &Node,
    source: &[u8],
    definitions: &mut Vec<Definition>,
    exports: &mut Vec<ExportInfo>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "type_spec" {
            if let Some(name_node) = child.child_by_field_name("name") {
                let name = node_text(&name_node, source).to_string();
                let type_node = child.child_by_field_name("type");
                let kind = match type_node.as_ref().map(|n| n.kind()) {
                    Some("interface_type") => SymbolKind::Interface,
                    Some("struct_type") => SymbolKind::Class, // Use Class for structs
                    _ => SymbolKind::TypeAlias,
                };
                let def = Definition {
                    name: name.clone(),
                    kind,
                    start_line: child.start_position().row + 1,
                    end_line: child.end_position().row + 1,
                };
                if name.chars().next().map_or(false, |c| c.is_uppercase()) {
                    exports.push(ExportInfo {
                        name: name.clone(),
                        is_default: false,
                        is_reexport: false,
                        source: None,
                        line: def.start_line,
                    });
                }
                definitions.push(def);
            }
        }
        // type_alias is handled by same pattern (type_spec fallthrough above)
        if child.kind() == "type_alias" {
            if let Some(name_node) = child.child_by_field_name("name") {
                let name = node_text(&name_node, source).to_string();
                let def = Definition {
                    name: name.clone(),
                    kind: SymbolKind::TypeAlias,
                    start_line: child.start_position().row + 1,
                    end_line: child.end_position().row + 1,
                };
                if name.chars().next().map_or(false, |c| c.is_uppercase()) {
                    exports.push(ExportInfo {
                        name: name.clone(),
                        is_default: false,
                        is_reexport: false,
                        source: None,
                        line: def.start_line,
                    });
                }
                definitions.push(def);
            }
        }
    }
}

fn extract_go_const_defs(
    node: &Node,
    source: &[u8],
    definitions: &mut Vec<Definition>,
    exports: &mut Vec<ExportInfo>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "const_spec" {
            // const_spec can have multiple names: const A, B = 1, 2
            let mut name_cursor = child.walk();
            for name_child in child.children_by_field_name("name", &mut name_cursor) {
                if name_child.kind() == "identifier" {
                    let name = node_text(&name_child, source).to_string();
                    let def = Definition {
                        name: name.clone(),
                        kind: SymbolKind::Constant,
                        start_line: child.start_position().row + 1,
                        end_line: child.end_position().row + 1,
                    };
                    if name.chars().next().map_or(false, |c| c.is_uppercase()) {
                        exports.push(ExportInfo {
                            name: name.clone(),
                            is_default: false,
                            is_reexport: false,
                            source: None,
                            line: def.start_line,
                        });
                    }
                    definitions.push(def);
                }
            }
        }
    }
}

fn extract_go_var_defs(
    node: &Node,
    source: &[u8],
    definitions: &mut Vec<Definition>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "var_spec" => {
                let mut name_cursor = child.walk();
                for name_child in child.children_by_field_name("name", &mut name_cursor) {
                    if name_child.kind() == "identifier" {
                        definitions.push(Definition {
                            name: node_text(&name_child, source).to_string(),
                            kind: SymbolKind::Constant, // Use Constant for package-level vars
                            start_line: child.start_position().row + 1,
                            end_line: child.end_position().row + 1,
                        });
                    }
                }
            }
            "var_spec_list" => {
                let mut list_cursor = child.walk();
                for spec in child.named_children(&mut list_cursor) {
                    if spec.kind() == "var_spec" {
                        let mut name_cursor = spec.walk();
                        for name_child in spec.children_by_field_name("name", &mut name_cursor) {
                            if name_child.kind() == "identifier" {
                                definitions.push(Definition {
                                    name: node_text(&name_child, source).to_string(),
                                    kind: SymbolKind::Constant,
                                    start_line: spec.start_position().row + 1,
                                    end_line: spec.end_position().row + 1,
                                });
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Data flow extraction: Go
// ---------------------------------------------------------------------------

fn extract_go_data_flow(source: &str) -> Result<DataFlowInfo, AstError> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_go::LANGUAGE.into())
        .map_err(|e| AstError::LanguageError(e.to_string()))?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| AstError::ParseError("tree-sitter failed to parse".into()))?;

    let root = tree.root_node();
    let src = source.as_bytes();
    let mut assignments = Vec::new();
    let mut calls_with_args = Vec::new();

    collect_go_data_flow(&root, src, &mut assignments, &mut calls_with_args, &None);

    Ok(DataFlowInfo {
        assignments,
        calls_with_args,
    })
}

fn collect_go_data_flow(
    node: &Node,
    source: &[u8],
    assignments: &mut Vec<VarCallAssignment>,
    calls: &mut Vec<CallWithArgs>,
    containing: &Option<String>,
) {
    // Update containing function context.
    let new_containing = match node.kind() {
        "function_declaration" => node
            .child_by_field_name("name")
            .map(|n| node_text(&n, source).to_string()),
        "method_declaration" => node
            .child_by_field_name("name")
            .map(|n| node_text(&n, source).to_string()),
        _ => None,
    };

    let effective = if new_containing.is_some() {
        &new_containing
    } else {
        containing
    };

    // Detect short variable declaration from call: `x := foo()`
    if node.kind() == "short_var_declaration" {
        if let (Some(left), Some(right)) = (
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ) {
            // Get the first identifier in the expression_list
            if left.kind() == "expression_list" {
                let first_id = left.named_child(0);
                if let Some(first_id) = first_id {
                    if first_id.kind() == "identifier" {
                        // Check if right side is a call
                        if right.kind() == "expression_list" {
                            let first_val = right.named_child(0);
                            if let Some(first_val) = first_val {
                                if first_val.kind() == "call_expression" {
                                    if let Some(callee) =
                                        extract_callee(&first_val, source)
                                    {
                                        assignments.push(VarCallAssignment {
                                            variable: node_text(&first_id, source)
                                                .to_string(),
                                            callee,
                                            line: node.start_position().row + 1,
                                            containing_function: effective.clone(),
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Detect call expressions with arguments
    if node.kind() == "call_expression" {
        if let Some(callee) = extract_callee(node, source) {
            let arguments = extract_go_argument_texts(node, source);
            calls.push(CallWithArgs {
                callee,
                arguments,
                line: node.start_position().row + 1,
                containing_function: effective.clone(),
            });
        }
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_go_data_flow(&child, source, assignments, calls, effective);
    }
}

/// Extract argument texts from a Go call's argument_list.
fn extract_go_argument_texts(call_node: &Node, source: &[u8]) -> Vec<String> {
    let args_node = match call_node.child_by_field_name("arguments") {
        Some(n) => n,
        None => return vec![],
    };

    let mut args = Vec::new();
    let mut cursor = args_node.walk();
    for child in args_node.named_children(&mut cursor) {
        let text = node_text(&child, source).to_string();
        if !text.is_empty() {
            args.push(text);
        }
    }
    args
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod tests {
    use super::*;

    // === TypeScript imports ===

    #[test]
    fn test_parse_ts_imports() {
        let source = r#"
import React from 'react';
import { useState, useEffect } from 'react';
import * as path from 'path';
import { foo as bar } from './utils';
"#;
        let result = parse_file("app.ts", source).unwrap();
        assert_eq!(result.imports.len(), 4);

        // Default import
        assert_eq!(result.imports[0].source, "react");
        assert!(result.imports[0].is_default);
        assert!(!result.imports[0].is_namespace);
        assert_eq!(result.imports[0].names.len(), 1);
        assert_eq!(result.imports[0].names[0].name, "React");

        // Named imports
        assert_eq!(result.imports[1].source, "react");
        assert!(!result.imports[1].is_default);
        assert_eq!(result.imports[1].names.len(), 2);
        assert_eq!(result.imports[1].names[0].name, "useState");
        assert_eq!(result.imports[1].names[1].name, "useEffect");

        // Namespace import
        assert_eq!(result.imports[2].source, "path");
        assert!(result.imports[2].is_namespace);
        assert_eq!(result.imports[2].names[0].name, "path");

        // Aliased import
        assert_eq!(result.imports[3].source, "./utils");
        assert_eq!(result.imports[3].names[0].name, "foo");
        assert_eq!(
            result.imports[3].names[0].alias,
            Some("bar".to_string())
        );
    }

    #[test]
    fn test_parse_ts_default_and_named_import() {
        let source = r#"import React, { useState } from 'react';"#;
        let result = parse_file("app.ts", source).unwrap();
        assert_eq!(result.imports.len(), 1);
        let imp = &result.imports[0];
        assert!(imp.is_default);
        assert_eq!(imp.names.len(), 2);
        assert_eq!(imp.names[0].name, "React");
        assert_eq!(imp.names[1].name, "useState");
    }

    #[test]
    fn test_parse_ts_side_effect_import() {
        let source = r#"import './polyfill';"#;
        let result = parse_file("app.ts", source).unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "./polyfill");
        assert!(result.imports[0].names.is_empty());
    }

    // === TypeScript exports ===

    #[test]
    fn test_parse_ts_exports() {
        let source = r#"
export function greet() {}
export default function main() {}
export { foo, bar };
export { baz } from './other';
export const VALUE = 42;
"#;
        let result = parse_file("lib.ts", source).unwrap();

        // export function greet
        let greet_export = result.exports.iter().find(|e| e.name == "greet").unwrap();
        assert!(!greet_export.is_default);
        assert!(!greet_export.is_reexport);

        // export default function main
        let main_export = result.exports.iter().find(|e| e.name == "main").unwrap();
        assert!(main_export.is_default);

        // export { foo, bar }
        let foo_export = result.exports.iter().find(|e| e.name == "foo").unwrap();
        assert!(!foo_export.is_default);
        assert!(!foo_export.is_reexport);

        let bar_export = result.exports.iter().find(|e| e.name == "bar").unwrap();
        assert!(!bar_export.is_reexport);

        // export { baz } from './other'
        let baz_export = result.exports.iter().find(|e| e.name == "baz").unwrap();
        assert!(baz_export.is_reexport);
        assert_eq!(baz_export.source, Some("./other".to_string()));

        // export const VALUE
        let val_export = result.exports.iter().find(|e| e.name == "VALUE").unwrap();
        assert!(!val_export.is_default);
    }

    #[test]
    fn test_parse_ts_wildcard_reexport() {
        let source = r#"export * from './all';"#;
        let result = parse_file("index.ts", source).unwrap();
        assert_eq!(result.exports.len(), 1);
        assert_eq!(result.exports[0].name, "*");
        assert!(result.exports[0].is_reexport);
        assert_eq!(result.exports[0].source, Some("./all".to_string()));
    }

    #[test]
    fn test_parse_ts_export_default_expression() {
        let source = r#"
const app = createApp();
export default app;
"#;
        let result = parse_file("app.ts", source).unwrap();
        let default_export = result.exports.iter().find(|e| e.is_default).unwrap();
        assert_eq!(default_export.name, "app");
    }

    // === TypeScript definitions ===

    #[test]
    fn test_parse_ts_functions() {
        let source = r#"
function greet(name: string): string {
    return `Hello ${name}`;
}

const double = (x: number) => x * 2;

class Calculator {
    add(a: number, b: number): number {
        return a + b;
    }
    subtract(a: number, b: number): number {
        return a - b;
    }
}
"#;
        let result = parse_file("math.ts", source).unwrap();

        // function declaration
        let greet = result
            .definitions
            .iter()
            .find(|d| d.name == "greet")
            .unwrap();
        assert_eq!(greet.kind, SymbolKind::Function);

        // arrow function
        let double = result
            .definitions
            .iter()
            .find(|d| d.name == "double")
            .unwrap();
        assert_eq!(double.kind, SymbolKind::Function);

        // class
        let calc = result
            .definitions
            .iter()
            .find(|d| d.name == "Calculator")
            .unwrap();
        assert_eq!(calc.kind, SymbolKind::Class);

        // methods
        let add = result
            .definitions
            .iter()
            .find(|d| d.name == "add")
            .unwrap();
        assert_eq!(add.kind, SymbolKind::Function);

        let sub = result
            .definitions
            .iter()
            .find(|d| d.name == "subtract")
            .unwrap();
        assert_eq!(sub.kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_ts_interface_and_type() {
        let source = r#"
interface User {
    name: string;
    age: number;
}

type UserId = string;
"#;
        let result = parse_file("types.ts", source).unwrap();

        let user_iface = result
            .definitions
            .iter()
            .find(|d| d.name == "User")
            .unwrap();
        assert_eq!(user_iface.kind, SymbolKind::Interface);

        let user_id = result
            .definitions
            .iter()
            .find(|d| d.name == "UserId")
            .unwrap();
        assert_eq!(user_id.kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn test_parse_ts_constants() {
        let source = r#"
const MAX_RETRIES = 3;
const API_URL = "https://example.com";
"#;
        let result = parse_file("config.ts", source).unwrap();
        assert_eq!(result.definitions.len(), 2);

        let max = result
            .definitions
            .iter()
            .find(|d| d.name == "MAX_RETRIES")
            .unwrap();
        assert_eq!(max.kind, SymbolKind::Constant);
    }

    // === TypeScript call sites ===

    #[test]
    fn test_parse_ts_call_sites() {
        let source = r#"
function processUser(user: User) {
    const validated = validateUser(user);
    const saved = db.save(validated);
    notifyAdmin(saved.id);
}
"#;
        let result = parse_file("handler.ts", source).unwrap();

        let call_names: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(call_names.contains(&"validateUser"));
        assert!(call_names.contains(&"db.save"));
        assert!(call_names.contains(&"notifyAdmin"));

        // All calls should be inside processUser
        for call in &result.call_sites {
            assert_eq!(
                call.containing_function,
                Some("processUser".to_string())
            );
        }
    }

    #[test]
    fn test_parse_ts_call_sites_in_arrow() {
        let source = r#"
const handler = (req: Request) => {
    const data = parseBody(req);
    return respond(data);
};
"#;
        let result = parse_file("handler.ts", source).unwrap();
        let call_names: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(call_names.contains(&"parseBody"));
        assert!(call_names.contains(&"respond"));

        for call in &result.call_sites {
            assert_eq!(
                call.containing_function,
                Some("handler".to_string())
            );
        }
    }

    // === Python imports ===

    #[test]
    fn test_parse_python_imports() {
        let source = r#"
import os
import json as j
from pathlib import Path
from typing import List, Optional
from . import utils
from ..models import User as U
"#;
        let result = parse_file("app.py", source).unwrap();
        assert_eq!(result.imports.len(), 6);

        // import os
        assert_eq!(result.imports[0].source, "os");
        assert!(result.imports[0].is_namespace);
        assert_eq!(result.imports[0].names[0].name, "os");

        // import json as j
        assert_eq!(result.imports[1].source, "json");
        assert_eq!(result.imports[1].names[0].name, "json");
        assert_eq!(result.imports[1].names[0].alias, Some("j".to_string()));

        // from pathlib import Path
        assert_eq!(result.imports[2].source, "pathlib");
        assert!(!result.imports[2].is_namespace);
        assert_eq!(result.imports[2].names[0].name, "Path");

        // from typing import List, Optional
        assert_eq!(result.imports[3].source, "typing");
        assert_eq!(result.imports[3].names.len(), 2);
        assert_eq!(result.imports[3].names[0].name, "List");
        assert_eq!(result.imports[3].names[1].name, "Optional");

        // from . import utils (relative import)
        assert_eq!(result.imports[4].source, ".");
        assert_eq!(result.imports[4].names[0].name, "utils");

        // from ..models import User as U
        assert!(result.imports[5].source.contains("models"));
        assert_eq!(result.imports[5].names[0].name, "User");
        assert_eq!(result.imports[5].names[0].alias, Some("U".to_string()));
    }

    // === Python definitions ===

    #[test]
    fn test_parse_python_functions() {
        let source = r#"
def greet(name: str) -> str:
    return f"Hello {name}"

class UserService:
    def create_user(self, data: dict) -> User:
        return User(**data)

    def delete_user(self, user_id: int) -> None:
        pass
"#;
        let result = parse_file("service.py", source).unwrap();

        // Top-level function
        let greet = result
            .definitions
            .iter()
            .find(|d| d.name == "greet")
            .unwrap();
        assert_eq!(greet.kind, SymbolKind::Function);

        // Class
        let svc = result
            .definitions
            .iter()
            .find(|d| d.name == "UserService")
            .unwrap();
        assert_eq!(svc.kind, SymbolKind::Class);

        // Methods
        let create = result
            .definitions
            .iter()
            .find(|d| d.name == "create_user")
            .unwrap();
        assert_eq!(create.kind, SymbolKind::Function);

        let delete = result
            .definitions
            .iter()
            .find(|d| d.name == "delete_user")
            .unwrap();
        assert_eq!(delete.kind, SymbolKind::Function);
    }

    #[test]
    fn test_parse_python_decorated_functions() {
        let source = r#"
from flask import Flask
app = Flask(__name__)

@app.route('/users', methods=['GET'])
def list_users():
    return get_all_users()

@staticmethod
def helper():
    pass
"#;
        let result = parse_file("routes.py", source).unwrap();

        let list_users = result
            .definitions
            .iter()
            .find(|d| d.name == "list_users")
            .unwrap();
        assert_eq!(list_users.kind, SymbolKind::Function);

        let helper = result
            .definitions
            .iter()
            .find(|d| d.name == "helper")
            .unwrap();
        assert_eq!(helper.kind, SymbolKind::Function);
    }

    // === Python class hierarchy ===

    #[test]
    fn test_parse_python_class_hierarchy() {
        let source = r#"
class Animal:
    pass

class Dog(Animal):
    def bark(self):
        pass

class GuideDog(Dog, ServiceAnimal):
    pass
"#;
        // Verify class definitions are detected
        let result = parse_file("models.py", source).unwrap();
        let classes: Vec<&str> = result
            .definitions
            .iter()
            .filter(|d| d.kind == SymbolKind::Class)
            .map(|d| d.name.as_str())
            .collect();
        assert!(classes.contains(&"Animal"));
        assert!(classes.contains(&"Dog"));
        assert!(classes.contains(&"GuideDog"));

        // Verify base class extraction
        let animal_bases = get_python_class_bases(source, "Animal").unwrap();
        assert!(animal_bases.is_empty());

        let dog_bases = get_python_class_bases(source, "Dog").unwrap();
        assert_eq!(dog_bases, vec!["Animal"]);

        let guide_bases = get_python_class_bases(source, "GuideDog").unwrap();
        assert_eq!(guide_bases, vec!["Dog", "ServiceAnimal"]);
    }

    // === Unknown language ===

    #[test]
    fn test_parse_unknown_language() {
        let source = "some random content that is not code";
        let result = parse_file("main.xyz", source).unwrap();
        assert_eq!(result.language, Language::Unknown);
        assert!(result.definitions.is_empty());
        assert!(result.imports.is_empty());
        assert!(result.exports.is_empty());
        assert!(result.call_sites.is_empty());
    }

    // === Changed symbols detection ===

    #[test]
    fn test_changed_symbols_detection() {
        let old_source = r#"
function foo() {}
function bar() {}
const VALUE = 42;
"#;
        let new_source = r#"
function foo() {
    return 1;
}
function baz() {}
const VALUE = 42;
"#;
        let old = parse_file("lib.ts", old_source).unwrap();
        let new = parse_file("lib.ts", new_source).unwrap();
        let changes = detect_changed_symbols(&old, &new);

        let added: Vec<&str> = changes
            .iter()
            .filter_map(|c| match c {
                SymbolChange::Added(d) => Some(d.name.as_str()),
                _ => None,
            })
            .collect();
        assert!(added.contains(&"baz"), "baz should be added");

        let removed: Vec<&str> = changes
            .iter()
            .filter_map(|c| match c {
                SymbolChange::Removed(d) => Some(d.name.as_str()),
                _ => None,
            })
            .collect();
        assert!(removed.contains(&"bar"), "bar should be removed");

        let modified: Vec<&str> = changes
            .iter()
            .filter_map(|c| match c {
                SymbolChange::Modified { old, .. } => Some(old.name.as_str()),
                _ => None,
            })
            .collect();
        assert!(modified.contains(&"foo"), "foo should be modified");

        // VALUE unchanged
        assert!(
            !changes
                .iter()
                .any(|c| match c {
                    SymbolChange::Added(d) | SymbolChange::Removed(d) => d.name == "VALUE",
                    SymbolChange::Modified { old, .. } => old.name == "VALUE",
                }),
            "VALUE should be unchanged"
        );
    }

    #[test]
    fn test_changed_symbols_no_changes() {
        let source = "function foo() {}\n";
        let old = parse_file("lib.ts", source).unwrap();
        let new = parse_file("lib.ts", source).unwrap();
        let changes = detect_changed_symbols(&old, &new);
        assert!(changes.is_empty());
    }

    // === Language detection ===

    #[test]
    fn test_language_from_path() {
        assert_eq!(Language::from_path("app.ts"), Language::TypeScript);
        assert_eq!(Language::from_path("app.tsx"), Language::TypeScript);
        assert_eq!(Language::from_path("app.js"), Language::JavaScript);
        assert_eq!(Language::from_path("app.jsx"), Language::JavaScript);
        assert_eq!(Language::from_path("app.mjs"), Language::JavaScript);
        assert_eq!(Language::from_path("app.cjs"), Language::JavaScript);
        assert_eq!(Language::from_path("app.py"), Language::Python);
        assert_eq!(Language::from_path("app.pyi"), Language::Python);
        assert_eq!(Language::from_path("app.go"), Language::Go);
        assert_eq!(Language::from_path("app.rs"), Language::Rust);
        assert_eq!(Language::from_path("Makefile"), Language::Unknown);
    }

    // === Line numbers ===

    #[test]
    fn test_definition_line_numbers() {
        let source = "function foo() {\n  return 1;\n}\n\nfunction bar() {\n  return 2;\n}\n";
        let result = parse_file("lib.ts", source).unwrap();

        let foo = result
            .definitions
            .iter()
            .find(|d| d.name == "foo")
            .unwrap();
        assert_eq!(foo.start_line, 1);
        assert_eq!(foo.end_line, 3);

        let bar = result
            .definitions
            .iter()
            .find(|d| d.name == "bar")
            .unwrap();
        assert_eq!(bar.start_line, 5);
        assert_eq!(bar.end_line, 7);
    }

    // === Performance ===

    #[test]
    fn test_large_file_performance() {
        // Generate a 10K+ line TypeScript file
        let mut source = String::with_capacity(2_000_000);
        for i in 0..3000 {
            source.push_str(&format!(
                "function func_{i}(x: number): number {{\n  return x * {i};\n}}\n\n"
            ));
        }
        for i in 0..500 {
            source.push_str(&format!(
                "const arrow_{i} = (x: number) => x + {i};\n"
            ));
        }
        for i in 0..100 {
            source.push_str(&format!(
                "class Class_{i} {{\n  method_a() {{ return func_{i}(1); }}\n  method_b() {{ return arrow_{i}(2); }}\n}}\n\n"
            ));
        }

        let line_count = source.lines().count();
        assert!(
            line_count > 10_000,
            "generated file should have 10K+ lines, got {line_count}"
        );

        let start = std::time::Instant::now();
        let result = parse_file("large.ts", &source).unwrap();
        let elapsed = start.elapsed();

        assert!(
            elapsed.as_millis() < 500,
            "parsing 10K+ line file took {}ms, should be < 500ms",
            elapsed.as_millis()
        );

        // Sanity check: we extracted definitions
        assert!(result.definitions.len() > 3000);
        assert!(!result.call_sites.is_empty());
    }

    // === Python call sites ===

    #[test]
    fn test_parse_python_call_sites() {
        let source = r#"
def process(data):
    validated = validate(data)
    result = db.save(validated)
    return result
"#;
        let result = parse_file("handler.py", source).unwrap();
        let call_names: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(call_names.contains(&"validate"));
        assert!(call_names.contains(&"db.save"));

        for call in &result.call_sites {
            assert_eq!(
                call.containing_function,
                Some("process".to_string())
            );
        }
    }

    // === Edge cases ===

    #[test]
    fn test_empty_source() {
        let result = parse_file("empty.ts", "").unwrap();
        assert!(result.definitions.is_empty());
        assert!(result.imports.is_empty());
        assert!(result.exports.is_empty());
        assert!(result.call_sites.is_empty());
    }

    #[test]
    fn test_parse_js_file_uses_typescript_parser() {
        let source = "function hello() { console.log('hi'); }\n";
        let result = parse_file("app.js", source).unwrap();
        assert_eq!(result.language, Language::JavaScript);
        assert_eq!(result.definitions.len(), 1);
        assert_eq!(result.definitions[0].name, "hello");
    }

    #[test]
    fn test_export_class_with_methods() {
        let source = r#"
export class Router {
    get(path: string) {}
    post(path: string) {}
}
"#;
        let result = parse_file("router.ts", source).unwrap();

        let class_export = result.exports.iter().find(|e| e.name == "Router").unwrap();
        assert!(!class_export.is_default);

        let methods: Vec<&str> = result
            .definitions
            .iter()
            .filter(|d| d.name == "get" || d.name == "post")
            .map(|d| d.name.as_str())
            .collect();
        assert!(methods.contains(&"get"));
        assert!(methods.contains(&"post"));
    }

    // ========================================================================
    // Data flow extraction — TypeScript
    // ========================================================================

    #[test]
    fn test_data_flow_ts_simple_assignment() {
        let source = r#"
function handler(req: any) {
    const data = parseBody(req);
    return respond(data);
}
"#;
        let info = extract_data_flow_info("handler.ts", source).unwrap();

        // Should detect `const data = parseBody(req)`
        assert_eq!(info.assignments.len(), 1);
        assert_eq!(info.assignments[0].variable, "data");
        assert_eq!(info.assignments[0].callee, "parseBody");
        assert_eq!(
            info.assignments[0].containing_function,
            Some("handler".to_string())
        );

        // Should detect both calls with their arguments
        let parse_call = info
            .calls_with_args
            .iter()
            .find(|c| c.callee == "parseBody")
            .unwrap();
        assert!(parse_call.arguments.contains(&"req".to_string()));

        let respond_call = info
            .calls_with_args
            .iter()
            .find(|c| c.callee == "respond")
            .unwrap();
        assert!(respond_call.arguments.contains(&"data".to_string()));
    }

    #[test]
    fn test_data_flow_ts_method_call_assignment() {
        let source = r#"
function process() {
    const user = db.findOne(id);
    return transform(user);
}
"#;
        let info = extract_data_flow_info("service.ts", source).unwrap();

        assert_eq!(info.assignments.len(), 1);
        assert_eq!(info.assignments[0].variable, "user");
        assert_eq!(info.assignments[0].callee, "db.findOne");
    }

    #[test]
    fn test_data_flow_ts_await_assignment() {
        let source = r#"
async function handler(req: any) {
    const data = await fetchData(req.id);
    return process(data);
}
"#;
        let info = extract_data_flow_info("handler.ts", source).unwrap();

        // Should unwrap the await and capture the call
        assert_eq!(info.assignments.len(), 1);
        assert_eq!(info.assignments[0].variable, "data");
        assert_eq!(info.assignments[0].callee, "fetchData");
    }

    #[test]
    fn test_data_flow_ts_chained_assignments() {
        let source = r#"
function pipeline(input: any) {
    const validated = validate(input);
    const processed = transform(validated);
    const result = save(processed);
    return result;
}
"#;
        let info = extract_data_flow_info("pipeline.ts", source).unwrap();

        assert_eq!(info.assignments.len(), 3);

        let vars: Vec<&str> = info.assignments.iter().map(|a| a.variable.as_str()).collect();
        assert!(vars.contains(&"validated"));
        assert!(vars.contains(&"processed"));
        assert!(vars.contains(&"result"));

        let callees: Vec<&str> = info.assignments.iter().map(|a| a.callee.as_str()).collect();
        assert!(callees.contains(&"validate"));
        assert!(callees.contains(&"transform"));
        assert!(callees.contains(&"save"));
    }

    #[test]
    fn test_data_flow_ts_call_arguments_multiple() {
        let source = r#"
function merge(a: any, b: any) {
    const x = getFirst();
    const y = getSecond();
    return combine(x, y, 42);
}
"#;
        let info = extract_data_flow_info("merge.ts", source).unwrap();

        let combine_call = info
            .calls_with_args
            .iter()
            .find(|c| c.callee == "combine")
            .unwrap();
        assert!(combine_call.arguments.contains(&"x".to_string()));
        assert!(combine_call.arguments.contains(&"y".to_string()));
        // 42 is a literal, should also be captured as argument text
        assert!(combine_call.arguments.contains(&"42".to_string()));
    }

    #[test]
    fn test_data_flow_ts_arrow_function() {
        let source = r#"
const handler = (req: any) => {
    const data = parseBody(req);
    return respond(data);
};
"#;
        let info = extract_data_flow_info("handler.ts", source).unwrap();

        assert_eq!(info.assignments.len(), 1);
        assert_eq!(info.assignments[0].variable, "data");
        assert_eq!(info.assignments[0].callee, "parseBody");
        assert_eq!(
            info.assignments[0].containing_function,
            Some("handler".to_string())
        );
    }

    #[test]
    fn test_data_flow_ts_no_assignments() {
        let source = r#"
function simple() {
    console.log("hello");
    return 42;
}
"#;
        let info = extract_data_flow_info("simple.ts", source).unwrap();
        assert!(info.assignments.is_empty());
    }

    #[test]
    fn test_data_flow_ts_module_level() {
        let source = r#"
const config = loadConfig();
startServer(config);
"#;
        let info = extract_data_flow_info("main.ts", source).unwrap();

        assert_eq!(info.assignments.len(), 1);
        assert_eq!(info.assignments[0].variable, "config");
        assert_eq!(info.assignments[0].callee, "loadConfig");
        // Module-level has no containing function
        assert_eq!(info.assignments[0].containing_function, None);

        let start_call = info
            .calls_with_args
            .iter()
            .find(|c| c.callee == "startServer")
            .unwrap();
        assert!(start_call.arguments.contains(&"config".to_string()));
    }

    #[test]
    fn test_data_flow_ts_nested_call_as_argument() {
        let source = r#"
function process() {
    return save(transform(input));
}
"#;
        let info = extract_data_flow_info("process.ts", source).unwrap();

        // The inner call `transform(input)` should be captured
        let save_call = info
            .calls_with_args
            .iter()
            .find(|c| c.callee == "save")
            .unwrap();
        // The argument to save is the full nested call text
        assert_eq!(save_call.arguments.len(), 1);
        assert!(save_call.arguments[0].contains("transform"));
    }

    #[test]
    fn test_data_flow_ts_non_call_value_ignored() {
        let source = r#"
function process() {
    const x = 42;
    const y = "hello";
    const z = someVar;
    return x;
}
"#;
        let info = extract_data_flow_info("process.ts", source).unwrap();

        // None of these are function call assignments
        assert!(
            info.assignments.is_empty(),
            "literal and variable assignments should not be captured"
        );
    }

    // ========================================================================
    // Data flow extraction — Python
    // ========================================================================

    #[test]
    fn test_data_flow_python_simple_assignment() {
        let source = r#"
def handler(req):
    data = parse_body(req)
    return respond(data)
"#;
        let info = extract_data_flow_info("handler.py", source).unwrap();

        assert_eq!(info.assignments.len(), 1);
        assert_eq!(info.assignments[0].variable, "data");
        assert_eq!(info.assignments[0].callee, "parse_body");
        assert_eq!(
            info.assignments[0].containing_function,
            Some("handler".to_string())
        );

        let respond_call = info
            .calls_with_args
            .iter()
            .find(|c| c.callee == "respond")
            .unwrap();
        assert!(respond_call.arguments.contains(&"data".to_string()));
    }

    #[test]
    fn test_data_flow_python_chained() {
        let source = r#"
def pipeline(raw):
    validated = validate(raw)
    processed = transform(validated)
    save(processed)
"#;
        let info = extract_data_flow_info("pipeline.py", source).unwrap();

        assert_eq!(info.assignments.len(), 2);

        let vars: Vec<&str> = info.assignments.iter().map(|a| a.variable.as_str()).collect();
        assert!(vars.contains(&"validated"));
        assert!(vars.contains(&"processed"));
    }

    #[test]
    fn test_data_flow_python_method_call() {
        let source = r#"
def get_user(user_id):
    user = db.find_one(user_id)
    return serialize(user)
"#;
        let info = extract_data_flow_info("service.py", source).unwrap();

        assert_eq!(info.assignments.len(), 1);
        assert_eq!(info.assignments[0].callee, "db.find_one");
    }

    #[test]
    fn test_data_flow_unknown_language() {
        let info = extract_data_flow_info("main.rs", "fn main() {}").unwrap();
        assert!(info.assignments.is_empty());
        assert!(info.calls_with_args.is_empty());
    }

    #[test]
    fn test_data_flow_empty_source() {
        let info = extract_data_flow_info("empty.ts", "").unwrap();
        assert!(info.assignments.is_empty());
        assert!(info.calls_with_args.is_empty());
    }

    #[test]
    fn test_data_flow_ts_multiple_consumers() {
        let source = r#"
function process() {
    const data = fetchData();
    validate(data);
    transform(data);
    save(data);
}
"#;
        let info = extract_data_flow_info("process.ts", source).unwrap();

        // One assignment, three consumers
        assert_eq!(info.assignments.len(), 1);
        assert_eq!(info.assignments[0].variable, "data");

        let consumers_using_data: Vec<&str> = info
            .calls_with_args
            .iter()
            .filter(|c| c.arguments.contains(&"data".to_string()))
            .map(|c| c.callee.as_str())
            .collect();
        assert!(consumers_using_data.contains(&"validate"));
        assert!(consumers_using_data.contains(&"transform"));
        assert!(consumers_using_data.contains(&"save"));
    }

    // ========================================================================
    // Phase 8 audit: edge case tests
    // ========================================================================

    #[test]
    fn test_ts_enum_declaration_not_captured() {
        // Known limitation: TS enums are not extracted as definitions.
        // This test documents the behavior so it's visible.
        let source = r#"
enum Color {
    Red,
    Green,
    Blue,
}
"#;
        let result = parse_file("types.ts", source).unwrap();
        // Enums are not captured — this documents the gap.
        assert!(
            result.definitions.iter().all(|d| d.name != "Color"),
            "TS enums are not captured by the current parser"
        );
    }

    #[test]
    fn test_changed_symbols_same_span_different_body() {
        // If a function changes body but keeps the same line count,
        // detect_changed_symbols won't flag it as modified (by design — compares span size).
        let old_source = "function foo() {\n  return 1;\n}\n";
        let new_source = "function foo() {\n  return 2;\n}\n";
        let old = parse_file("lib.ts", old_source).unwrap();
        let new = parse_file("lib.ts", new_source).unwrap();
        let changes = detect_changed_symbols(&old, &new);
        // Same span size → not detected as modified (design limitation)
        assert!(
            changes.is_empty(),
            "same-span changes are not detected by span comparison"
        );
    }

    #[test]
    fn test_ts_abstract_class() {
        let source = r#"
abstract class BaseService {
    abstract process(): void;
    helper() { return 1; }
}
"#;
        let result = parse_file("service.ts", source).unwrap();
        let base_svc = result.definitions.iter().find(|d| d.name == "BaseService");
        assert!(base_svc.is_some(), "abstract classes should be captured");
        assert_eq!(base_svc.unwrap().kind, SymbolKind::Class);

        // Method inside abstract class
        assert!(result.definitions.iter().any(|d| d.name == "helper"));
    }

    #[test]
    fn test_ts_generator_function() {
        let source = r#"
function* generate() {
    yield 1;
    yield 2;
}
"#;
        let result = parse_file("gen.ts", source).unwrap();
        let gen = result.definitions.iter().find(|d| d.name == "generate");
        assert!(gen.is_some(), "generator functions should be captured");
        assert_eq!(gen.unwrap().kind, SymbolKind::Function);
    }

    #[test]
    fn test_ts_multiple_classes_with_methods() {
        let source = r#"
class A {
    foo() {}
}
class B {
    foo() {}
    bar() {}
}
"#;
        let result = parse_file("classes.ts", source).unwrap();
        let classes: Vec<&str> = result
            .definitions
            .iter()
            .filter(|d| d.kind == SymbolKind::Class)
            .map(|d| d.name.as_str())
            .collect();
        assert!(classes.contains(&"A"));
        assert!(classes.contains(&"B"));

        // Both classes have foo() methods — both should be captured
        let foos: Vec<&Definition> = result
            .definitions
            .iter()
            .filter(|d| d.name == "foo" && d.kind == SymbolKind::Function)
            .collect();
        assert_eq!(foos.len(), 2, "both foo() methods should be captured");
    }

    #[test]
    fn test_ts_unicode_identifiers() {
        let source = r#"
function grüßen(名前: string): string {
    return `Hello ${名前}`;
}
const αβγ = 42;
"#;
        let result = parse_file("unicode.ts", source).unwrap();
        assert!(result.definitions.iter().any(|d| d.name == "grüßen"));
        assert!(result.definitions.iter().any(|d| d.name == "αβγ"));
    }

    #[test]
    fn test_ts_syntax_error_partial_parse() {
        // tree-sitter does partial parsing on syntax errors but recovery is
        // not guaranteed for all subsequent definitions.
        let source = r#"
function valid() { return 1; }
const x = {{{
function alsoValid() { return 2; }
"#;
        let result = parse_file("broken.ts", source).unwrap();
        // The definition before the error should be extracted
        assert!(result.definitions.iter().any(|d| d.name == "valid"));
        // Parsing doesn't fail — no panic, just potentially missing later defs
        assert!(result.language == Language::TypeScript);
    }

    #[test]
    fn test_ts_export_default_class() {
        let source = r#"export default class App {
    render() {}
}"#;
        let result = parse_file("app.ts", source).unwrap();
        let app_export = result.exports.iter().find(|e| e.name == "App");
        assert!(app_export.is_some(), "export default class should be captured");
        assert!(app_export.unwrap().is_default);
    }

    #[test]
    fn test_ts_export_interface_and_type() {
        let source = r#"
export interface Config {
    port: number;
}
export type ID = string;
"#;
        let result = parse_file("types.ts", source).unwrap();
        assert!(result.exports.iter().any(|e| e.name == "Config"));
        assert!(result.exports.iter().any(|e| e.name == "ID"));
        assert!(result.definitions.iter().any(|d| d.name == "Config" && d.kind == SymbolKind::Interface));
        assert!(result.definitions.iter().any(|d| d.name == "ID" && d.kind == SymbolKind::TypeAlias));
    }

    #[test]
    fn test_python_decorated_class_with_methods() {
        let source = r#"
@dataclass
class User:
    name: str

    def greet(self):
        pass

    @staticmethod
    def create(name):
        pass
"#;
        let result = parse_file("models.py", source).unwrap();
        assert!(result.definitions.iter().any(|d| d.name == "User" && d.kind == SymbolKind::Class));
        assert!(result.definitions.iter().any(|d| d.name == "greet"));
        assert!(result.definitions.iter().any(|d| d.name == "create"));
    }

    #[test]
    fn test_python_wildcard_import() {
        let source = "from os.path import *\n";
        let result = parse_file("app.py", source).unwrap();
        assert_eq!(result.imports.len(), 1);
        assert!(result.imports[0].names.iter().any(|n| n.name == "*"));
    }

    #[test]
    fn test_python_relative_import_parent() {
        let source = "from .. import utils\n";
        let result = parse_file("sub/mod.py", source).unwrap();
        assert_eq!(result.imports.len(), 1);
        assert_eq!(result.imports[0].source, "..");
    }

    #[test]
    fn test_ts_deeply_nested_calls() {
        // Verify recursive call collection handles nesting
        let source = r#"
function outer() {
    function middle() {
        function inner() {
            deepCall();
        }
        middleCall();
    }
    outerCall();
}
"#;
        let result = parse_file("nested.ts", source).unwrap();
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"deepCall"));
        assert!(callees.contains(&"middleCall"));
        assert!(callees.contains(&"outerCall"));

        // Containing function resolution
        let deep = result.call_sites.iter().find(|c| c.callee == "deepCall").unwrap();
        assert_eq!(deep.containing_function, Some("inner".to_string()));
    }

    #[test]
    fn test_ts_module_level_calls_no_containing() {
        let source = "init();\nconfigure();\n";
        let result = parse_file("init.ts", source).unwrap();
        for call in &result.call_sites {
            assert_eq!(call.containing_function, None, "top-level calls should have no containing function");
        }
    }

    #[test]
    fn test_language_from_path_edge_cases() {
        assert_eq!(Language::from_path(""), Language::Unknown);
        assert_eq!(Language::from_path("noext"), Language::Unknown);
        assert_eq!(Language::from_path(".ts"), Language::TypeScript);
        assert_eq!(Language::from_path("a/b/c.py"), Language::Python);
        assert_eq!(Language::from_path("my.module.ts"), Language::TypeScript);
    }

    #[test]
    fn test_ts_comments_only_file() {
        let source = r#"
// This is a comment
/* block comment */
/** JSDoc */
"#;
        let result = parse_file("comments.ts", source).unwrap();
        assert!(result.definitions.is_empty());
        assert!(result.imports.is_empty());
        assert!(result.exports.is_empty());
        assert!(result.call_sites.is_empty());
    }

    #[test]
    fn test_data_flow_python_keyword_args_only() {
        let source = r#"
def main():
    result = connect(host='localhost', port=5432)
"#;
        let info = extract_data_flow_info("main.py", source).unwrap();

        let connect_call = info
            .calls_with_args
            .iter()
            .find(|c| c.callee == "connect")
            .unwrap();
        // Keyword arg values should be captured
        assert!(connect_call.arguments.contains(&"'localhost'".to_string()));
        assert!(connect_call.arguments.contains(&"5432".to_string()));
    }

    #[test]
    fn test_ts_let_var_declarations() {
        let source = r#"
let mutable = 42;
var legacy = "old";
"#;
        let result = parse_file("vars.ts", source).unwrap();
        assert!(result.definitions.iter().any(|d| d.name == "mutable"));
        assert!(result.definitions.iter().any(|d| d.name == "legacy"));
    }

    #[test]
    fn test_ts_export_multiple_vars() {
        let source = "export const A = 1, B = 2;\n";
        let result = parse_file("consts.ts", source).unwrap();
        assert!(result.exports.iter().any(|e| e.name == "A"));
        assert!(result.exports.iter().any(|e| e.name == "B"));
    }

    // ========================================================================
    // Go parsing tests
    // ========================================================================

    #[test]
    fn test_go_language_detection() {
        assert_eq!(Language::from_path("main.go"), Language::Go);
        assert_eq!(Language::from_path("handlers/user.go"), Language::Go);
    }

    #[test]
    fn test_go_simple_imports() {
        let source = r#"
package main

import "fmt"
import "net/http"
"#;
        let result = parse_file("main.go", source).unwrap();
        assert_eq!(result.language, Language::Go);
        assert_eq!(result.imports.len(), 2);

        assert_eq!(result.imports[0].source, "fmt");
        assert!(result.imports[0].is_namespace);
        assert_eq!(result.imports[0].names[0].name, "fmt");

        assert_eq!(result.imports[1].source, "net/http");
        assert!(result.imports[1].is_namespace);
        assert_eq!(result.imports[1].names[0].name, "http");
    }

    #[test]
    fn test_go_grouped_imports() {
        let source = r#"
package main

import (
    "fmt"
    "net/http"
    "github.com/gin-gonic/gin"
)
"#;
        let result = parse_file("main.go", source).unwrap();
        assert_eq!(result.imports.len(), 3);
        assert_eq!(result.imports[0].source, "fmt");
        assert_eq!(result.imports[1].source, "net/http");
        assert_eq!(result.imports[2].source, "github.com/gin-gonic/gin");
        assert_eq!(result.imports[2].names[0].name, "gin");
    }

    #[test]
    fn test_go_aliased_import() {
        let source = r#"
package main

import (
    myhttp "net/http"
    _ "database/sql"
)
"#;
        let result = parse_file("main.go", source).unwrap();
        assert_eq!(result.imports.len(), 2);

        // Aliased import
        assert_eq!(result.imports[0].source, "net/http");
        assert_eq!(result.imports[0].names[0].name, "http");
        assert_eq!(result.imports[0].names[0].alias, Some("myhttp".to_string()));

        // Blank import (side-effect only)
        assert_eq!(result.imports[1].source, "database/sql");
        assert!(result.imports[1].names.is_empty());
    }

    #[test]
    fn test_go_function_definitions() {
        let source = r#"
package main

func main() {
    fmt.Println("Hello")
}

func greet(name string) string {
    return "Hello " + name
}

func add(a, b int) int {
    return a + b
}
"#;
        let result = parse_file("main.go", source).unwrap();
        assert_eq!(result.language, Language::Go);

        let fns: Vec<&str> = result
            .definitions
            .iter()
            .filter(|d| d.kind == SymbolKind::Function)
            .map(|d| d.name.as_str())
            .collect();
        assert!(fns.contains(&"main"));
        assert!(fns.contains(&"greet"));
        assert!(fns.contains(&"add"));
    }

    #[test]
    fn test_go_struct_definitions() {
        let source = r#"
package models

type User struct {
    ID   int
    Name string
    Email string
}

type Config struct {
    Port int
    Host string
}
"#;
        let result = parse_file("models.go", source).unwrap();

        let user = result.definitions.iter().find(|d| d.name == "User").unwrap();
        assert_eq!(user.kind, SymbolKind::Class); // structs map to Class

        let config = result.definitions.iter().find(|d| d.name == "Config").unwrap();
        assert_eq!(config.kind, SymbolKind::Class);
    }

    #[test]
    fn test_go_interface_definitions() {
        let source = r#"
package service

type UserService interface {
    GetUser(id int) (*User, error)
    CreateUser(data UserInput) (*User, error)
    DeleteUser(id int) error
}

type Repository interface {
    Find(id int) (interface{}, error)
    Save(entity interface{}) error
}
"#;
        let result = parse_file("service.go", source).unwrap();

        let user_svc = result.definitions.iter().find(|d| d.name == "UserService").unwrap();
        assert_eq!(user_svc.kind, SymbolKind::Interface);

        let repo = result.definitions.iter().find(|d| d.name == "Repository").unwrap();
        assert_eq!(repo.kind, SymbolKind::Interface);
    }

    #[test]
    fn test_go_method_declarations() {
        let source = r#"
package models

type User struct {
    Name string
}

func (u *User) Greet() string {
    return "Hello " + u.Name
}

func (u User) String() string {
    return u.Name
}
"#;
        let result = parse_file("models.go", source).unwrap();

        let methods: Vec<&str> = result
            .definitions
            .iter()
            .filter(|d| d.kind == SymbolKind::Function)
            .map(|d| d.name.as_str())
            .collect();
        assert!(methods.contains(&"Greet"), "method Greet should be detected");
        assert!(methods.contains(&"String"), "method String should be detected");
    }

    #[test]
    fn test_go_constants() {
        let source = r#"
package config

const MaxRetries = 3
const (
    DefaultPort = 8080
    DefaultHost = "localhost"
)
"#;
        let result = parse_file("config.go", source).unwrap();

        let consts: Vec<&str> = result
            .definitions
            .iter()
            .filter(|d| d.kind == SymbolKind::Constant)
            .map(|d| d.name.as_str())
            .collect();
        assert!(consts.contains(&"MaxRetries"));
        assert!(consts.contains(&"DefaultPort"));
        assert!(consts.contains(&"DefaultHost"));
    }

    #[test]
    fn test_go_type_alias() {
        let source = r#"
package types

type UserID int64
type Handler func(w http.ResponseWriter, r *http.Request)
"#;
        let result = parse_file("types.go", source).unwrap();

        // UserID should be detected as TypeAlias (not struct/interface)
        let user_id = result.definitions.iter().find(|d| d.name == "UserID").unwrap();
        assert_eq!(user_id.kind, SymbolKind::TypeAlias);

        let handler = result.definitions.iter().find(|d| d.name == "Handler").unwrap();
        assert_eq!(handler.kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn test_go_call_sites() {
        let source = r#"
package main

import "fmt"

func process(data string) {
    validated := validate(data)
    result := db.Save(validated)
    fmt.Println(result)
}
"#;
        let result = parse_file("handler.go", source).unwrap();

        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"validate"));
        assert!(callees.contains(&"db.Save"));
        assert!(callees.contains(&"fmt.Println"));

        // All calls inside process function
        for call in &result.call_sites {
            assert_eq!(call.containing_function, Some("process".to_string()));
        }
    }

    #[test]
    fn test_go_call_sites_in_method() {
        let source = r#"
package service

func (s *UserService) Create(data UserInput) (*User, error) {
    validated := s.validate(data)
    return s.repo.Save(validated)
}
"#;
        let result = parse_file("service.go", source).unwrap();

        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"s.validate"));
        assert!(callees.contains(&"s.repo.Save"));

        for call in &result.call_sites {
            assert_eq!(call.containing_function, Some("Create".to_string()));
        }
    }

    #[test]
    fn test_go_exported_symbols() {
        let source = r#"
package models

type User struct {
    Name string
}

type internalState struct {
    cache map[string]string
}

func GetUser(id int) *User {
    return nil
}

func helper() {
}

const MaxSize = 100
const defaultTimeout = 30
"#;
        let result = parse_file("models.go", source).unwrap();

        let export_names: Vec<&str> = result.exports.iter().map(|e| e.name.as_str()).collect();
        // Uppercase = exported
        assert!(export_names.contains(&"User"));
        assert!(export_names.contains(&"GetUser"));
        assert!(export_names.contains(&"MaxSize"));
        // Lowercase = not exported
        assert!(!export_names.contains(&"internalState"));
        assert!(!export_names.contains(&"helper"));
        assert!(!export_names.contains(&"defaultTimeout"));
    }

    #[test]
    fn test_go_data_flow_short_var_decl() {
        let source = r#"
package main

func handler(req string) {
    data := parseBody(req)
    result := transform(data)
    save(result)
}
"#;
        let info = extract_data_flow_info("handler.go", source).unwrap();

        assert_eq!(info.assignments.len(), 2);

        let vars: Vec<&str> = info.assignments.iter().map(|a| a.variable.as_str()).collect();
        assert!(vars.contains(&"data"));
        assert!(vars.contains(&"result"));

        let callees: Vec<&str> = info.assignments.iter().map(|a| a.callee.as_str()).collect();
        assert!(callees.contains(&"parseBody"));
        assert!(callees.contains(&"transform"));
    }

    #[test]
    fn test_go_data_flow_method_call() {
        let source = r#"
package main

func getUser(id int) {
    user := db.FindOne(id)
    save(user)
}
"#;
        let info = extract_data_flow_info("service.go", source).unwrap();

        assert_eq!(info.assignments.len(), 1);
        assert_eq!(info.assignments[0].variable, "user");
        assert_eq!(info.assignments[0].callee, "db.FindOne");
        assert_eq!(info.assignments[0].containing_function, Some("getUser".to_string()));
    }

    #[test]
    fn test_go_data_flow_call_with_args() {
        let source = r#"
package main

func process() {
    x := getFirst()
    y := getSecond()
    combine(x, y, 42)
}
"#;
        let info = extract_data_flow_info("process.go", source).unwrap();

        let combine = info
            .calls_with_args
            .iter()
            .find(|c| c.callee == "combine")
            .unwrap();
        assert!(combine.arguments.contains(&"x".to_string()));
        assert!(combine.arguments.contains(&"y".to_string()));
        assert!(combine.arguments.contains(&"42".to_string()));
    }

    #[test]
    fn test_go_empty_source() {
        let source = "package main\n";
        let result = parse_file("empty.go", source).unwrap();
        assert_eq!(result.language, Language::Go);
        assert!(result.definitions.is_empty());
        assert!(result.imports.is_empty());
        assert!(result.call_sites.is_empty());
    }

    #[test]
    fn test_go_goroutine_call() {
        let source = r#"
package main

func startWorker() {
    go processQueue()
    go handleMessages()
}
"#;
        let result = parse_file("worker.go", source).unwrap();

        // Goroutine calls should still be detected as call sites
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"processQueue"));
        assert!(callees.contains(&"handleMessages"));
    }

    #[test]
    fn test_go_var_declaration() {
        let source = r#"
package config

var GlobalConfig Config
var (
    Logger  *log.Logger
    Verbose bool
)
"#;
        let result = parse_file("config.go", source).unwrap();
        let vars: Vec<&str> = result.definitions.iter().map(|d| d.name.as_str()).collect();
        assert!(vars.contains(&"GlobalConfig"));
        assert!(vars.contains(&"Logger"));
        assert!(vars.contains(&"Verbose"));
    }

    #[test]
    fn test_go_http_handler_pattern() {
        let source = r#"
package main

import "net/http"

func main() {
    http.HandleFunc("/users", handleUsers)
    http.ListenAndServe(":8080", nil)
}

func handleUsers(w http.ResponseWriter, r *http.Request) {
    fmt.Fprintln(w, "users")
}
"#;
        let result = parse_file("main.go", source).unwrap();

        let fns: Vec<&str> = result
            .definitions
            .iter()
            .filter(|d| d.kind == SymbolKind::Function)
            .map(|d| d.name.as_str())
            .collect();
        assert!(fns.contains(&"main"));
        assert!(fns.contains(&"handleUsers"));

        // Verify http import
        assert!(result.imports.iter().any(|i| i.source == "net/http"));

        // Verify call sites
        let callees: Vec<&str> = result.call_sites.iter().map(|c| c.callee.as_str()).collect();
        assert!(callees.contains(&"http.HandleFunc"));
        assert!(callees.contains(&"http.ListenAndServe"));
    }
}
