//! BFS reachability computation and graph utilities for clustering.

use std::collections::{HashMap, HashSet, VecDeque};

use petgraph::Direction;

use crate::graph::SymbolGraph;
use crate::types::{Entrypoint, EntrypointType, FlowEdge};

/// BFS from an entrypoint using bidirectional traversal, returning file_path → minimum graph distance.
///
/// Pass 1 (forward): follows outgoing edges with cost=1 per hop — the natural data flow direction.
/// Pass 2 (reverse): follows incoming edges with cost=2 per hop — files that depend on the group.
/// The higher reverse cost ensures forward-reachable files always win when both paths exist.
pub(super) fn compute_file_reachability(
    graph: &SymbolGraph,
    entry_file: &str,
    entry_symbol: &str,
) -> HashMap<String, usize> {
    let forward = bfs_pass(graph, entry_file, entry_symbol, Direction::Outgoing, 1);
    let reverse = bfs_pass(graph, entry_file, entry_symbol, Direction::Incoming, 2);

    // Merge: keep minimum distance for each file
    let mut merged = forward;
    for (file, rev_dist) in reverse {
        let entry = merged.entry(file).or_insert(rev_dist);
        if rev_dist < *entry {
            *entry = rev_dist;
        }
    }
    merged
}

/// Single-direction BFS pass from an entrypoint, with configurable cost per hop.
pub(super) fn bfs_pass(
    graph: &SymbolGraph,
    entry_file: &str,
    entry_symbol: &str,
    direction: Direction,
    cost_per_hop: usize,
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

        for neighbor in graph.graph.neighbors_directed(node, direction) {
            if visited.insert(neighbor) {
                queue.push_back((neighbor, dist + cost_per_hop));
            }
        }
    }

    file_distances
}

/// Collect all graph edges where both endpoints belong to files in the group.
pub(super) fn collect_internal_edges(
    graph: &SymbolGraph,
    group_files: &HashSet<&str>,
) -> Vec<FlowEdge> {
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
pub(super) fn generate_group_name(ep: &Entrypoint) -> String {
    // Extract file basename without extension
    let basename = ep.file
        .rsplit('/')
        .next()
        .unwrap_or(&ep.file)
        .rsplit('.')
        .last()
        .unwrap_or(&ep.file);

    // Use symbol if it differs from the file basename; otherwise just use basename
    let label = if ep.symbol == basename
        || ep.symbol == "default"
        || ep.symbol == "module"
    {
        basename.to_string()
    } else {
        format!("{} ({})", ep.symbol, basename)
    };

    match ep.entrypoint_type {
        EntrypointType::HttpRoute => format!("{} route", label),
        EntrypointType::CliCommand => format!("{} CLI", label),
        EntrypointType::QueueConsumer => format!("{} consumer", label),
        EntrypointType::CronJob => format!("{} scheduled", label),
        EntrypointType::ReactPage => format!("{} page", label),
        EntrypointType::TestFile => format!("{} test", label),
        EntrypointType::EventHandler => format!("{} event", label),
        EntrypointType::EffectService => format!("{} Effect service", label),
    }
}
