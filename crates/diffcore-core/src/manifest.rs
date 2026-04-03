//! Groups manifest — an editable JSON format for flow groupings.
//!
//! The manifest is a simplified representation of the analysis groupings
//! that can be exported, edited (by humans or AI agents), and re-imported.
//! This enables an iterative refinement loop:
//!
//! 1. `diffcore analyze` → produces initial groupings
//! 2. `diffcore export-groups -o groups.json` → editable manifest
//! 3. Agent/human edits `groups.json` (rename, move files, merge/split groups)
//! 4. `diffcore import-groups -i groups.json` → applies custom groupings
//! 5. Desktop app watches `groups.json` for live updates

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::types::{AnalysisOutput, FileChange, FlowEdge, FlowGroup, InfrastructureGroup};

/// A groups manifest — the editable format for flow groupings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GroupsManifest {
    /// Schema version for forward compatibility.
    pub version: String,
    /// The flow groups with editable names and file assignments.
    pub groups: Vec<ManifestGroup>,
    /// Files not assigned to any group.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unassigned_files: Vec<String>,
}

/// A single group in the manifest — simplified for editing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManifestGroup {
    /// Group name (editable — rename to describe the change).
    pub name: String,
    /// Files in this group (editable — move between groups).
    pub files: Vec<String>,
    /// Review order (editable — lower = review first).
    pub review_order: u32,
    /// Optional description for AI agents to understand intent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl GroupsManifest {
    /// Current schema version.
    pub const VERSION: &'static str = "1.0.0";
}

/// Export an `AnalysisOutput` to a `GroupsManifest`.
pub fn export_manifest(analysis: &AnalysisOutput) -> GroupsManifest {
    let groups = analysis
        .groups
        .iter()
        .map(|g| ManifestGroup {
            name: g.name.clone(),
            files: g.files.iter().map(|f| f.path.clone()).collect(),
            review_order: g.review_order,
            description: None,
        })
        .collect();

    let unassigned_files = analysis
        .infrastructure_group
        .as_ref()
        .map(|ig| ig.files.clone())
        .unwrap_or_default();

    GroupsManifest {
        version: GroupsManifest::VERSION.to_string(),
        groups,
        unassigned_files,
    }
}

/// Import a `GroupsManifest` into an existing `AnalysisOutput`, replacing its groups.
///
/// Preserves file metadata (role, changes, symbols_changed) from the original analysis.
/// Files not found in the original analysis get default metadata.
/// Edges are preserved only within groups (cross-group edges are dropped).
pub fn import_manifest(
    analysis: &AnalysisOutput,
    manifest: &GroupsManifest,
) -> AnalysisOutput {
    // Build a lookup: file path → (FileChange, group edges)
    let mut file_lookup: std::collections::HashMap<&str, &FileChange> =
        std::collections::HashMap::new();
    let mut edge_lookup: Vec<(&str, &FlowEdge)> = Vec::new();

    for group in &analysis.groups {
        for file in &group.files {
            file_lookup.insert(&file.path, file);
        }
        for edge in &group.edges {
            // Index edges by their "from" file
            let from_file = edge.from.split("::").next().unwrap_or(&edge.from);
            edge_lookup.push((from_file, edge));
        }
    }

    // Build new groups from manifest
    let mut new_groups: Vec<FlowGroup> = Vec::new();
    let mut all_assigned: std::collections::HashSet<String> = std::collections::HashSet::new();

    for (idx, mg) in manifest.groups.iter().enumerate() {
        let files: Vec<FileChange> = mg
            .files
            .iter()
            .map(|path| {
                all_assigned.insert(path.clone());
                file_lookup
                    .get(path.as_str())
                    .map(|fc| (*fc).clone())
                    .unwrap_or_else(|| FileChange {
                        path: path.clone(),
                        flow_position: 0,
                        role: crate::types::FileRole::Utility,
                        changes: crate::types::ChangeStats {
                            additions: 0,
                            deletions: 0,
                        },
                        symbols_changed: vec![],
                    })
            })
            .collect();

        // Collect edges where both from and to files are in this group
        let group_file_set: std::collections::HashSet<&str> =
            mg.files.iter().map(|s| s.as_str()).collect();
        let edges: Vec<FlowEdge> = edge_lookup
            .iter()
            .filter(|(from_file, edge)| {
                let to_file = edge.to.split("::").next().unwrap_or(&edge.to);
                group_file_set.contains(from_file) && group_file_set.contains(to_file)
            })
            .map(|(_, edge)| (*edge).clone())
            .collect();

        // Find entrypoint from original groups if any file matches
        let entrypoint = analysis
            .groups
            .iter()
            .flat_map(|g| g.entrypoint.as_ref())
            .find(|ep| group_file_set.contains(ep.file.as_str()))
            .cloned();

        let risk_score = if files.is_empty() {
            0.0
        } else {
            // Preserve average risk from original groups for files in this group
            let total: f64 = files
                .iter()
                .filter_map(|f| {
                    analysis
                        .groups
                        .iter()
                        .find(|g| g.files.iter().any(|gf| gf.path == f.path))
                        .map(|g| g.risk_score)
                })
                .sum();
            total / files.len() as f64
        };

        new_groups.push(FlowGroup {
            id: format!("group_{}", idx + 1),
            name: mg.name.clone(),
            entrypoint,
            files,
            edges,
            risk_score,
            review_order: mg.review_order,
        });
    }

    // Build infrastructure group from unassigned files
    let infra_files: Vec<String> = manifest.unassigned_files.clone();
    let infrastructure_group = if infra_files.is_empty() {
        None
    } else {
        Some(InfrastructureGroup {
            files: infra_files,
            sub_groups: vec![],
            reason: "Listed as unassigned in groups manifest".to_string(),
        })
    };

    AnalysisOutput {
        version: analysis.version.clone(),
        diff_source: analysis.diff_source.clone(),
        summary: crate::types::AnalysisSummary {
            total_files_changed: analysis.summary.total_files_changed,
            total_groups: new_groups.len() as u32,
            languages_detected: analysis.summary.languages_detected.clone(),
            frameworks_detected: analysis.summary.frameworks_detected.clone(),
        },
        groups: new_groups,
        infrastructure_group,
        annotations: analysis.annotations.clone(),
    }
}

/// Read a manifest from a JSON file.
pub fn read_manifest(path: &Path) -> Result<GroupsManifest, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read manifest: {}", e))?;
    serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse manifest JSON: {}", e))
}

/// Write a manifest to a JSON file.
pub fn write_manifest(path: &Path, manifest: &GroupsManifest) -> Result<(), String> {
    let json = serde_json::to_string_pretty(manifest)
        .map_err(|e| format!("Failed to serialize manifest: {}", e))?;
    std::fs::write(path, json)
        .map_err(|e| format!("Failed to write manifest: {}", e))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
mod tests {
    use super::*;
    use crate::types::*;

    fn make_analysis() -> AnalysisOutput {
        AnalysisOutput {
            version: "1.0.0".to_string(),
            diff_source: DiffSource {
                diff_type: DiffType::BranchComparison,
                base: Some("main".to_string()),
                head: Some("feature".to_string()),
                base_sha: Some("abc".to_string()),
                head_sha: Some("def".to_string()),
            },
            summary: AnalysisSummary {
                total_files_changed: 4,
                total_groups: 2,
                languages_detected: vec!["typescript".to_string()],
                frameworks_detected: vec![],
            },
            groups: vec![
                FlowGroup {
                    id: "group_1".to_string(),
                    name: "auth flow".to_string(),
                    entrypoint: Some(Entrypoint {
                        file: "src/auth.ts".to_string(),
                        symbol: "login".to_string(),
                        entrypoint_type: EntrypointType::HttpRoute,
                    }),
                    files: vec![
                        FileChange {
                            path: "src/auth.ts".to_string(),
                            flow_position: 0,
                            role: FileRole::Entrypoint,
                            changes: ChangeStats { additions: 10, deletions: 2 },
                            symbols_changed: vec!["login".to_string()],
                        },
                        FileChange {
                            path: "src/users.ts".to_string(),
                            flow_position: 1,
                            role: FileRole::Service,
                            changes: ChangeStats { additions: 5, deletions: 0 },
                            symbols_changed: vec!["getUser".to_string()],
                        },
                    ],
                    edges: vec![FlowEdge {
                        from: "src/auth.ts::login".to_string(),
                        to: "src/users.ts::getUser".to_string(),
                        edge_type: EdgeType::Calls,
                    }],
                    risk_score: 0.65,
                    review_order: 1,
                },
                FlowGroup {
                    id: "group_2".to_string(),
                    name: "config update".to_string(),
                    entrypoint: None,
                    files: vec![FileChange {
                        path: "src/config.ts".to_string(),
                        flow_position: 0,
                        role: FileRole::Config,
                        changes: ChangeStats { additions: 3, deletions: 1 },
                        symbols_changed: vec![],
                    }],
                    edges: vec![],
                    risk_score: 0.2,
                    review_order: 2,
                },
            ],
            infrastructure_group: Some(InfrastructureGroup {
                files: vec!["package.json".to_string()],
                sub_groups: vec![],
                reason: "No entrypoint reachability".to_string(),
            }),
            annotations: None,
        }
    }

    #[test]
    fn export_produces_correct_structure() {
        let analysis = make_analysis();
        let manifest = export_manifest(&analysis);

        assert_eq!(manifest.version, "1.0.0");
        assert_eq!(manifest.groups.len(), 2);
        assert_eq!(manifest.groups[0].name, "auth flow");
        assert_eq!(manifest.groups[0].files, vec!["src/auth.ts", "src/users.ts"]);
        assert_eq!(manifest.groups[0].review_order, 1);
        assert_eq!(manifest.groups[1].name, "config update");
        assert_eq!(manifest.groups[1].files, vec!["src/config.ts"]);
        assert_eq!(manifest.unassigned_files, vec!["package.json"]);
    }

    #[test]
    fn import_roundtrip_preserves_groups() {
        let analysis = make_analysis();
        let manifest = export_manifest(&analysis);
        let reimported = import_manifest(&analysis, &manifest);

        assert_eq!(reimported.groups.len(), 2);
        assert_eq!(reimported.groups[0].name, "auth flow");
        assert_eq!(reimported.groups[0].files.len(), 2);
        assert_eq!(reimported.groups[0].files[0].path, "src/auth.ts");
        assert_eq!(reimported.groups[0].files[0].role, FileRole::Entrypoint);
        assert_eq!(reimported.groups[0].review_order, 1);
    }

    #[test]
    fn import_preserves_file_metadata() {
        let analysis = make_analysis();
        let manifest = export_manifest(&analysis);
        let reimported = import_manifest(&analysis, &manifest);

        let auth_file = &reimported.groups[0].files[0];
        assert_eq!(auth_file.changes.additions, 10);
        assert_eq!(auth_file.changes.deletions, 2);
        assert_eq!(auth_file.symbols_changed, vec!["login"]);
    }

    #[test]
    fn import_preserves_edges_within_group() {
        let analysis = make_analysis();
        let manifest = export_manifest(&analysis);
        let reimported = import_manifest(&analysis, &manifest);

        assert_eq!(reimported.groups[0].edges.len(), 1);
        assert_eq!(reimported.groups[0].edges[0].from, "src/auth.ts::login");
    }

    #[test]
    fn import_drops_cross_group_edges() {
        let mut analysis = make_analysis();
        // Add a cross-group edge
        analysis.groups[0].edges.push(FlowEdge {
            from: "src/auth.ts::login".to_string(),
            to: "src/config.ts::getConfig".to_string(),
            edge_type: EdgeType::Calls,
        });

        let manifest = export_manifest(&analysis);
        let reimported = import_manifest(&analysis, &manifest);

        // Only the intra-group edge should survive
        assert_eq!(reimported.groups[0].edges.len(), 1);
    }

    #[test]
    fn import_with_renamed_group() {
        let analysis = make_analysis();
        let mut manifest = export_manifest(&analysis);
        manifest.groups[0].name = "authentication & login".to_string();

        let reimported = import_manifest(&analysis, &manifest);
        assert_eq!(reimported.groups[0].name, "authentication & login");
    }

    #[test]
    fn import_with_moved_file() {
        let analysis = make_analysis();
        let mut manifest = export_manifest(&analysis);
        // Move src/users.ts from group 0 to group 1
        manifest.groups[0].files.retain(|f| f != "src/users.ts");
        manifest.groups[1].files.push("src/users.ts".to_string());

        let reimported = import_manifest(&analysis, &manifest);
        assert_eq!(reimported.groups[0].files.len(), 1);
        assert_eq!(reimported.groups[1].files.len(), 2);
        assert!(reimported.groups[1].files.iter().any(|f| f.path == "src/users.ts"));
    }

    #[test]
    fn import_with_merged_groups() {
        let analysis = make_analysis();
        let manifest = GroupsManifest {
            version: "1.0.0".to_string(),
            groups: vec![ManifestGroup {
                name: "all changes".to_string(),
                files: vec![
                    "src/auth.ts".to_string(),
                    "src/users.ts".to_string(),
                    "src/config.ts".to_string(),
                ],
                review_order: 1,
                description: Some("Merged into one group".to_string()),
            }],
            unassigned_files: vec!["package.json".to_string()],
        };

        let reimported = import_manifest(&analysis, &manifest);
        assert_eq!(reimported.groups.len(), 1);
        assert_eq!(reimported.groups[0].files.len(), 3);
        assert_eq!(reimported.summary.total_groups, 1);
    }

    #[test]
    fn import_with_split_group() {
        let analysis = make_analysis();
        let manifest = GroupsManifest {
            version: "1.0.0".to_string(),
            groups: vec![
                ManifestGroup {
                    name: "auth entry".to_string(),
                    files: vec!["src/auth.ts".to_string()],
                    review_order: 1,
                    description: None,
                },
                ManifestGroup {
                    name: "user service".to_string(),
                    files: vec!["src/users.ts".to_string()],
                    review_order: 2,
                    description: None,
                },
                ManifestGroup {
                    name: "config".to_string(),
                    files: vec!["src/config.ts".to_string()],
                    review_order: 3,
                    description: None,
                },
            ],
            unassigned_files: vec!["package.json".to_string()],
        };

        let reimported = import_manifest(&analysis, &manifest);
        assert_eq!(reimported.groups.len(), 3);
        assert_eq!(reimported.summary.total_groups, 3);
    }

    #[test]
    fn import_preserves_entrypoint() {
        let analysis = make_analysis();
        let manifest = export_manifest(&analysis);
        let reimported = import_manifest(&analysis, &manifest);

        assert!(reimported.groups[0].entrypoint.is_some());
        assert_eq!(reimported.groups[0].entrypoint.as_ref().unwrap().symbol, "login");
    }

    #[test]
    fn manifest_json_roundtrip() {
        let analysis = make_analysis();
        let manifest = export_manifest(&analysis);

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("groups.json");
        write_manifest(&path, &manifest).unwrap();
        let loaded = read_manifest(&path).unwrap();

        assert_eq!(manifest, loaded);
    }

    #[test]
    fn import_unknown_file_gets_default_metadata() {
        let analysis = make_analysis();
        let manifest = GroupsManifest {
            version: "1.0.0".to_string(),
            groups: vec![ManifestGroup {
                name: "new group".to_string(),
                files: vec!["src/brand_new.ts".to_string()],
                review_order: 1,
                description: None,
            }],
            unassigned_files: vec![],
        };

        let reimported = import_manifest(&analysis, &manifest);
        assert_eq!(reimported.groups[0].files[0].path, "src/brand_new.ts");
        assert_eq!(reimported.groups[0].files[0].role, FileRole::Utility);
    }

    #[test]
    fn import_preserves_infrastructure_from_manifest() {
        let analysis = make_analysis();
        let manifest = GroupsManifest {
            version: "1.0.0".to_string(),
            groups: vec![ManifestGroup {
                name: "main".to_string(),
                files: vec!["src/auth.ts".to_string()],
                review_order: 1,
                description: None,
            }],
            unassigned_files: vec!["package.json".to_string(), "Dockerfile".to_string()],
        };

        let reimported = import_manifest(&analysis, &manifest);
        assert!(reimported.infrastructure_group.is_some());
        let infra = reimported.infrastructure_group.unwrap();
        assert_eq!(infra.files.len(), 2);
        assert!(infra.files.contains(&"Dockerfile".to_string()));
    }
}
