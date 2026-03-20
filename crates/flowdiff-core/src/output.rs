//! JSON output module: constructs AnalysisOutput from pipeline results and serializes to JSON.
//!
//! Responsibilities:
//! - Build `AnalysisOutput` from diff extraction, AST parsing, clustering, and ranking results
//! - Generate Mermaid flow diagrams for each group
//! - Serialize to JSON (stdout or file)

use std::collections::HashSet;
use std::io::Write;

use crate::ast::{Language, ParsedFile};
use crate::cluster::ClusterResult;
use crate::git::DiffResult;
use crate::rank::is_risk_path;
use crate::types::{
    AnalysisOutput, AnalysisSummary, DiffSource, DiffType, FlowGroup, RankedGroup,
};

/// Errors from output operations.
#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("JSON serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Build an `AnalysisOutput` from pipeline results.
///
/// Combines diff metadata, parsed file info, cluster results, and ranking
/// into the final JSON-serializable output structure.
pub fn build_analysis_output(
    diff_result: &DiffResult,
    diff_source: DiffSource,
    parsed_files: &[ParsedFile],
    cluster_result: &ClusterResult,
    ranked_groups: &[RankedGroup],
) -> AnalysisOutput {
    // Detect languages from parsed files.
    let languages_detected: Vec<String> = {
        let mut langs: Vec<String> = parsed_files
            .iter()
            .map(|f| f.language)
            .collect::<HashSet<_>>()
            .into_iter()
            .filter(|l| *l != Language::Unknown)
            .map(|l| format!("{:?}", l).to_lowercase())
            .collect();
        langs.sort();
        langs
    };

    // Apply ranking to groups: update risk_score and review_order from ranked results.
    let groups: Vec<FlowGroup> = cluster_result
        .groups
        .iter()
        .map(|group| {
            let ranked = ranked_groups
                .iter()
                .find(|r| r.group_id == group.id);
            FlowGroup {
                risk_score: ranked.map_or(group.risk_score, |r| r.composite_score),
                review_order: ranked.map_or(group.review_order, |r| r.review_order),
                ..group.clone()
            }
        })
        .collect();

    let frameworks_detected = crate::flow::detect_frameworks(parsed_files);

    let summary = AnalysisSummary {
        total_files_changed: diff_result.files.len() as u32,
        total_groups: groups.len() as u32,
        languages_detected,
        frameworks_detected,
    };

    AnalysisOutput {
        version: "1.0.0".to_string(),
        diff_source,
        summary,
        groups,
        infrastructure_group: cluster_result.infrastructure.clone(),
        annotations: None,
    }
}

/// Create a `DiffSource` for a branch comparison.
pub fn diff_source_branch(
    base: &str,
    head: &str,
    base_sha: Option<&str>,
    head_sha: Option<&str>,
) -> DiffSource {
    DiffSource {
        diff_type: DiffType::BranchComparison,
        base: Some(base.to_string()),
        head: Some(head.to_string()),
        base_sha: base_sha.map(|s| s.to_string()),
        head_sha: head_sha.map(|s| s.to_string()),
    }
}

/// Create a `DiffSource` for a commit range.
pub fn diff_source_range(range: &str, base_sha: Option<&str>, head_sha: Option<&str>) -> DiffSource {
    DiffSource {
        diff_type: DiffType::CommitRange,
        base: Some(range.to_string()),
        head: None,
        base_sha: base_sha.map(|s| s.to_string()),
        head_sha: head_sha.map(|s| s.to_string()),
    }
}

/// Create a `DiffSource` for staged changes.
pub fn diff_source_staged() -> DiffSource {
    DiffSource {
        diff_type: DiffType::Staged,
        base: None,
        head: None,
        base_sha: None,
        head_sha: None,
    }
}

/// Create a `DiffSource` for unstaged changes.
pub fn diff_source_unstaged() -> DiffSource {
    DiffSource {
        diff_type: DiffType::Unstaged,
        base: None,
        head: None,
        base_sha: None,
        head_sha: None,
    }
}

/// Generate a Mermaid flow diagram for a flow group.
///
/// Produces a `graph TD` diagram with nodes for each file and edges
/// showing data flow relationships.
pub fn generate_mermaid(group: &FlowGroup) -> String {
    if group.files.is_empty() {
        return "graph TD\n  empty[No files]".to_string();
    }

    let mut lines = vec!["graph TD".to_string()];

    // Collect nodes: assign short IDs (A, B, C, ...) for readability.
    let node_ids: Vec<(String, String)> = group
        .files
        .iter()
        .enumerate()
        .map(|(i, file)| {
            let id = node_id_from_index(i);
            let label = short_label(&file.path);
            (id, label)
        })
        .collect();

    // Build file -> node_id lookup for edge resolution.
    let file_to_node: std::collections::HashMap<&str, &str> = group
        .files
        .iter()
        .enumerate()
        .map(|(i, file)| (file.path.as_str(), node_ids[i].0.as_str()))
        .collect();

    // Add node declarations.
    for (id, label) in &node_ids {
        lines.push(format!("  {}[{}]", id, label));
    }

    // Add edges from the group's flow edges.
    let mut seen_edges: HashSet<(String, String)> = HashSet::new();
    for edge in &group.edges {
        // Extract file path from symbol ID (format: "path::symbol").
        let from_file = edge.from.split("::").next().unwrap_or(&edge.from);
        let to_file = edge.to.split("::").next().unwrap_or(&edge.to);

        if let (Some(&from_node), Some(&to_node)) =
            (file_to_node.get(from_file), file_to_node.get(to_file))
        {
            let edge_key = (from_node.to_string(), to_node.to_string());
            if from_node != to_node && seen_edges.insert(edge_key) {
                let label = edge_type_label(&edge.edge_type);
                if label.is_empty() {
                    lines.push(format!("  {} --> {}", from_node, to_node));
                } else {
                    lines.push(format!("  {} -->|{}| {}", from_node, label, to_node));
                }
            }
        }
    }

    lines.join("\n")
}

/// Serialize `AnalysisOutput` to a JSON string (pretty-printed).
pub fn to_json(output: &AnalysisOutput) -> Result<String, OutputError> {
    Ok(serde_json::to_string_pretty(output)?)
}

/// Serialize `AnalysisOutput` to compact JSON.
pub fn to_json_compact(output: &AnalysisOutput) -> Result<String, OutputError> {
    Ok(serde_json::to_string(output)?)
}

/// Write `AnalysisOutput` as JSON to a writer (file or stdout).
pub fn write_json<W: Write>(output: &AnalysisOutput, writer: &mut W) -> Result<(), OutputError> {
    serde_json::to_writer_pretty(&mut *writer, output)?;
    writeln!(writer)?;
    Ok(())
}

/// Compute aggregate risk flags from a set of file paths.
///
/// Useful for summarizing risk at the group level.
pub fn compute_group_risk_flags(file_paths: &[&str]) -> GroupRiskFlags {
    if file_paths.is_empty() {
        return GroupRiskFlags::default();
    }
    let mut flags = GroupRiskFlags {
        has_test_only: true, // Start true, AND with each file's test status.
        ..Default::default()
    };
    for path in file_paths {
        let result = is_risk_path(path);
        flags.has_schema_change |= result.is_schema;
        flags.has_auth_change |= result.is_auth;
        flags.has_api_change |= result.is_api;
        flags.has_test_only &= result.is_test;
    }
    flags
}

/// Aggregate risk flags for a group of files.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GroupRiskFlags {
    pub has_schema_change: bool,
    pub has_auth_change: bool,
    pub has_api_change: bool,
    /// True only if ALL files in the group are test files.
    pub has_test_only: bool,
}

// ── Internal helpers ──

/// Generate a node ID from an index (A, B, ..., Z, AA, AB, ...).
fn node_id_from_index(i: usize) -> String {
    if i < 26 {
        ((b'A' + i as u8) as char).to_string()
    } else {
        let first = (b'A' + (i / 26 - 1) as u8) as char;
        let second = (b'A' + (i % 26) as u8) as char;
        format!("{}{}", first, second)
    }
}

/// Extract a short label from a file path (filename + parent dir).
fn short_label(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.len() {
        0 => path.to_string(),
        1 => parts[0].to_string(),
        _ => format!("{}/{}", parts[parts.len() - 2], parts[parts.len() - 1]),
    }
}

/// Convert an EdgeType to a short Mermaid edge label.
fn edge_type_label(edge_type: &crate::types::EdgeType) -> &'static str {
    use crate::types::EdgeType;
    match edge_type {
        EdgeType::Imports => "imports",
        EdgeType::Calls => "calls",
        EdgeType::Extends => "extends",
        EdgeType::Instantiates => "new",
        EdgeType::Reads => "reads",
        EdgeType::Writes => "writes",
        EdgeType::Emits => "emits",
        EdgeType::Handles => "handles",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod tests {
    use super::*;
    use crate::types::*;

    // ── Helpers ──

    fn sample_diff_result(file_count: usize) -> DiffResult {
        DiffResult {
            files: (0..file_count)
                .map(|i| crate::git::FileDiff {
                    old_path: Some(format!("src/file{}.ts", i)),
                    new_path: Some(format!("src/file{}.ts", i)),
                    old_content: Some("// old".to_string()),
                    new_content: Some("// new".to_string()),
                    hunks: vec![],
                    status: crate::git::FileStatus::Modified,
                    additions: 10,
                    deletions: 5,
                    is_binary: false,
                })
                .collect(),
            base_sha: Some("abc123".to_string()),
            head_sha: Some("def456".to_string()),
        }
    }

    fn sample_parsed_files() -> Vec<ParsedFile> {
        vec![
            ParsedFile {
                path: "src/route.ts".to_string(),
                language: Language::TypeScript,
                definitions: vec![],
                imports: vec![],
                exports: vec![],
                call_sites: vec![],
            },
            ParsedFile {
                path: "src/service.py".to_string(),
                language: Language::Python,
                definitions: vec![],
                imports: vec![],
                exports: vec![],
                call_sites: vec![],
            },
        ]
    }

    fn sample_flow_group(id: &str, name: &str) -> FlowGroup {
        FlowGroup {
            id: id.to_string(),
            name: name.to_string(),
            entrypoint: Some(Entrypoint {
                file: "src/route.ts".to_string(),
                symbol: "POST".to_string(),
                entrypoint_type: EntrypointType::HttpRoute,
            }),
            files: vec![
                FileChange {
                    path: "src/route.ts".to_string(),
                    flow_position: 0,
                    role: FileRole::Entrypoint,
                    changes: ChangeStats {
                        additions: 25,
                        deletions: 10,
                    },
                    symbols_changed: vec!["POST".to_string()],
                },
                FileChange {
                    path: "src/services/user.ts".to_string(),
                    flow_position: 1,
                    role: FileRole::Service,
                    changes: ChangeStats {
                        additions: 15,
                        deletions: 5,
                    },
                    symbols_changed: vec!["createUser".to_string()],
                },
                FileChange {
                    path: "src/repo/user-repo.ts".to_string(),
                    flow_position: 2,
                    role: FileRole::Repository,
                    changes: ChangeStats {
                        additions: 8,
                        deletions: 2,
                    },
                    symbols_changed: vec!["insert".to_string()],
                },
            ],
            edges: vec![
                FlowEdge {
                    from: "src/route.ts::POST".to_string(),
                    to: "src/services/user.ts::createUser".to_string(),
                    edge_type: EdgeType::Calls,
                },
                FlowEdge {
                    from: "src/services/user.ts::createUser".to_string(),
                    to: "src/repo/user-repo.ts::insert".to_string(),
                    edge_type: EdgeType::Calls,
                },
            ],
            risk_score: 0.0,
            review_order: 0,
        }
    }

    fn sample_cluster_result() -> ClusterResult {
        ClusterResult {
            groups: vec![
                sample_flow_group("group_1", "POST /api/users creation flow"),
                FlowGroup {
                    id: "group_2".to_string(),
                    name: "GET /api/health check".to_string(),
                    entrypoint: None,
                    files: vec![FileChange {
                        path: "src/health.ts".to_string(),
                        flow_position: 0,
                        role: FileRole::Handler,
                        changes: ChangeStats {
                            additions: 3,
                            deletions: 1,
                        },
                        symbols_changed: vec!["healthCheck".to_string()],
                    }],
                    edges: vec![],
                    risk_score: 0.0,
                    review_order: 0,
                },
            ],
            infrastructure: Some(InfrastructureGroup {
                files: vec!["tsconfig.json".to_string(), "package.json".to_string()],
                reason: "Not reachable from any detected entrypoint".to_string(),
            }),
        }
    }

    fn sample_ranked_groups() -> Vec<RankedGroup> {
        vec![
            RankedGroup {
                group_id: "group_1".to_string(),
                composite_score: 0.82,
                review_order: 1,
            },
            RankedGroup {
                group_id: "group_2".to_string(),
                composite_score: 0.35,
                review_order: 2,
            },
        ]
    }

    // ── build_analysis_output tests ──

    #[test]
    fn test_build_output_version() {
        let output = build_analysis_output(
            &sample_diff_result(3),
            diff_source_branch("main", "feature", Some("abc"), Some("def")),
            &sample_parsed_files(),
            &sample_cluster_result(),
            &sample_ranked_groups(),
        );
        assert_eq!(output.version, "1.0.0");
    }

    #[test]
    fn test_build_output_diff_source_branch() {
        let output = build_analysis_output(
            &sample_diff_result(3),
            diff_source_branch("main", "feature", Some("abc123"), Some("def456")),
            &sample_parsed_files(),
            &sample_cluster_result(),
            &sample_ranked_groups(),
        );
        assert_eq!(output.diff_source.diff_type, DiffType::BranchComparison);
        assert_eq!(output.diff_source.base, Some("main".to_string()));
        assert_eq!(output.diff_source.head, Some("feature".to_string()));
        assert_eq!(output.diff_source.base_sha, Some("abc123".to_string()));
        assert_eq!(output.diff_source.head_sha, Some("def456".to_string()));
    }

    #[test]
    fn test_build_output_summary_file_count() {
        let output = build_analysis_output(
            &sample_diff_result(5),
            diff_source_staged(),
            &sample_parsed_files(),
            &sample_cluster_result(),
            &sample_ranked_groups(),
        );
        assert_eq!(output.summary.total_files_changed, 5);
    }

    #[test]
    fn test_build_output_summary_group_count() {
        let output = build_analysis_output(
            &sample_diff_result(3),
            diff_source_staged(),
            &sample_parsed_files(),
            &sample_cluster_result(),
            &sample_ranked_groups(),
        );
        assert_eq!(output.summary.total_groups, 2);
    }

    #[test]
    fn test_build_output_languages_detected() {
        let output = build_analysis_output(
            &sample_diff_result(2),
            diff_source_staged(),
            &sample_parsed_files(),
            &sample_cluster_result(),
            &sample_ranked_groups(),
        );
        assert!(output.summary.languages_detected.contains(&"typescript".to_string()));
        assert!(output.summary.languages_detected.contains(&"python".to_string()));
        assert!(!output.summary.languages_detected.contains(&"unknown".to_string()));
    }

    #[test]
    fn test_build_output_languages_sorted() {
        let output = build_analysis_output(
            &sample_diff_result(2),
            diff_source_staged(),
            &sample_parsed_files(),
            &sample_cluster_result(),
            &sample_ranked_groups(),
        );
        let langs = &output.summary.languages_detected;
        let mut sorted = langs.clone();
        sorted.sort();
        assert_eq!(*langs, sorted, "languages should be sorted alphabetically");
    }

    #[test]
    fn test_build_output_unknown_language_excluded() {
        let files = vec![ParsedFile {
            path: "Makefile".to_string(),
            language: Language::Unknown,
            definitions: vec![],
            imports: vec![],
            exports: vec![],
            call_sites: vec![],
        }];
        let output = build_analysis_output(
            &sample_diff_result(1),
            diff_source_staged(),
            &files,
            &sample_cluster_result(),
            &sample_ranked_groups(),
        );
        assert!(output.summary.languages_detected.is_empty());
    }

    #[test]
    fn test_build_output_applies_ranking() {
        let output = build_analysis_output(
            &sample_diff_result(3),
            diff_source_staged(),
            &sample_parsed_files(),
            &sample_cluster_result(),
            &sample_ranked_groups(),
        );
        let g1 = output.groups.iter().find(|g| g.id == "group_1").unwrap();
        assert_eq!(g1.risk_score, 0.82);
        assert_eq!(g1.review_order, 1);

        let g2 = output.groups.iter().find(|g| g.id == "group_2").unwrap();
        assert_eq!(g2.risk_score, 0.35);
        assert_eq!(g2.review_order, 2);
    }

    #[test]
    fn test_build_output_unranked_group_keeps_defaults() {
        // If a group has no matching ranked entry, keep its original values.
        let output = build_analysis_output(
            &sample_diff_result(3),
            diff_source_staged(),
            &sample_parsed_files(),
            &sample_cluster_result(),
            &[], // no ranked groups
        );
        let g1 = output.groups.iter().find(|g| g.id == "group_1").unwrap();
        assert_eq!(g1.risk_score, 0.0);
        assert_eq!(g1.review_order, 0);
    }

    #[test]
    fn test_build_output_infrastructure_group() {
        let output = build_analysis_output(
            &sample_diff_result(3),
            diff_source_staged(),
            &sample_parsed_files(),
            &sample_cluster_result(),
            &sample_ranked_groups(),
        );
        let infra = output.infrastructure_group.as_ref().unwrap();
        assert_eq!(infra.files, vec!["tsconfig.json", "package.json"]);
        assert!(!infra.reason.is_empty());
    }

    #[test]
    fn test_build_output_no_infrastructure_group() {
        let cluster = ClusterResult {
            groups: vec![sample_flow_group("g1", "test group")],
            infrastructure: None,
        };
        let output = build_analysis_output(
            &sample_diff_result(3),
            diff_source_staged(),
            &sample_parsed_files(),
            &cluster,
            &[],
        );
        assert!(output.infrastructure_group.is_none());
    }

    #[test]
    fn test_build_output_annotations_null() {
        let output = build_analysis_output(
            &sample_diff_result(1),
            diff_source_staged(),
            &sample_parsed_files(),
            &sample_cluster_result(),
            &sample_ranked_groups(),
        );
        assert!(output.annotations.is_none());
    }

    #[test]
    fn test_build_output_empty_diff() {
        let diff = DiffResult {
            files: vec![],
            base_sha: None,
            head_sha: None,
        };
        let cluster = ClusterResult {
            groups: vec![],
            infrastructure: None,
        };
        let output = build_analysis_output(
            &diff,
            diff_source_staged(),
            &[],
            &cluster,
            &[],
        );
        assert_eq!(output.summary.total_files_changed, 0);
        assert_eq!(output.summary.total_groups, 0);
        assert!(output.groups.is_empty());
        assert!(output.infrastructure_group.is_none());
    }

    // ── DiffSource constructor tests ──

    #[test]
    fn test_diff_source_branch() {
        let ds = diff_source_branch("main", "feat", Some("aaa"), Some("bbb"));
        assert_eq!(ds.diff_type, DiffType::BranchComparison);
        assert_eq!(ds.base, Some("main".to_string()));
        assert_eq!(ds.head, Some("feat".to_string()));
    }

    #[test]
    fn test_diff_source_range() {
        let ds = diff_source_range("HEAD~5..HEAD", Some("aaa"), Some("bbb"));
        assert_eq!(ds.diff_type, DiffType::CommitRange);
        assert_eq!(ds.base, Some("HEAD~5..HEAD".to_string()));
        assert!(ds.head.is_none());
    }

    #[test]
    fn test_diff_source_staged() {
        let ds = diff_source_staged();
        assert_eq!(ds.diff_type, DiffType::Staged);
        assert!(ds.base.is_none());
        assert!(ds.head.is_none());
    }

    #[test]
    fn test_diff_source_unstaged() {
        let ds = diff_source_unstaged();
        assert_eq!(ds.diff_type, DiffType::Unstaged);
    }

    // ── JSON serialization tests ──

    #[test]
    fn test_json_schema_compliance() {
        let output = build_analysis_output(
            &sample_diff_result(3),
            diff_source_branch("main", "feature", Some("abc123"), Some("def456")),
            &sample_parsed_files(),
            &sample_cluster_result(),
            &sample_ranked_groups(),
        );
        let json = to_json(&output).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        // Top-level fields exist and have correct types.
        assert_eq!(parsed["version"], "1.0.0");
        assert!(parsed["diff_source"].is_object());
        assert!(parsed["summary"].is_object());
        assert!(parsed["groups"].is_array());
        assert!(parsed["annotations"].is_null());

        // diff_source fields
        assert_eq!(parsed["diff_source"]["diff_type"], "BranchComparison");
        assert_eq!(parsed["diff_source"]["base"], "main");
        assert_eq!(parsed["diff_source"]["head"], "feature");

        // summary fields
        assert_eq!(parsed["summary"]["total_files_changed"], 3);
        assert_eq!(parsed["summary"]["total_groups"], 2);
        assert!(parsed["summary"]["languages_detected"].is_array());
        assert!(parsed["summary"]["frameworks_detected"].is_array());

        // groups array
        let groups = parsed["groups"].as_array().unwrap();
        assert_eq!(groups.len(), 2);

        // First group structure
        let g1 = &groups[0];
        assert!(g1["id"].is_string());
        assert!(g1["name"].is_string());
        assert!(g1["entrypoint"].is_object() || g1["entrypoint"].is_null());
        assert!(g1["files"].is_array());
        assert!(g1["edges"].is_array());
        assert!(g1["risk_score"].is_number());
        assert!(g1["review_order"].is_number());
    }

    #[test]
    fn test_json_roundtrip() {
        let output = build_analysis_output(
            &sample_diff_result(3),
            diff_source_branch("main", "feature", Some("abc"), Some("def")),
            &sample_parsed_files(),
            &sample_cluster_result(),
            &sample_ranked_groups(),
        );
        let json = to_json(&output).unwrap();
        let deserialized: AnalysisOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(output, deserialized);
    }

    #[test]
    fn test_json_compact() {
        let output = build_analysis_output(
            &sample_diff_result(1),
            diff_source_staged(),
            &[],
            &ClusterResult {
                groups: vec![],
                infrastructure: None,
            },
            &[],
        );
        let json = to_json_compact(&output).unwrap();
        // Compact JSON should have no newlines (single line).
        assert!(!json.contains('\n'));
        // Should still be valid JSON.
        let _: serde_json::Value = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn test_write_json_to_buffer() {
        let output = build_analysis_output(
            &sample_diff_result(1),
            diff_source_staged(),
            &[],
            &ClusterResult {
                groups: vec![],
                infrastructure: None,
            },
            &[],
        );
        let mut buf: Vec<u8> = Vec::new();
        write_json(&output, &mut buf).unwrap();
        let written = String::from_utf8(buf).unwrap();
        assert!(written.contains("\"version\""));
        assert!(written.ends_with('\n'));
    }

    #[test]
    fn test_json_file_changes_structure() {
        let output = build_analysis_output(
            &sample_diff_result(3),
            diff_source_staged(),
            &sample_parsed_files(),
            &sample_cluster_result(),
            &sample_ranked_groups(),
        );
        let json = to_json(&output).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        let files = parsed["groups"][0]["files"].as_array().unwrap();
        let f0 = &files[0];
        assert!(f0["path"].is_string());
        assert!(f0["flow_position"].is_number());
        assert!(f0["role"].is_string());
        assert!(f0["changes"].is_object());
        assert!(f0["changes"]["additions"].is_number());
        assert!(f0["changes"]["deletions"].is_number());
        assert!(f0["symbols_changed"].is_array());
    }

    #[test]
    fn test_json_edges_structure() {
        let output = build_analysis_output(
            &sample_diff_result(3),
            diff_source_staged(),
            &sample_parsed_files(),
            &sample_cluster_result(),
            &sample_ranked_groups(),
        );
        let json = to_json(&output).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        let edges = parsed["groups"][0]["edges"].as_array().unwrap();
        assert!(!edges.is_empty());
        let e0 = &edges[0];
        assert!(e0["from"].is_string());
        assert!(e0["to"].is_string());
        assert!(e0["edge_type"].is_string());
    }

    #[test]
    fn test_json_infrastructure_group_structure() {
        let output = build_analysis_output(
            &sample_diff_result(3),
            diff_source_staged(),
            &sample_parsed_files(),
            &sample_cluster_result(),
            &sample_ranked_groups(),
        );
        let json = to_json(&output).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        let infra = &parsed["infrastructure_group"];
        assert!(infra["files"].is_array());
        assert!(infra["reason"].is_string());
    }

    // ── Mermaid generation tests ──

    #[test]
    fn test_mermaid_basic_flow() {
        let group = sample_flow_group("g1", "test");
        let mermaid = generate_mermaid(&group);

        assert!(mermaid.starts_with("graph TD"));
        // Should have nodes for all 3 files.
        assert!(mermaid.contains("A["));
        assert!(mermaid.contains("B["));
        assert!(mermaid.contains("C["));
        // Should have edges.
        assert!(mermaid.contains("-->"));
    }

    #[test]
    fn test_mermaid_edge_labels() {
        let group = sample_flow_group("g1", "test");
        let mermaid = generate_mermaid(&group);
        // The edges are Calls type, so should have "calls" label.
        assert!(mermaid.contains("|calls|"));
    }

    #[test]
    fn test_mermaid_empty_group() {
        let group = FlowGroup {
            id: "empty".to_string(),
            name: "empty".to_string(),
            entrypoint: None,
            files: vec![],
            edges: vec![],
            risk_score: 0.0,
            review_order: 0,
        };
        let mermaid = generate_mermaid(&group);
        assert!(mermaid.contains("graph TD"));
        assert!(mermaid.contains("empty[No files]"));
    }

    #[test]
    fn test_mermaid_no_duplicate_edges() {
        let group = FlowGroup {
            id: "g1".to_string(),
            name: "test".to_string(),
            entrypoint: None,
            files: vec![
                FileChange {
                    path: "a.ts".to_string(),
                    flow_position: 0,
                    role: FileRole::Entrypoint,
                    changes: ChangeStats { additions: 1, deletions: 0 },
                    symbols_changed: vec![],
                },
                FileChange {
                    path: "b.ts".to_string(),
                    flow_position: 1,
                    role: FileRole::Service,
                    changes: ChangeStats { additions: 1, deletions: 0 },
                    symbols_changed: vec![],
                },
            ],
            edges: vec![
                FlowEdge {
                    from: "a.ts::foo".to_string(),
                    to: "b.ts::bar".to_string(),
                    edge_type: EdgeType::Calls,
                },
                FlowEdge {
                    from: "a.ts::baz".to_string(),
                    to: "b.ts::qux".to_string(),
                    edge_type: EdgeType::Calls,
                },
            ],
            risk_score: 0.0,
            review_order: 0,
        };
        let mermaid = generate_mermaid(&group);
        // Both edges are between the same files, so only one Mermaid edge.
        let arrow_count = mermaid.matches("-->").count();
        assert_eq!(arrow_count, 1, "duplicate file-level edges should be deduplicated");
    }

    #[test]
    fn test_mermaid_no_self_edges() {
        let group = FlowGroup {
            id: "g1".to_string(),
            name: "test".to_string(),
            entrypoint: None,
            files: vec![FileChange {
                path: "a.ts".to_string(),
                flow_position: 0,
                role: FileRole::Utility,
                changes: ChangeStats { additions: 1, deletions: 0 },
                symbols_changed: vec![],
            }],
            edges: vec![FlowEdge {
                from: "a.ts::foo".to_string(),
                to: "a.ts::bar".to_string(),
                edge_type: EdgeType::Calls,
            }],
            risk_score: 0.0,
            review_order: 0,
        };
        let mermaid = generate_mermaid(&group);
        // Should not have any edges (self-edge on same file).
        assert_eq!(mermaid.matches("-->").count(), 0);
    }

    #[test]
    fn test_mermaid_short_labels() {
        let group = FlowGroup {
            id: "g1".to_string(),
            name: "test".to_string(),
            entrypoint: None,
            files: vec![FileChange {
                path: "src/handlers/auth.ts".to_string(),
                flow_position: 0,
                role: FileRole::Handler,
                changes: ChangeStats { additions: 1, deletions: 0 },
                symbols_changed: vec![],
            }],
            edges: vec![],
            risk_score: 0.0,
            review_order: 0,
        };
        let mermaid = generate_mermaid(&group);
        // Label should be "handlers/auth.ts" not the full path.
        assert!(mermaid.contains("handlers/auth.ts"));
    }

    #[test]
    fn test_mermaid_all_edge_types() {
        use crate::types::EdgeType;
        let types = [
            (EdgeType::Imports, "imports"),
            (EdgeType::Calls, "calls"),
            (EdgeType::Extends, "extends"),
            (EdgeType::Instantiates, "new"),
            (EdgeType::Reads, "reads"),
            (EdgeType::Writes, "writes"),
            (EdgeType::Emits, "emits"),
            (EdgeType::Handles, "handles"),
        ];
        for (edge_type, expected_label) in types {
            let group = FlowGroup {
                id: "g1".to_string(),
                name: "test".to_string(),
                entrypoint: None,
                files: vec![
                    FileChange {
                        path: "a.ts".to_string(),
                        flow_position: 0,
                        role: FileRole::Utility,
                        changes: ChangeStats { additions: 1, deletions: 0 },
                        symbols_changed: vec![],
                    },
                    FileChange {
                        path: "b.ts".to_string(),
                        flow_position: 1,
                        role: FileRole::Utility,
                        changes: ChangeStats { additions: 1, deletions: 0 },
                        symbols_changed: vec![],
                    },
                ],
                edges: vec![FlowEdge {
                    from: "a.ts::x".to_string(),
                    to: "b.ts::y".to_string(),
                    edge_type,
                }],
                risk_score: 0.0,
                review_order: 0,
            };
            let mermaid = generate_mermaid(&group);
            assert!(
                mermaid.contains(&format!("|{}|", expected_label)),
                "edge type {:?} should produce label '{}'",
                group.edges[0].edge_type,
                expected_label
            );
        }
    }

    // ── GroupRiskFlags tests ──

    #[test]
    fn test_risk_flags_schema_change() {
        let flags = compute_group_risk_flags(&["src/db/schema.prisma", "src/handler.ts"]);
        assert!(flags.has_schema_change);
        assert!(!flags.has_auth_change);
    }

    #[test]
    fn test_risk_flags_auth_change() {
        let flags = compute_group_risk_flags(&["src/auth/middleware.ts"]);
        assert!(flags.has_auth_change);
    }

    #[test]
    fn test_risk_flags_api_change() {
        let flags = compute_group_risk_flags(&["src/api/users.ts"]);
        assert!(flags.has_api_change);
    }

    #[test]
    fn test_risk_flags_test_only_true() {
        let flags = compute_group_risk_flags(&[
            "src/handlers/__tests__/auth.test.ts",
            "tests/integration.spec.ts",
        ]);
        assert!(flags.has_test_only);
    }

    #[test]
    fn test_risk_flags_test_only_false_mixed() {
        let flags = compute_group_risk_flags(&[
            "src/service.ts",
            "src/__tests__/service.test.ts",
        ]);
        assert!(!flags.has_test_only);
    }

    #[test]
    fn test_risk_flags_empty() {
        let flags = compute_group_risk_flags(&[]);
        assert!(!flags.has_test_only);
        assert!(!flags.has_schema_change);
    }

    // ── Internal helper tests ──

    #[test]
    fn test_node_id_from_index() {
        assert_eq!(node_id_from_index(0), "A");
        assert_eq!(node_id_from_index(25), "Z");
        assert_eq!(node_id_from_index(26), "AA");
        assert_eq!(node_id_from_index(27), "AB");
    }

    #[test]
    fn test_short_label() {
        assert_eq!(short_label("src/handlers/auth.ts"), "handlers/auth.ts");
        assert_eq!(short_label("auth.ts"), "auth.ts");
        assert_eq!(short_label("a/b/c/d.ts"), "c/d.ts");
    }
}
