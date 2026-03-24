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
    InfraCategory, InfraSubGroup, InfrastructureGroup,
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
                sub_groups: vec![],
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
        let sub_groups = sub_cluster_infra_files(&infra_files, graph);
        Some(InfrastructureGroup {
            files: infra_files,
            sub_groups,
            reason: "Not reachable from any detected entrypoint".to_string(),
        })
    };

    ClusterResult {
        groups,
        infrastructure,
    }
}

/// BFS from an entrypoint using bidirectional traversal, returning file_path → minimum graph distance.
///
/// Pass 1 (forward): follows outgoing edges with cost=1 per hop — the natural data flow direction.
/// Pass 2 (reverse): follows incoming edges with cost=2 per hop — files that depend on the group.
/// The higher reverse cost ensures forward-reachable files always win when both paths exist.
fn compute_file_reachability(
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
fn bfs_pass(
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

// ---------------------------------------------------------------------------
// Infrastructure sub-clustering
// ---------------------------------------------------------------------------

/// Classify a file path into an infrastructure category by convention.
pub fn classify_by_convention(path: &str) -> InfraCategory {
    if is_true_infrastructure(path) {
        return InfraCategory::Infrastructure;
    }

    let lower = path.to_lowercase();
    let filename = lower.rsplit('/').next().unwrap_or(&lower);
    let ext = filename.rsplit('.').next().unwrap_or("");

    // Schemas/Types
    if lower.contains("/schemas/")
        || lower.starts_with("schemas/")
        || lower.contains("/schema/")
        || lower.starts_with("schema/")
        || filename.contains(".schema.")
        || filename.contains(".dto.")
        || lower.contains("/types/")
        || lower.starts_with("types/")
    {
        return InfraCategory::Schema;
    }

    // Migrations
    if lower.contains("/migrations/")
        || lower.starts_with("migrations/")
        || lower.contains("/migrate/")
        || lower.starts_with("migrate/")
        || filename.contains(".migration.")
        || lower.contains("/seeds/")
        || lower.starts_with("seeds/")
        || lower.contains("/fixtures/")
        || lower.starts_with("fixtures/")
    {
        return InfraCategory::Migration;
    }

    // Scripts
    if matches!(ext, "sh" | "bash" | "zsh" | "ps1")
        || lower.contains("/scripts/")
        || lower.starts_with("scripts/")
    {
        return InfraCategory::Script;
    }

    // Deployment
    if (lower.contains("/deploy/")
        || lower.starts_with("deploy/")
        || lower.contains("/deployment/")
        || lower.starts_with("deployment/"))
        && !is_true_infrastructure(path)
    {
        return InfraCategory::Deployment;
    }

    // Documentation
    if matches!(ext, "md" | "mdx" | "rst" | "txt")
        || lower.contains("/docs/")
        || lower.starts_with("docs/")
        || lower.contains("/documentation/")
        || lower.starts_with("documentation/")
    {
        return InfraCategory::Documentation;
    }

    // Lint configs
    if filename.starts_with(".eslint")
        || filename.starts_with(".prettier")
        || filename.starts_with(".stylelint")
        || filename == ".editorconfig"
        || filename == ".clang-format"
        || filename == "rustfmt.toml"
        || filename == ".rubocop.yml"
        || filename == ".flake8"
        || filename == "mypy.ini"
        || filename == ".golangci.yml"
    {
        return InfraCategory::Lint;
    }

    // Test utilities
    if lower.contains("/test-utils/")
        || lower.contains("/test-helpers/")
        || lower.contains("/__fixtures__/")
        || lower.contains("/test/fixtures/")
        || lower.contains("/testutils/")
    {
        return InfraCategory::TestUtil;
    }

    // Generated code
    if lower.contains("/generated/")
        || lower.contains("/__generated__/")
        || filename.contains(".generated.")
        || filename.ends_with(".g.dart")
        || filename.ends_with(".pb.go")
    {
        return InfraCategory::Generated;
    }

    InfraCategory::Unclassified
}

/// Check if a file is true infrastructure (Docker, CI/CD, env configs, build configs, etc.).
fn is_true_infrastructure(path: &str) -> bool {
    let lower = path.to_lowercase();
    let filename = lower.rsplit('/').next().unwrap_or(&lower);

    // Environment/Secrets
    if filename.starts_with(".env") || filename.ends_with(".env") {
        return true;
    }

    // Docker
    if filename.starts_with("dockerfile") || filename.starts_with("docker-compose") || filename == ".dockerignore" {
        return true;
    }

    // CI/CD
    if lower.contains(".github/workflows/")
        || filename == ".gitlab-ci.yml"
        || filename == "jenkinsfile"
        || lower.contains(".circleci/")
        || filename == ".travis.yml"
        || filename == "azure-pipelines.yml"
        || filename == "bitbucket-pipelines.yml"
    {
        return true;
    }

    // Container orchestration
    if lower.contains("k8s/")
        || lower.contains("kubernetes/")
        || lower.contains("helm/")
        || filename.contains(".helmrelease.")
    {
        return true;
    }

    // Terraform/IaC
    if lower.contains("terraform/")
        || filename.ends_with(".tf")
        || filename.ends_with(".tfvars")
        || lower.contains("pulumi/")
        || filename.starts_with("pulumi.")
        || lower.contains("cdk/")
        || lower.contains("cloudformation/")
    {
        return true;
    }

    // Package manager configs
    if matches!(
        filename,
        "package.json"
            | "cargo.toml"
            | "go.mod"
            | "go.sum"
            | "requirements.txt"
            | "pipfile"
            | "pyproject.toml"
            | "gemfile"
            | "pom.xml"
            | "package.swift"
            | "build.sbt"
            | "composer.json"
    ) || filename.starts_with("build.gradle")
        || filename.ends_with(".csproj")
    {
        return true;
    }

    // Lock files
    if matches!(
        filename,
        "package-lock.json"
            | "yarn.lock"
            | "pnpm-lock.yaml"
            | "cargo.lock"
            | "gemfile.lock"
            | "poetry.lock"
            | "composer.lock"
    ) {
        return true;
    }

    // Build tool configs
    if filename.starts_with("tsconfig")
        || filename.starts_with("webpack.")
        || filename.starts_with("vite.")
        || filename.starts_with("rollup.")
        || filename.starts_with("esbuild.")
        || filename.starts_with("babel.")
        || filename == "makefile"
        || filename == "cmakelists.txt"
        || filename.ends_with(".mk")
        || filename == "build.rs"
    {
        return true;
    }

    // IDE/editor configs
    if lower.contains(".vscode/") || lower.contains(".idea/") || lower.contains(".eclipse/") {
        return true;
    }

    // MCP/tool configs
    if filename == ".mcp.json"
        || lower.contains(".mcp/")
        || filename == ".tool-versions"
        || filename == ".nvmrc"
        || filename == ".node-version"
        || filename == ".python-version"
        || filename == ".ruby-version"
    {
        return true;
    }

    // Git configs
    if matches!(filename, ".gitignore" | ".gitattributes" | ".gitmodules") {
        return true;
    }

    false
}

/// Sub-cluster infrastructure files into semantically organized sub-groups.
pub fn sub_cluster_infra_files(files: &[String], graph: &SymbolGraph) -> Vec<InfraSubGroup> {
    let mut remaining: HashSet<String> = files.iter().cloned().collect();
    let mut category_files: BTreeMap<String, (InfraCategory, Vec<String>)> = BTreeMap::new();

    // Phase 1: Convention-based classification
    for file in files {
        let category = classify_by_convention(file);
        if category != InfraCategory::Unclassified {
            let name = category_display_name(&category);
            category_files
                .entry(name.clone())
                .or_insert_with(|| (category.clone(), Vec::new()))
                .1
                .push(file.clone());
            remaining.remove(file);
        }
    }

    let mut sub_groups: Vec<InfraSubGroup> = category_files
        .into_iter()
        .map(|(name, (category, mut files))| {
            files.sort();
            InfraSubGroup {
                name,
                category,
                files,
            }
        })
        .collect();

    // Phase 2: Import-edge clustering (for remaining files)
    if !remaining.is_empty() {
        let components = find_connected_components(&remaining, graph);
        for component in components {
            if component.len() > 1 {
                let name = common_directory_prefix(&component);
                for f in &component {
                    remaining.remove(f);
                }
                let mut files: Vec<String> = component;
                files.sort();
                sub_groups.push(InfraSubGroup {
                    name,
                    category: InfraCategory::DirectoryGroup,
                    files,
                });
            }
        }
    }

    // Phase 3: Directory proximity (for remaining files)
    if !remaining.is_empty() {
        let dir_groups = group_by_directory(&remaining);
        for (dir, mut files) in dir_groups {
            if files.len() >= 2 {
                for f in &files {
                    remaining.remove(f);
                }
                files.sort();
                sub_groups.push(InfraSubGroup {
                    name: dir,
                    category: InfraCategory::DirectoryGroup,
                    files,
                });
            }
        }
    }

    // Phase 4: Remaining → Unclassified
    if !remaining.is_empty() {
        let mut files: Vec<String> = remaining.into_iter().collect();
        files.sort();
        sub_groups.push(InfraSubGroup {
            name: "Unclassified".to_string(),
            category: InfraCategory::Unclassified,
            files,
        });
    }

    // Ensure deterministic ordering: sort by name so HashSet iteration order doesn't matter.
    sub_groups.sort_by(|a, b| a.name.cmp(&b.name));

    sub_groups
}

pub(crate) fn category_display_name(cat: &InfraCategory) -> String {
    match cat {
        InfraCategory::Infrastructure => "Infrastructure".to_string(),
        InfraCategory::Schema => "Schemas".to_string(),
        InfraCategory::Script => "Scripts".to_string(),
        InfraCategory::Migration => "Migrations".to_string(),
        InfraCategory::Deployment => "Deployment".to_string(),
        InfraCategory::Documentation => "Documentation".to_string(),
        InfraCategory::Lint => "Lint".to_string(),
        InfraCategory::TestUtil => "Test utilities".to_string(),
        InfraCategory::Generated => "Generated".to_string(),
        InfraCategory::DirectoryGroup => "Directory group".to_string(),
        InfraCategory::Unclassified => "Unclassified".to_string(),
    }
}

/// Find connected components among a set of files using graph edges.
fn find_connected_components(files: &HashSet<String>, graph: &SymbolGraph) -> Vec<Vec<String>> {
    // Build adjacency among the remaining files using graph edges.
    let mut adj: HashMap<String, HashSet<String>> = HashMap::new();
    for (from_id, to_id, _) in graph.edges() {
        let from_file = graph.get_symbol(from_id).map(|s| s.file.clone());
        let to_file = graph.get_symbol(to_id).map(|s| s.file.clone());
        if let (Some(ff), Some(tf)) = (from_file, to_file) {
            if files.contains(&ff) && files.contains(&tf) && ff != tf {
                adj.entry(ff.clone()).or_default().insert(tf.clone());
                adj.entry(tf).or_default().insert(ff);
            }
        }
    }

    let mut visited: HashSet<String> = HashSet::new();
    let mut components = Vec::new();

    for file in files {
        if visited.contains(file) {
            continue;
        }
        let mut component = Vec::new();
        let mut queue = VecDeque::new();
        queue.push_back(file.clone());
        visited.insert(file.clone());

        while let Some(f) = queue.pop_front() {
            component.push(f.clone());
            if let Some(neighbors) = adj.get(&f) {
                for n in neighbors {
                    if visited.insert(n.clone()) {
                        queue.push_back(n.clone());
                    }
                }
            }
        }
        components.push(component);
    }
    components
}

/// Group files by their parent directory.
fn group_by_directory(files: &HashSet<String>) -> Vec<(String, Vec<String>)> {
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for file in files {
        let dir = file
            .rfind('/')
            .map(|i| &file[..=i])
            .unwrap_or("")
            .to_string();
        groups.entry(dir).or_default().push(file.clone());
    }
    groups.into_iter().collect()
}

/// Find the common directory prefix for a set of files.
fn common_directory_prefix(files: &[String]) -> String {
    if files.is_empty() {
        return "Unknown".to_string();
    }
    if files.len() == 1 {
        return files[0]
            .rfind('/')
            .map(|i| files[0][..=i].to_string())
            .unwrap_or_else(|| files[0].clone());
    }

    let first = &files[0];
    let mut prefix_len = 0;
    for (i, c) in first.char_indices() {
        if files[1..].iter().all(|f| f.get(..=i).map_or(false, |s| s.ends_with(c) && s == &first[..=i])) {
            if c == '/' {
                prefix_len = i + 1;
            }
        } else {
            break;
        }
    }

    if prefix_len > 0 {
        first[..prefix_len].to_string()
    } else {
        "Mixed".to_string()
    }
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
        EntrypointType::EffectService => format!("{} Effect service flow", ep.symbol),
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
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

            /// Group file order is topologically valid w.r.t. BFS distance:
            /// (a) flow_position values are monotonically non-decreasing in
            ///     each group's file list, and
            /// (b) for every internal forward edge where the source has a
            ///     strictly smaller flow_position than the target, the source
            ///     file appears before the target in the file list.
            /// This is the BFS-tree topological order guarantee.
            /// Spec §13.7 property #2.
            #[test]
            fn prop_group_file_order_topologically_valid(
                chain_len in 2usize..7,
                extra_edges in prop::collection::vec((1usize..6, 1usize..6), 0..4)
            ) {
                // Build a chain of files: f0 → f1 → f2 → ... → f(N-1)
                let files: Vec<String> = (0..chain_len)
                    .map(|i| format!("src/f{}.ts", i))
                    .collect();

                let func_ids: Vec<String> = (0..chain_len)
                    .map(|i| format!("src/f{}.ts::func{}", i, i))
                    .collect();

                let node_data: Vec<(String, String, SymbolKind)> = (0..chain_len)
                    .flat_map(|i| {
                        vec![
                            (files[i].clone(), files[i].clone(), SymbolKind::Module),
                            (func_ids[i].clone(), files[i].clone(), SymbolKind::Function),
                        ]
                    })
                    .collect();

                let node_refs: Vec<(&str, &str, SymbolKind)> = node_data
                    .iter()
                    .map(|(id, file, kind)| (id.as_str(), file.as_str(), kind.clone()))
                    .collect();

                // Chain edges: f0 → f1 → f2 → ...
                let mut edge_data: Vec<(String, String, EdgeType)> = Vec::new();
                for i in 0..chain_len - 1 {
                    edge_data.push((files[i].clone(), func_ids[i + 1].clone(), EdgeType::Imports));
                    edge_data.push((func_ids[i].clone(), func_ids[i + 1].clone(), EdgeType::Calls));
                }

                // Add extra forward edges (skip-connections in the DAG)
                for (from_raw, to_raw) in &extra_edges {
                    let from_idx = from_raw % chain_len;
                    let to_idx = to_raw % chain_len;
                    if from_idx < to_idx {
                        edge_data.push((
                            func_ids[from_idx].clone(),
                            func_ids[to_idx].clone(),
                            EdgeType::Calls,
                        ));
                    }
                }

                let edge_refs: Vec<(&str, &str, EdgeType)> = edge_data
                    .iter()
                    .map(|(f, t, e)| (f.as_str(), t.as_str(), e.clone()))
                    .collect();

                let graph = make_graph(&node_refs, &edge_refs);
                let entrypoints = vec![ep(&files[0], "func0", EntrypointType::HttpRoute)];
                let changed: Vec<String> = files.clone();

                let result = cluster_files(&graph, &entrypoints, &changed);

                for group in &result.groups {
                    // (a) flow_position is monotonically non-decreasing
                    for window in group.files.windows(2) {
                        prop_assert!(
                            window[0].flow_position <= window[1].flow_position,
                            "flow_position not monotonic: {} (fp={}) followed by {} (fp={})",
                            window[0].path, window[0].flow_position,
                            window[1].path, window[1].flow_position,
                        );
                    }

                    // (b) For edges where source has strictly smaller
                    //     flow_position, source appears before target.
                    let pos_map: std::collections::HashMap<&str, (usize, u32)> = group
                        .files
                        .iter()
                        .enumerate()
                        .map(|(idx, fc)| (fc.path.as_str(), (idx, fc.flow_position)))
                        .collect();

                    for edge in &group.edges {
                        let from_file = edge.from.split("::").next().unwrap_or(&edge.from);
                        let to_file = edge.to.split("::").next().unwrap_or(&edge.to);
                        if from_file == to_file {
                            continue;
                        }
                        if let (Some(&(from_idx, from_fp)), Some(&(to_idx, to_fp))) =
                            (pos_map.get(from_file), pos_map.get(to_file))
                        {
                            if from_fp < to_fp {
                                prop_assert!(
                                    from_idx < to_idx,
                                    "BFS-tree topological violation: {} (fp={}, idx={}) \
                                     has edge to {} (fp={}, idx={})",
                                    from_file, from_fp, from_idx,
                                    to_file, to_fp, to_idx,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    // ===================================================================
    // Phase 8 audit: edge case tests
    // ===================================================================

    #[test]
    fn test_cyclic_graph_no_infinite_loop() {
        // A imports B, B imports A — BFS with visited set handles cycles
        let graph = make_graph(
            &[
                ("src/a.ts", "src/a.ts", SymbolKind::Module),
                ("src/a.ts::funcA", "src/a.ts", SymbolKind::Function),
                ("src/b.ts", "src/b.ts", SymbolKind::Module),
                ("src/b.ts::funcB", "src/b.ts", SymbolKind::Function),
            ],
            &[
                ("src/a.ts", "src/b.ts::funcB", EdgeType::Imports),
                ("src/a.ts::funcA", "src/b.ts::funcB", EdgeType::Calls),
                ("src/b.ts", "src/a.ts::funcA", EdgeType::Imports),
                ("src/b.ts::funcB", "src/a.ts::funcA", EdgeType::Calls),
            ],
        );

        let entrypoints = vec![ep("src/a.ts", "funcA", EntrypointType::HttpRoute)];
        let files = changed(&["src/a.ts", "src/b.ts"]);

        let result = cluster_files(&graph, &entrypoints, &files);
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups[0].files.len(), 2);
        assert!(result.infrastructure.is_none());
    }

    #[test]
    fn test_equal_distance_tiebreak_by_ep_index() {
        // Both entrypoints reach shared.ts at distance 1 — tie-break by ep_idx
        let graph = make_graph(
            &[
                ("src/ep_a.ts", "src/ep_a.ts", SymbolKind::Module),
                ("src/ep_a.ts::handleA", "src/ep_a.ts", SymbolKind::Function),
                ("src/ep_b.ts", "src/ep_b.ts", SymbolKind::Module),
                ("src/ep_b.ts::handleB", "src/ep_b.ts", SymbolKind::Function),
                ("src/shared.ts", "src/shared.ts", SymbolKind::Module),
                ("src/shared.ts::helper", "src/shared.ts", SymbolKind::Function),
            ],
            &[
                ("src/ep_a.ts", "src/shared.ts::helper", EdgeType::Imports),
                ("src/ep_a.ts::handleA", "src/shared.ts::helper", EdgeType::Calls),
                ("src/ep_b.ts", "src/shared.ts::helper", EdgeType::Imports),
                ("src/ep_b.ts::handleB", "src/shared.ts::helper", EdgeType::Calls),
            ],
        );

        let entrypoints = vec![
            ep("src/ep_a.ts", "handleA", EntrypointType::HttpRoute),
            ep("src/ep_b.ts", "handleB", EntrypointType::HttpRoute),
        ];
        let files = changed(&["src/ep_a.ts", "src/ep_b.ts", "src/shared.ts"]);

        let result = cluster_files(&graph, &entrypoints, &files);
        assert_eq!(result.groups.len(), 2);

        // shared.ts should go to the first entrypoint (ep_idx 0)
        let group_a = result
            .groups
            .iter()
            .find(|g| g.entrypoint.as_ref().unwrap().file == "src/ep_a.ts")
            .expect("should have group for ep_a");
        let group_a_files: Vec<&str> = group_a.files.iter().map(|f| f.path.as_str()).collect();
        assert!(
            group_a_files.contains(&"src/shared.ts"),
            "at equal distance, shared.ts should go to first entrypoint (lower ep_idx)"
        );
    }

    #[test]
    fn test_deep_chain_ordering() {
        // A → B → C → D → E — verify BFS distance ordering
        let graph = make_graph(
            &[
                ("src/a.ts", "src/a.ts", SymbolKind::Module),
                ("src/a.ts::start", "src/a.ts", SymbolKind::Function),
                ("src/b.ts", "src/b.ts", SymbolKind::Module),
                ("src/b.ts::step1", "src/b.ts", SymbolKind::Function),
                ("src/c.ts", "src/c.ts", SymbolKind::Module),
                ("src/c.ts::step2", "src/c.ts", SymbolKind::Function),
                ("src/d.ts", "src/d.ts", SymbolKind::Module),
                ("src/d.ts::step3", "src/d.ts", SymbolKind::Function),
                ("src/e.ts", "src/e.ts", SymbolKind::Module),
                ("src/e.ts::finish", "src/e.ts", SymbolKind::Function),
            ],
            &[
                ("src/a.ts::start", "src/b.ts::step1", EdgeType::Calls),
                ("src/b.ts::step1", "src/c.ts::step2", EdgeType::Calls),
                ("src/c.ts::step2", "src/d.ts::step3", EdgeType::Calls),
                ("src/d.ts::step3", "src/e.ts::finish", EdgeType::Calls),
            ],
        );

        let entrypoints = vec![ep("src/a.ts", "start", EntrypointType::HttpRoute)];
        let files = changed(&["src/a.ts", "src/b.ts", "src/c.ts", "src/d.ts", "src/e.ts"]);

        let result = cluster_files(&graph, &entrypoints, &files);
        assert_eq!(result.groups.len(), 1);
        let paths: Vec<&str> = result.groups[0].files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["src/a.ts", "src/b.ts", "src/c.ts", "src/d.ts", "src/e.ts"]);

        // flow_position should be sequential
        for (i, fc) in result.groups[0].files.iter().enumerate() {
            assert_eq!(fc.flow_position, i as u32);
        }
    }

    #[test]
    fn test_entrypoint_role_assigned_correctly() {
        let graph = make_graph(
            &[
                ("src/route.ts", "src/route.ts", SymbolKind::Module),
                ("src/route.ts::handle", "src/route.ts", SymbolKind::Function),
                ("src/services/user.ts", "src/services/user.ts", SymbolKind::Module),
                ("src/services/user.ts::getUser", "src/services/user.ts", SymbolKind::Function),
            ],
            &[
                ("src/route.ts::handle", "src/services/user.ts::getUser", EdgeType::Calls),
            ],
        );

        let entrypoints = vec![ep("src/route.ts", "handle", EntrypointType::HttpRoute)];
        let files = changed(&["src/route.ts", "src/services/user.ts"]);

        let result = cluster_files(&graph, &entrypoints, &files);
        let group = &result.groups[0];

        // Entrypoint file gets FileRole::Entrypoint
        let route_file = group.files.iter().find(|f| f.path == "src/route.ts").unwrap();
        assert_eq!(route_file.role, FileRole::Entrypoint);

        // Other files get inferred roles
        let service_file = group.files.iter().find(|f| f.path == "src/services/user.ts").unwrap();
        assert_eq!(service_file.role, FileRole::Service);
    }

    #[test]
    fn test_file_role_priority_ordering() {
        // When path matches multiple roles, the first match wins
        assert_eq!(infer_file_role("src/test-handler.ts"), FileRole::Handler);
        assert_eq!(infer_file_role("src/service-test.ts"), FileRole::Service);
        assert_eq!(infer_file_role("src/test-utils.ts"), FileRole::Test);
        assert_eq!(infer_file_role("src/repo-config.ts"), FileRole::Repository);
    }

    #[test]
    fn test_group_name_all_entrypoint_types() {
        assert_eq!(
            generate_group_name(&ep("f", "TestSuite", EntrypointType::TestFile)),
            "TestSuite test flow"
        );
        assert_eq!(
            generate_group_name(&ep("f", "onClick", EntrypointType::EventHandler)),
            "onClick event flow"
        );
        assert_eq!(
            generate_group_name(&ep("f", "UserService", EntrypointType::EffectService)),
            "UserService Effect service flow"
        );
    }

    #[test]
    fn test_large_number_of_entrypoints() {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        let mut entrypoints = Vec::new();
        let mut files = Vec::new();

        // 20 entrypoints, each with a downstream file
        for i in 0..20 {
            let ep_file = format!("src/route_{}.ts", i);
            let svc_file = format!("src/svc_{}.ts", i);
            let ep_id = format!("src/route_{}.ts::handle{}", i, i);
            let svc_id = format!("src/svc_{}.ts::process{}", i, i);

            nodes.push((ep_file.clone(), ep_file.clone(), SymbolKind::Module));
            nodes.push((ep_id.clone(), ep_file.clone(), SymbolKind::Function));
            nodes.push((svc_file.clone(), svc_file.clone(), SymbolKind::Module));
            nodes.push((svc_id.clone(), svc_file.clone(), SymbolKind::Function));

            edges.push((ep_id.clone(), svc_id.clone(), EdgeType::Calls));

            entrypoints.push(ep(&ep_file, &format!("handle{}", i), EntrypointType::HttpRoute));
            files.push(ep_file);
            files.push(svc_file);
        }

        // Build graph from owned data
        let node_refs: Vec<(&str, &str, SymbolKind)> = nodes
            .iter()
            .map(|(a, b, k)| (a.as_str(), b.as_str(), k.clone()))
            .collect();
        let edge_refs: Vec<(&str, &str, EdgeType)> = edges
            .iter()
            .map(|(a, b, e)| (a.as_str(), b.as_str(), e.clone()))
            .collect();
        let graph = make_graph(&node_refs, &edge_refs);

        let result = cluster_files(&graph, &entrypoints, &files);
        assert_eq!(result.groups.len(), 20, "should produce 20 groups");
        assert!(result.infrastructure.is_none());

        // Each group should have exactly 2 files
        for group in &result.groups {
            assert_eq!(group.files.len(), 2);
        }
    }

    #[test]
    fn test_fan_out_topology() {
        // Single entrypoint fans out to 5 independent leaf files
        let graph = make_graph(
            &[
                ("src/hub.ts", "src/hub.ts", SymbolKind::Module),
                ("src/hub.ts::dispatch", "src/hub.ts", SymbolKind::Function),
                ("src/leaf1.ts", "src/leaf1.ts", SymbolKind::Module),
                ("src/leaf1.ts::handle1", "src/leaf1.ts", SymbolKind::Function),
                ("src/leaf2.ts", "src/leaf2.ts", SymbolKind::Module),
                ("src/leaf2.ts::handle2", "src/leaf2.ts", SymbolKind::Function),
                ("src/leaf3.ts", "src/leaf3.ts", SymbolKind::Module),
                ("src/leaf3.ts::handle3", "src/leaf3.ts", SymbolKind::Function),
            ],
            &[
                ("src/hub.ts::dispatch", "src/leaf1.ts::handle1", EdgeType::Calls),
                ("src/hub.ts::dispatch", "src/leaf2.ts::handle2", EdgeType::Calls),
                ("src/hub.ts::dispatch", "src/leaf3.ts::handle3", EdgeType::Calls),
            ],
        );

        let entrypoints = vec![ep("src/hub.ts", "dispatch", EntrypointType::HttpRoute)];
        let files = changed(&["src/hub.ts", "src/leaf1.ts", "src/leaf2.ts", "src/leaf3.ts"]);

        let result = cluster_files(&graph, &entrypoints, &files);
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups[0].files.len(), 4);

        // hub.ts should be first (distance 0), leaves at distance 1 (alphabetical order)
        let paths: Vec<&str> = result.groups[0].files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths[0], "src/hub.ts");
    }

    #[test]
    fn test_diamond_dependency() {
        // A → B, A → C, B → D, C → D (diamond shape)
        let graph = make_graph(
            &[
                ("src/a.ts", "src/a.ts", SymbolKind::Module),
                ("src/a.ts::start", "src/a.ts", SymbolKind::Function),
                ("src/b.ts", "src/b.ts", SymbolKind::Module),
                ("src/b.ts::left", "src/b.ts", SymbolKind::Function),
                ("src/c.ts", "src/c.ts", SymbolKind::Module),
                ("src/c.ts::right", "src/c.ts", SymbolKind::Function),
                ("src/d.ts", "src/d.ts", SymbolKind::Module),
                ("src/d.ts::join", "src/d.ts", SymbolKind::Function),
            ],
            &[
                ("src/a.ts::start", "src/b.ts::left", EdgeType::Calls),
                ("src/a.ts::start", "src/c.ts::right", EdgeType::Calls),
                ("src/b.ts::left", "src/d.ts::join", EdgeType::Calls),
                ("src/c.ts::right", "src/d.ts::join", EdgeType::Calls),
            ],
        );

        let entrypoints = vec![ep("src/a.ts", "start", EntrypointType::HttpRoute)];
        let files = changed(&["src/a.ts", "src/b.ts", "src/c.ts", "src/d.ts"]);

        let result = cluster_files(&graph, &entrypoints, &files);
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups[0].files.len(), 4);

        // d.ts reached via BFS distance 2, b and c at distance 1
        let paths: Vec<&str> = result.groups[0].files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths[0], "src/a.ts"); // distance 0
        // b.ts and c.ts at distance 1 (alphabetical tiebreak)
        assert_eq!(paths[1], "src/b.ts");
        assert_eq!(paths[2], "src/c.ts");
        assert_eq!(paths[3], "src/d.ts"); // distance 2
    }

    #[test]
    fn test_group_ids_are_sequential() {
        let graph = make_graph(
            &[
                ("src/a.ts", "src/a.ts", SymbolKind::Module),
                ("src/a.ts::x", "src/a.ts", SymbolKind::Function),
                ("src/b.ts", "src/b.ts", SymbolKind::Module),
                ("src/b.ts::y", "src/b.ts", SymbolKind::Function),
            ],
            &[],
        );

        let entrypoints = vec![
            ep("src/a.ts", "x", EntrypointType::HttpRoute),
            ep("src/b.ts", "y", EntrypointType::CliCommand),
        ];
        let files = changed(&["src/a.ts", "src/b.ts"]);

        let result = cluster_files(&graph, &entrypoints, &files);
        let ids: Vec<&str> = result.groups.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(ids, vec!["group_1", "group_2"]);
    }

    // ===================================================================
    // Phase 2: Bidirectional Reachability tests (spec §2.4)
    // ===================================================================

    #[test]
    fn test_reverse_reachable_not_infra() {
        // File A has an edge TO the entrypoint file (reverse direction).
        // A should end up in the entrypoint's group, not infrastructure.
        let graph = make_graph(
            &[
                ("src/route.ts", "src/route.ts", SymbolKind::Module),
                ("src/route.ts::handle", "src/route.ts", SymbolKind::Function),
                ("src/caller.ts", "src/caller.ts", SymbolKind::Module),
                ("src/caller.ts::invoke", "src/caller.ts", SymbolKind::Function),
            ],
            &[
                // caller.ts imports from route.ts (reverse edge from entrypoint's perspective)
                ("src/caller.ts", "src/route.ts::handle", EdgeType::Imports),
                ("src/caller.ts::invoke", "src/route.ts::handle", EdgeType::Calls),
            ],
        );

        let entrypoints = vec![ep("src/route.ts", "handle", EntrypointType::HttpRoute)];
        let files = changed(&["src/route.ts", "src/caller.ts"]);

        let result = cluster_files(&graph, &entrypoints, &files);

        assert_eq!(result.groups.len(), 1);
        let group_files: Vec<&str> = result.groups[0]
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        assert!(
            group_files.contains(&"src/caller.ts"),
            "reverse-reachable file should be in entrypoint's group, not infrastructure"
        );
        assert!(result.infrastructure.is_none(), "no infrastructure files");
    }

    #[test]
    fn test_forward_preferred_over_reverse() {
        // File X is reachable forward (dist 1) and reverse (dist 2).
        // Should use forward distance (1).
        let graph = make_graph(
            &[
                ("src/entry.ts", "src/entry.ts", SymbolKind::Module),
                ("src/entry.ts::start", "src/entry.ts", SymbolKind::Function),
                ("src/target.ts", "src/target.ts", SymbolKind::Module),
                ("src/target.ts::process", "src/target.ts", SymbolKind::Function),
            ],
            &[
                // Forward: entry → target
                ("src/entry.ts::start", "src/target.ts::process", EdgeType::Calls),
                // Reverse: target → entry (target imports from entry)
                ("src/target.ts", "src/entry.ts::start", EdgeType::Imports),
            ],
        );

        let entrypoints = vec![ep("src/entry.ts", "start", EntrypointType::HttpRoute)];
        let files = changed(&["src/entry.ts", "src/target.ts"]);

        let result = cluster_files(&graph, &entrypoints, &files);

        assert_eq!(result.groups.len(), 1);
        // target.ts should be at flow_position 1 (forward distance), not 2 (reverse)
        let target = result.groups[0]
            .files
            .iter()
            .find(|f| f.path == "src/target.ts")
            .expect("target.ts should be in group");
        assert_eq!(target.flow_position, 1, "should use forward distance");
    }

    #[test]
    fn test_reverse_only_grouped() {
        // File Z is ONLY reachable via reverse edges. It should still be grouped.
        let graph = make_graph(
            &[
                ("src/entry.ts", "src/entry.ts", SymbolKind::Module),
                ("src/entry.ts::start", "src/entry.ts", SymbolKind::Function),
                ("src/dep.ts", "src/dep.ts", SymbolKind::Module),
                ("src/dep.ts::use_entry", "src/dep.ts", SymbolKind::Function),
            ],
            &[
                // Only reverse: dep.ts depends on entry (dep imports entry)
                ("src/dep.ts::use_entry", "src/entry.ts::start", EdgeType::Calls),
            ],
        );

        let entrypoints = vec![ep("src/entry.ts", "start", EntrypointType::HttpRoute)];
        let files = changed(&["src/entry.ts", "src/dep.ts"]);

        let result = cluster_files(&graph, &entrypoints, &files);

        assert_eq!(result.groups.len(), 1);
        let group_files: Vec<&str> = result.groups[0]
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        assert!(
            group_files.contains(&"src/dep.ts"),
            "reverse-only reachable file should be grouped"
        );
        assert!(result.infrastructure.is_none());
    }

    #[test]
    fn test_mixed_multi_hop_bidirectional() {
        // Complex graph: entry → A → B (forward), C → entry (reverse)
        // C depends on entry, so it should be grouped via reverse BFS
        let graph = make_graph(
            &[
                ("src/entry.ts", "src/entry.ts", SymbolKind::Module),
                ("src/entry.ts::start", "src/entry.ts", SymbolKind::Function),
                ("src/a.ts", "src/a.ts", SymbolKind::Module),
                ("src/a.ts::func_a", "src/a.ts", SymbolKind::Function),
                ("src/b.ts", "src/b.ts", SymbolKind::Module),
                ("src/b.ts::func_b", "src/b.ts", SymbolKind::Function),
                ("src/c.ts", "src/c.ts", SymbolKind::Module),
                ("src/c.ts::func_c", "src/c.ts", SymbolKind::Function),
            ],
            &[
                // Forward: entry → A → B
                ("src/entry.ts::start", "src/a.ts::func_a", EdgeType::Calls),
                ("src/a.ts::func_a", "src/b.ts::func_b", EdgeType::Calls),
                // Reverse: C depends on entry (C imports entry)
                ("src/c.ts::func_c", "src/entry.ts::start", EdgeType::Calls),
            ],
        );

        let entrypoints = vec![ep("src/entry.ts", "start", EntrypointType::HttpRoute)];
        let files = changed(&["src/entry.ts", "src/a.ts", "src/b.ts", "src/c.ts"]);

        let result = cluster_files(&graph, &entrypoints, &files);

        assert_eq!(result.groups.len(), 1);
        let group_files: Vec<&str> = result.groups[0]
            .files
            .iter()
            .map(|f| f.path.as_str())
            .collect();
        assert!(group_files.contains(&"src/c.ts"), "C should be grouped via reverse edge to entry");
        assert!(result.infrastructure.is_none());
    }

    #[test]
    fn test_existing_forward_tests_unchanged() {
        // Verify that the existing forward-only test still passes identically.
        // (test_single_entrypoint_group already covers this — this is a sanity check
        // that forward behavior is preserved.)
        let graph = make_graph(
            &[
                ("src/route.ts", "src/route.ts", SymbolKind::Module),
                ("src/route.ts::handlePost", "src/route.ts", SymbolKind::Function),
                ("src/service.ts", "src/service.ts", SymbolKind::Module),
                ("src/service.ts::createUser", "src/service.ts", SymbolKind::Function),
            ],
            &[
                ("src/route.ts", "src/service.ts::createUser", EdgeType::Imports),
                ("src/route.ts::handlePost", "src/service.ts::createUser", EdgeType::Calls),
            ],
        );

        let entrypoints = vec![ep("src/route.ts", "handlePost", EntrypointType::HttpRoute)];
        let files = changed(&["src/route.ts", "src/service.ts"]);

        let result = cluster_files(&graph, &entrypoints, &files);
        assert_eq!(result.groups.len(), 1);
        assert!(result.infrastructure.is_none());
        let paths: Vec<&str> = result.groups[0].files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths[0], "src/route.ts");
        assert_eq!(paths[1], "src/service.ts");
    }

    // ===================================================================
    // Phase 4: Infrastructure Sub-Clustering tests (spec §3.9)
    // ===================================================================

    #[test]
    fn test_classify_only_true_infra() {
        assert_eq!(classify_by_convention("Dockerfile"), InfraCategory::Infrastructure);
        assert_eq!(classify_by_convention(".env.dev"), InfraCategory::Infrastructure);
        assert_eq!(classify_by_convention("tsconfig.json"), InfraCategory::Infrastructure);
        assert_eq!(classify_by_convention("package.json"), InfraCategory::Infrastructure);
        assert_eq!(classify_by_convention(".github/workflows/ci.yml"), InfraCategory::Infrastructure);
        assert_eq!(classify_by_convention("Cargo.toml"), InfraCategory::Infrastructure);
        assert_eq!(classify_by_convention("Cargo.lock"), InfraCategory::Infrastructure);
    }

    #[test]
    fn test_classify_schemas() {
        assert_eq!(classify_by_convention("schemas/user.ts"), InfraCategory::Schema);
        assert_eq!(classify_by_convention("src/schema/billing.ts"), InfraCategory::Schema);
        assert_eq!(classify_by_convention("src/user.schema.ts"), InfraCategory::Schema);
        assert_eq!(classify_by_convention("src/user.dto.ts"), InfraCategory::Schema);
        assert_eq!(classify_by_convention("src/types/index.ts"), InfraCategory::Schema);
    }

    #[test]
    fn test_classify_scripts() {
        assert_eq!(classify_by_convention("scripts/deploy.sh"), InfraCategory::Script);
        assert_eq!(classify_by_convention("scripts/setup.sh"), InfraCategory::Script);
        assert_eq!(classify_by_convention("init.bash"), InfraCategory::Script);
        assert_eq!(classify_by_convention("clean.zsh"), InfraCategory::Script);
    }

    #[test]
    fn test_classify_migrations() {
        assert_eq!(classify_by_convention("migrations/001.sql"), InfraCategory::Migration);
        assert_eq!(classify_by_convention("db/migrations/002.ts"), InfraCategory::Migration);
        assert_eq!(classify_by_convention("seeds/users.ts"), InfraCategory::Migration);
    }

    #[test]
    fn test_classify_docs() {
        assert_eq!(classify_by_convention("docs/README.md"), InfraCategory::Documentation);
        assert_eq!(classify_by_convention("docs/setup.md"), InfraCategory::Documentation);
        assert_eq!(classify_by_convention("CHANGELOG.md"), InfraCategory::Documentation);
    }

    #[test]
    fn test_classify_lint() {
        assert_eq!(classify_by_convention(".eslintrc.json"), InfraCategory::Lint);
        assert_eq!(classify_by_convention(".prettierrc"), InfraCategory::Lint);
        assert_eq!(classify_by_convention("rustfmt.toml"), InfraCategory::Lint);
    }

    #[test]
    fn test_classify_test_utils() {
        assert_eq!(classify_by_convention("src/test-utils/helpers.ts"), InfraCategory::TestUtil);
        assert_eq!(classify_by_convention("test/__fixtures__/data.json"), InfraCategory::TestUtil);
    }

    #[test]
    fn test_classify_generated() {
        assert_eq!(classify_by_convention("src/generated/types.ts"), InfraCategory::Generated);
        assert_eq!(classify_by_convention("src/__generated__/schema.ts"), InfraCategory::Generated);
        assert_eq!(classify_by_convention("src/api.generated.ts"), InfraCategory::Generated);
    }

    #[test]
    fn test_classify_unclassified() {
        assert_eq!(classify_by_convention("src/random-file.ts"), InfraCategory::Unclassified);
        assert_eq!(classify_by_convention("src/utils/helpers.ts"), InfraCategory::Unclassified);
    }

    // ===================================================================
    // Exhaustive spec §3.3 coverage: every pattern listed in the spec
    // "What IS Infrastructure" table must classify as Infrastructure.
    // ===================================================================

    #[test]
    fn test_infra_exhaustive_env_patterns() {
        // Spec §3.3 — Environment/Secrets: `.env*`, `*.env`, `.env.dev`, `.env.prod`, `.env.local`
        for path in &[
            ".env",
            ".env.dev",
            ".env.prod",
            ".env.local",
            ".env.staging",
            ".env.test",
            "app.env",
            "config/settings.env",
        ] {
            assert_eq!(
                classify_by_convention(path),
                InfraCategory::Infrastructure,
                "expected Infrastructure for env pattern: {}",
                path
            );
        }
    }

    #[test]
    fn test_infra_exhaustive_docker_patterns() {
        // Spec §3.3 — Docker: `Dockerfile*`, `docker-compose*`, `.dockerignore`
        for path in &[
            "Dockerfile",
            "Dockerfile.prod",
            "Dockerfile.dev",
            "docker-compose.yml",
            "docker-compose.override.yml",
            "docker-compose.prod.yml",
            ".dockerignore",
        ] {
            assert_eq!(
                classify_by_convention(path),
                InfraCategory::Infrastructure,
                "expected Infrastructure for docker pattern: {}",
                path
            );
        }
    }

    #[test]
    fn test_infra_exhaustive_cicd_patterns() {
        // Spec §3.3 — CI/CD: `.github/workflows/*`, `.gitlab-ci.yml`, `Jenkinsfile`,
        // `.circleci/*`, `.travis.yml`, `azure-pipelines.yml`, `bitbucket-pipelines.yml`
        for path in &[
            ".github/workflows/ci.yml",
            ".github/workflows/deploy.yml",
            ".github/workflows/test.yml",
            ".gitlab-ci.yml",
            "Jenkinsfile",
            ".circleci/config.yml",
            ".circleci/setup.yml",
            ".travis.yml",
            "azure-pipelines.yml",
            "bitbucket-pipelines.yml",
        ] {
            assert_eq!(
                classify_by_convention(path),
                InfraCategory::Infrastructure,
                "expected Infrastructure for CI/CD pattern: {}",
                path
            );
        }
    }

    #[test]
    fn test_infra_exhaustive_container_orch_patterns() {
        // Spec §3.3 — Container orchestration: `k8s/*`, `kubernetes/*`, `helm/*`, `*.helmrelease.*`
        for path in &[
            "k8s/deployment.yml",
            "k8s/service.yml",
            "k8s/ingress.yml",
            "kubernetes/deployment.yml",
            "kubernetes/namespace.yml",
            "helm/Chart.yaml",
            "helm/values.yaml",
            "infra/helm/templates/app.yaml",
            "app.helmrelease.yaml",
            "web.helmrelease.yml",
        ] {
            assert_eq!(
                classify_by_convention(path),
                InfraCategory::Infrastructure,
                "expected Infrastructure for container orch pattern: {}",
                path
            );
        }
    }

    #[test]
    fn test_infra_exhaustive_iac_patterns() {
        // Spec §3.3 — Terraform/IaC: `terraform/*`, `*.tf`, `*.tfvars`,
        // `pulumi/*`, `Pulumi.*`, `cdk/*`, `cloudformation/*`
        for path in &[
            "terraform/main.tf",
            "terraform/variables.tf",
            "infra/terraform/outputs.tf",
            "main.tf",
            "variables.tf",
            "prod.tfvars",
            "staging.tfvars",
            "pulumi/index.ts",
            "pulumi/Pulumi.yaml",
            "Pulumi.yaml",
            "Pulumi.dev.yaml",
            "cdk/app.ts",
            "cdk/lib/stack.ts",
            "cloudformation/stack.yaml",
            "cloudformation/template.json",
        ] {
            assert_eq!(
                classify_by_convention(path),
                InfraCategory::Infrastructure,
                "expected Infrastructure for IaC pattern: {}",
                path
            );
        }
    }

    #[test]
    fn test_infra_exhaustive_package_mgr_patterns() {
        // Spec §3.3 — Package manager configs
        for path in &[
            "package.json",
            "Cargo.toml",
            "go.mod",
            "go.sum",
            "requirements.txt",
            "Pipfile",
            "pyproject.toml",
            "Gemfile",
            "pom.xml",
            "build.gradle",
            "build.gradle.kts",
            "app/app.csproj",
            "Package.swift",
            "build.sbt",
            "composer.json",
        ] {
            assert_eq!(
                classify_by_convention(path),
                InfraCategory::Infrastructure,
                "expected Infrastructure for package manager pattern: {}",
                path
            );
        }
    }

    #[test]
    fn test_infra_exhaustive_lock_file_patterns() {
        // Spec §3.3 — Lock files
        for path in &[
            "package-lock.json",
            "yarn.lock",
            "pnpm-lock.yaml",
            "Cargo.lock",
            "Gemfile.lock",
            "poetry.lock",
            "composer.lock",
        ] {
            assert_eq!(
                classify_by_convention(path),
                InfraCategory::Infrastructure,
                "expected Infrastructure for lock file pattern: {}",
                path
            );
        }
    }

    #[test]
    fn test_infra_exhaustive_build_tool_patterns() {
        // Spec §3.3 — Build tool configs
        for path in &[
            "tsconfig.json",
            "tsconfig.app.json",
            "tsconfig.spec.json",
            "webpack.config.js",
            "webpack.prod.js",
            "vite.config.ts",
            "vite.config.js",
            "rollup.config.js",
            "rollup.config.mjs",
            "esbuild.config.mjs",
            "esbuild.js",
            "babel.config.js",
            "babel.config.json",
            "Makefile",
            "CMakeLists.txt",
            "build.mk",
            "rules.mk",
            "build.rs",
        ] {
            assert_eq!(
                classify_by_convention(path),
                InfraCategory::Infrastructure,
                "expected Infrastructure for build tool pattern: {}",
                path
            );
        }
    }

    #[test]
    fn test_infra_exhaustive_ide_patterns() {
        // Spec §3.3 — IDE/editor: `.vscode/*`, `.idea/*`, `.eclipse/*`
        for path in &[
            ".vscode/settings.json",
            ".vscode/extensions.json",
            ".vscode/launch.json",
            ".idea/workspace.xml",
            ".idea/modules.xml",
            ".idea/.gitignore",
            ".eclipse/.project",
            ".eclipse/.classpath",
        ] {
            assert_eq!(
                classify_by_convention(path),
                InfraCategory::Infrastructure,
                "expected Infrastructure for IDE pattern: {}",
                path
            );
        }
    }

    #[test]
    fn test_infra_exhaustive_mcp_tool_patterns() {
        // Spec §3.3 — MCP/tool configs
        for path in &[
            ".mcp.json",
            ".mcp/config.json",
            ".mcp/servers.json",
            ".tool-versions",
            ".nvmrc",
            ".node-version",
            ".python-version",
            ".ruby-version",
        ] {
            assert_eq!(
                classify_by_convention(path),
                InfraCategory::Infrastructure,
                "expected Infrastructure for MCP/tool pattern: {}",
                path
            );
        }
    }

    #[test]
    fn test_infra_exhaustive_git_patterns() {
        // Spec §3.3 — Git configs: `.gitignore`, `.gitattributes`, `.gitmodules`
        for path in &[".gitignore", ".gitattributes", ".gitmodules"] {
            assert_eq!(
                classify_by_convention(path),
                InfraCategory::Infrastructure,
                "expected Infrastructure for git pattern: {}",
                path
            );
        }
    }

    // ===================================================================
    // Exhaustive spec §3.4 + §3.7 coverage: every non-infrastructure
    // category pattern must classify to its correct category.
    // ===================================================================

    #[test]
    fn test_classify_exhaustive_schema_patterns() {
        // Spec §3.4/§3.7 — Schemas/Types
        for path in &[
            "schemas/user.ts",
            "src/schemas/billing.ts",
            "schema/order.ts",
            "src/schema/product.ts",
            "src/user.schema.ts",
            "src/order.schema.json",
            "src/user.dto.ts",
            "src/billing.dto.ts",
            "types/index.ts",
            "src/types/api.ts",
        ] {
            assert_eq!(
                classify_by_convention(path),
                InfraCategory::Schema,
                "expected Schema for pattern: {}",
                path
            );
        }
    }

    #[test]
    fn test_classify_exhaustive_migration_patterns() {
        // Spec §3.4/§3.7 — Migrations
        for path in &[
            "migrations/001.sql",
            "db/migrations/002_add_users.ts",
            "migrate/003.sql",
            "src/migrate/schema.ts",
            "src/order.migration.ts",
            "seeds/users.ts",
            "db/seeds/products.json",
            "fixtures/test-data.json",
            "test/fixtures/setup.sql",
        ] {
            assert_eq!(
                classify_by_convention(path),
                InfraCategory::Migration,
                "expected Migration for pattern: {}",
                path
            );
        }
    }

    #[test]
    fn test_classify_exhaustive_script_patterns() {
        // Spec §3.4/§3.7 — Scripts
        for path in &[
            "scripts/deploy.sh",
            "scripts/setup.sh",
            "scripts/seed-db.sh",
            "init.bash",
            "clean.zsh",
            "setup.ps1",
            "install.sh",
        ] {
            assert_eq!(
                classify_by_convention(path),
                InfraCategory::Script,
                "expected Script for pattern: {}",
                path
            );
        }
    }

    #[test]
    fn test_classify_exhaustive_deployment_patterns() {
        // Spec §3.4/§3.7 — Deployment
        for path in &[
            "deploy/app.yaml",
            "deploy/staging.yaml",
            "deployment/config.yaml",
            "infra/deployment/service.yaml",
        ] {
            assert_eq!(
                classify_by_convention(path),
                InfraCategory::Deployment,
                "expected Deployment for pattern: {}",
                path
            );
        }
    }

    #[test]
    fn test_classify_exhaustive_documentation_patterns() {
        // Spec §3.4/§3.7 — Documentation
        for path in &[
            "README.md",
            "CHANGELOG.md",
            "CONTRIBUTING.md",
            "docs/setup.md",
            "docs/api.md",
            "documentation/guide.md",
            "src/overview.mdx",
            "docs/architecture.rst",
            "notes.txt",
        ] {
            assert_eq!(
                classify_by_convention(path),
                InfraCategory::Documentation,
                "expected Documentation for pattern: {}",
                path
            );
        }
    }

    #[test]
    fn test_classify_exhaustive_lint_patterns() {
        // Spec §3.3/§3.7 — Linter/formatter configs (separate from Infrastructure)
        for path in &[
            ".eslintrc.json",
            ".eslintrc.js",
            ".eslintrc.yml",
            ".eslintrc",
            ".prettierrc",
            ".prettierrc.json",
            ".prettierrc.yml",
            ".stylelintrc",
            ".stylelintrc.json",
            ".editorconfig",
            ".clang-format",
            "rustfmt.toml",
            ".rubocop.yml",
            ".flake8",
            "mypy.ini",
            ".golangci.yml",
        ] {
            assert_eq!(
                classify_by_convention(path),
                InfraCategory::Lint,
                "expected Lint for pattern: {}",
                path
            );
        }
    }

    #[test]
    fn test_classify_exhaustive_test_util_patterns() {
        // Spec §3.4/§3.7 — Test utilities
        for path in &[
            "src/test-utils/helpers.ts",
            "src/test-utils/render.tsx",
            "test/test-helpers/mock-db.ts",
            "test/__fixtures__/data.json",
            "test/__fixtures__/sample.ts",
            "src/testutils/factory.ts",
        ] {
            assert_eq!(
                classify_by_convention(path),
                InfraCategory::TestUtil,
                "expected TestUtil for pattern: {}",
                path
            );
        }
    }

    #[test]
    fn test_classify_exhaustive_generated_patterns() {
        // Spec §3.4/§3.7 — Generated code
        for path in &[
            "src/generated/types.ts",
            "lib/generated/api.ts",
            "src/__generated__/schema.ts",
            "lib/__generated__/graphql.ts",
            "src/api.generated.ts",
            "src/models.generated.rs",
            "lib/widget.g.dart",
            "proto/service.pb.go",
        ] {
            assert_eq!(
                classify_by_convention(path),
                InfraCategory::Generated,
                "expected Generated for pattern: {}",
                path
            );
        }
    }

    // ===================================================================
    // Boundary tests: patterns that should NOT be Infrastructure
    // ===================================================================

    #[test]
    fn test_infra_false_positive_guards() {
        // These paths look infra-adjacent but should NOT be Infrastructure
        let non_infra_paths = &[
            ("src/utils/helpers.ts", "source code util"),
            ("src/services/auth.ts", "application service"),
            ("src/models/user.ts", "application model"),
            ("src/config/database.ts", "app config code"),
            ("lib/core/engine.rs", "core library code"),
            ("src/api/client.ts", "api client code"),
            ("src/index.ts", "app entry"),
            ("main.go", "go main"),
        ];
        for (path, desc) in non_infra_paths {
            assert_ne!(
                classify_by_convention(path),
                InfraCategory::Infrastructure,
                "{} should NOT be Infrastructure: {}",
                path,
                desc
            );
        }
    }

    #[test]
    fn test_sub_cluster_only_true_infra() {
        let graph = make_graph(&[], &[]);
        let files = vec![
            "Dockerfile".to_string(),
            ".env.dev".to_string(),
            "tsconfig.json".to_string(),
        ];
        let sub_groups = sub_cluster_infra_files(&files, &graph);
        assert_eq!(sub_groups.len(), 1);
        assert_eq!(sub_groups[0].category, InfraCategory::Infrastructure);
        assert_eq!(sub_groups[0].files.len(), 3);
    }

    #[test]
    fn test_sub_cluster_schemas_separated() {
        let graph = make_graph(&[], &[]);
        let files = vec![
            "schemas/user.ts".to_string(),
            "schemas/billing.ts".to_string(),
            "Dockerfile".to_string(),
        ];
        let sub_groups = sub_cluster_infra_files(&files, &graph);
        assert!(sub_groups.iter().any(|g| g.category == InfraCategory::Infrastructure));
        assert!(sub_groups.iter().any(|g| g.category == InfraCategory::Schema));
    }

    #[test]
    fn test_sub_cluster_scripts_grouped() {
        let graph = make_graph(&[], &[]);
        let files = vec![
            "scripts/deploy.sh".to_string(),
            "scripts/setup.sh".to_string(),
        ];
        let sub_groups = sub_cluster_infra_files(&files, &graph);
        assert_eq!(sub_groups.len(), 1);
        assert_eq!(sub_groups[0].category, InfraCategory::Script);
        assert_eq!(sub_groups[0].files.len(), 2);
    }

    #[test]
    fn test_sub_cluster_dir_proximity() {
        let graph = make_graph(&[], &[]);
        let files = vec![
            "mcp/langfuse.ts".to_string(),
            "mcp/spotlight.ts".to_string(),
        ];
        let sub_groups = sub_cluster_infra_files(&files, &graph);
        // Two unclassified files in same dir → DirectoryGroup
        assert!(sub_groups.iter().any(|g| g.category == InfraCategory::DirectoryGroup));
    }

    #[test]
    fn test_sub_cluster_mixed_all_categories() {
        let graph = make_graph(&[], &[]);
        let files = vec![
            "Dockerfile".to_string(),
            "schemas/user.ts".to_string(),
            "scripts/deploy.sh".to_string(),
            "docs/setup.md".to_string(),
            "src/random-file.ts".to_string(),
        ];
        let sub_groups = sub_cluster_infra_files(&files, &graph);
        let categories: Vec<&InfraCategory> = sub_groups.iter().map(|g| &g.category).collect();
        assert!(categories.contains(&&InfraCategory::Infrastructure));
        assert!(categories.contains(&&InfraCategory::Schema));
        assert!(categories.contains(&&InfraCategory::Script));
        assert!(categories.contains(&&InfraCategory::Documentation));
        assert!(categories.contains(&&InfraCategory::Unclassified));
    }

    #[test]
    fn test_sub_cluster_import_edge_clustering() {
        // Two ungrouped code files that import each other
        let graph = make_graph(
            &[
                ("src/foo.ts", "src/foo.ts", SymbolKind::Module),
                ("src/foo.ts::doFoo", "src/foo.ts", SymbolKind::Function),
                ("src/bar.ts", "src/bar.ts", SymbolKind::Module),
                ("src/bar.ts::doBar", "src/bar.ts", SymbolKind::Function),
            ],
            &[("src/foo.ts", "src/bar.ts::doBar", EdgeType::Imports)],
        );
        let files = vec!["src/foo.ts".to_string(), "src/bar.ts".to_string()];
        let sub_groups = sub_cluster_infra_files(&files, &graph);
        assert_eq!(sub_groups.len(), 1);
        assert_eq!(sub_groups[0].category, InfraCategory::DirectoryGroup);
        assert_eq!(sub_groups[0].files.len(), 2);
    }

    #[test]
    fn test_sub_cluster_single_unclassified() {
        let graph = make_graph(&[], &[]);
        let files = vec!["src/random-file.ts".to_string()];
        let sub_groups = sub_cluster_infra_files(&files, &graph);
        assert_eq!(sub_groups.len(), 1);
        assert_eq!(sub_groups[0].category, InfraCategory::Unclassified);
    }

    #[test]
    fn test_sub_cluster_empty_input() {
        let graph = make_graph(&[], &[]);
        let files: Vec<String> = vec![];
        let sub_groups = sub_cluster_infra_files(&files, &graph);
        assert!(sub_groups.is_empty());
    }

    #[test]
    fn test_sub_cluster_docs_grouped() {
        let graph = make_graph(&[], &[]);
        let files = vec![
            "docs/README.md".to_string(),
            "docs/setup.md".to_string(),
        ];
        let sub_groups = sub_cluster_infra_files(&files, &graph);
        assert_eq!(sub_groups.len(), 1);
        assert_eq!(sub_groups[0].category, InfraCategory::Documentation);
    }

    #[test]
    fn test_sub_cluster_migrations_grouped() {
        let graph = make_graph(&[], &[]);
        let files = vec![
            "migrations/001.sql".to_string(),
            "migrations/002.sql".to_string(),
        ];
        let sub_groups = sub_cluster_infra_files(&files, &graph);
        assert_eq!(sub_groups.len(), 1);
        assert_eq!(sub_groups[0].category, InfraCategory::Migration);
    }

    // ===================================================================
    // BFS internal distance correctness tests
    // ===================================================================

    #[test]
    fn test_bfs_forward_chain_distances() {
        // Forward chain: entry → a → b → c
        // Expected distances with cost=1: entry=0, a=1, b=2, c=3
        let graph = make_graph(
            &[
                ("src/entry.ts", "src/entry.ts", SymbolKind::Module),
                ("src/entry.ts::main", "src/entry.ts", SymbolKind::Function),
                ("src/a.ts", "src/a.ts", SymbolKind::Module),
                ("src/a.ts::fa", "src/a.ts", SymbolKind::Function),
                ("src/b.ts", "src/b.ts", SymbolKind::Module),
                ("src/b.ts::fb", "src/b.ts", SymbolKind::Function),
                ("src/c.ts", "src/c.ts", SymbolKind::Module),
                ("src/c.ts::fc", "src/c.ts", SymbolKind::Function),
            ],
            &[
                ("src/entry.ts::main", "src/a.ts::fa", EdgeType::Calls),
                ("src/a.ts::fa", "src/b.ts::fb", EdgeType::Calls),
                ("src/b.ts::fb", "src/c.ts::fc", EdgeType::Calls),
            ],
        );

        let forward = bfs_pass(&graph, "src/entry.ts", "main", Direction::Outgoing, 1);
        assert_eq!(forward.get("src/entry.ts"), Some(&0));
        assert_eq!(forward.get("src/a.ts"), Some(&1));
        assert_eq!(forward.get("src/b.ts"), Some(&2));
        assert_eq!(forward.get("src/c.ts"), Some(&3));
    }

    #[test]
    fn test_bfs_reverse_chain_costs_double() {
        // Reverse chain: a→entry, b→a, c→b (edges point toward entry)
        // With cost_per_hop=2, reverse BFS from entry finds: a=2, b=4, c=6
        let graph = make_graph(
            &[
                ("src/entry.ts", "src/entry.ts", SymbolKind::Module),
                ("src/entry.ts::main", "src/entry.ts", SymbolKind::Function),
                ("src/a.ts", "src/a.ts", SymbolKind::Module),
                ("src/a.ts::fa", "src/a.ts", SymbolKind::Function),
                ("src/b.ts", "src/b.ts", SymbolKind::Module),
                ("src/b.ts::fb", "src/b.ts", SymbolKind::Function),
                ("src/c.ts", "src/c.ts", SymbolKind::Module),
                ("src/c.ts::fc", "src/c.ts", SymbolKind::Function),
            ],
            &[
                ("src/a.ts::fa", "src/entry.ts::main", EdgeType::Calls),
                ("src/b.ts::fb", "src/a.ts::fa", EdgeType::Calls),
                ("src/c.ts::fc", "src/b.ts::fb", EdgeType::Calls),
            ],
        );

        let reverse = bfs_pass(&graph, "src/entry.ts", "main", Direction::Incoming, 2);
        assert_eq!(reverse.get("src/entry.ts"), Some(&0));
        assert_eq!(reverse.get("src/a.ts"), Some(&2));
        assert_eq!(reverse.get("src/b.ts"), Some(&4));
        assert_eq!(reverse.get("src/c.ts"), Some(&6));
    }

    #[test]
    fn test_compute_reachability_merge_picks_minimum() {
        // x is reachable forward at dist 3 (entry→a→b→x) and reverse at dist 2 (x→entry)
        // Merged should pick min(3, 2) = 2
        let graph = make_graph(
            &[
                ("src/entry.ts", "src/entry.ts", SymbolKind::Module),
                ("src/entry.ts::main", "src/entry.ts", SymbolKind::Function),
                ("src/a.ts", "src/a.ts", SymbolKind::Module),
                ("src/a.ts::fa", "src/a.ts", SymbolKind::Function),
                ("src/b.ts", "src/b.ts", SymbolKind::Module),
                ("src/b.ts::fb", "src/b.ts", SymbolKind::Function),
                ("src/x.ts", "src/x.ts", SymbolKind::Module),
                ("src/x.ts::fx", "src/x.ts", SymbolKind::Function),
            ],
            &[
                // Forward: entry→a→b→x (distance 3)
                ("src/entry.ts::main", "src/a.ts::fa", EdgeType::Calls),
                ("src/a.ts::fa", "src/b.ts::fb", EdgeType::Calls),
                ("src/b.ts::fb", "src/x.ts::fx", EdgeType::Calls),
                // Reverse: x calls entry (reverse distance = 2)
                ("src/x.ts::fx", "src/entry.ts::main", EdgeType::Calls),
            ],
        );

        let merged = compute_file_reachability(&graph, "src/entry.ts", "main");
        assert_eq!(merged.get("src/entry.ts"), Some(&0));
        assert_eq!(merged.get("src/x.ts"), Some(&2), "should pick min(forward=3, reverse=2)");
        assert_eq!(merged.get("src/a.ts"), Some(&1));
    }

    #[test]
    fn test_bfs_disconnected_file_not_reached() {
        let graph = make_graph(
            &[
                ("src/entry.ts", "src/entry.ts", SymbolKind::Module),
                ("src/entry.ts::main", "src/entry.ts", SymbolKind::Function),
                ("src/connected.ts", "src/connected.ts", SymbolKind::Module),
                ("src/connected.ts::fc", "src/connected.ts", SymbolKind::Function),
                ("src/isolated.ts", "src/isolated.ts", SymbolKind::Module),
                ("src/isolated.ts::fi", "src/isolated.ts", SymbolKind::Function),
            ],
            &[
                ("src/entry.ts::main", "src/connected.ts::fc", EdgeType::Calls),
            ],
        );

        let forward = bfs_pass(&graph, "src/entry.ts", "main", Direction::Outgoing, 1);
        assert!(forward.contains_key("src/connected.ts"));
        assert!(!forward.contains_key("src/isolated.ts"));
    }

    #[test]
    fn test_bfs_entry_distance_always_zero() {
        let graph = make_graph(
            &[
                ("src/entry.ts", "src/entry.ts", SymbolKind::Module),
                ("src/entry.ts::main", "src/entry.ts", SymbolKind::Function),
            ],
            &[],
        );

        let forward = bfs_pass(&graph, "src/entry.ts", "main", Direction::Outgoing, 1);
        assert_eq!(forward.get("src/entry.ts"), Some(&0));

        let reverse = bfs_pass(&graph, "src/entry.ts", "main", Direction::Incoming, 2);
        assert_eq!(reverse.get("src/entry.ts"), Some(&0));
    }

    // ===================================================================
    // Property-based tests for bidirectional BFS and sub-clustering
    // ===================================================================

    mod proptests_bidir {
        use super::*;
        use proptest::prelude::*;
        use crate::graph::{SerializableEdge, SerializableGraph, SymbolNode};

        /// Build a forward chain graph: f0 → f1 → f2 → ... → f(n-1)
        fn build_forward_chain(n: usize) -> SymbolGraph {
            let mut nodes = Vec::new();
            let mut edges = Vec::new();
            for i in 0..n {
                let file = format!("src/f{}.ts", i);
                let func_id = format!("src/f{}.ts::func{}", i, i);
                nodes.push(SymbolNode {
                    id: file.clone(),
                    name: file.clone(),
                    file: file.clone(),
                    kind: SymbolKind::Module,
                });
                nodes.push(SymbolNode {
                    id: func_id.clone(),
                    name: format!("func{}", i),
                    file,
                    kind: SymbolKind::Function,
                });
                if i > 0 {
                    edges.push(SerializableEdge {
                        from: format!("src/f{}.ts::func{}", i - 1, i - 1),
                        to: func_id,
                        edge_type: EdgeType::Calls,
                    });
                }
            }
            SymbolGraph::from_serializable(&SerializableGraph { nodes, edges })
        }

        /// Build a reverse chain graph: f1→f0, f2→f1, ... (each file calls its predecessor)
        fn build_reverse_chain(n: usize) -> SymbolGraph {
            let mut nodes = Vec::new();
            let mut edges = Vec::new();
            for i in 0..n {
                let file = format!("src/f{}.ts", i);
                let func_id = format!("src/f{}.ts::func{}", i, i);
                nodes.push(SymbolNode {
                    id: file.clone(),
                    name: file.clone(),
                    file: file.clone(),
                    kind: SymbolKind::Module,
                });
                nodes.push(SymbolNode {
                    id: func_id.clone(),
                    name: format!("func{}", i),
                    file,
                    kind: SymbolKind::Function,
                });
                if i > 0 {
                    // Reverse: file i calls file i-1 (edge points toward f0/entry)
                    edges.push(SerializableEdge {
                        from: func_id,
                        to: format!("src/f{}.ts::func{}", i - 1, i - 1),
                        edge_type: EdgeType::Calls,
                    });
                }
            }
            SymbolGraph::from_serializable(&SerializableGraph { nodes, edges })
        }

        proptest! {
            /// Bidirectional BFS: every file still ends up in exactly one group or infrastructure.
            #[test]
            fn prop_bidir_every_file_placed(
                n_files in 1usize..8,
            ) {
                let files: Vec<String> = (0..n_files)
                    .map(|i| format!("src/f{}.ts", i))
                    .collect();

                // Empty graph — entrypoint file is grouped, rest goes to infrastructure.
                let graph = make_graph(&[], &[]);
                let ep_file = &files[0];
                let entrypoints = vec![ep(ep_file, "main", EntrypointType::CliCommand)];

                let result = cluster_files(&graph, &entrypoints, &files);

                let mut all: Vec<String> = Vec::new();
                for g in &result.groups {
                    for f in &g.files {
                        all.push(f.path.clone());
                    }
                }
                if let Some(ref infra) = result.infrastructure {
                    all.extend(infra.files.clone());
                }
                all.sort();
                let mut expected = files.clone();
                expected.sort();
                expected.dedup();
                prop_assert_eq!(all, expected);
            }

            /// Sub-clustering never loses files.
            #[test]
            fn prop_sub_cluster_preserves_all_files(
                n_files in 0usize..10,
            ) {
                let files: Vec<String> = (0..n_files)
                    .map(|i| format!("src/f{}.ts", i))
                    .collect();
                let graph = make_graph(&[], &[]);

                let sub_groups = sub_cluster_infra_files(&files, &graph);

                let mut all_sub: Vec<String> = sub_groups
                    .iter()
                    .flat_map(|g| g.files.clone())
                    .collect();
                all_sub.sort();
                let mut expected = files.clone();
                expected.sort();
                expected.dedup();
                prop_assert_eq!(all_sub, expected);
            }

            /// Sub-clustering: no file appears in two sub-groups.
            #[test]
            fn prop_sub_cluster_no_duplicates(
                n_files in 0usize..10,
            ) {
                let files: Vec<String> = (0..n_files)
                    .map(|i| format!("src/f{}.ts", i))
                    .collect();
                let graph = make_graph(&[], &[]);
                let sub_groups = sub_cluster_infra_files(&files, &graph);

                let all: Vec<&String> = sub_groups.iter().flat_map(|g| &g.files).collect();
                let unique: std::collections::HashSet<&String> = all.iter().cloned().collect();
                prop_assert_eq!(all.len(), unique.len(), "no file should appear in two sub-groups");
            }

            /// classify_by_convention is pure (same input → same output).
            #[test]
            fn prop_classify_deterministic(
                path in "[a-z/.]{1,40}",
            ) {
                let c1 = classify_by_convention(&path);
                let c2 = classify_by_convention(&path);
                prop_assert_eq!(c1, c2);
            }

            // ── Bidirectional BFS property tests ────────────────────────

            /// Forward chain: file at position i has distance i from entry.
            #[test]
            fn prop_forward_chain_distances_sequential(n in 2usize..10) {
                let graph = build_forward_chain(n);
                let forward = bfs_pass(&graph, "src/f0.ts", "func0", Direction::Outgoing, 1);
                for i in 0..n {
                    let file = format!("src/f{}.ts", i);
                    prop_assert_eq!(
                        forward.get(&file).copied(), Some(i),
                        "file {} should be at forward distance {}", file, i,
                    );
                }
            }

            /// Reverse chain: file at position i has reverse distance 2*i (cost_per_hop=2).
            #[test]
            fn prop_reverse_chain_distances_doubled(n in 2usize..10) {
                let graph = build_reverse_chain(n);
                let reverse = bfs_pass(&graph, "src/f0.ts", "func0", Direction::Incoming, 2);
                for i in 0..n {
                    let file = format!("src/f{}.ts", i);
                    prop_assert_eq!(
                        reverse.get(&file).copied(), Some(i * 2),
                        "file {} should be at reverse distance {}", file, i * 2,
                    );
                }
            }

            /// Forward chain: all files grouped with the entrypoint (no infrastructure).
            #[test]
            fn prop_forward_chain_all_grouped(n in 2usize..8) {
                let graph = build_forward_chain(n);
                let files: Vec<String> = (0..n).map(|i| format!("src/f{}.ts", i)).collect();
                let entrypoints = vec![ep("src/f0.ts", "func0", EntrypointType::HttpRoute)];
                let result = cluster_files(&graph, &entrypoints, &files);
                prop_assert_eq!(result.groups.len(), 1);
                prop_assert!(result.infrastructure.is_none(), "no infrastructure with forward chain");
                prop_assert_eq!(result.groups[0].files.len(), n);
            }

            /// Reverse chain: all files still grouped via bidirectional BFS (not infrastructure).
            #[test]
            fn prop_reverse_chain_all_grouped(n in 2usize..8) {
                let graph = build_reverse_chain(n);
                let files: Vec<String> = (0..n).map(|i| format!("src/f{}.ts", i)).collect();
                let entrypoints = vec![ep("src/f0.ts", "func0", EntrypointType::HttpRoute)];
                let result = cluster_files(&graph, &entrypoints, &files);
                prop_assert_eq!(result.groups.len(), 1);
                prop_assert!(
                    result.infrastructure.is_none(),
                    "reverse-reachable files should not be infrastructure",
                );
                prop_assert_eq!(result.groups[0].files.len(), n);
            }

            /// In a forward chain, flow_position preserves distance order.
            #[test]
            fn prop_forward_chain_flow_order_preserved(n in 3usize..8) {
                let graph = build_forward_chain(n);
                let files: Vec<String> = (0..n).map(|i| format!("src/f{}.ts", i)).collect();
                let entrypoints = vec![ep("src/f0.ts", "func0", EntrypointType::HttpRoute)];
                let result = cluster_files(&graph, &entrypoints, &files);
                prop_assert_eq!(result.groups.len(), 1);

                let positions: Vec<u32> = result.groups[0]
                    .files
                    .iter()
                    .map(|f| f.flow_position)
                    .collect();
                for w in positions.windows(2) {
                    prop_assert!(w[0] <= w[1], "flow positions should be non-decreasing");
                }
            }

            /// Merge picks minimum: when a file is reachable both forward and reverse,
            /// the merged distance equals min(forward_distance, reverse_distance).
            #[test]
            fn prop_merge_picks_minimum(n in 3usize..8) {
                // Cyclic graph: forward chain f0→f1→...→f(n-1) + back-edge f(n-1)→f0.
                // Reverse BFS from f0 follows incoming edges: finds f(n-1) at dist 2,
                // f(n-2) at dist 4, etc. Forward BFS finds fi at dist i.
                // Merged distance should be min(forward, reverse) for each file.
                let mut nodes = Vec::new();
                let mut edges = Vec::new();
                for i in 0..n {
                    let file = format!("src/f{}.ts", i);
                    let func_id = format!("src/f{}.ts::func{}", i, i);
                    nodes.push(SymbolNode {
                        id: file.clone(),
                        name: file.clone(),
                        file: file.clone(),
                        kind: SymbolKind::Module,
                    });
                    nodes.push(SymbolNode {
                        id: func_id.clone(),
                        name: format!("func{}", i),
                        file,
                        kind: SymbolKind::Function,
                    });
                    if i > 0 {
                        edges.push(SerializableEdge {
                            from: format!("src/f{}.ts::func{}", i - 1, i - 1),
                            to: func_id.clone(),
                            edge_type: EdgeType::Calls,
                        });
                    }
                }
                // Back-edge: last file calls first file
                edges.push(SerializableEdge {
                    from: format!("src/f{}.ts::func{}", n - 1, n - 1),
                    to: "src/f0.ts::func0".to_string(),
                    edge_type: EdgeType::Calls,
                });
                let graph = SymbolGraph::from_serializable(&SerializableGraph { nodes, edges });

                let forward = bfs_pass(&graph, "src/f0.ts", "func0", Direction::Outgoing, 1);
                let reverse = bfs_pass(&graph, "src/f0.ts", "func0", Direction::Incoming, 2);
                let merged = compute_file_reachability(&graph, "src/f0.ts", "func0");

                for (file, &dist) in &merged {
                    let fwd = forward.get(file).copied().unwrap_or(usize::MAX);
                    let rev = reverse.get(file).copied().unwrap_or(usize::MAX);
                    prop_assert_eq!(
                        dist, std::cmp::min(fwd, rev),
                        "file {} merged={} should be min(forward={}, reverse={})",
                        file, dist, fwd, rev,
                    );
                }
            }

            /// Files with no graph edges to any entrypoint always land in infrastructure.
            #[test]
            fn prop_disconnected_file_in_infra(n_extra in 1usize..8) {
                let entry_file = "src/entry.ts";
                let mut files = vec![entry_file.to_string()];
                for i in 0..n_extra {
                    files.push(format!("src/extra{}.ts", i));
                }

                // Graph has only the entry node — no edges to extra files.
                let graph = make_graph(
                    &[(entry_file, entry_file, SymbolKind::Module)],
                    &[],
                );
                let entrypoints = vec![ep(entry_file, "main", EntrypointType::HttpRoute)];
                let result = cluster_files(&graph, &entrypoints, &files);

                prop_assert_eq!(result.groups.len(), 1, "one group for the entrypoint");
                prop_assert_eq!(
                    result.groups[0].files.len(), 1,
                    "only the entrypoint file in the group",
                );
                prop_assert!(result.infrastructure.is_some(), "disconnected files → infra");
                prop_assert_eq!(
                    result.infrastructure.as_ref().unwrap().files.len(), n_extra,
                    "all extra files should be in infrastructure",
                );
            }

            /// The entrypoint file itself always has flow_position 0 in its group.
            #[test]
            fn prop_entry_distance_always_zero(n in 2usize..8) {
                let graph = build_forward_chain(n);
                let files: Vec<String> = (0..n).map(|i| format!("src/f{}.ts", i)).collect();
                let entrypoints = vec![ep("src/f0.ts", "func0", EntrypointType::HttpRoute)];
                let result = cluster_files(&graph, &entrypoints, &files);

                prop_assert_eq!(result.groups.len(), 1);
                let entry = result.groups[0]
                    .files
                    .iter()
                    .find(|f| f.path == "src/f0.ts");
                prop_assert!(entry.is_some(), "entrypoint file should be in the group");
                prop_assert_eq!(
                    entry.unwrap().flow_position, 0,
                    "entrypoint file should have flow_position 0",
                );
            }

            /// Reverse-only reachable files sort after immediate forward-reachable files
            /// in flow position (reverse cost=2 > forward cost=1).
            #[test]
            fn prop_reverse_flow_position_after_forward(n_rev in 1usize..5) {
                // Graph: entry → fwd0 (forward, dist 1)
                //        rev_i → entry for each i (reverse, dist 2 each)
                // fwd0 at dist 1 should have lower flow_position than all rev files at dist 2.
                let entry = "src/entry.ts";
                let fwd = "src/fwd0.ts";
                let mut nodes = vec![
                    SymbolNode { id: entry.to_string(), name: entry.to_string(),
                                 file: entry.to_string(), kind: SymbolKind::Module },
                    SymbolNode { id: format!("{}::main", entry), name: "main".to_string(),
                                 file: entry.to_string(), kind: SymbolKind::Function },
                    SymbolNode { id: fwd.to_string(), name: fwd.to_string(),
                                 file: fwd.to_string(), kind: SymbolKind::Module },
                    SymbolNode { id: format!("{}::func0", fwd), name: "func0".to_string(),
                                 file: fwd.to_string(), kind: SymbolKind::Function },
                ];
                let mut edges = vec![
                    SerializableEdge {
                        from: format!("{}::main", entry),
                        to: format!("{}::func0", fwd),
                        edge_type: EdgeType::Calls,
                    },
                ];
                for i in 0..n_rev {
                    let rev_file = format!("src/rev{}.ts", i);
                    let rev_func = format!("{}::rfunc{}", rev_file, i);
                    nodes.push(SymbolNode {
                        id: rev_file.clone(), name: rev_file.clone(),
                        file: rev_file.clone(), kind: SymbolKind::Module,
                    });
                    nodes.push(SymbolNode {
                        id: rev_func.clone(), name: format!("rfunc{}", i),
                        file: rev_file, kind: SymbolKind::Function,
                    });
                    // Reverse file calls entry → incoming edge to entry
                    edges.push(SerializableEdge {
                        from: rev_func,
                        to: format!("{}::main", entry),
                        edge_type: EdgeType::Calls,
                    });
                }

                let graph = SymbolGraph::from_serializable(&SerializableGraph { nodes, edges });
                let mut files = vec![entry.to_string(), fwd.to_string()];
                for i in 0..n_rev { files.push(format!("src/rev{}.ts", i)); }
                let entrypoints = vec![ep(entry, "main", EntrypointType::HttpRoute)];
                let result = cluster_files(&graph, &entrypoints, &files);

                prop_assert_eq!(result.groups.len(), 1);
                prop_assert!(result.infrastructure.is_none(), "all files should be grouped");

                let fwd_pos = result.groups[0].files.iter()
                    .find(|f| f.path == fwd)
                    .map(|f| f.flow_position)
                    .unwrap();
                for fc in &result.groups[0].files {
                    if fc.path.contains("rev") {
                        prop_assert!(
                            fc.flow_position >= fwd_pos,
                            "reverse file {} (pos {}) should sort after forward file (pos {})",
                            fc.path, fc.flow_position, fwd_pos,
                        );
                    }
                }
            }

            /// When a file is forward-reachable from one entrypoint and reverse-reachable
            /// from another, it is assigned to the forward-reachable entrypoint's group
            /// (forward cost=1 < reverse cost=2).
            #[test]
            fn prop_multi_entrypoint_forward_preferred(n_shared in 1usize..4) {
                // ep0 → shared_i (forward, dist 1)
                // shared_i → ep1 (so reverse BFS from ep1 finds shared_i at dist 2)
                // shared files should land in ep0's group.
                let ep0_file = "src/ep0.ts";
                let ep1_file = "src/ep1.ts";
                let mut nodes = vec![
                    SymbolNode { id: ep0_file.to_string(), name: ep0_file.to_string(),
                                 file: ep0_file.to_string(), kind: SymbolKind::Module },
                    SymbolNode { id: format!("{}::handler0", ep0_file), name: "handler0".to_string(),
                                 file: ep0_file.to_string(), kind: SymbolKind::Function },
                    SymbolNode { id: ep1_file.to_string(), name: ep1_file.to_string(),
                                 file: ep1_file.to_string(), kind: SymbolKind::Module },
                    SymbolNode { id: format!("{}::handler1", ep1_file), name: "handler1".to_string(),
                                 file: ep1_file.to_string(), kind: SymbolKind::Function },
                ];
                let mut edges = Vec::new();

                for i in 0..n_shared {
                    let shared = format!("src/shared{}.ts", i);
                    let shared_func = format!("{}::func{}", shared, i);
                    nodes.push(SymbolNode {
                        id: shared.clone(), name: shared.clone(),
                        file: shared.clone(), kind: SymbolKind::Module,
                    });
                    nodes.push(SymbolNode {
                        id: shared_func.clone(), name: format!("func{}", i),
                        file: shared, kind: SymbolKind::Function,
                    });
                    // Forward from ep0 → shared (dist 1)
                    edges.push(SerializableEdge {
                        from: format!("{}::handler0", ep0_file),
                        to: shared_func.clone(),
                        edge_type: EdgeType::Calls,
                    });
                    // shared → ep1 (incoming edge to ep1 for reverse BFS)
                    edges.push(SerializableEdge {
                        from: shared_func,
                        to: format!("{}::handler1", ep1_file),
                        edge_type: EdgeType::Calls,
                    });
                }

                let graph = SymbolGraph::from_serializable(&SerializableGraph { nodes, edges });
                let mut files = vec![ep0_file.to_string(), ep1_file.to_string()];
                for i in 0..n_shared { files.push(format!("src/shared{}.ts", i)); }
                let entrypoints = vec![
                    ep(ep0_file, "handler0", EntrypointType::HttpRoute),
                    ep(ep1_file, "handler1", EntrypointType::HttpRoute),
                ];
                let result = cluster_files(&graph, &entrypoints, &files);

                // Find ep0's group
                let ep0_group = result.groups.iter().find(|g|
                    g.files.iter().any(|f| f.path == ep0_file)
                );
                prop_assert!(ep0_group.is_some(), "ep0 should have a group");

                // All shared files should be in ep0's group (forward dist 1 < reverse dist 2)
                for i in 0..n_shared {
                    let shared = format!("src/shared{}.ts", i);
                    prop_assert!(
                        ep0_group.unwrap().files.iter().any(|f| f.path == shared),
                        "shared file {} should be in ep0's group (forward preferred)",
                        shared,
                    );
                }
            }

            // ── Sub-clustering property tests ───────────────────────────

            /// Files within each sub-group are always sorted.
            #[test]
            fn prop_sub_cluster_files_sorted(n in 0usize..10) {
                let files: Vec<String> = (0..n).map(|i| format!("src/f{}.ts", i)).collect();
                let graph = make_graph(&[], &[]);
                let sub_groups = sub_cluster_infra_files(&files, &graph);
                for sg in &sub_groups {
                    let mut sorted = sg.files.clone();
                    sorted.sort();
                    prop_assert_eq!(&sg.files, &sorted, "files in '{}' should be sorted", sg.name);
                }
            }

            /// Convention-classified files always land in their convention category.
            #[test]
            fn prop_sub_cluster_convention_categories_match(
                infra_idx in 0usize..5,
                schema_idx in 0usize..3,
                script_idx in 0usize..3,
            ) {
                let infra_paths = ["Dockerfile", "package.json", ".env.dev", "tsconfig.json", "Cargo.toml"];
                let schema_paths = ["schemas/user.ts", "src/user.dto.ts", "types/index.ts"];
                let script_paths = ["scripts/deploy.sh", "init.bash", "scripts/setup.zsh"];

                let files: Vec<String> = vec![
                    infra_paths[infra_idx % infra_paths.len()].to_string(),
                    schema_paths[schema_idx % schema_paths.len()].to_string(),
                    script_paths[script_idx % script_paths.len()].to_string(),
                ];

                let graph = make_graph(&[], &[]);
                let sub_groups = sub_cluster_infra_files(&files, &graph);

                for sg in &sub_groups {
                    for f in &sg.files {
                        let expected = classify_by_convention(f);
                        if expected != InfraCategory::Unclassified {
                            prop_assert_eq!(
                                &sg.category, &expected,
                                "file '{}' classified as {:?} but in sub-group {:?}",
                                f, expected, sg.category,
                            );
                        }
                    }
                }
            }

            /// Sub-clustering with realistic mixed paths preserves all files.
            #[test]
            fn prop_sub_cluster_realistic_paths_preserved(
                n_infra in 0usize..4,
                n_schema in 0usize..3,
                n_docs in 0usize..3,
                n_code in 0usize..4,
            ) {
                let mut files: Vec<String> = Vec::new();
                for i in 0..n_infra {
                    files.push(format!(".env.f{}", i));
                }
                for i in 0..n_schema {
                    files.push(format!("schemas/s{}.ts", i));
                }
                for i in 0..n_docs {
                    files.push(format!("docs/d{}.md", i));
                }
                for i in 0..n_code {
                    files.push(format!("src/c{}.ts", i));
                }
                files.sort();
                files.dedup();

                let graph = make_graph(&[], &[]);
                let sub_groups = sub_cluster_infra_files(&files, &graph);

                let mut all_sub: Vec<String> = sub_groups
                    .iter()
                    .flat_map(|g| g.files.clone())
                    .collect();
                all_sub.sort();
                prop_assert_eq!(all_sub, files, "all files must appear in sub-groups");

                // No duplicates
                let unique: std::collections::HashSet<&String> =
                    sub_groups.iter().flat_map(|g| &g.files).collect();
                let total: usize = sub_groups.iter().map(|g| g.files.len()).sum();
                prop_assert_eq!(unique.len(), total, "no duplicates across sub-groups");
            }

            /// sub_cluster_infra_files is pure: same input → identical output.
            #[test]
            fn prop_sub_cluster_deterministic(
                n_infra in 0usize..3,
                n_schema in 0usize..3,
                n_code in 0usize..3,
            ) {
                let mut files: Vec<String> = Vec::new();
                for i in 0..n_infra {
                    files.push(format!("Dockerfile.f{}", i));
                }
                for i in 0..n_schema {
                    files.push(format!("schemas/s{}.ts", i));
                }
                for i in 0..n_code {
                    files.push(format!("src/c{}.ts", i));
                }
                files.sort();
                files.dedup();

                let graph = make_graph(&[], &[]);
                let r1 = sub_cluster_infra_files(&files, &graph);
                let r2 = sub_cluster_infra_files(&files, &graph);
                prop_assert_eq!(r1, r2, "sub_cluster_infra_files should be deterministic");
            }

            // ── Classification invariant property tests ──────────────────

            /// Infrastructure filenames classify as Infrastructure regardless of
            /// directory nesting depth.
            #[test]
            fn prop_infra_dir_depth_invariant(depth in 0usize..6) {
                let infra_filenames = [
                    "Dockerfile", "docker-compose.yml", ".dockerignore",
                    "package.json", "Cargo.toml", "go.mod", "go.sum",
                    "Cargo.lock", "yarn.lock", "pnpm-lock.yaml",
                    "Makefile", "build.rs", "CMakeLists.txt",
                    ".gitignore", ".gitattributes",
                    ".tool-versions", ".nvmrc", ".node-version",
                ];
                let prefix: String = (0..depth).map(|i| format!("d{}/", i)).collect();
                for fname in &infra_filenames {
                    let path = format!("{}{}", prefix, fname);
                    prop_assert_eq!(
                        classify_by_convention(&path),
                        InfraCategory::Infrastructure,
                        "infra file '{}' nested {} deep should still be Infrastructure",
                        path, depth,
                    );
                }
            }

            /// Schema directory paths classify as Schema regardless of nesting.
            #[test]
            fn prop_schema_dir_depth_invariant(depth in 0usize..5, idx in 0usize..5) {
                let prefix: String = (0..depth).map(|i| format!("d{}/", i)).collect();
                let path = format!("{}schemas/file{}.ts", prefix, idx);
                prop_assert_eq!(
                    classify_by_convention(&path),
                    InfraCategory::Schema,
                    "schema path '{}' should be Schema regardless of depth",
                    path,
                );
            }

            /// Source code files (*.ts, *.js, *.py, *.rs) in src/ are never Infrastructure.
            #[test]
            fn prop_source_code_never_infrastructure(
                name_len in 1usize..15,
                ext_idx in 0usize..4,
            ) {
                let exts = ["ts", "js", "py", "rs"];
                let ext = exts[ext_idx % exts.len()];
                let name: String = (0..name_len).map(|i| (b'a' + (i as u8 % 26)) as char).collect();
                let path = format!("src/{}.{}", name, ext);
                prop_assert_ne!(
                    classify_by_convention(&path),
                    InfraCategory::Infrastructure,
                    "source code file '{}' should never be Infrastructure",
                    path,
                );
            }

            /// Infrastructure and Lint categories are disjoint: no path
            /// classifies as both via the convention classifier.
            #[test]
            fn prop_infra_lint_disjoint(idx in 0usize..16) {
                let lint_files = [
                    ".eslintrc.json", ".eslintrc.js", ".eslintrc.yml", ".eslintrc",
                    ".prettierrc", ".prettierrc.json", ".prettierrc.yml",
                    ".stylelintrc", ".stylelintrc.json",
                    ".editorconfig", ".clang-format", "rustfmt.toml",
                    ".rubocop.yml", ".flake8", "mypy.ini", ".golangci.yml",
                ];
                let file = lint_files[idx % lint_files.len()];
                let category = classify_by_convention(file);
                prop_assert_eq!(
                    category,
                    InfraCategory::Lint,
                    "lint file '{}' should be Lint, not Infrastructure",
                    file,
                );
            }

            /// Every InfraCategory variant (except DirectoryGroup/Unclassified)
            /// has at least one representative path that classifies correctly.
            #[test]
            fn prop_all_categories_reachable(cat_idx in 0usize..9) {
                let representatives: [(InfraCategory, &str); 9] = [
                    (InfraCategory::Infrastructure, "Dockerfile"),
                    (InfraCategory::Schema, "schemas/user.ts"),
                    (InfraCategory::Script, "scripts/deploy.sh"),
                    (InfraCategory::Migration, "migrations/001.sql"),
                    (InfraCategory::Deployment, "deploy/app.yaml"),
                    (InfraCategory::Documentation, "docs/README.md"),
                    (InfraCategory::Lint, ".eslintrc.json"),
                    (InfraCategory::TestUtil, "src/test-utils/helpers.ts"),
                    (InfraCategory::Generated, "src/generated/types.ts"),
                ];
                let (expected, path) = &representatives[cat_idx % representatives.len()];
                prop_assert_eq!(
                    &classify_by_convention(path),
                    expected,
                    "representative path '{}' should classify as {:?}",
                    path, expected,
                );
            }
        }
    }
}
