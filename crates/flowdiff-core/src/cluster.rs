//! Semantic clustering: groups changed files into flow groups.
//!
//! Algorithm (from spec §4.6):
//! 1. For each entrypoint, compute forward reachability via BFS on the symbol graph
//! 2. Intersect each reachability set with the changed file set ΔF
//! 3. Files reachable from the same entrypoint and in ΔF belong to the same flow group
//! 4. Files in ΔF not reachable from any entrypoint form an "infrastructure/shared" group
//! 5. Files reachable from multiple entrypoints get assigned to the group with shortest path

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use petgraph::Direction;

use crate::graph::SymbolGraph;
use crate::types::{
    ChangeStats, Entrypoint, EntrypointType, FileChange, FileRole, FlowEdge, FlowGroup,
    InfrastructureGroup,
};

/// Result of semantic clustering.
#[derive(Debug, Clone, PartialEq)]
pub struct ClusterResult {
    pub groups: Vec<FlowGroup>,
    pub infrastructure: Option<InfrastructureGroup>,
}

/// Cluster changed files into semantic flow groups based on entrypoint reachability.
///
/// Each entrypoint seeds a BFS through the symbol graph. Changed files reachable from
/// an entrypoint join that entrypoint's flow group. Files reachable from multiple
/// entrypoints are assigned to the nearest one (shortest graph distance). Unreachable
/// files are placed in the infrastructure group.
pub fn cluster_files(
    graph: &SymbolGraph,
    entrypoints: &[Entrypoint],
    changed_files: &[String],
) -> ClusterResult {
    if changed_files.is_empty() {
        return ClusterResult {
            groups: vec![],
            infrastructure: None,
        };
    }

    // Deduplicate and sort changed files for determinism.
    let changed_set: Vec<String> = {
        let mut s: Vec<String> = changed_files.to_vec();
        s.sort();
        s.dedup();
        s
    };

    if entrypoints.is_empty() {
        return ClusterResult {
            groups: vec![],
            infrastructure: Some(InfrastructureGroup {
                files: changed_set,
                reason: "Not reachable from any detected entrypoint".to_string(),
            }),
        };
    }

    // Step 1: Compute file-level reachability for each entrypoint.
    let reachability: Vec<HashMap<String, usize>> = entrypoints
        .iter()
        .map(|ep| {
            let mut reach = compute_file_reachability(graph, &ep.file, &ep.symbol);
            // The entrypoint file is always reachable at distance 0.
            reach.entry(ep.file.clone()).or_insert(0);
            reach
        })
        .collect();

    // Step 2: Assign each changed file to the nearest entrypoint.
    let mut assignments: BTreeMap<String, (usize, usize)> = BTreeMap::new(); // file -> (ep_idx, dist)
    let mut infra_files: Vec<String> = Vec::new();

    for file in &changed_set {
        let mut best: Option<(usize, usize)> = None;

        for (ep_idx, reach) in reachability.iter().enumerate() {
            if let Some(&dist) = reach.get(file.as_str()) {
                match best {
                    None => best = Some((ep_idx, dist)),
                    Some((best_ep, best_dist)) => {
                        if dist < best_dist || (dist == best_dist && ep_idx < best_ep) {
                            best = Some((ep_idx, dist));
                        }
                    }
                }
            }
        }

        match best {
            Some(assignment) => {
                assignments.insert(file.clone(), assignment);
            }
            None => {
                infra_files.push(file.clone());
            }
        }
    }

    // Step 3: Group assigned files by entrypoint.
    let mut group_map: BTreeMap<usize, Vec<(String, usize)>> = BTreeMap::new();
    for (file, (ep_idx, dist)) in &assignments {
        group_map
            .entry(*ep_idx)
            .or_default()
            .push((file.clone(), *dist));
    }

    // Step 4: Build FlowGroup for each entrypoint that has changed files.
    let mut groups: Vec<FlowGroup> = Vec::new();
    for (group_num, (ep_idx, mut files)) in group_map.into_iter().enumerate() {
        let ep = &entrypoints[ep_idx];

        // Sort files by flow position (BFS distance), then alphabetically for determinism.
        files.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));

        let file_changes: Vec<FileChange> = files
            .iter()
            .enumerate()
            .map(|(pos, (path, _))| {
                let role = if *path == ep.file {
                    FileRole::Entrypoint
                } else {
                    infer_file_role(path)
                };
                FileChange {
                    path: path.clone(),
                    flow_position: pos as u32,
                    role,
                    changes: ChangeStats {
                        additions: 0,
                        deletions: 0,
                    },
                    symbols_changed: vec![],
                }
            })
            .collect();

        // Collect edges internal to this group.
        let group_file_set: HashSet<&str> = files.iter().map(|(f, _)| f.as_str()).collect();
        let edges = collect_internal_edges(graph, &group_file_set);

        groups.push(FlowGroup {
            id: format!("group_{}", group_num + 1),
            name: generate_group_name(ep),
            entrypoint: Some(ep.clone()),
            files: file_changes,
            edges,
            risk_score: 0.0,
            review_order: 0,
        });
    }

    let infrastructure = if infra_files.is_empty() {
        None
    } else {
        Some(InfrastructureGroup {
            files: infra_files,
            reason: "Not reachable from any detected entrypoint".to_string(),
        })
    };

    ClusterResult {
        groups,
        infrastructure,
    }
}

/// BFS forward from an entrypoint, returning file_path → minimum graph distance.
fn compute_file_reachability(
    graph: &SymbolGraph,
    entry_file: &str,
    entry_symbol: &str,
) -> HashMap<String, usize> {
    let mut file_distances: HashMap<String, usize> = HashMap::new();
    let mut visited: HashSet<petgraph::graph::NodeIndex> = HashSet::new();
    let mut queue: VecDeque<(petgraph::graph::NodeIndex, usize)> = VecDeque::new();

    // Seed BFS from the entrypoint symbol node and the file's module node.
    let symbol_id = format!("{}::{}", entry_file, entry_symbol);
    if let Some(idx) = graph.get_node(&symbol_id) {
        queue.push_back((idx, 0));
        visited.insert(idx);
    }
    if let Some(idx) = graph.get_node(entry_file) {
        if visited.insert(idx) {
            queue.push_back((idx, 0));
        }
    }

    while let Some((node, dist)) = queue.pop_front() {
        let sym = &graph.graph[node];
        let file = &sym.file;

        let entry = file_distances.entry(file.clone()).or_insert(dist);
        if dist < *entry {
            *entry = dist;
        }

        for neighbor in graph.graph.neighbors_directed(node, Direction::Outgoing) {
            if visited.insert(neighbor) {
                queue.push_back((neighbor, dist + 1));
            }
        }
    }

    file_distances
}

/// Collect all graph edges where both endpoints belong to files in the group.
fn collect_internal_edges(graph: &SymbolGraph, group_files: &HashSet<&str>) -> Vec<FlowEdge> {
    let mut edges: Vec<FlowEdge> = graph
        .edges()
        .into_iter()
        .filter_map(|(from, to, edge_type)| {
            let from_file = graph.get_symbol(from).map(|s| s.file.as_str())?;
            let to_file = graph.get_symbol(to).map(|s| s.file.as_str())?;
            if group_files.contains(from_file) && group_files.contains(to_file) {
                Some(FlowEdge {
                    from: from.to_string(),
                    to: to.to_string(),
                    edge_type: edge_type.clone(),
                })
            } else {
                None
            }
        })
        .collect();

    // Sort for deterministic output.
    edges.sort_by(|a, b| a.from.cmp(&b.from).then_with(|| a.to.cmp(&b.to)));
    edges
}

/// Generate a human-readable name for a flow group based on its entrypoint.
fn generate_group_name(ep: &Entrypoint) -> String {
    match ep.entrypoint_type {
        EntrypointType::HttpRoute => format!("{} route flow", ep.symbol),
        EntrypointType::CliCommand => format!("{} CLI flow", ep.symbol),
        EntrypointType::QueueConsumer => format!("{} consumer flow", ep.symbol),
        EntrypointType::CronJob => format!("{} scheduled flow", ep.symbol),
        EntrypointType::ReactPage => format!("{} page flow", ep.symbol),
        EntrypointType::TestFile => format!("{} test flow", ep.symbol),
        EntrypointType::EventHandler => format!("{} event flow", ep.symbol),
    }
}

/// Infer a file's role from its path using heuristic patterns.
fn infer_file_role(path: &str) -> FileRole {
    let lower = path.to_lowercase();
    if lower.contains("handler") || lower.contains("controller") || lower.contains("route") {
        FileRole::Handler
    } else if lower.contains("service") {
        FileRole::Service
    } else if lower.contains("repo") || lower.contains("repository") || lower.contains("dal") {
        FileRole::Repository
    } else if lower.contains("model") || lower.contains("schema") || lower.contains("entity") {
        FileRole::Model
    } else if lower.contains("config") || lower.contains("setting") {
        FileRole::Config
    } else if lower.contains("test") || lower.contains("spec") {
        FileRole::Test
    } else if lower.contains("util") || lower.contains("helper") || lower.contains("lib") {
        FileRole::Utility
    } else {
        FileRole::Infrastructure
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{SerializableEdge, SerializableGraph, SymbolGraph, SymbolNode};
    use crate::types::{EdgeType, EntrypointType, SymbolKind};

    /// Helper: build a SymbolGraph from explicit nodes and edges.
    fn make_graph(
        nodes: &[(&str, &str, SymbolKind)],
        edges: &[(&str, &str, EdgeType)],
    ) -> SymbolGraph {
        let sg = SerializableGraph {
            nodes: nodes
                .iter()
                .map(|(id, file, kind)| SymbolNode {
                    id: id.to_string(),
                    name: id.rsplit("::").next().unwrap_or(id).to_string(),
                    file: file.to_string(),
                    kind: kind.clone(),
                })
                .collect(),
            edges: edges
                .iter()
                .map(|(from, to, et)| SerializableEdge {
                    from: from.to_string(),
                    to: to.to_string(),
                    edge_type: et.clone(),
                })
                .collect(),
        };
        SymbolGraph::from_serializable(&sg)
    }

    fn ep(file: &str, symbol: &str, ep_type: EntrypointType) -> Entrypoint {
        Entrypoint {
            file: file.to_string(),
            symbol: symbol.to_string(),
            entrypoint_type: ep_type,
        }
    }

    fn changed(files: &[&str]) -> Vec<String> {
        files.iter().map(|s| s.to_string()).collect()
    }

    // ===================================================================
    // Unit tests from spec §12.2 — Cluster Layer
    // ===================================================================

    #[test]
    fn test_single_entrypoint_group() {
        // route.ts → service.ts → repo.ts (linear chain)
        let graph = make_graph(
            &[
                ("src/route.ts", "src/route.ts", SymbolKind::Module),
                ("src/route.ts::handlePost", "src/route.ts", SymbolKind::Function),
                ("src/service.ts", "src/service.ts", SymbolKind::Module),
                ("src/service.ts::createUser", "src/service.ts", SymbolKind::Function),
                ("src/repo.ts", "src/repo.ts", SymbolKind::Module),
                ("src/repo.ts::insert", "src/repo.ts", SymbolKind::Function),
            ],
            &[
                ("src/route.ts", "src/service.ts::createUser", EdgeType::Imports),
                ("src/route.ts::handlePost", "src/service.ts::createUser", EdgeType::Calls),
                ("src/service.ts", "src/repo.ts::insert", EdgeType::Imports),
                ("src/service.ts::createUser", "src/repo.ts::insert", EdgeType::Calls),
            ],
        );

        let entrypoints = vec![ep("src/route.ts", "handlePost", EntrypointType::HttpRoute)];
        let files = changed(&["src/route.ts", "src/service.ts", "src/repo.ts"]);

        let result = cluster_files(&graph, &entrypoints, &files);

        assert_eq!(result.groups.len(), 1, "should produce exactly one group");
        assert!(result.infrastructure.is_none(), "no infrastructure files");

        let group = &result.groups[0];
        assert_eq!(group.files.len(), 3, "all three files in group");
        assert_eq!(group.entrypoint.as_ref().unwrap().file, "src/route.ts");

        // Verify flow ordering: route first, then service, then repo.
        let paths: Vec<&str> = group.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths[0], "src/route.ts");
        assert_eq!(paths[1], "src/service.ts");
        assert_eq!(paths[2], "src/repo.ts");
    }

    #[test]
    fn test_multiple_entrypoints() {
        // Two independent chains: route_a → service_a, route_b → service_b
        let graph = make_graph(
            &[
                ("src/route_a.ts", "src/route_a.ts", SymbolKind::Module),
                ("src/route_a.ts::getUsers", "src/route_a.ts", SymbolKind::Function),
                ("src/service_a.ts", "src/service_a.ts", SymbolKind::Module),
                ("src/service_a.ts::listUsers", "src/service_a.ts", SymbolKind::Function),
                ("src/route_b.ts", "src/route_b.ts", SymbolKind::Module),
                ("src/route_b.ts::getOrders", "src/route_b.ts", SymbolKind::Function),
                ("src/service_b.ts", "src/service_b.ts", SymbolKind::Module),
                ("src/service_b.ts::listOrders", "src/service_b.ts", SymbolKind::Function),
            ],
            &[
                ("src/route_a.ts::getUsers", "src/service_a.ts::listUsers", EdgeType::Calls),
                ("src/route_a.ts", "src/service_a.ts::listUsers", EdgeType::Imports),
                ("src/route_b.ts::getOrders", "src/service_b.ts::listOrders", EdgeType::Calls),
                ("src/route_b.ts", "src/service_b.ts::listOrders", EdgeType::Imports),
            ],
        );

        let entrypoints = vec![
            ep("src/route_a.ts", "getUsers", EntrypointType::HttpRoute),
            ep("src/route_b.ts", "getOrders", EntrypointType::HttpRoute),
        ];
        let files = changed(&[
            "src/route_a.ts",
            "src/service_a.ts",
            "src/route_b.ts",
            "src/service_b.ts",
        ]);

        let result = cluster_files(&graph, &entrypoints, &files);

        assert_eq!(result.groups.len(), 2, "should produce two groups");
        assert!(result.infrastructure.is_none());

        // Group 1 has route_a + service_a.
        let g1_files: Vec<&str> = result.groups[0]
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        assert!(g1_files.contains(&"src/route_a.ts"));
        assert!(g1_files.contains(&"src/service_a.ts"));

        // Group 2 has route_b + service_b.
        let g2_files: Vec<&str> = result.groups[1]
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        assert!(g2_files.contains(&"src/route_b.ts"));
        assert!(g2_files.contains(&"src/service_b.ts"));
    }

    #[test]
    fn test_shared_file_assignment() {
        // route_a.ts → utils.ts (distance 1)
        // route_b.ts → other.ts → utils.ts (distance 2 from route_b)
        // utils.ts should be assigned to route_a (shorter path).
        let graph = make_graph(
            &[
                ("src/route_a.ts", "src/route_a.ts", SymbolKind::Module),
                ("src/route_a.ts::handleA", "src/route_a.ts", SymbolKind::Function),
                ("src/route_b.ts", "src/route_b.ts", SymbolKind::Module),
                ("src/route_b.ts::handleB", "src/route_b.ts", SymbolKind::Function),
                ("src/other.ts", "src/other.ts", SymbolKind::Module),
                ("src/other.ts::transform", "src/other.ts", SymbolKind::Function),
                ("src/utils.ts", "src/utils.ts", SymbolKind::Module),
                ("src/utils.ts::validate", "src/utils.ts", SymbolKind::Function),
            ],
            &[
                // route_a directly imports utils
                ("src/route_a.ts", "src/utils.ts::validate", EdgeType::Imports),
                ("src/route_a.ts::handleA", "src/utils.ts::validate", EdgeType::Calls),
                // route_b → other → utils (longer chain)
                ("src/route_b.ts", "src/other.ts::transform", EdgeType::Imports),
                ("src/route_b.ts::handleB", "src/other.ts::transform", EdgeType::Calls),
                ("src/other.ts", "src/utils.ts::validate", EdgeType::Imports),
                ("src/other.ts::transform", "src/utils.ts::validate", EdgeType::Calls),
            ],
        );

        let entrypoints = vec![
            ep("src/route_a.ts", "handleA", EntrypointType::HttpRoute),
            ep("src/route_b.ts", "handleB", EntrypointType::HttpRoute),
        ];
        let files = changed(&[
            "src/route_a.ts",
            "src/route_b.ts",
            "src/other.ts",
            "src/utils.ts",
        ]);

        let result = cluster_files(&graph, &entrypoints, &files);

        assert_eq!(result.groups.len(), 2);
        assert!(result.infrastructure.is_none());

        // Find which group has route_a as entrypoint.
        let group_a = result
            .groups
            .iter()
            .find(|g| g.entrypoint.as_ref().unwrap().file == "src/route_a.ts")
            .expect("should have group for route_a");
        let group_a_files: Vec<&str> = group_a.files.iter().map(|f| f.path.as_str()).collect();

        // utils.ts should be in route_a's group (shorter path).
        assert!(
            group_a_files.contains(&"src/utils.ts"),
            "utils.ts should be assigned to route_a group (shorter path)"
        );

        // other.ts should be in route_b's group.
        let group_b = result
            .groups
            .iter()
            .find(|g| g.entrypoint.as_ref().unwrap().file == "src/route_b.ts")
            .expect("should have group for route_b");
        let group_b_files: Vec<&str> = group_b.files.iter().map(|f| f.path.as_str()).collect();
        assert!(
            group_b_files.contains(&"src/other.ts"),
            "other.ts should be in route_b group"
        );
    }

    #[test]
    fn test_infrastructure_group() {
        // route.ts → service.ts (connected via entrypoint)
        // config.ts (isolated, not reachable from entrypoint)
        let graph = make_graph(
            &[
                ("src/route.ts", "src/route.ts", SymbolKind::Module),
                ("src/route.ts::handle", "src/route.ts", SymbolKind::Function),
                ("src/service.ts", "src/service.ts", SymbolKind::Module),
                ("src/service.ts::process", "src/service.ts", SymbolKind::Function),
                ("src/config.ts", "src/config.ts", SymbolKind::Module),
            ],
            &[
                ("src/route.ts", "src/service.ts::process", EdgeType::Imports),
                ("src/route.ts::handle", "src/service.ts::process", EdgeType::Calls),
            ],
        );

        let entrypoints = vec![ep("src/route.ts", "handle", EntrypointType::HttpRoute)];
        let files = changed(&["src/route.ts", "src/service.ts", "src/config.ts"]);

        let result = cluster_files(&graph, &entrypoints, &files);

        assert_eq!(result.groups.len(), 1);

        let group_files: Vec<&str> = result.groups[0]
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        assert!(group_files.contains(&"src/route.ts"));
        assert!(group_files.contains(&"src/service.ts"));
        assert!(!group_files.contains(&"src/config.ts"));

        let infra = result.infrastructure.as_ref().expect("should have infrastructure group");
        assert!(infra.files.contains(&"src/config.ts".to_string()));
    }

    #[test]
    fn test_empty_diff() {
        let graph = make_graph(&[], &[]);
        let result = cluster_files(&graph, &[], &[]);

        assert!(result.groups.is_empty());
        assert!(result.infrastructure.is_none());
    }

    #[test]
    fn test_all_infrastructure() {
        // No entrypoints → everything goes to infrastructure.
        let graph = make_graph(
            &[
                ("src/a.ts", "src/a.ts", SymbolKind::Module),
                ("src/b.ts", "src/b.ts", SymbolKind::Module),
            ],
            &[("src/a.ts", "src/b.ts", EdgeType::Imports)],
        );

        let files = changed(&["src/a.ts", "src/b.ts"]);
        let result = cluster_files(&graph, &[], &files);

        assert!(result.groups.is_empty());
        let infra = result.infrastructure.as_ref().expect("should have infrastructure");
        assert_eq!(infra.files.len(), 2);
    }

    #[test]
    fn test_disconnected_components() {
        // Three isolated files, no edges between them, no entrypoints.
        let graph = make_graph(
            &[
                ("src/a.ts", "src/a.ts", SymbolKind::Module),
                ("src/b.ts", "src/b.ts", SymbolKind::Module),
                ("src/c.ts", "src/c.ts", SymbolKind::Module),
            ],
            &[],
        );

        let files = changed(&["src/a.ts", "src/b.ts", "src/c.ts"]);
        let result = cluster_files(&graph, &[], &files);

        assert!(result.groups.is_empty());
        let infra = result.infrastructure.as_ref().expect("should have infrastructure");
        assert_eq!(infra.files.len(), 3);
    }

    #[test]
    fn test_group_file_ordering() {
        // Linear chain: entry → mid → leaf. Files should be ordered by BFS distance.
        let graph = make_graph(
            &[
                ("src/entry.ts", "src/entry.ts", SymbolKind::Module),
                ("src/entry.ts::start", "src/entry.ts", SymbolKind::Function),
                ("src/mid.ts", "src/mid.ts", SymbolKind::Module),
                ("src/mid.ts::transform", "src/mid.ts", SymbolKind::Function),
                ("src/leaf.ts", "src/leaf.ts", SymbolKind::Module),
                ("src/leaf.ts::persist", "src/leaf.ts", SymbolKind::Function),
            ],
            &[
                ("src/entry.ts", "src/mid.ts::transform", EdgeType::Imports),
                ("src/entry.ts::start", "src/mid.ts::transform", EdgeType::Calls),
                ("src/mid.ts", "src/leaf.ts::persist", EdgeType::Imports),
                ("src/mid.ts::transform", "src/leaf.ts::persist", EdgeType::Calls),
            ],
        );

        let entrypoints = vec![ep("src/entry.ts", "start", EntrypointType::HttpRoute)];
        let files = changed(&["src/entry.ts", "src/mid.ts", "src/leaf.ts"]);

        let result = cluster_files(&graph, &entrypoints, &files);

        assert_eq!(result.groups.len(), 1);
        let paths: Vec<&str> = result.groups[0]
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect();

        // Entrypoint first, then downstream in flow order.
        assert_eq!(paths, vec!["src/entry.ts", "src/mid.ts", "src/leaf.ts"]);

        // Verify flow_position values are sequential.
        for (i, fc) in result.groups[0].files.iter().enumerate() {
            assert_eq!(fc.flow_position, i as u32);
        }
    }

    #[test]
    fn test_deterministic_output() {
        let graph = make_graph(
            &[
                ("src/route.ts", "src/route.ts", SymbolKind::Module),
                ("src/route.ts::handle", "src/route.ts", SymbolKind::Function),
                ("src/service.ts", "src/service.ts", SymbolKind::Module),
                ("src/service.ts::process", "src/service.ts", SymbolKind::Function),
                ("src/repo.ts", "src/repo.ts", SymbolKind::Module),
                ("src/repo.ts::save", "src/repo.ts", SymbolKind::Function),
            ],
            &[
                ("src/route.ts", "src/service.ts::process", EdgeType::Imports),
                ("src/route.ts::handle", "src/service.ts::process", EdgeType::Calls),
                ("src/service.ts", "src/repo.ts::save", EdgeType::Imports),
                ("src/service.ts::process", "src/repo.ts::save", EdgeType::Calls),
            ],
        );

        let entrypoints = vec![ep("src/route.ts", "handle", EntrypointType::HttpRoute)];
        let files = changed(&["src/route.ts", "src/service.ts", "src/repo.ts"]);

        // Run 10 times and verify identical output.
        let baseline = cluster_files(&graph, &entrypoints, &files);
        for _ in 0..10 {
            let result = cluster_files(&graph, &entrypoints, &files);
            assert_eq!(result, baseline, "output must be deterministic");
        }
    }

    // ===================================================================
    // Additional edge case tests
    // ===================================================================

    #[test]
    fn test_entrypoint_file_not_in_changed_set() {
        // Entrypoint file didn't change, but downstream files did.
        let graph = make_graph(
            &[
                ("src/route.ts", "src/route.ts", SymbolKind::Module),
                ("src/route.ts::handle", "src/route.ts", SymbolKind::Function),
                ("src/service.ts", "src/service.ts", SymbolKind::Module),
                ("src/service.ts::process", "src/service.ts", SymbolKind::Function),
            ],
            &[
                ("src/route.ts", "src/service.ts::process", EdgeType::Imports),
                ("src/route.ts::handle", "src/service.ts::process", EdgeType::Calls),
            ],
        );

        let entrypoints = vec![ep("src/route.ts", "handle", EntrypointType::HttpRoute)];
        // Only service.ts changed, not route.ts.
        let files = changed(&["src/service.ts"]);

        let result = cluster_files(&graph, &entrypoints, &files);

        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups[0].files.len(), 1);
        assert_eq!(result.groups[0].files[0].path, "src/service.ts");
    }

    #[test]
    fn test_multiple_entrypoints_same_file() {
        // Two entrypoints in the same file (e.g., GET and POST handlers).
        let graph = make_graph(
            &[
                ("src/route.ts", "src/route.ts", SymbolKind::Module),
                ("src/route.ts::getHandler", "src/route.ts", SymbolKind::Function),
                ("src/route.ts::postHandler", "src/route.ts", SymbolKind::Function),
                ("src/read_service.ts", "src/read_service.ts", SymbolKind::Module),
                ("src/read_service.ts::fetchAll", "src/read_service.ts", SymbolKind::Function),
                ("src/write_service.ts", "src/write_service.ts", SymbolKind::Module),
                ("src/write_service.ts::create", "src/write_service.ts", SymbolKind::Function),
            ],
            &[
                ("src/route.ts::getHandler", "src/read_service.ts::fetchAll", EdgeType::Calls),
                ("src/route.ts::postHandler", "src/write_service.ts::create", EdgeType::Calls),
            ],
        );

        let entrypoints = vec![
            ep("src/route.ts", "getHandler", EntrypointType::HttpRoute),
            ep("src/route.ts", "postHandler", EntrypointType::HttpRoute),
        ];
        let files = changed(&[
            "src/route.ts",
            "src/read_service.ts",
            "src/write_service.ts",
        ]);

        let result = cluster_files(&graph, &entrypoints, &files);

        // route.ts is assigned to the first entrypoint (tie-break by index).
        // Each service should go to its respective entrypoint's group.
        assert_eq!(result.groups.len(), 2);
        assert!(result.infrastructure.is_none());
    }

    #[test]
    fn test_entrypoint_not_in_graph() {
        // Entrypoint file exists but has no graph nodes (e.g., unparsed language).
        let graph = make_graph(&[], &[]);

        let entrypoints = vec![ep("src/main.rs", "main", EntrypointType::CliCommand)];
        let files = changed(&["src/main.rs", "src/other.rs"]);

        let result = cluster_files(&graph, &entrypoints, &files);

        // main.rs should be in the group (entrypoint file always at distance 0).
        // other.rs is unreachable → infrastructure.
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups[0].files.len(), 1);
        assert_eq!(result.groups[0].files[0].path, "src/main.rs");

        let infra = result.infrastructure.as_ref().expect("should have infrastructure");
        assert!(infra.files.contains(&"src/other.rs".to_string()));
    }

    #[test]
    fn test_group_has_internal_edges() {
        let graph = make_graph(
            &[
                ("src/route.ts", "src/route.ts", SymbolKind::Module),
                ("src/route.ts::handle", "src/route.ts", SymbolKind::Function),
                ("src/service.ts", "src/service.ts", SymbolKind::Module),
                ("src/service.ts::process", "src/service.ts", SymbolKind::Function),
            ],
            &[
                ("src/route.ts", "src/service.ts::process", EdgeType::Imports),
                ("src/route.ts::handle", "src/service.ts::process", EdgeType::Calls),
            ],
        );

        let entrypoints = vec![ep("src/route.ts", "handle", EntrypointType::HttpRoute)];
        let files = changed(&["src/route.ts", "src/service.ts"]);

        let result = cluster_files(&graph, &entrypoints, &files);
        let edges = &result.groups[0].edges;

        assert!(!edges.is_empty(), "group should have internal edges");
        assert!(
            edges.iter().any(|e| e.edge_type == EdgeType::Calls),
            "should include call edges"
        );
        assert!(
            edges.iter().any(|e| e.edge_type == EdgeType::Imports),
            "should include import edges"
        );
    }

    #[test]
    fn test_file_role_inference() {
        assert_eq!(infer_file_role("src/handlers/auth.ts"), FileRole::Handler);
        assert_eq!(infer_file_role("src/controllers/user.ts"), FileRole::Handler);
        assert_eq!(infer_file_role("src/services/user.ts"), FileRole::Service);
        assert_eq!(infer_file_role("src/repository/user.ts"), FileRole::Repository);
        assert_eq!(infer_file_role("src/models/user.ts"), FileRole::Model);
        assert_eq!(infer_file_role("src/config/db.ts"), FileRole::Config);
        assert_eq!(infer_file_role("src/__tests__/auth.ts"), FileRole::Test);
        assert_eq!(infer_file_role("src/utils/hash.ts"), FileRole::Utility);
        assert_eq!(infer_file_role("src/lib/crypto.ts"), FileRole::Utility);
        assert_eq!(infer_file_role("src/main.ts"), FileRole::Infrastructure);
    }

    #[test]
    fn test_group_name_generation() {
        assert_eq!(
            generate_group_name(&ep("f", "POST", EntrypointType::HttpRoute)),
            "POST route flow"
        );
        assert_eq!(
            generate_group_name(&ep("f", "main", EntrypointType::CliCommand)),
            "main CLI flow"
        );
        assert_eq!(
            generate_group_name(&ep("f", "processQueue", EntrypointType::QueueConsumer)),
            "processQueue consumer flow"
        );
        assert_eq!(
            generate_group_name(&ep("f", "cleanup", EntrypointType::CronJob)),
            "cleanup scheduled flow"
        );
        assert_eq!(
            generate_group_name(&ep("f", "HomePage", EntrypointType::ReactPage)),
            "HomePage page flow"
        );
    }

    #[test]
    fn test_duplicate_changed_files() {
        // Duplicate entries in changed_files should be deduplicated.
        let graph = make_graph(
            &[("src/a.ts", "src/a.ts", SymbolKind::Module)],
            &[],
        );

        let entrypoints = vec![ep("src/a.ts", "main", EntrypointType::CliCommand)];
        let files = changed(&["src/a.ts", "src/a.ts", "src/a.ts"]);

        let result = cluster_files(&graph, &entrypoints, &files);
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups[0].files.len(), 1);
    }

    // ===================================================================
    // Property-based tests
    // ===================================================================

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        fn file_path_strategy() -> impl Strategy<Value = String> {
            "[a-z]{1,5}"
                .prop_map(|name| format!("src/{}.ts", name))
        }

        fn changed_files_strategy() -> impl Strategy<Value = Vec<String>> {
            prop::collection::vec(file_path_strategy(), 1..8)
        }

        proptest! {
            /// Every changed file appears in exactly one group or in infrastructure.
            #[test]
            fn prop_every_file_in_exactly_one_group(
                files in changed_files_strategy()
            ) {
                let graph = make_graph(&[], &[]);
                // No entrypoints → all infra. But principle still holds.
                let result = cluster_files(&graph, &[], &files);

                let mut all_assigned: Vec<String> = Vec::new();
                for group in &result.groups {
                    for fc in &group.files {
                        all_assigned.push(fc.path.clone());
                    }
                }
                if let Some(ref infra) = result.infrastructure {
                    all_assigned.extend(infra.files.clone());
                }

                // Deduplicate changed_files for comparison.
                let mut expected: Vec<String> = files.clone();
                expected.sort();
                expected.dedup();
                all_assigned.sort();

                prop_assert_eq!(all_assigned, expected,
                    "every changed file must appear in exactly one place");
            }

            /// Empty diff always produces empty result.
            #[test]
            fn prop_empty_diff_empty_result(_dummy in 0u32..1) {
                let graph = make_graph(&[], &[]);
                let result = cluster_files(&graph, &[], &[]);
                prop_assert!(result.groups.is_empty());
                prop_assert!(result.infrastructure.is_none());
            }

            /// Single file diff with entrypoint produces single group.
            #[test]
            fn prop_single_file_single_group(
                path in file_path_strategy()
            ) {
                let graph = make_graph(
                    &[(&path, &path, SymbolKind::Module)],
                    &[],
                );
                let entrypoints = vec![ep(&path, "main", EntrypointType::CliCommand)];
                let files = vec![path.clone()];

                let result = cluster_files(&graph, &entrypoints, &files);

                prop_assert_eq!(result.groups.len(), 1, "single file should produce one group");
                prop_assert_eq!(result.groups[0].files.len(), 1);
                prop_assert!(result.infrastructure.is_none());
            }

            /// No entrypoints → all files in infrastructure.
            #[test]
            fn prop_no_entrypoints_all_infra(
                files in changed_files_strategy()
            ) {
                let graph = make_graph(&[], &[]);
                let result = cluster_files(&graph, &[], &files);

                prop_assert!(result.groups.is_empty(),
                    "no entrypoints should produce no groups");

                let infra = result.infrastructure.as_ref().unwrap();
                let mut expected: Vec<String> = files.clone();
                expected.sort();
                expected.dedup();
                prop_assert_eq!(&infra.files, &expected);
            }

            /// Clustering is deterministic: same input → same output.
            #[test]
            fn prop_deterministic(
                files in changed_files_strategy()
            ) {
                let graph = make_graph(&[], &[]);
                let eps: Vec<Entrypoint> = vec![];
                let r1 = cluster_files(&graph, &eps, &files);
                let r2 = cluster_files(&graph, &eps, &files);
                prop_assert_eq!(r1, r2, "must be deterministic");
            }

            /// Graph with no edges and entrypoints → only entrypoint files in groups,
            /// rest in infrastructure.
            #[test]
            fn prop_no_edges_only_entrypoint_files_in_groups(
                ep_file in file_path_strategy(),
                other_files in prop::collection::vec(file_path_strategy(), 1..5)
            ) {
                // Ensure ep_file is different from other_files.
                let ep_file_str = format!("src/ep_{}.ts", &ep_file[4..]);
                let graph = make_graph(
                    &[(&ep_file_str, &ep_file_str, SymbolKind::Module)],
                    &[],
                );
                let entrypoints = vec![ep(&ep_file_str, "main", EntrypointType::CliCommand)];

                let mut all_files = other_files.clone();
                all_files.push(ep_file_str.clone());

                let result = cluster_files(&graph, &entrypoints, &all_files);

                // The entrypoint file should be in a group.
                let grouped_files: Vec<&str> = result.groups
                    .iter()
                    .flat_map(|g| g.files.iter().map(|f| f.path.as_str()))
                    .collect();
                prop_assert!(grouped_files.contains(&ep_file_str.as_str()),
                    "entrypoint file should be in a group");
            }
        }
    }
}
