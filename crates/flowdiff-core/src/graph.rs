//! Symbol graph construction using petgraph.
//!
//! Builds a directed graph `G = (V, E)` from parsed AST data where:
//! - Vertices are symbols (functions, classes, types, modules)
//! - Edges represent relationships (imports, calls, extends)

use std::collections::HashMap;

use petgraph::graph::{DiGraph, NodeIndex};
use serde::{Deserialize, Serialize};

use crate::ast::{Definition, ExportInfo, Language, ParsedFile};
use crate::ir::{IrExport, IrFile, IrImportSpecifier, TypeDefKind};
use crate::types::{EdgeType, SymbolKind};

/// A node in the symbol graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SymbolNode {
    /// Unique identifier: `file_path::symbol_name`
    pub id: String,
    /// The symbol name.
    pub name: String,
    /// The file this symbol belongs to.
    pub file: String,
    /// The kind of symbol.
    pub kind: SymbolKind,
}

/// An edge in the symbol graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub edge_type: EdgeType,
}

/// The complete symbol graph built from parsed files.
#[derive(Debug)]
pub struct SymbolGraph {
    pub graph: DiGraph<SymbolNode, GraphEdge>,
    /// Map from symbol id (`file::name`) to node index for fast lookup.
    id_to_index: HashMap<String, NodeIndex>,
}

/// Errors from graph construction.
#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("graph serialization error: {0}")]
    SerializationError(String),
}

/// Serializable representation for roundtrip testing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SerializableGraph {
    pub nodes: Vec<SymbolNode>,
    pub edges: Vec<SerializableEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SerializableEdge {
    pub from: String,
    pub to: String,
    pub edge_type: EdgeType,
}

impl SymbolGraph {
    /// Build a symbol graph from a collection of parsed files.
    pub fn build(files: &[ParsedFile]) -> Self {
        let mut graph = DiGraph::new();
        let mut id_to_index: HashMap<String, NodeIndex> = HashMap::new();

        // Phase 1: Add all symbol nodes.
        for file in files {
            // Add a file-level module node.
            let module_id = file.path.clone();
            let module_node = SymbolNode {
                id: module_id.clone(),
                name: file_stem(&file.path),
                file: file.path.clone(),
                kind: SymbolKind::Module,
            };
            let idx = graph.add_node(module_node);
            id_to_index.insert(module_id, idx);

            // Add definition nodes.
            for def in &file.definitions {
                let sym_id = format!("{}::{}", file.path, def.name);
                if id_to_index.contains_key(&sym_id) {
                    continue; // skip duplicates (e.g. methods with same name)
                }
                let node = SymbolNode {
                    id: sym_id.clone(),
                    name: def.name.clone(),
                    file: file.path.clone(),
                    kind: def.kind.clone(),
                };
                let idx = graph.add_node(node);
                id_to_index.insert(sym_id, idx);
            }
        }

        // Build lookup structures for import resolution.
        let file_exports = build_export_map(files);
        let file_defs = build_definition_map(files);

        // Phase 2: Add edges.
        for file in files {
            add_import_edges(file, files, &file_exports, &file_defs, &id_to_index, &mut graph);
            add_call_edges(file, files, &file_exports, &file_defs, &id_to_index, &mut graph);
            add_extends_edges(file, files, &file_defs, &id_to_index, &mut graph);
        }

        SymbolGraph { graph, id_to_index }
    }

    /// Build a symbol graph from IR files (declarative query engine / IR path).
    ///
    /// This is the primary entry point for graph construction from the IR pipeline.
    /// It consumes `IrFile` types directly, enabling richer edge construction
    /// (e.g., class extends edges from `IrTypeDef.bases`).
    pub fn build_from_ir(files: &[IrFile]) -> Self {
        let mut graph = DiGraph::new();
        let mut id_to_index: HashMap<String, NodeIndex> = HashMap::new();

        // Phase 1: Add all symbol nodes.
        for file in files {
            // Module node.
            let module_id = file.path.clone();
            let module_node = SymbolNode {
                id: module_id.clone(),
                name: file_stem(&file.path),
                file: file.path.clone(),
                kind: SymbolKind::Module,
            };
            let idx = graph.add_node(module_node);
            id_to_index.insert(module_id, idx);

            // Function nodes.
            for f in &file.functions {
                let sym_id = format!("{}::{}", file.path, f.name);
                if id_to_index.contains_key(&sym_id) {
                    continue;
                }
                let node = SymbolNode {
                    id: sym_id.clone(),
                    name: f.name.clone(),
                    file: file.path.clone(),
                    kind: SymbolKind::Function,
                };
                let idx = graph.add_node(node);
                id_to_index.insert(sym_id, idx);
            }

            // Type definition nodes (class, struct, interface, type alias).
            for t in &file.type_defs {
                let sym_id = format!("{}::{}", file.path, t.name);
                if id_to_index.contains_key(&sym_id) {
                    continue;
                }
                let kind = match t.kind {
                    TypeDefKind::Class => SymbolKind::Class,
                    TypeDefKind::Struct => SymbolKind::Struct,
                    TypeDefKind::Interface => SymbolKind::Interface,
                    TypeDefKind::TypeAlias => SymbolKind::TypeAlias,
                    TypeDefKind::Enum => SymbolKind::Class,
                };
                let node = SymbolNode {
                    id: sym_id.clone(),
                    name: t.name.clone(),
                    file: file.path.clone(),
                    kind,
                };
                let idx = graph.add_node(node);
                id_to_index.insert(sym_id, idx);
            }

            // Constant nodes.
            for c in &file.constants {
                let sym_id = format!("{}::{}", file.path, c.name);
                if id_to_index.contains_key(&sym_id) {
                    continue;
                }
                let node = SymbolNode {
                    id: sym_id.clone(),
                    name: c.name.clone(),
                    file: file.path.clone(),
                    kind: SymbolKind::Constant,
                };
                let idx = graph.add_node(node);
                id_to_index.insert(sym_id, idx);
            }
        }

        // Build lookup structures.
        let file_exports = build_ir_export_map(files);
        let file_def_names = build_ir_def_names_map(files);
        let known_paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();

        // Phase 2: Add edges.
        for file in files {
            add_ir_import_edges(
                file,
                &file_exports,
                &file_def_names,
                &id_to_index,
                &known_paths,
                &mut graph,
            );
            add_ir_call_edges(
                file,
                files,
                &file_def_names,
                &id_to_index,
                &known_paths,
                &mut graph,
            );
            add_ir_extends_edges(
                file,
                files,
                &id_to_index,
                &known_paths,
                &mut graph,
            );
        }

        SymbolGraph { graph, id_to_index }
    }

    /// Number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Number of edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Look up a node index by symbol id.
    pub fn get_node(&self, id: &str) -> Option<NodeIndex> {
        self.id_to_index.get(id).copied()
    }

    /// Get the symbol node data for a given id.
    pub fn get_symbol(&self, id: &str) -> Option<&SymbolNode> {
        self.id_to_index
            .get(id)
            .map(|idx| &self.graph[*idx])
    }

    /// Get all node ids in the graph.
    pub fn node_ids(&self) -> Vec<&str> {
        self.id_to_index.keys().map(|s| s.as_str()).collect()
    }

    /// Add an edge between two nodes by their indices.
    pub fn add_edge(&mut self, from: NodeIndex, to: NodeIndex, edge: GraphEdge) {
        self.graph.add_edge(from, to, edge);
    }

    /// Get all edges as (from_id, to_id, edge_type) tuples.
    pub fn edges(&self) -> Vec<(&str, &str, &EdgeType)> {
        self.graph
            .edge_indices()
            .filter_map(|e| {
                let (src, tgt) = self.graph.edge_endpoints(e)?;
                let edge = &self.graph[e];
                Some((
                    self.graph[src].id.as_str(),
                    self.graph[tgt].id.as_str(),
                    &edge.edge_type,
                ))
            })
            .collect()
    }

    /// Serialize the graph to a JSON-friendly structure.
    pub fn to_serializable(&self) -> SerializableGraph {
        let nodes: Vec<SymbolNode> = self
            .graph
            .node_indices()
            .map(|i| self.graph[i].clone())
            .collect();

        let edges: Vec<SerializableEdge> = self
            .graph
            .edge_indices()
            .filter_map(|e| {
                let (src, tgt) = self.graph.edge_endpoints(e)?;
                Some(SerializableEdge {
                    from: self.graph[src].id.clone(),
                    to: self.graph[tgt].id.clone(),
                    edge_type: self.graph[e].edge_type.clone(),
                })
            })
            .collect();

        SerializableGraph { nodes, edges }
    }

    /// Deserialize from a serializable graph back into a SymbolGraph.
    pub fn from_serializable(sg: &SerializableGraph) -> Self {
        let mut graph = DiGraph::new();
        let mut id_to_index: HashMap<String, NodeIndex> = HashMap::new();

        for node in &sg.nodes {
            let idx = graph.add_node(node.clone());
            id_to_index.insert(node.id.clone(), idx);
        }

        for edge in &sg.edges {
            if let (Some(&src), Some(&tgt)) =
                (id_to_index.get(&edge.from), id_to_index.get(&edge.to))
            {
                graph.add_edge(src, tgt, GraphEdge {
                    edge_type: edge.edge_type.clone(),
                });
            }
        }

        SymbolGraph { graph, id_to_index }
    }
}

// ---------------------------------------------------------------------------
// Import resolution helpers
// ---------------------------------------------------------------------------

/// Map from file path to its exported symbol names.
fn build_export_map(files: &[ParsedFile]) -> HashMap<String, Vec<ExportInfo>> {
    files
        .iter()
        .map(|f| (f.path.clone(), f.exports.clone()))
        .collect()
}

/// Map from file path to its definitions.
fn build_definition_map(files: &[ParsedFile]) -> HashMap<String, Vec<Definition>> {
    files
        .iter()
        .map(|f| (f.path.clone(), f.definitions.clone()))
        .collect()
}

/// Resolve an import source path (e.g. `./utils`, `../models/user`) relative to the
/// importing file, returning the resolved file path if it exists in our file set.
///
/// Handles both JS/TS-style (`./utils`, `../models/user`) and Python-style
/// (`.models`, `..models`, `.models.user`) relative imports.
fn resolve_import_path(
    import_source: &str,
    importer_path: &str,
    known_files: &[&str],
) -> Option<String> {
    // Only resolve relative imports
    if !import_source.starts_with('.') {
        return None;
    }

    // Convert Python-style dot imports to path-style.
    // `.models` → `./models`, `..models` → `../models`, `.models.user` → `./models/user`
    let normalized_source = normalize_python_import(import_source);

    let importer_dir = parent_dir(importer_path);
    let resolved = normalize_path(&format!("{}/{}", importer_dir, normalized_source));

    // Try exact match first, then with common extensions.
    let candidates = [
        resolved.clone(),
        format!("{}.ts", resolved),
        format!("{}.tsx", resolved),
        format!("{}.js", resolved),
        format!("{}.jsx", resolved),
        format!("{}.py", resolved),
        format!("{}/index.ts", resolved),
        format!("{}/index.js", resolved),
        format!("{}/index.tsx", resolved),
    ];

    for candidate in &candidates {
        if known_files.contains(&candidate.as_str()) {
            return Some(candidate.clone());
        }
    }

    None
}

/// Get the parent directory of a file path.
fn parent_dir(path: &str) -> String {
    match path.rfind('/') {
        Some(pos) => path[..pos].to_string(),
        None => ".".to_string(),
    }
}

/// Get the file stem (filename without extension).
fn file_stem(path: &str) -> String {
    let filename = path.rsplit('/').next().unwrap_or(path);
    match filename.find('.') {
        Some(pos) => filename[..pos].to_string(),
        None => filename.to_string(),
    }
}

/// Normalize a path by resolving `.` and `..` segments.
fn normalize_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// Convert Python-style dot imports to path-style relative imports.
///
/// - `.models` → `./models`
/// - `..models` → `../models`
/// - `.models.user` → `./models/user`
/// - `.` → `.`
/// - `...utils.helpers` → `../../utils/helpers`
fn normalize_python_import(source: &str) -> String {
    // Count leading dots.
    let dot_count = source.chars().take_while(|c| *c == '.').count();
    let remainder = &source[dot_count..];

    if dot_count == 0 {
        return source.to_string();
    }

    // Build the relative prefix: `.` → `./`, `..` → `../`, `...` → `../../`
    let prefix = if dot_count == 1 {
        ".".to_string()
    } else {
        let mut p = String::new();
        for i in 0..dot_count - 1 {
            if i > 0 {
                p.push('/');
            }
            p.push_str("..");
        }
        p
    };

    if remainder.is_empty() {
        return prefix;
    }

    // Convert remaining dots (module separators) to slashes.
    let path_part = remainder.replace('.', "/");
    format!("{}/{}", prefix, path_part)
}

// ---------------------------------------------------------------------------
// Edge construction
// ---------------------------------------------------------------------------

/// Add import edges: file A imports symbol from file B → edge from A's module to B's symbol.
fn add_import_edges(
    file: &ParsedFile,
    all_files: &[ParsedFile],
    file_exports: &HashMap<String, Vec<ExportInfo>>,
    file_defs: &HashMap<String, Vec<Definition>>,
    id_to_index: &HashMap<String, NodeIndex>,
    graph: &mut DiGraph<SymbolNode, GraphEdge>,
) {
    let known_paths: Vec<&str> = all_files.iter().map(|f| f.path.as_str()).collect();

    for import in &file.imports {
        let resolved = match resolve_import_path(&import.source, &file.path, &known_paths) {
            Some(p) => p,
            None => continue,
        };

        let from_module_id = file.path.clone();
        let from_idx = match id_to_index.get(&from_module_id) {
            Some(idx) => *idx,
            None => continue,
        };

        // For each imported name, find matching export or definition in target file.
        if import.names.is_empty() {
            // Side-effect import: create module-to-module edge.
            if let Some(&to_idx) = id_to_index.get(&resolved) {
                graph.add_edge(
                    from_idx,
                    to_idx,
                    GraphEdge {
                        edge_type: EdgeType::Imports,
                    },
                );
            }
            continue;
        }

        for imported_name in &import.names {
            let target_name = &imported_name.name;

            // Try to find the symbol in the target file's definitions.
            let target_sym_id = format!("{}::{}", resolved, target_name);
            if let Some(&to_idx) = id_to_index.get(&target_sym_id) {
                graph.add_edge(
                    from_idx,
                    to_idx,
                    GraphEdge {
                        edge_type: EdgeType::Imports,
                    },
                );
                continue;
            }

            // If importing a default, check if target has a matching export/def.
            if import.is_default || import.is_namespace {
                // Link to the module node itself.
                if let Some(&to_idx) = id_to_index.get(&resolved) {
                    graph.add_edge(
                        from_idx,
                        to_idx,
                        GraphEdge {
                            edge_type: EdgeType::Imports,
                        },
                    );
                }
                continue;
            }

            // Check re-exports: target file may re-export from another file.
            if let Some(exports) = file_exports.get(&resolved) {
                for export in exports {
                    if export.name == *target_name && export.is_reexport {
                        if let Some(ref reexport_source) = export.source {
                            if let Some(reexport_resolved) =
                                resolve_import_path(reexport_source, &resolved, &known_paths)
                            {
                                let reexport_sym_id =
                                    format!("{}::{}", reexport_resolved, target_name);
                                if let Some(&to_idx) = id_to_index.get(&reexport_sym_id) {
                                    graph.add_edge(
                                        from_idx,
                                        to_idx,
                                        GraphEdge {
                                            edge_type: EdgeType::Imports,
                                        },
                                    );
                                }
                            }
                        }
                    }
                }
            }

            // Fallback: Python-style — definition name matches directly.
            if let Some(defs) = file_defs.get(&resolved) {
                if defs.iter().any(|d| d.name == *target_name) {
                    let sym_id = format!("{}::{}", resolved, target_name);
                    if let Some(&to_idx) = id_to_index.get(&sym_id) {
                        graph.add_edge(
                            from_idx,
                            to_idx,
                            GraphEdge {
                                edge_type: EdgeType::Imports,
                            },
                        );
                    }
                }
            }
        }
    }
}

/// Add call edges: function A calls function B → edge from A to B.
fn add_call_edges(
    file: &ParsedFile,
    all_files: &[ParsedFile],
    file_exports: &HashMap<String, Vec<ExportInfo>>,
    file_defs: &HashMap<String, Vec<Definition>>,
    id_to_index: &HashMap<String, NodeIndex>,
    graph: &mut DiGraph<SymbolNode, GraphEdge>,
) {
    let known_paths: Vec<&str> = all_files.iter().map(|f| f.path.as_str()).collect();

    // Build a map of imported names → resolved symbol ids for this file.
    let import_map = build_import_resolution_map(
        file,
        all_files,
        file_exports,
        file_defs,
        &known_paths,
    );

    for call in &file.call_sites {
        // Determine the calling symbol.
        let caller_id = match &call.containing_function {
            Some(func_name) => format!("{}::{}", file.path, func_name),
            None => file.path.clone(), // module-level call
        };

        let caller_idx = match id_to_index.get(&caller_id) {
            Some(idx) => *idx,
            None => {
                // If the containing function isn't found, try module node.
                match id_to_index.get(&file.path) {
                    Some(idx) => *idx,
                    None => continue,
                }
            }
        };

        // Resolve the callee.
        let callee_name = &call.callee;

        // Simple name (e.g., `validateUser`) — look up in import map or local defs.
        if let Some(target_id) = import_map.get(callee_name.as_str()) {
            if let Some(&to_idx) = id_to_index.get(target_id.as_str()) {
                if caller_idx != to_idx {
                    graph.add_edge(
                        caller_idx,
                        to_idx,
                        GraphEdge {
                            edge_type: EdgeType::Calls,
                        },
                    );
                }
            }
            continue;
        }

        // Method call (e.g., `db.save`) — check if `db` is an imported name.
        if let Some(dot_pos) = callee_name.find('.') {
            let receiver = &callee_name[..dot_pos];
            if let Some(target_module) = import_map.get(receiver) {
                // The receiver resolves to a module/class — look for the method.
                let method = &callee_name[dot_pos + 1..];
                // Try file::method first.
                let method_id = format!("{}::{}", target_module.trim_end_matches("::*"), method);
                if let Some(&to_idx) = id_to_index.get(&method_id) {
                    if caller_idx != to_idx {
                        graph.add_edge(
                            caller_idx,
                            to_idx,
                            GraphEdge {
                                edge_type: EdgeType::Calls,
                            },
                        );
                    }
                    continue;
                }
                // Try module node as fallback.
                if let Some(&to_idx) = id_to_index.get(target_module.as_str()) {
                    if caller_idx != to_idx {
                        graph.add_edge(
                            caller_idx,
                            to_idx,
                            GraphEdge {
                                edge_type: EdgeType::Calls,
                            },
                        );
                    }
                    continue;
                }
            }
        }

        // Local function call — same file.
        let local_id = format!("{}::{}", file.path, callee_name);
        if let Some(&to_idx) = id_to_index.get(&local_id) {
            if caller_idx != to_idx {
                graph.add_edge(
                    caller_idx,
                    to_idx,
                    GraphEdge {
                        edge_type: EdgeType::Calls,
                    },
                );
            }
        }
    }
}

/// Build a map from imported name → resolved symbol id for a given file.
fn build_import_resolution_map(
    file: &ParsedFile,
    all_files: &[ParsedFile],
    _file_exports: &HashMap<String, Vec<ExportInfo>>,
    _file_defs: &HashMap<String, Vec<Definition>>,
    known_paths: &[&str],
) -> HashMap<String, String> {
    let mut map = HashMap::new();

    for import in &file.imports {
        let resolved = match resolve_import_path(&import.source, &file.path, known_paths) {
            Some(p) => p,
            None => continue,
        };

        if import.is_namespace {
            // `import * as X from './mod'` or Python `import X`
            // Map X → resolved module path.
            for name in &import.names {
                let local_name = name.alias.as_ref().unwrap_or(&name.name);
                map.insert(local_name.clone(), resolved.clone());
            }
            continue;
        }

        for name in &import.names {
            let local_name = name.alias.as_ref().unwrap_or(&name.name);
            // Try to resolve to a specific symbol in the target file.
            let target_sym_id = format!("{}::{}", resolved, name.name);

            // Check if this symbol exists in the target file's definitions.
            let target_file = all_files.iter().find(|f| f.path == resolved);
            if let Some(tf) = target_file {
                if tf.definitions.iter().any(|d| d.name == name.name) {
                    map.insert(local_name.clone(), target_sym_id);
                    continue;
                }
            }

            // Default import — map to module.
            if import.is_default {
                map.insert(local_name.clone(), resolved.clone());
            } else {
                // Map to the symbol id even if we can't verify it exists.
                map.insert(local_name.clone(), target_sym_id);
            }
        }
    }

    map
}

/// Add extends edges for class inheritance (Python).
fn add_extends_edges(
    file: &ParsedFile,
    all_files: &[ParsedFile],
    file_defs: &HashMap<String, Vec<Definition>>,
    id_to_index: &HashMap<String, NodeIndex>,
    graph: &mut DiGraph<SymbolNode, GraphEdge>,
) {
    if file.language != Language::Python {
        // For now, extends edges are only for Python (tree-sitter class bases).
        // TS/JS class extends could be added via AST node child inspection.
        return;
    }

    let known_paths: Vec<&str> = all_files.iter().map(|f| f.path.as_str()).collect();

    // Build import resolution for this file.
    let import_map = build_import_resolution_map(
        file,
        all_files,
        &HashMap::new(),
        file_defs,
        &known_paths,
    );

    for def in &file.definitions {
        if def.kind != SymbolKind::Class {
            continue;
        }

        let child_id = format!("{}::{}", file.path, def.name);
        let child_idx = match id_to_index.get(&child_id) {
            Some(idx) => *idx,
            None => continue,
        };

        // Get base classes from the original source.
        // We need access to the source, but ParsedFile doesn't store it.
        // Instead, look for definitions with the same name in imported files or locally.
        // Check local file first.
        if let Some(defs) = file_defs.get(&file.path) {
            for base_def in defs {
                if base_def.kind == SymbolKind::Class && base_def.name != def.name {
                    // We can't directly know if this class extends the other from ParsedFile alone.
                    // This would require re-parsing or storing base classes in Definition.
                    // For now, we rely on import edges + call sites for cross-file relationships.
                }
            }
        }

        // Check if any imported class matches a known base class pattern.
        // We use call sites that match `ClassName(...)` as constructor calls → Instantiates edges.
        for call in &file.call_sites {
            if call.callee == def.name {
                continue; // Skip self-references.
            }
            // Check if this is a class instantiation (calling a known class name).
            if let Some(target_id) = import_map.get(call.callee.as_str()) {
                if let Some(&to_idx) = id_to_index.get(target_id.as_str()) {
                    // Check if target is a class.
                    if graph[to_idx].kind == SymbolKind::Class {
                        // This is likely an instantiation, not extends.
                        // We'll handle extends separately when we have class bases data.
                    }
                }
            }
        }

        // For extends edges, we rely on the caller providing class base info.
        // The `get_python_class_bases` function exists in ast.rs but requires source code.
        // Graph construction from ParsedFile alone can detect extends via import patterns.
        let _ = child_idx; // Used when base class info is available.
    }
}

// ---------------------------------------------------------------------------
// IR-based lookup helpers
// ---------------------------------------------------------------------------

/// Map from file path to its IR exports.
fn build_ir_export_map(files: &[IrFile]) -> HashMap<String, Vec<IrExport>> {
    files
        .iter()
        .map(|f| (f.path.clone(), f.exports.clone()))
        .collect()
}

/// Map from file path to (name, kind) pairs for all definitions.
fn build_ir_def_names_map(files: &[IrFile]) -> HashMap<String, Vec<(String, SymbolKind)>> {
    files
        .iter()
        .map(|f| {
            let mut defs = Vec::new();
            for func in &f.functions {
                defs.push((func.name.clone(), SymbolKind::Function));
            }
            for td in &f.type_defs {
                let kind = match td.kind {
                    TypeDefKind::Class => SymbolKind::Class,
                    TypeDefKind::Struct => SymbolKind::Struct,
                    TypeDefKind::Interface => SymbolKind::Interface,
                    TypeDefKind::TypeAlias => SymbolKind::TypeAlias,
                    TypeDefKind::Enum => SymbolKind::Class,
                };
                defs.push((td.name.clone(), kind));
            }
            for c in &f.constants {
                defs.push((c.name.clone(), SymbolKind::Constant));
            }
            (f.path.clone(), defs)
        })
        .collect()
}

/// Add import edges from IR imports.
fn add_ir_import_edges(
    file: &IrFile,
    file_exports: &HashMap<String, Vec<IrExport>>,
    file_defs: &HashMap<String, Vec<(String, SymbolKind)>>,
    id_to_index: &HashMap<String, NodeIndex>,
    known_paths: &[&str],
    graph: &mut DiGraph<SymbolNode, GraphEdge>,
) {
    let from_idx = match id_to_index.get(&file.path) {
        Some(idx) => *idx,
        None => return,
    };

    for import in &file.imports {
        let resolved = match resolve_import_path(&import.source, &file.path, known_paths) {
            Some(p) => p,
            None => continue,
        };

        // Check if this import is side-effect only.
        let is_side_effect = import.specifiers.is_empty()
            || import
                .specifiers
                .iter()
                .all(|s| matches!(s, IrImportSpecifier::SideEffect));

        if is_side_effect {
            if let Some(&to_idx) = id_to_index.get(&resolved) {
                graph.add_edge(
                    from_idx,
                    to_idx,
                    GraphEdge {
                        edge_type: EdgeType::Imports,
                    },
                );
            }
            continue;
        }

        for spec in &import.specifiers {
            match spec {
                IrImportSpecifier::Named { name, .. } => {
                    // Try to find the symbol by name in the target file.
                    let target_sym_id = format!("{}::{}", resolved, name);
                    if let Some(&to_idx) = id_to_index.get(&target_sym_id) {
                        graph.add_edge(
                            from_idx,
                            to_idx,
                            GraphEdge {
                                edge_type: EdgeType::Imports,
                            },
                        );
                        continue;
                    }

                    // Check re-exports.
                    if let Some(exports) = file_exports.get(&resolved) {
                        for export in exports {
                            if export.name == *name && export.is_reexport {
                                if let Some(ref reexport_source) = export.source {
                                    if let Some(reexport_resolved) =
                                        resolve_import_path(reexport_source, &resolved, known_paths)
                                    {
                                        let reexport_sym_id =
                                            format!("{}::{}", reexport_resolved, name);
                                        if let Some(&to_idx) =
                                            id_to_index.get(&reexport_sym_id)
                                        {
                                            graph.add_edge(
                                                from_idx,
                                                to_idx,
                                                GraphEdge {
                                                    edge_type: EdgeType::Imports,
                                                },
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Fallback: definition name matches directly.
                    if let Some(defs) = file_defs.get(&resolved) {
                        if defs.iter().any(|(n, _): &(String, SymbolKind)| n == name) {
                            let sym_id = format!("{}::{}", resolved, name);
                            if let Some(&to_idx) = id_to_index.get(&sym_id) {
                                graph.add_edge(
                                    from_idx,
                                    to_idx,
                                    GraphEdge {
                                        edge_type: EdgeType::Imports,
                                    },
                                );
                            }
                        }
                    }
                }
                IrImportSpecifier::Default(_) | IrImportSpecifier::Namespace(_) => {
                    // Link to the module node.
                    if let Some(&to_idx) = id_to_index.get(&resolved) {
                        graph.add_edge(
                            from_idx,
                            to_idx,
                            GraphEdge {
                                edge_type: EdgeType::Imports,
                            },
                        );
                    }
                }
                IrImportSpecifier::SideEffect => {
                    // Already handled above.
                }
            }
        }
    }
}

/// Build import resolution map from IR imports for call edge resolution.
fn build_ir_import_resolution_map(
    file: &IrFile,
    all_files: &[IrFile],
    _file_defs: &HashMap<String, Vec<(String, SymbolKind)>>,
    known_paths: &[&str],
) -> HashMap<String, String> {
    let mut map = HashMap::new();

    for import in &file.imports {
        let resolved = match resolve_import_path(&import.source, &file.path, known_paths) {
            Some(p) => p,
            None => continue,
        };

        for spec in &import.specifiers {
            match spec {
                IrImportSpecifier::Namespace(local) => {
                    map.insert(local.clone(), resolved.clone());
                }
                IrImportSpecifier::Named { name, alias } => {
                    let local_name = alias.as_deref().unwrap_or(name.as_str());
                    let target_sym_id = format!("{}::{}", resolved, name);

                    // Check if this symbol exists in the target file.
                    let target_file = all_files.iter().find(|f| f.path == resolved);
                    if let Some(tf) = target_file {
                        let has_def = tf.functions.iter().any(|d| d.name == *name)
                            || tf.type_defs.iter().any(|d| d.name == *name)
                            || tf.constants.iter().any(|d| d.name == *name);
                        if has_def {
                            map.insert(local_name.to_string(), target_sym_id);
                            continue;
                        }
                    }

                    // Map to the symbol id even if we can't verify.
                    map.insert(local_name.to_string(), target_sym_id);
                }
                IrImportSpecifier::Default(local) => {
                    map.insert(local.clone(), resolved.clone());
                }
                IrImportSpecifier::SideEffect => {}
            }
        }
    }

    map
}

/// Add call edges from IR call expressions.
fn add_ir_call_edges(
    file: &IrFile,
    all_files: &[IrFile],
    file_defs: &HashMap<String, Vec<(String, SymbolKind)>>,
    id_to_index: &HashMap<String, NodeIndex>,
    known_paths: &[&str],
    graph: &mut DiGraph<SymbolNode, GraphEdge>,
) {
    let import_map = build_ir_import_resolution_map(file, all_files, file_defs, known_paths);

    for call in &file.call_expressions {
        let caller_id = match &call.containing_function {
            Some(func_name) => format!("{}::{}", file.path, func_name),
            None => file.path.clone(),
        };

        let caller_idx = match id_to_index.get(&caller_id) {
            Some(idx) => *idx,
            None => match id_to_index.get(&file.path) {
                Some(idx) => *idx,
                None => continue,
            },
        };

        let callee_name = &call.callee;

        // Simple name — look up in import map or local defs.
        if let Some(target_id) = import_map.get(callee_name.as_str()) {
            if let Some(&to_idx) = id_to_index.get(target_id.as_str()) {
                if caller_idx != to_idx {
                    graph.add_edge(
                        caller_idx,
                        to_idx,
                        GraphEdge {
                            edge_type: EdgeType::Calls,
                        },
                    );
                }
            }
            continue;
        }

        // Method call (e.g., `db.save`).
        if let Some(dot_pos) = callee_name.find('.') {
            let receiver = &callee_name[..dot_pos];
            if let Some(target_module) = import_map.get(receiver) {
                let method = &callee_name[dot_pos + 1..];
                let method_id =
                    format!("{}::{}", target_module.trim_end_matches("::*"), method);
                if let Some(&to_idx) = id_to_index.get(&method_id) {
                    if caller_idx != to_idx {
                        graph.add_edge(
                            caller_idx,
                            to_idx,
                            GraphEdge {
                                edge_type: EdgeType::Calls,
                            },
                        );
                    }
                    continue;
                }
                if let Some(&to_idx) = id_to_index.get(target_module.as_str()) {
                    if caller_idx != to_idx {
                        graph.add_edge(
                            caller_idx,
                            to_idx,
                            GraphEdge {
                                edge_type: EdgeType::Calls,
                            },
                        );
                    }
                    continue;
                }
            }
        }

        // Local function call.
        let local_id = format!("{}::{}", file.path, callee_name);
        if let Some(&to_idx) = id_to_index.get(&local_id) {
            if caller_idx != to_idx {
                graph.add_edge(
                    caller_idx,
                    to_idx,
                    GraphEdge {
                        edge_type: EdgeType::Calls,
                    },
                );
            }
        }
    }
}

/// Add extends edges from IR type definitions with bases.
///
/// Unlike the ParsedFile-based version which cannot determine class bases,
/// the IR path has `IrTypeDef.bases` populated from the query engine, enabling
/// real extends edge construction.
fn add_ir_extends_edges(
    file: &IrFile,
    all_files: &[IrFile],
    id_to_index: &HashMap<String, NodeIndex>,
    known_paths: &[&str],
    graph: &mut DiGraph<SymbolNode, GraphEdge>,
) {
    let import_map = build_ir_import_resolution_map(
        file,
        all_files,
        &HashMap::new(),
        known_paths,
    );

    for td in &file.type_defs {
        if td.bases.is_empty() {
            continue;
        }

        let child_id = format!("{}::{}", file.path, td.name);
        let child_idx = match id_to_index.get(&child_id) {
            Some(idx) => *idx,
            None => continue,
        };

        for base in &td.bases {
            // Try imported name first.
            if let Some(target_id) = import_map.get(base.as_str()) {
                if let Some(&to_idx) = id_to_index.get(target_id.as_str()) {
                    if child_idx != to_idx {
                        graph.add_edge(
                            child_idx,
                            to_idx,
                            GraphEdge {
                                edge_type: EdgeType::Extends,
                            },
                        );
                    }
                    continue;
                }
            }

            // Try local definition.
            let local_id = format!("{}::{}", file.path, base);
            if let Some(&to_idx) = id_to_index.get(&local_id) {
                if child_idx != to_idx {
                    graph.add_edge(
                        child_idx,
                        to_idx,
                        GraphEdge {
                            edge_type: EdgeType::Extends,
                        },
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{self, ParsedFile};
    use crate::types::SymbolKind;

    /// Helper: parse multiple files and build a graph.
    fn build_graph_from_sources(files: &[(&str, &str)]) -> SymbolGraph {
        let parsed: Vec<ParsedFile> = files
            .iter()
            .map(|(path, source)| ast::parse_file(path, source).unwrap())
            .collect();
        SymbolGraph::build(&parsed)
    }

    /// Helper: check if an edge exists between two symbol ids with a given type.
    fn has_edge(graph: &SymbolGraph, from: &str, to: &str, edge_type: &EdgeType) -> bool {
        graph
            .edges()
            .iter()
            .any(|(f, t, et)| *f == from && *t == to && *et == edge_type)
    }

    /// Helper: count edges of a specific type.
    fn count_edges_of_type(graph: &SymbolGraph, edge_type: &EdgeType) -> usize {
        graph
            .edges()
            .iter()
            .filter(|(_, _, et)| *et == edge_type)
            .count()
    }

    // === Import edge tests ===

    #[test]
    fn test_build_import_edges() {
        let graph = build_graph_from_sources(&[
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
function handle() { validate({}); }
"#,
            ),
        ]);

        // handler.ts module should import validate and sanitize from utils.ts
        assert!(
            has_edge(
                &graph,
                "src/handler.ts",
                "src/utils.ts::validate",
                &EdgeType::Imports
            ),
            "should have import edge to validate"
        );
        assert!(
            has_edge(
                &graph,
                "src/handler.ts",
                "src/utils.ts::sanitize",
                &EdgeType::Imports
            ),
            "should have import edge to sanitize"
        );
    }

    #[test]
    fn test_build_import_edges_default() {
        let graph = build_graph_from_sources(&[
            (
                "src/app.ts",
                r#"
const app = createApp();
export default app;
"#,
            ),
            (
                "src/main.ts",
                r#"
import App from './app';
"#,
            ),
        ]);

        // Default import should link to the module node.
        assert!(
            has_edge(&graph, "src/main.ts", "src/app.ts", &EdgeType::Imports),
            "should have import edge for default import"
        );
    }

    #[test]
    fn test_build_import_edges_namespace() {
        let graph = build_graph_from_sources(&[
            (
                "src/utils.ts",
                r#"
export function foo() {}
export function bar() {}
"#,
            ),
            (
                "src/main.ts",
                r#"
import * as utils from './utils';
"#,
            ),
        ]);

        assert!(
            has_edge(&graph, "src/main.ts", "src/utils.ts", &EdgeType::Imports),
            "namespace import should link to module node"
        );
    }

    #[test]
    fn test_side_effect_import() {
        let graph = build_graph_from_sources(&[
            ("src/polyfill.ts", "// polyfill code"),
            (
                "src/main.ts",
                r#"
import './polyfill';
"#,
            ),
        ]);

        assert!(
            has_edge(
                &graph,
                "src/main.ts",
                "src/polyfill.ts",
                &EdgeType::Imports
            ),
            "side-effect import should create module-to-module edge"
        );
    }

    // === Call edge tests ===

    #[test]
    fn test_build_call_edges() {
        let graph = build_graph_from_sources(&[
            (
                "src/utils.ts",
                r#"
export function validate(data: any) { return data; }
"#,
            ),
            (
                "src/handler.ts",
                r#"
import { validate } from './utils';
function processRequest(req: any) {
    const v = validate(req.body);
    return v;
}
"#,
            ),
        ]);

        assert!(
            has_edge(
                &graph,
                "src/handler.ts::processRequest",
                "src/utils.ts::validate",
                &EdgeType::Calls
            ),
            "processRequest should have call edge to validate"
        );
    }

    #[test]
    fn test_build_call_edges_local() {
        let graph = build_graph_from_sources(&[(
            "src/service.ts",
            r#"
function helper() { return 42; }
function main() {
    const x = helper();
    return x;
}
"#,
        )]);

        assert!(
            has_edge(
                &graph,
                "src/service.ts::main",
                "src/service.ts::helper",
                &EdgeType::Calls
            ),
            "main should have call edge to local helper"
        );
    }

    #[test]
    fn test_build_call_edges_method_on_import() {
        let graph = build_graph_from_sources(&[
            (
                "src/db.ts",
                r#"
export function save(data: any) { return data; }
export function find(id: string) { return {}; }
"#,
            ),
            (
                "src/service.ts",
                r#"
import * as db from './db';
function createUser(data: any) {
    return db.save(data);
}
"#,
            ),
        ]);

        assert!(
            has_edge(
                &graph,
                "src/service.ts::createUser",
                "src/db.ts::save",
                &EdgeType::Calls
            ),
            "should resolve method call on namespace import"
        );
    }

    #[test]
    fn test_no_self_call_edge() {
        let graph = build_graph_from_sources(&[(
            "src/lib.ts",
            r#"
function recurse(n: number): number {
    if (n <= 0) return 0;
    return recurse(n - 1);
}
"#,
        )]);

        // Recursive calls should not create self-edges.
        let self_edges: Vec<_> = graph
            .edges()
            .into_iter()
            .filter(|(f, t, _)| f == t)
            .collect();
        assert!(
            self_edges.is_empty(),
            "recursive function should not create self-edges"
        );
    }

    // === Graph structure tests ===

    #[test]
    fn test_graph_node_count() {
        let graph = build_graph_from_sources(&[
            (
                "src/a.ts",
                r#"
export function foo() {}
export function bar() {}
"#,
            ),
            (
                "src/b.ts",
                r#"
export class Baz {}
"#,
            ),
        ]);

        // 2 module nodes + 2 functions + 1 class = 5
        assert_eq!(graph.node_count(), 5);
    }

    #[test]
    fn test_graph_edge_count() {
        let graph = build_graph_from_sources(&[
            (
                "src/utils.ts",
                r#"
export function validate(x: any) { return x; }
"#,
            ),
            (
                "src/handler.ts",
                r#"
import { validate } from './utils';
function handle() { validate({}); }
"#,
            ),
        ]);

        // 1 import edge + 1 call edge = 2
        let import_count = count_edges_of_type(&graph, &EdgeType::Imports);
        let call_count = count_edges_of_type(&graph, &EdgeType::Calls);
        assert_eq!(import_count, 1, "should have 1 import edge");
        assert_eq!(call_count, 1, "should have 1 call edge");
    }

    #[test]
    fn test_cyclic_imports() {
        let graph = build_graph_from_sources(&[
            (
                "src/a.ts",
                r#"
import { funcB } from './b';
export function funcA() { funcB(); }
"#,
            ),
            (
                "src/b.ts",
                r#"
import { funcA } from './a';
export function funcB() { funcA(); }
"#,
            ),
        ]);

        // Should handle cycles without panic/infinite loop.
        assert!(graph.node_count() > 0);

        // Both import edges should exist.
        assert!(has_edge(
            &graph,
            "src/a.ts",
            "src/b.ts::funcB",
            &EdgeType::Imports
        ));
        assert!(has_edge(
            &graph,
            "src/b.ts",
            "src/a.ts::funcA",
            &EdgeType::Imports
        ));

        // Both call edges should exist.
        assert!(has_edge(
            &graph,
            "src/a.ts::funcA",
            "src/b.ts::funcB",
            &EdgeType::Calls
        ));
        assert!(has_edge(
            &graph,
            "src/b.ts::funcB",
            "src/a.ts::funcA",
            &EdgeType::Calls
        ));
    }

    #[test]
    fn test_reexport_chains() {
        let graph = build_graph_from_sources(&[
            (
                "src/core/validate.ts",
                r#"
export function validate(data: any) { return data; }
"#,
            ),
            (
                "src/core/index.ts",
                r#"
export { validate } from './validate';
"#,
            ),
            (
                "src/handler.ts",
                r#"
import { validate } from './core/index';
function handle() { validate({}); }
"#,
            ),
        ]);

        // The import from handler should resolve through the barrel file to the actual definition.
        assert!(
            has_edge(
                &graph,
                "src/handler.ts",
                "src/core/validate.ts::validate",
                &EdgeType::Imports
            ),
            "should resolve re-export chain through barrel file"
        );
    }

    #[test]
    fn test_graph_serialization_roundtrip() {
        let original = build_graph_from_sources(&[
            (
                "src/a.ts",
                r#"
export function foo() {}
"#,
            ),
            (
                "src/b.ts",
                r#"
import { foo } from './a';
function bar() { foo(); }
"#,
            ),
        ]);

        let serialized = original.to_serializable();
        let json = serde_json::to_string(&serialized).unwrap();
        let deserialized_data: SerializableGraph = serde_json::from_str(&json).unwrap();
        let restored = SymbolGraph::from_serializable(&deserialized_data);

        assert_eq!(original.node_count(), restored.node_count());
        assert_eq!(original.edge_count(), restored.edge_count());

        // Verify all nodes match.
        let orig_serialized = original.to_serializable();
        assert_eq!(orig_serialized, deserialized_data);
    }

    #[test]
    fn test_empty_files() {
        let graph = build_graph_from_sources(&[]);
        assert_eq!(graph.node_count(), 0);
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn test_single_file_no_edges() {
        let graph = build_graph_from_sources(&[(
            "src/lib.ts",
            r#"
function hello() { console.log('hi'); }
"#,
        )]);

        // 1 module node + 1 function node = 2
        assert_eq!(graph.node_count(), 2);
        // console.log is external, no edge should be created.
        assert_eq!(
            count_edges_of_type(&graph, &EdgeType::Calls),
            0,
            "external calls should not create edges"
        );
    }

    #[test]
    fn test_python_import_edges() {
        let graph = build_graph_from_sources(&[
            (
                "src/models.py",
                r#"
class User:
    def __init__(self, name):
        self.name = name
"#,
            ),
            (
                "src/service.py",
                r#"
from .models import User

def create_user(name):
    return User(name)
"#,
            ),
        ]);

        assert!(
            has_edge(
                &graph,
                "src/service.py",
                "src/models.py::User",
                &EdgeType::Imports
            ),
            "Python from-import should create import edge"
        );
    }

    #[test]
    fn test_python_call_edges() {
        let graph = build_graph_from_sources(&[
            (
                "src/utils.py",
                r#"
def validate(data):
    return data
"#,
            ),
            (
                "src/handler.py",
                r#"
from .utils import validate

def process(data):
    return validate(data)
"#,
            ),
        ]);

        assert!(
            has_edge(
                &graph,
                "src/handler.py::process",
                "src/utils.py::validate",
                &EdgeType::Calls
            ),
            "Python call should create call edge"
        );
    }

    #[test]
    fn test_cross_directory_imports() {
        let graph = build_graph_from_sources(&[
            (
                "src/models/user.ts",
                r#"
export interface User { name: string; }
"#,
            ),
            (
                "src/handlers/auth.ts",
                r#"
import { User } from '../models/user';
function login(user: User) {}
"#,
            ),
        ]);

        assert!(
            has_edge(
                &graph,
                "src/handlers/auth.ts",
                "src/models/user.ts::User",
                &EdgeType::Imports
            ),
            "should resolve cross-directory relative import with .."
        );
    }

    #[test]
    fn test_unknown_language_no_crash() {
        let graph = build_graph_from_sources(&[(
            "src/main.go",
            r#"
package main
import "fmt"
func main() { fmt.Println("hello") }
"#,
        )]);

        // Should have module node only, no definitions from unknown language.
        assert_eq!(graph.node_count(), 1);
        assert_eq!(graph.edge_count(), 0);
    }

    #[test]
    fn test_multiple_call_targets() {
        let graph = build_graph_from_sources(&[
            (
                "src/a.ts",
                r#"
export function alpha() { return 1; }
"#,
            ),
            (
                "src/b.ts",
                r#"
export function beta() { return 2; }
"#,
            ),
            (
                "src/c.ts",
                r#"
import { alpha } from './a';
import { beta } from './b';
function gamma() {
    alpha();
    beta();
}
"#,
            ),
        ]);

        assert!(has_edge(
            &graph,
            "src/c.ts::gamma",
            "src/a.ts::alpha",
            &EdgeType::Calls
        ));
        assert!(has_edge(
            &graph,
            "src/c.ts::gamma",
            "src/b.ts::beta",
            &EdgeType::Calls
        ));
    }

    #[test]
    fn test_aliased_import_call() {
        let graph = build_graph_from_sources(&[
            (
                "src/utils.ts",
                r#"
export function validate(data: any) { return data; }
"#,
            ),
            (
                "src/handler.ts",
                r#"
import { validate as check } from './utils';
function handle() { check({}); }
"#,
            ),
        ]);

        assert!(
            has_edge(
                &graph,
                "src/handler.ts::handle",
                "src/utils.ts::validate",
                &EdgeType::Calls
            ),
            "aliased import should resolve calls through the alias"
        );
    }

    #[test]
    fn test_index_file_resolution() {
        let graph = build_graph_from_sources(&[
            (
                "src/lib/index.ts",
                r#"
export function helper() { return 42; }
"#,
            ),
            (
                "src/main.ts",
                r#"
import { helper } from './lib';
function run() { helper(); }
"#,
            ),
        ]);

        // `./lib` should resolve to `src/lib/index.ts`
        assert!(
            has_edge(
                &graph,
                "src/main.ts",
                "src/lib/index.ts::helper",
                &EdgeType::Imports
            ),
            "should resolve ./lib to ./lib/index.ts"
        );
    }

    #[test]
    fn test_node_lookup() {
        let graph = build_graph_from_sources(&[(
            "src/app.ts",
            r#"
export function start() {}
export class Server {}
"#,
        )]);

        assert!(graph.get_node("src/app.ts").is_some());
        assert!(graph.get_node("src/app.ts::start").is_some());
        assert!(graph.get_node("src/app.ts::Server").is_some());
        assert!(graph.get_node("src/nonexistent.ts").is_none());

        let start = graph.get_symbol("src/app.ts::start").unwrap();
        assert_eq!(start.name, "start");
        assert_eq!(start.kind, SymbolKind::Function);
    }

    #[test]
    fn test_external_imports_no_edges() {
        let graph = build_graph_from_sources(&[(
            "src/app.ts",
            r#"
import express from 'express';
import { Router } from 'express';
const app = express();
"#,
        )]);

        // External packages (non-relative imports) should not create edges.
        assert_eq!(
            count_edges_of_type(&graph, &EdgeType::Imports),
            0,
            "external imports should not create edges"
        );
    }

    #[test]
    fn test_deterministic_output() {
        let files = &[
            (
                "src/a.ts",
                r#"
export function foo() {}
export function bar() {}
"#,
            ),
            (
                "src/b.ts",
                r#"
import { foo, bar } from './a';
function baz() { foo(); bar(); }
"#,
            ),
        ];

        let g1 = build_graph_from_sources(files);
        let g2 = build_graph_from_sources(files);

        assert_eq!(g1.node_count(), g2.node_count());
        assert_eq!(g1.edge_count(), g2.edge_count());
        assert_eq!(g1.to_serializable(), g2.to_serializable());
    }

    // === Property-based tests ===

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        /// Generate a random function name.
        fn func_name_strategy() -> impl Strategy<Value = String> {
            "[a-z][a-zA-Z0-9]{0,15}".prop_map(|s| s)
        }

        /// Generate a ParsedFile with random definitions.
        fn parsed_file_strategy() -> impl Strategy<Value = ParsedFile> {
            (
                "[a-z]{1,8}".prop_map(|s| format!("src/{}.ts", s)),
                prop::collection::vec(func_name_strategy(), 0..10),
            )
                .prop_map(|(path, func_names)| {
                    let definitions: Vec<Definition> = func_names
                        .iter()
                        .enumerate()
                        .map(|(i, name)| Definition {
                            name: name.clone(),
                            kind: SymbolKind::Function,
                            start_line: i + 1,
                            end_line: i + 3,
                        })
                        .collect();

                    ParsedFile {
                        path,
                        language: Language::TypeScript,
                        definitions,
                        imports: vec![],
                        exports: vec![],
                        call_sites: vec![],
                    }
                })
        }

        proptest! {
            #[test]
            fn prop_every_definition_has_node(files in prop::collection::vec(parsed_file_strategy(), 1..5)) {
                let graph = SymbolGraph::build(&files);

                for file in &files {
                    // Module node exists.
                    prop_assert!(graph.get_node(&file.path).is_some(),
                        "module node should exist for {}", file.path);

                    // Each unique definition has a node.
                    let mut seen = std::collections::HashSet::new();
                    for def in &file.definitions {
                        let sym_id = format!("{}::{}", file.path, def.name);
                        if seen.insert(sym_id.clone()) {
                            prop_assert!(graph.get_node(&sym_id).is_some(),
                                "node should exist for {}", sym_id);
                        }
                    }
                }
            }

            #[test]
            fn prop_node_count_at_least_file_count(files in prop::collection::vec(parsed_file_strategy(), 0..5)) {
                let graph = SymbolGraph::build(&files);
                // At minimum, one module node per file.
                prop_assert!(graph.node_count() >= files.len());
            }

            #[test]
            fn prop_no_self_edges(files in prop::collection::vec(parsed_file_strategy(), 0..5)) {
                let graph = SymbolGraph::build(&files);
                for (from, to, _) in graph.edges() {
                    prop_assert!(from != to, "self-edge found: {} -> {}", from, to);
                }
            }

            #[test]
            fn prop_serialization_roundtrip(files in prop::collection::vec(parsed_file_strategy(), 0..5)) {
                let graph = SymbolGraph::build(&files);
                let serialized = graph.to_serializable();
                let json = serde_json::to_string(&serialized).unwrap();
                let deserialized: SerializableGraph = serde_json::from_str(&json).unwrap();
                let restored = SymbolGraph::from_serializable(&deserialized);

                prop_assert_eq!(graph.node_count(), restored.node_count());
                prop_assert_eq!(graph.edge_count(), restored.edge_count());
            }

            #[test]
            fn prop_deterministic(files in prop::collection::vec(parsed_file_strategy(), 0..5)) {
                let g1 = SymbolGraph::build(&files);
                let g2 = SymbolGraph::build(&files);
                prop_assert_eq!(g1.node_count(), g2.node_count());
                prop_assert_eq!(g1.edge_count(), g2.edge_count());
            }

            #[test]
            fn prop_empty_input_empty_graph(_dummy in 0u32..1) {
                let graph = SymbolGraph::build(&[]);
                prop_assert_eq!(graph.node_count(), 0);
                prop_assert_eq!(graph.edge_count(), 0);
            }
        }
    }

    // =======================================================================
    // IR-based graph parity tests
    // =======================================================================

    mod ir_parity {
        use super::*;
        use crate::ir::IrFile;

        /// Helper: parse files and build graph via both paths, return both.
        fn build_both(files: &[(&str, &str)]) -> (SymbolGraph, SymbolGraph) {
            let parsed: Vec<ParsedFile> = files
                .iter()
                .map(|(path, source)| ast::parse_file(path, source).unwrap())
                .collect();
            let ir_files: Vec<IrFile> = parsed.iter().map(IrFile::from_parsed_file).collect();

            let graph_parsed = SymbolGraph::build(&parsed);
            let graph_ir = SymbolGraph::build_from_ir(&ir_files);
            (graph_parsed, graph_ir)
        }

        #[test]
        fn test_ir_parity_simple_import() {
            let (gp, gi) = build_both(&[
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
function handle() { validate({}); }
"#,
                ),
            ]);

            assert_eq!(
                gp.node_count(),
                gi.node_count(),
                "node counts should match"
            );
            assert_eq!(
                gp.edge_count(),
                gi.edge_count(),
                "edge counts should match"
            );
        }

        #[test]
        fn test_ir_parity_call_edges() {
            let (gp, gi) = build_both(&[
                (
                    "src/utils.ts",
                    r#"
export function validate(data: any) { return data; }
"#,
                ),
                (
                    "src/handler.ts",
                    r#"
import { validate } from './utils';
function processRequest(req: any) {
    const v = validate(req.body);
    return v;
}
"#,
                ),
            ]);

            assert_eq!(gp.node_count(), gi.node_count());
            assert_eq!(gp.edge_count(), gi.edge_count());

            // Verify specific edge exists in IR graph.
            assert!(
                has_edge(
                    &gi,
                    "src/handler.ts::processRequest",
                    "src/utils.ts::validate",
                    &EdgeType::Calls
                ),
                "IR graph should have call edge"
            );
        }

        #[test]
        fn test_ir_parity_namespace_import() {
            let (gp, gi) = build_both(&[
                (
                    "src/utils.ts",
                    r#"
export function foo() {}
export function bar() {}
"#,
                ),
                (
                    "src/main.ts",
                    r#"
import * as utils from './utils';
function main() {
    utils.foo();
}
"#,
                ),
            ]);

            assert_eq!(gp.node_count(), gi.node_count());
            assert_eq!(gp.edge_count(), gi.edge_count());
        }

        #[test]
        fn test_ir_parity_default_import() {
            let (gp, gi) = build_both(&[
                (
                    "src/utils.ts",
                    r#"
export default function doStuff() {}
"#,
                ),
                (
                    "src/main.ts",
                    r#"
import doStuff from './utils';
doStuff();
"#,
                ),
            ]);

            assert_eq!(gp.node_count(), gi.node_count());
            assert_eq!(gp.edge_count(), gi.edge_count());
        }

        #[test]
        fn test_ir_parity_python_imports() {
            let (gp, gi) = build_both(&[
                (
                    "models.py",
                    r#"
class User:
    pass

def create_user():
    pass
"#,
                ),
                (
                    "views.py",
                    r#"
from .models import User, create_user

def list_users():
    return create_user()
"#,
                ),
            ]);

            assert_eq!(gp.node_count(), gi.node_count());
            assert_eq!(gp.edge_count(), gi.edge_count());
        }

        #[test]
        fn test_ir_parity_reexport_chain() {
            let (gp, gi) = build_both(&[
                (
                    "src/core.ts",
                    r#"
export function coreFunc() {}
"#,
                ),
                (
                    "src/index.ts",
                    r#"
export { coreFunc } from './core';
"#,
                ),
                (
                    "src/consumer.ts",
                    r#"
import { coreFunc } from './index';
function use() { coreFunc(); }
"#,
                ),
            ]);

            assert_eq!(gp.node_count(), gi.node_count());
            assert_eq!(gp.edge_count(), gi.edge_count());
        }

        #[test]
        fn test_ir_parity_side_effect_import() {
            let (gp, gi) = build_both(&[
                (
                    "src/polyfill.ts",
                    r#"
export function polyfill() {}
"#,
                ),
                ("src/main.ts", r#"import './polyfill';"#),
            ]);

            assert_eq!(gp.node_count(), gi.node_count());
            assert_eq!(gp.edge_count(), gi.edge_count());
        }

        #[test]
        fn test_ir_parity_empty_input() {
            let gi = SymbolGraph::build_from_ir(&[]);
            assert_eq!(gi.node_count(), 0);
            assert_eq!(gi.edge_count(), 0);
        }

        #[test]
        fn test_ir_parity_local_call() {
            let (gp, gi) = build_both(&[(
                "src/app.ts",
                r#"
function helper() { return 42; }
function main() { helper(); }
"#,
            )]);

            assert_eq!(gp.node_count(), gi.node_count());
            assert_eq!(gp.edge_count(), gi.edge_count());

            assert!(
                has_edge(
                    &gi,
                    "src/app.ts::main",
                    "src/app.ts::helper",
                    &EdgeType::Calls
                ),
                "IR graph should have local call edge"
            );
        }

        #[test]
        fn test_ir_parity_aliased_import() {
            let (gp, gi) = build_both(&[
                (
                    "src/utils.ts",
                    r#"
export function validate() {}
"#,
                ),
                (
                    "src/main.ts",
                    r#"
import { validate as check } from './utils';
function run() { check(); }
"#,
                ),
            ]);

            assert_eq!(gp.node_count(), gi.node_count());
            assert_eq!(gp.edge_count(), gi.edge_count());
        }

        #[test]
        fn test_ir_parity_multiple_files() {
            let (gp, gi) = build_both(&[
                (
                    "src/db.ts",
                    r#"
export function query(sql: string) { return []; }
export function insert(data: any) { }
"#,
                ),
                (
                    "src/service.ts",
                    r#"
import { query, insert } from './db';
export function getUsers() { return query('SELECT * FROM users'); }
export function createUser(data: any) { insert(data); }
"#,
                ),
                (
                    "src/handler.ts",
                    r#"
import { getUsers, createUser } from './service';
function handleGet(req: any) { return getUsers(); }
function handlePost(req: any) { createUser(req.body); }
"#,
                ),
            ]);

            assert_eq!(
                gp.node_count(),
                gi.node_count(),
                "3-file graph node count should match"
            );
            assert_eq!(
                gp.edge_count(),
                gi.edge_count(),
                "3-file graph edge count should match"
            );
        }
    }

    // =======================================================================
    // IR-based graph property-based tests
    // =======================================================================

    mod ir_proptest {
        use super::*;
        use crate::ast::{Definition, Language, ParsedFile};
        use crate::ir::IrFile;
        use proptest::prelude::*;

        fn func_name_strategy() -> impl Strategy<Value = String> {
            "[a-z][a-zA-Z0-9]{0,15}".prop_map(|s| s)
        }

        fn parsed_file_strategy() -> impl Strategy<Value = ParsedFile> {
            (
                "[a-z]{1,8}".prop_map(|s| format!("src/{}.ts", s)),
                prop::collection::vec(func_name_strategy(), 0..10),
            )
                .prop_map(|(path, func_names)| {
                    let definitions: Vec<Definition> = func_names
                        .iter()
                        .enumerate()
                        .map(|(i, name)| Definition {
                            name: name.clone(),
                            kind: SymbolKind::Function,
                            start_line: i + 1,
                            end_line: i + 3,
                        })
                        .collect();

                    ParsedFile {
                        path,
                        language: Language::TypeScript,
                        definitions,
                        imports: vec![],
                        exports: vec![],
                        call_sites: vec![],
                    }
                })
        }

        proptest! {
            #[test]
            fn prop_ir_node_count_matches_parsed(files in prop::collection::vec(parsed_file_strategy(), 0..5)) {
                let ir_files: Vec<IrFile> = files.iter().map(|f| IrFile::from_parsed_file(f)).collect();
                let g_parsed = SymbolGraph::build(&files);
                let g_ir = SymbolGraph::build_from_ir(&ir_files);
                prop_assert_eq!(g_parsed.node_count(), g_ir.node_count(),
                    "node count mismatch: parsed={}, ir={}", g_parsed.node_count(), g_ir.node_count());
            }

            #[test]
            fn prop_ir_edge_count_matches_parsed(files in prop::collection::vec(parsed_file_strategy(), 0..5)) {
                let ir_files: Vec<IrFile> = files.iter().map(|f| IrFile::from_parsed_file(f)).collect();
                let g_parsed = SymbolGraph::build(&files);
                let g_ir = SymbolGraph::build_from_ir(&ir_files);
                prop_assert_eq!(g_parsed.edge_count(), g_ir.edge_count());
            }

            #[test]
            fn prop_ir_no_self_edges(files in prop::collection::vec(parsed_file_strategy(), 0..5)) {
                let ir_files: Vec<IrFile> = files.iter().map(|f| IrFile::from_parsed_file(f)).collect();
                let graph = SymbolGraph::build_from_ir(&ir_files);
                for (from, to, _) in graph.edges() {
                    prop_assert!(from != to, "self-edge found in IR graph: {} -> {}", from, to);
                }
            }

            #[test]
            fn prop_ir_deterministic(files in prop::collection::vec(parsed_file_strategy(), 0..5)) {
                let ir_files: Vec<IrFile> = files.iter().map(|f| IrFile::from_parsed_file(f)).collect();
                let g1 = SymbolGraph::build_from_ir(&ir_files);
                let g2 = SymbolGraph::build_from_ir(&ir_files);
                prop_assert_eq!(g1.node_count(), g2.node_count());
                prop_assert_eq!(g1.edge_count(), g2.edge_count());
            }

            #[test]
            fn prop_ir_empty_input_empty_graph(_dummy in 0u32..1) {
                let graph = SymbolGraph::build_from_ir(&[]);
                prop_assert_eq!(graph.node_count(), 0);
                prop_assert_eq!(graph.edge_count(), 0);
            }

            #[test]
            fn prop_ir_every_definition_has_node(files in prop::collection::vec(parsed_file_strategy(), 1..5)) {
                let ir_files: Vec<IrFile> = files.iter().map(|f| IrFile::from_parsed_file(f)).collect();
                let graph = SymbolGraph::build_from_ir(&ir_files);

                for ir_file in &ir_files {
                    prop_assert!(graph.get_node(&ir_file.path).is_some(),
                        "module node should exist for {}", ir_file.path);

                    let mut seen = std::collections::HashSet::new();
                    for func in &ir_file.functions {
                        let sym_id = format!("{}::{}", ir_file.path, func.name);
                        if seen.insert(sym_id.clone()) {
                            prop_assert!(graph.get_node(&sym_id).is_some(),
                                "node should exist for function {}", sym_id);
                        }
                    }
                    for td in &ir_file.type_defs {
                        let sym_id = format!("{}::{}", ir_file.path, td.name);
                        if seen.insert(sym_id.clone()) {
                            prop_assert!(graph.get_node(&sym_id).is_some(),
                                "node should exist for type def {}", sym_id);
                        }
                    }
                    for c in &ir_file.constants {
                        let sym_id = format!("{}::{}", ir_file.path, c.name);
                        if seen.insert(sym_id.clone()) {
                            prop_assert!(graph.get_node(&sym_id).is_some(),
                                "node should exist for constant {}", sym_id);
                        }
                    }
                }
            }
        }
    }
}
