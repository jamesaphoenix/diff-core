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
    Unknown,
}

impl Language {
    /// Detect language from file path extension.
    pub fn from_path(path: &str) -> Self {
        match path.rsplit('.').next().unwrap_or("") {
            "ts" | "tsx" => Language::TypeScript,
            "js" | "jsx" | "mjs" | "cjs" => Language::JavaScript,
            "py" | "pyi" => Language::Python,
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

/// Parse a source file and extract symbols, imports, exports, and call sites.
pub fn parse_file(path: &str, source: &str) -> Result<ParsedFile, AstError> {
    let language = Language::from_path(path);
    match language {
        Language::TypeScript | Language::JavaScript => parse_typescript(path, source, language),
        Language::Python => parse_python(path, source),
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
        | "method_definition" => node
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
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
        let source = r#"
package main

import "fmt"

func main() {
    fmt.Println("Hello")
}
"#;
        let result = parse_file("main.go", source).unwrap();
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
        assert_eq!(Language::from_path("app.go"), Language::Unknown);
        assert_eq!(Language::from_path("app.rs"), Language::Unknown);
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
}
