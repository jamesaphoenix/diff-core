//! Language-agnostic intermediate representation (IR) for parsed source files.
//!
//! All downstream pipeline stages (graph, flow, clustering, entrypoint detection)
//! should consume IR types, never raw tree-sitter nodes. The IR is the single
//! source of truth for what was extracted from source code.
//!
//! # Supported patterns
//!
//! - Simple assignments: `const x = foo()`
//! - Object destructuring: `const { a, b } = foo()`, `const { a: renamed } = foo()`
//! - Array destructuring: `const [first, ...rest] = bar()`
//! - Python tuple unpacking: `a, b = func()`
//! - Rust tuple destructuring: `let (a, b) = func()`
//! - Effect.ts yield* destructuring: `const { svc } = yield* _(Tag)`
//! - Spread/rest patterns, nested destructuring, default values
//! - Function/method definitions with parameters
//! - Class/struct/interface/type definitions
//! - Import/export declarations
//! - Call expressions with arguments

use crate::ast::{
    CallSite, CallWithArgs, DataFlowInfo, Definition, ExportInfo, ImportInfo, ImportedName,
    Language, ParsedFile, VarCallAssignment,
};
use crate::types::SymbolKind;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Source location
// ---------------------------------------------------------------------------

/// A span in the source code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start_line: usize,
    pub end_line: usize,
}

impl Span {
    pub fn new(start_line: usize, end_line: usize) -> Self {
        Self {
            start_line,
            end_line,
        }
    }

    pub fn single(line: usize) -> Self {
        Self {
            start_line: line,
            end_line: line,
        }
    }

    /// Number of lines this span covers.
    pub fn line_count(&self) -> usize {
        self.end_line.saturating_sub(self.start_line) + 1
    }
}

// ---------------------------------------------------------------------------
// Binding patterns (LHS of assignments, function parameters)
// ---------------------------------------------------------------------------

/// A binding pattern — the left-hand side of an assignment or a function parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IrPattern {
    /// Simple name binding: `x`, `const x = ...`
    Identifier(String),

    /// Object destructuring: `const { a, b: renamed, ...rest } = ...`
    ObjectDestructure {
        properties: Vec<DestructureProperty>,
        rest: Option<String>,
    },

    /// Array destructuring: `const [first, , third, ...rest] = ...`
    ArrayDestructure {
        /// `None` represents a hole (skipped element).
        elements: Vec<Option<IrPattern>>,
        rest: Option<String>,
    },

    /// Tuple destructuring (Python: `a, b = ...`, Rust: `let (a, b) = ...`)
    TupleDestructure { elements: Vec<IrPattern> },
}

impl IrPattern {
    /// Extract all bound variable names from this pattern (recursively).
    pub fn bound_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        self.collect_names(&mut names);
        names
    }

    fn collect_names(&self, out: &mut Vec<String>) {
        match self {
            IrPattern::Identifier(name) => out.push(name.clone()),
            IrPattern::ObjectDestructure { properties, rest } => {
                for prop in properties {
                    prop.value.collect_names(out);
                }
                if let Some(r) = rest {
                    out.push(r.clone());
                }
            }
            IrPattern::ArrayDestructure { elements, rest } => {
                for elem in elements.iter().flatten() {
                    elem.collect_names(out);
                }
                if let Some(r) = rest {
                    out.push(r.clone());
                }
            }
            IrPattern::TupleDestructure { elements } => {
                for elem in elements {
                    elem.collect_names(out);
                }
            }
        }
    }

    /// Returns true if this pattern is a simple identifier.
    pub fn is_identifier(&self) -> bool {
        matches!(self, IrPattern::Identifier(_))
    }

    /// Returns the identifier name if this is a simple identifier pattern.
    pub fn as_identifier(&self) -> Option<&str> {
        match self {
            IrPattern::Identifier(name) => Some(name.as_str()),
            _ => None,
        }
    }
}

/// A single property in an object destructuring pattern.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestructureProperty {
    /// The property key being destructured (e.g., `a` in `{ a: renamed }`).
    pub key: String,
    /// The binding pattern (can be nested destructure or simple rename).
    pub value: IrPattern,
    /// Default value expression text (e.g., `42` in `{ a = 42 }`).
    pub default_value: Option<String>,
}

// ---------------------------------------------------------------------------
// Expressions (RHS of assignments, call arguments)
// ---------------------------------------------------------------------------

/// An expression in the IR — the right-hand side of assignments and call arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IrExpression {
    /// A function/method call.
    Call(IrCallExpression),

    /// A variable reference.
    Identifier(String),

    /// An await expression: `await foo()`
    Await(Box<IrExpression>),

    /// A yield expression (Effect.ts): `yield* _(Tag)`
    Yield(Box<IrExpression>),

    /// Member access: `obj.prop`
    MemberAccess {
        object: Box<IrExpression>,
        property: String,
    },

    /// Any other expression represented as source text.
    Other(String),
}

impl IrExpression {
    /// Extract the callee name if this expression is (or wraps) a call.
    pub fn callee_name(&self) -> Option<&str> {
        match self {
            IrExpression::Call(call) => Some(call.callee.as_str()),
            IrExpression::Await(inner) | IrExpression::Yield(inner) => inner.callee_name(),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Declarations and definitions
// ---------------------------------------------------------------------------

/// IR node for a function or method definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrFunctionDef {
    pub name: String,
    pub kind: FunctionKind,
    pub span: Span,
    pub parameters: Vec<IrParameter>,
    pub is_async: bool,
    pub is_exported: bool,
    pub decorators: Vec<String>,
}

/// The kind of function definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FunctionKind {
    Function,
    Method,
    ArrowFunction,
    Generator,
    Constructor,
}

/// A function parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrParameter {
    /// The binding pattern (simple name or destructuring).
    pub pattern: IrPattern,
    /// Type annotation text if available.
    pub type_annotation: Option<String>,
    /// Default value expression text if available.
    pub default_value: Option<String>,
}

/// IR node for a class, struct, interface, type alias, or enum definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrTypeDef {
    pub name: String,
    pub kind: TypeDefKind,
    pub span: Span,
    /// Base classes/interfaces (extends/implements).
    pub bases: Vec<String>,
    pub is_exported: bool,
    pub decorators: Vec<String>,
}

/// The kind of type definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TypeDefKind {
    Class,
    Struct,
    Interface,
    TypeAlias,
    Enum,
}

/// IR node for a constant/variable declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrConstant {
    pub name: String,
    pub span: Span,
    pub is_exported: bool,
}

// ---------------------------------------------------------------------------
// Imports and exports
// ---------------------------------------------------------------------------

/// IR node for an import declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrImport {
    /// Module path/source (e.g., `"./utils"`, `"express"`).
    pub source: String,
    /// Import specifiers.
    pub specifiers: Vec<IrImportSpecifier>,
    pub span: Span,
}

/// A single import specifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum IrImportSpecifier {
    /// Named import: `import { foo } from ...` or `import { foo as bar } from ...`
    Named {
        name: String,
        alias: Option<String>,
    },
    /// Default import: `import Foo from ...`
    Default(String),
    /// Namespace import: `import * as ns from ...`
    Namespace(String),
    /// Side-effect import: `import 'module'` (Python: `import module`)
    SideEffect,
}

impl IrImportSpecifier {
    /// The local name this import binds (the name used in code after import).
    pub fn local_name(&self) -> Option<&str> {
        match self {
            IrImportSpecifier::Named { name, alias } => {
                Some(alias.as_deref().unwrap_or(name.as_str()))
            }
            IrImportSpecifier::Default(name) | IrImportSpecifier::Namespace(name) => {
                Some(name.as_str())
            }
            IrImportSpecifier::SideEffect => None,
        }
    }

    /// The original/remote name of the imported symbol.
    pub fn remote_name(&self) -> Option<&str> {
        match self {
            IrImportSpecifier::Named { name, .. } => Some(name.as_str()),
            IrImportSpecifier::Default(_) => Some("default"),
            IrImportSpecifier::Namespace(_) | IrImportSpecifier::SideEffect => None,
        }
    }
}

/// IR node for an export declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrExport {
    pub name: String,
    pub is_default: bool,
    pub is_reexport: bool,
    /// Source module for re-exports.
    pub source: Option<String>,
    pub span: Span,
}

// ---------------------------------------------------------------------------
// Call expressions and assignments
// ---------------------------------------------------------------------------

/// IR node for a function call expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrCallExpression {
    /// Resolved callee name (e.g., `"foo"`, `"obj.method"`).
    pub callee: String,
    /// Argument expression texts.
    pub arguments: Vec<String>,
    pub span: Span,
    /// Function containing this call.
    pub containing_function: Option<String>,
}

/// IR node for a variable assignment statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrAssignment {
    /// The binding pattern (simple name or destructuring).
    pub pattern: IrPattern,
    /// The right-hand side expression.
    pub value: IrExpression,
    pub span: Span,
    /// Function containing this assignment.
    pub containing_function: Option<String>,
}

// ---------------------------------------------------------------------------
// Complete IR for a file
// ---------------------------------------------------------------------------

/// The complete intermediate representation for a single source file.
///
/// This is the single source of truth that all downstream pipeline stages consume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IrFile {
    /// File path.
    pub path: String,
    /// Detected language.
    pub language: Language,
    /// Function and method definitions.
    pub functions: Vec<IrFunctionDef>,
    /// Class, struct, interface, type alias definitions.
    pub type_defs: Vec<IrTypeDef>,
    /// Constant/variable declarations.
    pub constants: Vec<IrConstant>,
    /// Import declarations.
    pub imports: Vec<IrImport>,
    /// Export declarations.
    pub exports: Vec<IrExport>,
    /// Call expressions found in the file.
    pub call_expressions: Vec<IrCallExpression>,
    /// Assignments (including destructuring).
    pub assignments: Vec<IrAssignment>,
}

impl IrFile {
    /// Create an empty IR file for the given path.
    pub fn empty(path: &str) -> Self {
        Self {
            path: path.to_string(),
            language: Language::from_path(path),
            functions: vec![],
            type_defs: vec![],
            constants: vec![],
            imports: vec![],
            exports: vec![],
            call_expressions: vec![],
            assignments: vec![],
        }
    }

    /// All definition names (functions + type_defs + constants).
    pub fn all_definition_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = Vec::new();
        for f in &self.functions {
            names.push(&f.name);
        }
        for t in &self.type_defs {
            names.push(&t.name);
        }
        for c in &self.constants {
            names.push(&c.name);
        }
        names
    }

    /// All exported names.
    pub fn exported_names(&self) -> Vec<&str> {
        self.exports.iter().map(|e| e.name.as_str()).collect()
    }

    /// All import sources.
    pub fn import_sources(&self) -> Vec<&str> {
        self.imports.iter().map(|i| i.source.as_str()).collect()
    }
}

// ---------------------------------------------------------------------------
// Conversion: ParsedFile → IrFile
// ---------------------------------------------------------------------------

impl IrFile {
    /// Convert a `ParsedFile` (from the existing AST layer) into an `IrFile`.
    ///
    /// This is a lossless conversion — all information from `ParsedFile` is preserved.
    /// Call `enrich_with_data_flow` afterwards to add assignment/call details from
    /// `DataFlowInfo`.
    pub fn from_parsed_file(parsed: &ParsedFile) -> Self {
        let functions = parsed
            .definitions
            .iter()
            .filter(|d| d.kind == SymbolKind::Function)
            .map(|d| IrFunctionDef {
                name: d.name.clone(),
                kind: FunctionKind::Function,
                span: Span::new(d.start_line, d.end_line),
                parameters: vec![],
                is_async: false,
                is_exported: false,
                decorators: vec![],
            })
            .collect();

        let type_defs = parsed
            .definitions
            .iter()
            .filter(|d| matches!(d.kind, SymbolKind::Class | SymbolKind::Interface | SymbolKind::TypeAlias | SymbolKind::Struct))
            .map(|d| IrTypeDef {
                name: d.name.clone(),
                kind: match d.kind {
                    SymbolKind::Class => TypeDefKind::Class,
                    SymbolKind::Interface => TypeDefKind::Interface,
                    SymbolKind::TypeAlias => TypeDefKind::TypeAlias,
                    SymbolKind::Struct => TypeDefKind::Struct,
                    _ => TypeDefKind::Class,
                },
                span: Span::new(d.start_line, d.end_line),
                bases: vec![],
                is_exported: false,
                decorators: vec![],
            })
            .collect();

        let constants = parsed
            .definitions
            .iter()
            .filter(|d| d.kind == SymbolKind::Constant)
            .map(|d| IrConstant {
                name: d.name.clone(),
                span: Span::new(d.start_line, d.end_line),
                is_exported: false,
            })
            .collect();

        let imports = parsed.imports.iter().map(convert_import).collect();
        let exports = parsed.exports.iter().map(convert_export).collect();

        let call_expressions = parsed
            .call_sites
            .iter()
            .map(|cs| IrCallExpression {
                callee: cs.callee.clone(),
                arguments: vec![],
                span: Span::single(cs.line),
                containing_function: cs.containing_function.clone(),
            })
            .collect();

        Self {
            path: parsed.path.clone(),
            language: parsed.language,
            functions,
            type_defs,
            constants,
            imports,
            exports,
            call_expressions,
            assignments: vec![],
        }
    }

    /// Enrich an `IrFile` with data flow information (assignments and call arguments).
    ///
    /// This adds variable assignments and enriches call expressions with argument details
    /// from a `DataFlowInfo` extraction.
    pub fn enrich_with_data_flow(&mut self, data_flow: &DataFlowInfo) {
        // Add assignments
        for assign in &data_flow.assignments {
            self.assignments.push(convert_assignment(assign));
        }

        // Enrich call expressions with argument info
        for call_with_args in &data_flow.calls_with_args {
            // Try to find matching call expression and enrich it
            let found = self.call_expressions.iter_mut().find(|ce| {
                ce.callee == call_with_args.callee
                    && ce.span.start_line == call_with_args.line
                    && ce.containing_function == call_with_args.containing_function
            });

            if let Some(existing) = found {
                existing.arguments = call_with_args.arguments.clone();
            } else {
                // Add as new call expression
                self.call_expressions.push(IrCallExpression {
                    callee: call_with_args.callee.clone(),
                    arguments: call_with_args.arguments.clone(),
                    span: Span::single(call_with_args.line),
                    containing_function: call_with_args.containing_function.clone(),
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Conversion: IrFile → ParsedFile (backward compat)
// ---------------------------------------------------------------------------

impl IrFile {
    /// Convert back to a `ParsedFile` for backward compatibility with existing pipeline
    /// stages that haven't been migrated to IR types yet.
    pub fn to_parsed_file(&self) -> ParsedFile {
        let mut definitions = Vec::new();

        for f in &self.functions {
            definitions.push(Definition {
                name: f.name.clone(),
                kind: SymbolKind::Function,
                start_line: f.span.start_line,
                end_line: f.span.end_line,
            });
        }

        for t in &self.type_defs {
            definitions.push(Definition {
                name: t.name.clone(),
                kind: match t.kind {
                    TypeDefKind::Class => SymbolKind::Class,
                    TypeDefKind::Struct => SymbolKind::Struct,
                    TypeDefKind::Interface => SymbolKind::Interface,
                    TypeDefKind::TypeAlias => SymbolKind::TypeAlias,
                    TypeDefKind::Enum => SymbolKind::Class, // Enum maps to Class for now
                },
                start_line: t.span.start_line,
                end_line: t.span.end_line,
            });
        }

        for c in &self.constants {
            definitions.push(Definition {
                name: c.name.clone(),
                kind: SymbolKind::Constant,
                start_line: c.span.start_line,
                end_line: c.span.end_line,
            });
        }

        let imports = self.imports.iter().map(revert_import).collect();
        let exports = self.exports.iter().map(revert_export).collect();

        let call_sites = self
            .call_expressions
            .iter()
            .map(|ce| CallSite {
                callee: ce.callee.clone(),
                line: ce.span.start_line,
                containing_function: ce.containing_function.clone(),
            })
            .collect();

        ParsedFile {
            path: self.path.clone(),
            language: self.language,
            definitions,
            imports,
            exports,
            call_sites,
        }
    }

    /// Convert assignments back to `DataFlowInfo` for backward compatibility.
    pub fn to_data_flow_info(&self) -> DataFlowInfo {
        let assignments = self
            .assignments
            .iter()
            .filter_map(|a| {
                let variable = a.pattern.as_identifier()?.to_string();
                let callee = a.value.callee_name()?.to_string();
                Some(VarCallAssignment {
                    variable,
                    callee,
                    line: a.span.start_line,
                    containing_function: a.containing_function.clone(),
                })
            })
            .collect();

        let calls_with_args = self
            .call_expressions
            .iter()
            .filter(|ce| !ce.arguments.is_empty())
            .map(|ce| CallWithArgs {
                callee: ce.callee.clone(),
                arguments: ce.arguments.clone(),
                line: ce.span.start_line,
                containing_function: ce.containing_function.clone(),
            })
            .collect();

        DataFlowInfo {
            assignments,
            calls_with_args,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal conversion helpers
// ---------------------------------------------------------------------------

fn convert_import(imp: &ImportInfo) -> IrImport {
    let mut specifiers = Vec::new();

    if imp.is_namespace {
        if let Some(name) = imp.names.first() {
            specifiers.push(IrImportSpecifier::Namespace(name.name.clone()));
        }
    } else if imp.is_default {
        if let Some(name) = imp.names.first() {
            specifiers.push(IrImportSpecifier::Default(name.name.clone()));
        }
    } else if imp.names.is_empty() {
        specifiers.push(IrImportSpecifier::SideEffect);
    } else {
        for name in &imp.names {
            specifiers.push(IrImportSpecifier::Named {
                name: name.name.clone(),
                alias: name.alias.clone(),
            });
        }
    }

    IrImport {
        source: imp.source.clone(),
        specifiers,
        span: Span::single(imp.line),
    }
}

fn convert_export(exp: &ExportInfo) -> IrExport {
    IrExport {
        name: exp.name.clone(),
        is_default: exp.is_default,
        is_reexport: exp.is_reexport,
        source: exp.source.clone(),
        span: Span::single(exp.line),
    }
}

fn convert_assignment(assign: &VarCallAssignment) -> IrAssignment {
    IrAssignment {
        pattern: IrPattern::Identifier(assign.variable.clone()),
        value: IrExpression::Call(IrCallExpression {
            callee: assign.callee.clone(),
            arguments: vec![],
            span: Span::single(assign.line),
            containing_function: assign.containing_function.clone(),
        }),
        span: Span::single(assign.line),
        containing_function: assign.containing_function.clone(),
    }
}

fn revert_import(ir: &IrImport) -> ImportInfo {
    let mut names = Vec::new();
    let mut is_default = false;
    let mut is_namespace = false;

    for spec in &ir.specifiers {
        match spec {
            IrImportSpecifier::Named { name, alias } => {
                names.push(ImportedName {
                    name: name.clone(),
                    alias: alias.clone(),
                });
            }
            IrImportSpecifier::Default(name) => {
                is_default = true;
                names.push(ImportedName {
                    name: name.clone(),
                    alias: None,
                });
            }
            IrImportSpecifier::Namespace(name) => {
                is_namespace = true;
                names.push(ImportedName {
                    name: name.clone(),
                    alias: None,
                });
            }
            IrImportSpecifier::SideEffect => {}
        }
    }

    ImportInfo {
        source: ir.source.clone(),
        names,
        is_default,
        is_namespace,
        line: ir.span.start_line,
    }
}

fn revert_export(ir: &IrExport) -> ExportInfo {
    ExportInfo {
        name: ir.name.clone(),
        is_default: ir.is_default,
        is_reexport: ir.is_reexport,
        source: ir.source.clone(),
        line: ir.span.start_line,
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod tests {
    use super::*;
    use crate::ast::{Definition, ImportedName};
    use crate::types::SymbolKind;

    // -----------------------------------------------------------------------
    // Span tests
    // -----------------------------------------------------------------------

    #[test]
    fn span_new() {
        let span = Span::new(5, 10);
        assert_eq!(span.start_line, 5);
        assert_eq!(span.end_line, 10);
    }

    #[test]
    fn span_single() {
        let span = Span::single(7);
        assert_eq!(span.start_line, 7);
        assert_eq!(span.end_line, 7);
    }

    #[test]
    fn span_line_count() {
        assert_eq!(Span::new(1, 1).line_count(), 1);
        assert_eq!(Span::new(1, 10).line_count(), 10);
        assert_eq!(Span::single(5).line_count(), 1);
    }

    #[test]
    fn span_line_count_zero_span() {
        // Edge case: end < start (shouldn't happen normally, but shouldn't panic)
        let span = Span::new(10, 5);
        assert_eq!(span.line_count(), 1); // saturating_sub returns 0, +1 = 1
    }

    // -----------------------------------------------------------------------
    // IrPattern tests
    // -----------------------------------------------------------------------

    #[test]
    fn pattern_identifier_bound_names() {
        let pat = IrPattern::Identifier("x".into());
        assert_eq!(pat.bound_names(), vec!["x"]);
    }

    #[test]
    fn pattern_identifier_is_identifier() {
        let pat = IrPattern::Identifier("x".into());
        assert!(pat.is_identifier());
        assert_eq!(pat.as_identifier(), Some("x"));
    }

    #[test]
    fn pattern_object_destructure_bound_names() {
        let pat = IrPattern::ObjectDestructure {
            properties: vec![
                DestructureProperty {
                    key: "a".into(),
                    value: IrPattern::Identifier("a".into()),
                    default_value: None,
                },
                DestructureProperty {
                    key: "b".into(),
                    value: IrPattern::Identifier("renamed".into()),
                    default_value: None,
                },
            ],
            rest: Some("rest".into()),
        };
        assert_eq!(pat.bound_names(), vec!["a", "renamed", "rest"]);
    }

    #[test]
    fn pattern_object_destructure_not_identifier() {
        let pat = IrPattern::ObjectDestructure {
            properties: vec![],
            rest: None,
        };
        assert!(!pat.is_identifier());
        assert_eq!(pat.as_identifier(), None);
    }

    #[test]
    fn pattern_array_destructure_bound_names() {
        let pat = IrPattern::ArrayDestructure {
            elements: vec![
                Some(IrPattern::Identifier("first".into())),
                None, // hole
                Some(IrPattern::Identifier("third".into())),
            ],
            rest: Some("rest".into()),
        };
        assert_eq!(pat.bound_names(), vec!["first", "third", "rest"]);
    }

    #[test]
    fn pattern_array_destructure_with_holes() {
        let pat = IrPattern::ArrayDestructure {
            elements: vec![None, None, Some(IrPattern::Identifier("x".into()))],
            rest: None,
        };
        assert_eq!(pat.bound_names(), vec!["x"]);
    }

    #[test]
    fn pattern_tuple_destructure_bound_names() {
        let pat = IrPattern::TupleDestructure {
            elements: vec![
                IrPattern::Identifier("a".into()),
                IrPattern::Identifier("b".into()),
                IrPattern::Identifier("c".into()),
            ],
        };
        assert_eq!(pat.bound_names(), vec!["a", "b", "c"]);
    }

    #[test]
    fn pattern_nested_destructure() {
        // const { a, inner: { b, c } } = foo()
        let pat = IrPattern::ObjectDestructure {
            properties: vec![
                DestructureProperty {
                    key: "a".into(),
                    value: IrPattern::Identifier("a".into()),
                    default_value: None,
                },
                DestructureProperty {
                    key: "inner".into(),
                    value: IrPattern::ObjectDestructure {
                        properties: vec![
                            DestructureProperty {
                                key: "b".into(),
                                value: IrPattern::Identifier("b".into()),
                                default_value: None,
                            },
                            DestructureProperty {
                                key: "c".into(),
                                value: IrPattern::Identifier("c".into()),
                                default_value: None,
                            },
                        ],
                        rest: None,
                    },
                    default_value: None,
                },
            ],
            rest: None,
        };
        assert_eq!(pat.bound_names(), vec!["a", "b", "c"]);
    }

    #[test]
    fn pattern_object_with_default_values() {
        let pat = IrPattern::ObjectDestructure {
            properties: vec![DestructureProperty {
                key: "x".into(),
                value: IrPattern::Identifier("x".into()),
                default_value: Some("42".into()),
            }],
            rest: None,
        };
        assert_eq!(pat.bound_names(), vec!["x"]);
        assert_eq!(
            pat,
            IrPattern::ObjectDestructure {
                properties: vec![DestructureProperty {
                    key: "x".into(),
                    value: IrPattern::Identifier("x".into()),
                    default_value: Some("42".into()),
                }],
                rest: None,
            }
        );
    }

    #[test]
    fn pattern_empty_object_destructure() {
        let pat = IrPattern::ObjectDestructure {
            properties: vec![],
            rest: None,
        };
        assert!(pat.bound_names().is_empty());
    }

    #[test]
    fn pattern_empty_array_destructure() {
        let pat = IrPattern::ArrayDestructure {
            elements: vec![],
            rest: None,
        };
        assert!(pat.bound_names().is_empty());
    }

    #[test]
    fn pattern_array_with_nested_object() {
        // const [{ a }, b] = foo()
        let pat = IrPattern::ArrayDestructure {
            elements: vec![
                Some(IrPattern::ObjectDestructure {
                    properties: vec![DestructureProperty {
                        key: "a".into(),
                        value: IrPattern::Identifier("a".into()),
                        default_value: None,
                    }],
                    rest: None,
                }),
                Some(IrPattern::Identifier("b".into())),
            ],
            rest: None,
        };
        assert_eq!(pat.bound_names(), vec!["a", "b"]);
    }

    // -----------------------------------------------------------------------
    // IrExpression tests
    // -----------------------------------------------------------------------

    #[test]
    fn expression_call_callee_name() {
        let expr = IrExpression::Call(IrCallExpression {
            callee: "foo".into(),
            arguments: vec![],
            span: Span::single(1),
            containing_function: None,
        });
        assert_eq!(expr.callee_name(), Some("foo"));
    }

    #[test]
    fn expression_await_call_callee_name() {
        let expr = IrExpression::Await(Box::new(IrExpression::Call(IrCallExpression {
            callee: "fetch".into(),
            arguments: vec![],
            span: Span::single(1),
            containing_function: None,
        })));
        assert_eq!(expr.callee_name(), Some("fetch"));
    }

    #[test]
    fn expression_yield_call_callee_name() {
        let expr = IrExpression::Yield(Box::new(IrExpression::Call(IrCallExpression {
            callee: "_(Tag)".into(),
            arguments: vec![],
            span: Span::single(1),
            containing_function: None,
        })));
        assert_eq!(expr.callee_name(), Some("_(Tag)"));
    }

    #[test]
    fn expression_identifier_no_callee() {
        let expr = IrExpression::Identifier("x".into());
        assert_eq!(expr.callee_name(), None);
    }

    #[test]
    fn expression_other_no_callee() {
        let expr = IrExpression::Other("1 + 2".into());
        assert_eq!(expr.callee_name(), None);
    }

    #[test]
    fn expression_member_access_no_callee() {
        let expr = IrExpression::MemberAccess {
            object: Box::new(IrExpression::Identifier("obj".into())),
            property: "prop".into(),
        };
        assert_eq!(expr.callee_name(), None);
    }

    // -----------------------------------------------------------------------
    // IrImportSpecifier tests
    // -----------------------------------------------------------------------

    #[test]
    fn import_specifier_named_local_name() {
        let spec = IrImportSpecifier::Named {
            name: "foo".into(),
            alias: None,
        };
        assert_eq!(spec.local_name(), Some("foo"));
        assert_eq!(spec.remote_name(), Some("foo"));
    }

    #[test]
    fn import_specifier_named_with_alias() {
        let spec = IrImportSpecifier::Named {
            name: "foo".into(),
            alias: Some("bar".into()),
        };
        assert_eq!(spec.local_name(), Some("bar"));
        assert_eq!(spec.remote_name(), Some("foo"));
    }

    #[test]
    fn import_specifier_default() {
        let spec = IrImportSpecifier::Default("React".into());
        assert_eq!(spec.local_name(), Some("React"));
        assert_eq!(spec.remote_name(), Some("default"));
    }

    #[test]
    fn import_specifier_namespace() {
        let spec = IrImportSpecifier::Namespace("ns".into());
        assert_eq!(spec.local_name(), Some("ns"));
        assert_eq!(spec.remote_name(), None);
    }

    #[test]
    fn import_specifier_side_effect() {
        let spec = IrImportSpecifier::SideEffect;
        assert_eq!(spec.local_name(), None);
        assert_eq!(spec.remote_name(), None);
    }

    // -----------------------------------------------------------------------
    // IrFile construction and accessors
    // -----------------------------------------------------------------------

    #[test]
    fn ir_file_empty() {
        let f = IrFile::empty("src/main.ts");
        assert_eq!(f.path, "src/main.ts");
        assert_eq!(f.language, Language::TypeScript);
        assert!(f.functions.is_empty());
        assert!(f.type_defs.is_empty());
        assert!(f.constants.is_empty());
        assert!(f.imports.is_empty());
        assert!(f.exports.is_empty());
        assert!(f.call_expressions.is_empty());
        assert!(f.assignments.is_empty());
    }

    #[test]
    fn ir_file_empty_python() {
        let f = IrFile::empty("app.py");
        assert_eq!(f.language, Language::Python);
    }

    #[test]
    fn ir_file_empty_unknown() {
        let f = IrFile::empty("Makefile");
        assert_eq!(f.language, Language::Unknown);
    }

    #[test]
    fn ir_file_all_definition_names() {
        let f = IrFile {
            path: "test.ts".into(),
            language: Language::TypeScript,
            functions: vec![IrFunctionDef {
                name: "doStuff".into(),
                kind: FunctionKind::Function,
                span: Span::new(1, 5),
                parameters: vec![],
                is_async: false,
                is_exported: false,
                decorators: vec![],
            }],
            type_defs: vec![IrTypeDef {
                name: "MyClass".into(),
                kind: TypeDefKind::Class,
                span: Span::new(10, 20),
                bases: vec![],
                is_exported: false,
                decorators: vec![],
            }],
            constants: vec![IrConstant {
                name: "MAX_SIZE".into(),
                span: Span::single(30),
                is_exported: false,
            }],
            imports: vec![],
            exports: vec![],
            call_expressions: vec![],
            assignments: vec![],
        };
        let names = f.all_definition_names();
        assert_eq!(names, vec!["doStuff", "MyClass", "MAX_SIZE"]);
    }

    #[test]
    fn ir_file_exported_names() {
        let f = IrFile {
            exports: vec![
                IrExport {
                    name: "foo".into(),
                    is_default: false,
                    is_reexport: false,
                    source: None,
                    span: Span::single(1),
                },
                IrExport {
                    name: "Bar".into(),
                    is_default: true,
                    is_reexport: false,
                    source: None,
                    span: Span::single(2),
                },
            ],
            ..IrFile::empty("test.ts")
        };
        assert_eq!(f.exported_names(), vec!["foo", "Bar"]);
    }

    #[test]
    fn ir_file_import_sources() {
        let f = IrFile {
            imports: vec![
                IrImport {
                    source: "./utils".into(),
                    specifiers: vec![],
                    span: Span::single(1),
                },
                IrImport {
                    source: "express".into(),
                    specifiers: vec![],
                    span: Span::single(2),
                },
            ],
            ..IrFile::empty("test.ts")
        };
        assert_eq!(f.import_sources(), vec!["./utils", "express"]);
    }

    // -----------------------------------------------------------------------
    // Conversion: ParsedFile → IrFile
    // -----------------------------------------------------------------------

    fn make_parsed_file() -> ParsedFile {
        ParsedFile {
            path: "src/handler.ts".into(),
            language: Language::TypeScript,
            definitions: vec![
                Definition {
                    name: "handleRequest".into(),
                    kind: SymbolKind::Function,
                    start_line: 5,
                    end_line: 20,
                },
                Definition {
                    name: "UserService".into(),
                    kind: SymbolKind::Class,
                    start_line: 25,
                    end_line: 50,
                },
                Definition {
                    name: "Config".into(),
                    kind: SymbolKind::Interface,
                    start_line: 52,
                    end_line: 55,
                },
                Definition {
                    name: "MAX_RETRIES".into(),
                    kind: SymbolKind::Constant,
                    start_line: 1,
                    end_line: 1,
                },
            ],
            imports: vec![
                ImportInfo {
                    source: "express".into(),
                    names: vec![ImportedName {
                        name: "Router".into(),
                        alias: None,
                    }],
                    is_default: false,
                    is_namespace: false,
                    line: 1,
                },
                ImportInfo {
                    source: "react".into(),
                    names: vec![ImportedName {
                        name: "React".into(),
                        alias: None,
                    }],
                    is_default: true,
                    is_namespace: false,
                    line: 2,
                },
            ],
            exports: vec![ExportInfo {
                name: "handleRequest".into(),
                is_default: false,
                is_reexport: false,
                source: None,
                line: 5,
            }],
            call_sites: vec![
                CallSite {
                    callee: "db.query".into(),
                    line: 15,
                    containing_function: Some("handleRequest".into()),
                },
                CallSite {
                    callee: "validate".into(),
                    line: 10,
                    containing_function: Some("handleRequest".into()),
                },
            ],
        }
    }

    #[test]
    fn from_parsed_file_preserves_path_and_language() {
        let parsed = make_parsed_file();
        let ir = IrFile::from_parsed_file(&parsed);
        assert_eq!(ir.path, "src/handler.ts");
        assert_eq!(ir.language, Language::TypeScript);
    }

    #[test]
    fn from_parsed_file_extracts_functions() {
        let parsed = make_parsed_file();
        let ir = IrFile::from_parsed_file(&parsed);
        assert_eq!(ir.functions.len(), 1);
        assert_eq!(ir.functions[0].name, "handleRequest");
        assert_eq!(ir.functions[0].span, Span::new(5, 20));
    }

    #[test]
    fn from_parsed_file_extracts_type_defs() {
        let parsed = make_parsed_file();
        let ir = IrFile::from_parsed_file(&parsed);
        assert_eq!(ir.type_defs.len(), 2);
        assert_eq!(ir.type_defs[0].name, "UserService");
        assert_eq!(ir.type_defs[0].kind, TypeDefKind::Class);
        assert_eq!(ir.type_defs[1].name, "Config");
        assert_eq!(ir.type_defs[1].kind, TypeDefKind::Interface);
    }

    #[test]
    fn from_parsed_file_extracts_constants() {
        let parsed = make_parsed_file();
        let ir = IrFile::from_parsed_file(&parsed);
        assert_eq!(ir.constants.len(), 1);
        assert_eq!(ir.constants[0].name, "MAX_RETRIES");
    }

    #[test]
    fn from_parsed_file_converts_imports() {
        let parsed = make_parsed_file();
        let ir = IrFile::from_parsed_file(&parsed);
        assert_eq!(ir.imports.len(), 2);

        // Named import
        assert_eq!(ir.imports[0].source, "express");
        assert_eq!(ir.imports[0].specifiers.len(), 1);
        assert!(matches!(
            &ir.imports[0].specifiers[0],
            IrImportSpecifier::Named { name, alias } if name == "Router" && alias.is_none()
        ));

        // Default import
        assert_eq!(ir.imports[1].source, "react");
        assert!(matches!(
            &ir.imports[1].specifiers[0],
            IrImportSpecifier::Default(name) if name == "React"
        ));
    }

    #[test]
    fn from_parsed_file_converts_exports() {
        let parsed = make_parsed_file();
        let ir = IrFile::from_parsed_file(&parsed);
        assert_eq!(ir.exports.len(), 1);
        assert_eq!(ir.exports[0].name, "handleRequest");
        assert!(!ir.exports[0].is_default);
    }

    #[test]
    fn from_parsed_file_converts_call_expressions() {
        let parsed = make_parsed_file();
        let ir = IrFile::from_parsed_file(&parsed);
        assert_eq!(ir.call_expressions.len(), 2);
        assert_eq!(ir.call_expressions[0].callee, "db.query");
        assert_eq!(ir.call_expressions[0].span.start_line, 15);
        assert_eq!(
            ir.call_expressions[0].containing_function,
            Some("handleRequest".into())
        );
    }

    #[test]
    fn from_parsed_file_no_assignments_without_enrichment() {
        let parsed = make_parsed_file();
        let ir = IrFile::from_parsed_file(&parsed);
        assert!(ir.assignments.is_empty());
    }

    // -----------------------------------------------------------------------
    // Conversion: namespace and aliased imports
    // -----------------------------------------------------------------------

    #[test]
    fn from_parsed_file_namespace_import() {
        let parsed = ParsedFile {
            path: "test.ts".into(),
            language: Language::TypeScript,
            definitions: vec![],
            imports: vec![ImportInfo {
                source: "lodash".into(),
                names: vec![ImportedName {
                    name: "_".into(),
                    alias: None,
                }],
                is_default: false,
                is_namespace: true,
                line: 1,
            }],
            exports: vec![],
            call_sites: vec![],
        };
        let ir = IrFile::from_parsed_file(&parsed);
        assert!(matches!(
            &ir.imports[0].specifiers[0],
            IrImportSpecifier::Namespace(name) if name == "_"
        ));
    }

    #[test]
    fn from_parsed_file_side_effect_import() {
        let parsed = ParsedFile {
            path: "test.ts".into(),
            language: Language::TypeScript,
            definitions: vec![],
            imports: vec![ImportInfo {
                source: "reflect-metadata".into(),
                names: vec![],
                is_default: false,
                is_namespace: false,
                line: 1,
            }],
            exports: vec![],
            call_sites: vec![],
        };
        let ir = IrFile::from_parsed_file(&parsed);
        assert!(matches!(
            &ir.imports[0].specifiers[0],
            IrImportSpecifier::SideEffect
        ));
    }

    #[test]
    fn from_parsed_file_aliased_import() {
        let parsed = ParsedFile {
            path: "test.ts".into(),
            language: Language::TypeScript,
            definitions: vec![],
            imports: vec![ImportInfo {
                source: "./utils".into(),
                names: vec![ImportedName {
                    name: "helper".into(),
                    alias: Some("myHelper".into()),
                }],
                is_default: false,
                is_namespace: false,
                line: 1,
            }],
            exports: vec![],
            call_sites: vec![],
        };
        let ir = IrFile::from_parsed_file(&parsed);
        match &ir.imports[0].specifiers[0] {
            IrImportSpecifier::Named { name, alias } => {
                assert_eq!(name, "helper");
                assert_eq!(alias.as_deref(), Some("myHelper"));
            }
            other => panic!("expected Named, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Enrich with DataFlowInfo
    // -----------------------------------------------------------------------

    #[test]
    fn enrich_adds_assignments() {
        let parsed = make_parsed_file();
        let mut ir = IrFile::from_parsed_file(&parsed);

        let data_flow = DataFlowInfo {
            assignments: vec![VarCallAssignment {
                variable: "result".into(),
                callee: "db.query".into(),
                line: 15,
                containing_function: Some("handleRequest".into()),
            }],
            calls_with_args: vec![],
        };

        ir.enrich_with_data_flow(&data_flow);
        assert_eq!(ir.assignments.len(), 1);
        assert_eq!(
            ir.assignments[0].pattern,
            IrPattern::Identifier("result".into())
        );
        assert_eq!(ir.assignments[0].value.callee_name(), Some("db.query"));
    }

    #[test]
    fn enrich_enriches_existing_call_expressions() {
        let parsed = make_parsed_file();
        let mut ir = IrFile::from_parsed_file(&parsed);

        let data_flow = DataFlowInfo {
            assignments: vec![],
            calls_with_args: vec![CallWithArgs {
                callee: "validate".into(),
                arguments: vec!["input".into(), "schema".into()],
                line: 10,
                containing_function: Some("handleRequest".into()),
            }],
        };

        ir.enrich_with_data_flow(&data_flow);
        // Should find the existing call and enrich it (not add a duplicate)
        let validate_calls: Vec<_> = ir
            .call_expressions
            .iter()
            .filter(|c| c.callee == "validate")
            .collect();
        assert_eq!(validate_calls.len(), 1);
        assert_eq!(validate_calls[0].arguments, vec!["input", "schema"]);
    }

    #[test]
    fn enrich_adds_new_call_when_no_match() {
        let mut ir = IrFile::empty("test.ts");
        let data_flow = DataFlowInfo {
            assignments: vec![],
            calls_with_args: vec![CallWithArgs {
                callee: "newFunc".into(),
                arguments: vec!["arg1".into()],
                line: 42,
                containing_function: None,
            }],
        };

        ir.enrich_with_data_flow(&data_flow);
        assert_eq!(ir.call_expressions.len(), 1);
        assert_eq!(ir.call_expressions[0].callee, "newFunc");
        assert_eq!(ir.call_expressions[0].arguments, vec!["arg1"]);
    }

    // -----------------------------------------------------------------------
    // Roundtrip: ParsedFile → IrFile → ParsedFile
    // -----------------------------------------------------------------------

    #[test]
    fn roundtrip_parsed_file_preserves_path_and_language() {
        let original = make_parsed_file();
        let ir = IrFile::from_parsed_file(&original);
        let roundtripped = ir.to_parsed_file();
        assert_eq!(roundtripped.path, original.path);
        assert_eq!(roundtripped.language, original.language);
    }

    #[test]
    fn roundtrip_parsed_file_preserves_definitions() {
        let original = make_parsed_file();
        let ir = IrFile::from_parsed_file(&original);
        let roundtripped = ir.to_parsed_file();
        // Definitions may be reordered (functions, type_defs, constants grouped separately)
        assert_eq!(roundtripped.definitions.len(), original.definitions.len());
        for orig_def in &original.definitions {
            let found = roundtripped
                .definitions
                .iter()
                .find(|d| d.name == orig_def.name);
            assert!(
                found.is_some(),
                "definition {} not found in roundtrip",
                orig_def.name
            );
            let found = found.unwrap();
            assert_eq!(found.kind, orig_def.kind);
            assert_eq!(found.start_line, orig_def.start_line);
            assert_eq!(found.end_line, orig_def.end_line);
        }
    }

    #[test]
    fn roundtrip_parsed_file_preserves_imports() {
        let original = make_parsed_file();
        let ir = IrFile::from_parsed_file(&original);
        let roundtripped = ir.to_parsed_file();
        assert_eq!(roundtripped.imports.len(), original.imports.len());
        for (orig, rt) in original.imports.iter().zip(roundtripped.imports.iter()) {
            assert_eq!(rt.source, orig.source);
            assert_eq!(rt.is_default, orig.is_default);
            assert_eq!(rt.is_namespace, orig.is_namespace);
            assert_eq!(rt.line, orig.line);
            assert_eq!(rt.names.len(), orig.names.len());
        }
    }

    #[test]
    fn roundtrip_parsed_file_preserves_exports() {
        let original = make_parsed_file();
        let ir = IrFile::from_parsed_file(&original);
        let roundtripped = ir.to_parsed_file();
        assert_eq!(roundtripped.exports.len(), original.exports.len());
        for (orig, rt) in original.exports.iter().zip(roundtripped.exports.iter()) {
            assert_eq!(rt.name, orig.name);
            assert_eq!(rt.is_default, orig.is_default);
            assert_eq!(rt.is_reexport, orig.is_reexport);
            assert_eq!(rt.source, orig.source);
            assert_eq!(rt.line, orig.line);
        }
    }

    #[test]
    fn roundtrip_parsed_file_preserves_call_sites() {
        let original = make_parsed_file();
        let ir = IrFile::from_parsed_file(&original);
        let roundtripped = ir.to_parsed_file();
        assert_eq!(roundtripped.call_sites.len(), original.call_sites.len());
        for (orig, rt) in original.call_sites.iter().zip(roundtripped.call_sites.iter()) {
            assert_eq!(rt.callee, orig.callee);
            assert_eq!(rt.line, orig.line);
            assert_eq!(rt.containing_function, orig.containing_function);
        }
    }

    // -----------------------------------------------------------------------
    // Roundtrip: DataFlowInfo enrichment
    // -----------------------------------------------------------------------

    #[test]
    fn roundtrip_data_flow_info() {
        let parsed = make_parsed_file();
        let mut ir = IrFile::from_parsed_file(&parsed);

        let original_df = DataFlowInfo {
            assignments: vec![VarCallAssignment {
                variable: "result".into(),
                callee: "fetchData".into(),
                line: 15,
                containing_function: Some("handleRequest".into()),
            }],
            calls_with_args: vec![CallWithArgs {
                callee: "validate".into(),
                arguments: vec!["input".into()],
                line: 10,
                containing_function: Some("handleRequest".into()),
            }],
        };

        ir.enrich_with_data_flow(&original_df);
        let roundtripped = ir.to_data_flow_info();

        assert_eq!(roundtripped.assignments.len(), 1);
        assert_eq!(roundtripped.assignments[0].variable, "result");
        assert_eq!(roundtripped.assignments[0].callee, "fetchData");
        assert_eq!(roundtripped.assignments[0].line, 15);
    }

    #[test]
    fn roundtrip_data_flow_info_destructured_assignment_excluded() {
        // Destructured assignments can't roundtrip to VarCallAssignment (which only supports
        // simple identifiers), so they should be excluded from the DataFlowInfo conversion.
        let mut ir = IrFile::empty("test.ts");
        ir.assignments.push(IrAssignment {
            pattern: IrPattern::ObjectDestructure {
                properties: vec![DestructureProperty {
                    key: "a".into(),
                    value: IrPattern::Identifier("a".into()),
                    default_value: None,
                }],
                rest: None,
            },
            value: IrExpression::Call(IrCallExpression {
                callee: "foo".into(),
                arguments: vec![],
                span: Span::single(1),
                containing_function: None,
            }),
            span: Span::single(1),
            containing_function: None,
        });

        let df = ir.to_data_flow_info();
        // Destructured assignment should NOT appear in VarCallAssignment
        assert!(df.assignments.is_empty());
    }

    // -----------------------------------------------------------------------
    // Serialization roundtrip
    // -----------------------------------------------------------------------

    #[test]
    fn serde_roundtrip_ir_file() {
        let parsed = make_parsed_file();
        let mut ir = IrFile::from_parsed_file(&parsed);
        ir.assignments.push(IrAssignment {
            pattern: IrPattern::ObjectDestructure {
                properties: vec![DestructureProperty {
                    key: "data".into(),
                    value: IrPattern::Identifier("data".into()),
                    default_value: None,
                }],
                rest: Some("extra".into()),
            },
            value: IrExpression::Await(Box::new(IrExpression::Call(IrCallExpression {
                callee: "fetchData".into(),
                arguments: vec!["url".into()],
                span: Span::single(42),
                containing_function: Some("handler".into()),
            }))),
            span: Span::single(42),
            containing_function: Some("handler".into()),
        });

        let json = serde_json::to_string(&ir).expect("serialize");
        let deserialized: IrFile = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ir, deserialized);
    }

    #[test]
    fn serde_roundtrip_ir_pattern_all_variants() {
        let patterns = vec![
            IrPattern::Identifier("x".into()),
            IrPattern::ObjectDestructure {
                properties: vec![DestructureProperty {
                    key: "a".into(),
                    value: IrPattern::Identifier("a".into()),
                    default_value: Some("0".into()),
                }],
                rest: Some("rest".into()),
            },
            IrPattern::ArrayDestructure {
                elements: vec![
                    Some(IrPattern::Identifier("first".into())),
                    None,
                    Some(IrPattern::Identifier("third".into())),
                ],
                rest: Some("rest".into()),
            },
            IrPattern::TupleDestructure {
                elements: vec![
                    IrPattern::Identifier("a".into()),
                    IrPattern::Identifier("b".into()),
                ],
            },
        ];

        for pat in &patterns {
            let json = serde_json::to_string(pat).expect("serialize");
            let deserialized: IrPattern = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(pat, &deserialized, "pattern roundtrip failed for {pat:?}");
        }
    }

    #[test]
    fn serde_roundtrip_ir_expression_all_variants() {
        let exprs = vec![
            IrExpression::Call(IrCallExpression {
                callee: "foo".into(),
                arguments: vec!["a".into(), "b".into()],
                span: Span::single(1),
                containing_function: None,
            }),
            IrExpression::Identifier("x".into()),
            IrExpression::Await(Box::new(IrExpression::Call(IrCallExpression {
                callee: "fetch".into(),
                arguments: vec![],
                span: Span::single(2),
                containing_function: None,
            }))),
            IrExpression::Yield(Box::new(IrExpression::Identifier("effect".into()))),
            IrExpression::MemberAccess {
                object: Box::new(IrExpression::Identifier("obj".into())),
                property: "prop".into(),
            },
            IrExpression::Other("1 + 2".into()),
        ];

        for expr in &exprs {
            let json = serde_json::to_string(expr).expect("serialize");
            let deserialized: IrExpression = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(expr, &deserialized, "expression roundtrip failed for {expr:?}");
        }
    }

    // -----------------------------------------------------------------------
    // Edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn empty_parsed_file_converts_to_empty_ir() {
        let parsed = ParsedFile {
            path: "empty.ts".into(),
            language: Language::TypeScript,
            definitions: vec![],
            imports: vec![],
            exports: vec![],
            call_sites: vec![],
        };
        let ir = IrFile::from_parsed_file(&parsed);
        assert!(ir.functions.is_empty());
        assert!(ir.type_defs.is_empty());
        assert!(ir.constants.is_empty());
        assert!(ir.imports.is_empty());
        assert!(ir.exports.is_empty());
        assert!(ir.call_expressions.is_empty());
        assert!(ir.assignments.is_empty());
    }

    #[test]
    fn unknown_language_produces_empty_ir() {
        let ir = IrFile::empty("Makefile");
        assert_eq!(ir.language, Language::Unknown);
        assert!(ir.all_definition_names().is_empty());
    }

    #[test]
    fn module_definition_excluded_from_functions_and_type_defs() {
        let parsed = ParsedFile {
            path: "test.ts".into(),
            language: Language::TypeScript,
            definitions: vec![Definition {
                name: "myModule".into(),
                kind: SymbolKind::Module,
                start_line: 1,
                end_line: 50,
            }],
            imports: vec![],
            exports: vec![],
            call_sites: vec![],
        };
        let ir = IrFile::from_parsed_file(&parsed);
        // Module doesn't map to function, type_def, or constant
        assert!(ir.functions.is_empty());
        assert!(ir.type_defs.is_empty());
        assert!(ir.constants.is_empty());
    }

    #[test]
    fn struct_definition_maps_to_type_def() {
        let parsed = ParsedFile {
            path: "test.rs".into(),
            language: Language::Rust,
            definitions: vec![Definition {
                name: "Config".into(),
                kind: SymbolKind::Struct,
                start_line: 1,
                end_line: 10,
            }],
            imports: vec![],
            exports: vec![],
            call_sites: vec![],
        };
        let ir = IrFile::from_parsed_file(&parsed);
        assert_eq!(ir.type_defs.len(), 1);
        assert_eq!(ir.type_defs[0].kind, TypeDefKind::Struct);
    }

    #[test]
    fn type_alias_definition_maps_to_type_def() {
        let parsed = ParsedFile {
            path: "test.ts".into(),
            language: Language::TypeScript,
            definitions: vec![Definition {
                name: "UserId".into(),
                kind: SymbolKind::TypeAlias,
                start_line: 1,
                end_line: 1,
            }],
            imports: vec![],
            exports: vec![],
            call_sites: vec![],
        };
        let ir = IrFile::from_parsed_file(&parsed);
        assert_eq!(ir.type_defs.len(), 1);
        assert_eq!(ir.type_defs[0].kind, TypeDefKind::TypeAlias);
    }

    #[test]
    fn multiple_named_imports_from_same_source() {
        let parsed = ParsedFile {
            path: "test.ts".into(),
            language: Language::TypeScript,
            definitions: vec![],
            imports: vec![ImportInfo {
                source: "express".into(),
                names: vec![
                    ImportedName {
                        name: "Router".into(),
                        alias: None,
                    },
                    ImportedName {
                        name: "Request".into(),
                        alias: None,
                    },
                    ImportedName {
                        name: "Response".into(),
                        alias: Some("Res".into()),
                    },
                ],
                is_default: false,
                is_namespace: false,
                line: 1,
            }],
            exports: vec![],
            call_sites: vec![],
        };
        let ir = IrFile::from_parsed_file(&parsed);
        assert_eq!(ir.imports[0].specifiers.len(), 3);
        match &ir.imports[0].specifiers[2] {
            IrImportSpecifier::Named { name, alias } => {
                assert_eq!(name, "Response");
                assert_eq!(alias.as_deref(), Some("Res"));
            }
            other => panic!("expected Named, got {other:?}"),
        }
    }

    #[test]
    fn reexport_conversion() {
        let parsed = ParsedFile {
            path: "index.ts".into(),
            language: Language::TypeScript,
            definitions: vec![],
            imports: vec![],
            exports: vec![ExportInfo {
                name: "helper".into(),
                is_default: false,
                is_reexport: true,
                source: Some("./utils".into()),
                line: 1,
            }],
            call_sites: vec![],
        };
        let ir = IrFile::from_parsed_file(&parsed);
        assert!(ir.exports[0].is_reexport);
        assert_eq!(ir.exports[0].source, Some("./utils".into()));
    }

    // -----------------------------------------------------------------------
    // IrAssignment construction patterns
    // -----------------------------------------------------------------------

    #[test]
    fn assignment_simple_const_from_call() {
        // const x = foo()
        let assign = IrAssignment {
            pattern: IrPattern::Identifier("x".into()),
            value: IrExpression::Call(IrCallExpression {
                callee: "foo".into(),
                arguments: vec![],
                span: Span::single(1),
                containing_function: None,
            }),
            span: Span::single(1),
            containing_function: None,
        };
        assert_eq!(assign.pattern.bound_names(), vec!["x"]);
        assert_eq!(assign.value.callee_name(), Some("foo"));
    }

    #[test]
    fn assignment_object_destructure_from_call() {
        // const { a, b } = foo()
        let assign = IrAssignment {
            pattern: IrPattern::ObjectDestructure {
                properties: vec![
                    DestructureProperty {
                        key: "a".into(),
                        value: IrPattern::Identifier("a".into()),
                        default_value: None,
                    },
                    DestructureProperty {
                        key: "b".into(),
                        value: IrPattern::Identifier("b".into()),
                        default_value: None,
                    },
                ],
                rest: None,
            },
            value: IrExpression::Call(IrCallExpression {
                callee: "foo".into(),
                arguments: vec![],
                span: Span::single(1),
                containing_function: None,
            }),
            span: Span::single(1),
            containing_function: None,
        };
        assert_eq!(assign.pattern.bound_names(), vec!["a", "b"]);
    }

    #[test]
    fn assignment_array_destructure_from_call() {
        // const [first, ...rest] = bar()
        let assign = IrAssignment {
            pattern: IrPattern::ArrayDestructure {
                elements: vec![Some(IrPattern::Identifier("first".into()))],
                rest: Some("rest".into()),
            },
            value: IrExpression::Call(IrCallExpression {
                callee: "bar".into(),
                arguments: vec![],
                span: Span::single(1),
                containing_function: None,
            }),
            span: Span::single(1),
            containing_function: None,
        };
        assert_eq!(assign.pattern.bound_names(), vec!["first", "rest"]);
    }

    #[test]
    fn assignment_python_tuple_unpack() {
        // a, b = func()
        let assign = IrAssignment {
            pattern: IrPattern::TupleDestructure {
                elements: vec![
                    IrPattern::Identifier("a".into()),
                    IrPattern::Identifier("b".into()),
                ],
            },
            value: IrExpression::Call(IrCallExpression {
                callee: "func".into(),
                arguments: vec![],
                span: Span::single(1),
                containing_function: None,
            }),
            span: Span::single(1),
            containing_function: None,
        };
        assert_eq!(assign.pattern.bound_names(), vec!["a", "b"]);
    }

    #[test]
    fn assignment_effect_ts_yield_destructure() {
        // const { svc } = yield* _(Tag)
        let assign = IrAssignment {
            pattern: IrPattern::ObjectDestructure {
                properties: vec![DestructureProperty {
                    key: "svc".into(),
                    value: IrPattern::Identifier("svc".into()),
                    default_value: None,
                }],
                rest: None,
            },
            value: IrExpression::Yield(Box::new(IrExpression::Call(IrCallExpression {
                callee: "_(Tag)".into(),
                arguments: vec![],
                span: Span::single(1),
                containing_function: None,
            }))),
            span: Span::single(1),
            containing_function: None,
        };
        assert_eq!(assign.pattern.bound_names(), vec!["svc"]);
        assert_eq!(assign.value.callee_name(), Some("_(Tag)"));
    }

    #[test]
    fn assignment_await_expression() {
        // const data = await fetch(url)
        let assign = IrAssignment {
            pattern: IrPattern::Identifier("data".into()),
            value: IrExpression::Await(Box::new(IrExpression::Call(IrCallExpression {
                callee: "fetch".into(),
                arguments: vec!["url".into()],
                span: Span::single(5),
                containing_function: Some("handler".into()),
            }))),
            span: Span::single(5),
            containing_function: Some("handler".into()),
        };
        assert_eq!(assign.value.callee_name(), Some("fetch"));
    }

    // -----------------------------------------------------------------------
    // IrFunctionDef with parameters
    // -----------------------------------------------------------------------

    #[test]
    fn function_def_with_simple_params() {
        let func = IrFunctionDef {
            name: "add".into(),
            kind: FunctionKind::Function,
            span: Span::new(1, 3),
            parameters: vec![
                IrParameter {
                    pattern: IrPattern::Identifier("a".into()),
                    type_annotation: Some("number".into()),
                    default_value: None,
                },
                IrParameter {
                    pattern: IrPattern::Identifier("b".into()),
                    type_annotation: Some("number".into()),
                    default_value: Some("0".into()),
                },
            ],
            is_async: false,
            is_exported: true,
            decorators: vec![],
        };
        let param_names: Vec<_> = func
            .parameters
            .iter()
            .flat_map(|p| p.pattern.bound_names())
            .collect();
        assert_eq!(param_names, vec!["a", "b"]);
    }

    #[test]
    fn function_def_with_destructured_param() {
        // function handler({ req, res }: Context) { ... }
        let func = IrFunctionDef {
            name: "handler".into(),
            kind: FunctionKind::Function,
            span: Span::new(1, 10),
            parameters: vec![IrParameter {
                pattern: IrPattern::ObjectDestructure {
                    properties: vec![
                        DestructureProperty {
                            key: "req".into(),
                            value: IrPattern::Identifier("req".into()),
                            default_value: None,
                        },
                        DestructureProperty {
                            key: "res".into(),
                            value: IrPattern::Identifier("res".into()),
                            default_value: None,
                        },
                    ],
                    rest: None,
                },
                type_annotation: Some("Context".into()),
                default_value: None,
            }],
            is_async: true,
            is_exported: false,
            decorators: vec!["route".into()],
        };
        let param_names: Vec<_> = func
            .parameters
            .iter()
            .flat_map(|p| p.pattern.bound_names())
            .collect();
        assert_eq!(param_names, vec!["req", "res"]);
    }

    // -----------------------------------------------------------------------
    // IrTypeDef tests
    // -----------------------------------------------------------------------

    #[test]
    fn type_def_with_bases() {
        let td = IrTypeDef {
            name: "AdminUser".into(),
            kind: TypeDefKind::Class,
            span: Span::new(1, 20),
            bases: vec!["User".into(), "Serializable".into()],
            is_exported: true,
            decorators: vec!["Entity".into()],
        };
        assert_eq!(td.bases.len(), 2);
        assert_eq!(td.decorators, vec!["Entity"]);
    }

    // -----------------------------------------------------------------------
    // Backward compatibility: IrFile → ParsedFile → IrFile roundtrip
    // -----------------------------------------------------------------------

    #[test]
    fn double_roundtrip_stability() {
        let original = make_parsed_file();
        let ir1 = IrFile::from_parsed_file(&original);
        let parsed1 = ir1.to_parsed_file();
        let ir2 = IrFile::from_parsed_file(&parsed1);
        let parsed2 = ir2.to_parsed_file();

        // After double roundtrip, definitions should be stable
        assert_eq!(parsed1.definitions.len(), parsed2.definitions.len());
        assert_eq!(parsed1.imports.len(), parsed2.imports.len());
        assert_eq!(parsed1.exports.len(), parsed2.exports.len());
        assert_eq!(parsed1.call_sites.len(), parsed2.call_sites.len());
    }
}

// ===========================================================================
// Property-based tests
// ===========================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    // -----------------------------------------------------------------------
    // Strategies
    // -----------------------------------------------------------------------

    fn identifier_strategy() -> impl Strategy<Value = String> {
        "[a-z][a-zA-Z0-9_]{0,15}".prop_map(|s| s)
    }

    fn pattern_strategy() -> impl Strategy<Value = IrPattern> {
        let leaf = identifier_strategy().prop_map(IrPattern::Identifier);

        leaf.prop_recursive(
            3,  // depth
            10, // max nodes
            3,  // items per collection
            |inner| {
                prop_oneof![
                    // Object destructure
                    (
                        prop::collection::vec(
                            (identifier_strategy(), inner.clone(), proptest::option::of(identifier_strategy())).prop_map(
                                |(key, value, default_value)| DestructureProperty {
                                    key,
                                    value,
                                    default_value,
                                }
                            ),
                            0..=3,
                        ),
                        proptest::option::of(identifier_strategy()),
                    )
                        .prop_map(|(properties, rest)| IrPattern::ObjectDestructure {
                            properties,
                            rest,
                        }),
                    // Array destructure
                    (
                        prop::collection::vec(proptest::option::of(inner.clone()), 0..=3),
                        proptest::option::of(identifier_strategy()),
                    )
                        .prop_map(|(elements, rest)| IrPattern::ArrayDestructure {
                            elements,
                            rest,
                        }),
                    // Tuple destructure
                    prop::collection::vec(inner, 1..=3)
                        .prop_map(|elements| IrPattern::TupleDestructure { elements }),
                ]
            },
        )
    }

    fn span_strategy() -> impl Strategy<Value = Span> {
        (1..1000usize, 0..100usize).prop_map(|(start, delta)| Span::new(start, start + delta))
    }

    fn language_strategy() -> impl Strategy<Value = Language> {
        prop_oneof![
            Just(Language::TypeScript),
            Just(Language::JavaScript),
            Just(Language::Python),
            Just(Language::Go),
            Just(Language::Rust),
            Just(Language::Java),
            Just(Language::CSharp),
            Just(Language::Php),
            Just(Language::Unknown),
        ]
    }

    fn ir_file_strategy() -> impl Strategy<Value = IrFile> {
        (
            identifier_strategy(),
            language_strategy(),
            prop::collection::vec(
                (identifier_strategy(), span_strategy()).prop_map(|(name, span)| IrFunctionDef {
                    name,
                    kind: FunctionKind::Function,
                    span,
                    parameters: vec![],
                    is_async: false,
                    is_exported: false,
                    decorators: vec![],
                }),
                0..5,
            ),
            prop::collection::vec(
                (identifier_strategy(), span_strategy()).prop_map(|(name, span)| IrTypeDef {
                    name,
                    kind: TypeDefKind::Class,
                    span,
                    bases: vec![],
                    is_exported: false,
                    decorators: vec![],
                }),
                0..3,
            ),
            prop::collection::vec(
                (identifier_strategy(), span_strategy()).prop_map(|(name, span)| IrConstant {
                    name,
                    span,
                    is_exported: false,
                }),
                0..3,
            ),
        )
            .prop_map(|(path, language, functions, type_defs, constants)| {
                let ext = match language {
                    Language::TypeScript => ".ts",
                    Language::JavaScript => ".js",
                    Language::Python => ".py",
                    Language::Go => ".go",
                    Language::Rust => ".rs",
                    Language::Java => ".java",
                    Language::CSharp => ".cs",
                    Language::Php => ".php",
                    Language::Unknown => "",
                };
                IrFile {
                    path: format!("src/{path}{ext}"),
                    language,
                    functions,
                    type_defs,
                    constants,
                    imports: vec![],
                    exports: vec![],
                    call_expressions: vec![],
                    assignments: vec![],
                }
            })
    }

    // -----------------------------------------------------------------------
    // Properties
    // -----------------------------------------------------------------------

    proptest! {
        /// bound_names never panics for any pattern shape.
        #[test]
        fn pattern_bound_names_never_panics(pat in pattern_strategy()) {
            let _ = pat.bound_names();
        }

        /// bound_names returns at least 1 name for identifier patterns.
        #[test]
        fn identifier_pattern_has_one_bound_name(name in identifier_strategy()) {
            let pat = IrPattern::Identifier(name.clone());
            let names = pat.bound_names();
            prop_assert_eq!(names.len(), 1);
            prop_assert_eq!(&names[0], &name);
        }

        /// is_identifier is true only for Identifier variant.
        #[test]
        fn is_identifier_correct(pat in pattern_strategy()) {
            let expected = matches!(pat, IrPattern::Identifier(_));
            prop_assert_eq!(pat.is_identifier(), expected);
        }

        /// Span line_count is always >= 1.
        #[test]
        fn span_line_count_at_least_one(span in span_strategy()) {
            prop_assert!(span.line_count() >= 1);
        }

        /// IrFile serialization roundtrip is lossless.
        #[test]
        fn ir_file_serde_roundtrip(ir in ir_file_strategy()) {
            let json = serde_json::to_string(&ir).expect("serialize");
            let deserialized: IrFile = serde_json::from_str(&json).expect("deserialize");
            prop_assert_eq!(&ir, &deserialized);
        }

        /// ParsedFile → IrFile → ParsedFile preserves definition count.
        #[test]
        fn parsed_to_ir_preserves_definition_count(ir in ir_file_strategy()) {
            let parsed = ir.to_parsed_file();
            let ir2 = IrFile::from_parsed_file(&parsed);
            let total_defs = ir.functions.len() + ir.type_defs.len() + ir.constants.len();
            let total_defs2 = ir2.functions.len() + ir2.type_defs.len() + ir2.constants.len();
            prop_assert_eq!(total_defs, total_defs2);
        }

        /// Pattern serialization roundtrip is lossless.
        #[test]
        fn pattern_serde_roundtrip(pat in pattern_strategy()) {
            let json = serde_json::to_string(&pat).expect("serialize");
            let deserialized: IrPattern = serde_json::from_str(&json).expect("deserialize");
            prop_assert_eq!(&pat, &deserialized);
        }

        /// all_definition_names count equals functions + type_defs + constants.
        #[test]
        fn all_definition_names_count(ir in ir_file_strategy()) {
            let expected = ir.functions.len() + ir.type_defs.len() + ir.constants.len();
            prop_assert_eq!(ir.all_definition_names().len(), expected);
        }

        /// Empty IrFile has no definitions.
        #[test]
        fn empty_ir_file_no_definitions(path in identifier_strategy()) {
            let ir = IrFile::empty(&path);
            prop_assert!(ir.all_definition_names().is_empty());
            prop_assert!(ir.exported_names().is_empty());
            prop_assert!(ir.import_sources().is_empty());
        }

        /// Enriching with empty DataFlowInfo is a no-op.
        #[test]
        fn enrich_empty_data_flow_is_noop(ir in ir_file_strategy()) {
            let mut enriched = ir.clone();
            enriched.enrich_with_data_flow(&DataFlowInfo {
                assignments: vec![],
                calls_with_args: vec![],
            });
            prop_assert_eq!(ir.assignments.len(), enriched.assignments.len());
        }
    }
}
