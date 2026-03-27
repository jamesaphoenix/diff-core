//! Semantic clustering: groups changed files into flow groups.
//!
//! Algorithm (from spec §4.6):
//! 1. For each entrypoint, compute forward reachability via BFS on the symbol graph
//! 2. Intersect each reachability set with the changed file set ΔF
//! 3. Files reachable from the same entrypoint and in ΔF belong to the same flow group
//! 4. Files in ΔF not reachable from any entrypoint form an "infrastructure/shared" group
//! 5. Files reachable from multiple entrypoints get assigned to the group with shortest path

mod bfs;
mod classify;
mod embeddings_refine;
mod infra;
mod merge;
mod rescue;
mod stem;

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
mod tests;

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::graph::SymbolGraph;
use crate::types::{
    ChangeStats, Entrypoint, FileChange, FileRole, FlowGroup, InfraCategory, InfrastructureGroup,
};

// Re-export public API
pub(crate) use classify::category_display_name;
pub use classify::classify_by_convention;
#[cfg(feature = "embeddings")]
pub use embeddings_refine::refine_with_embeddings;
pub use infra::sub_cluster_infra_files;

// Internal imports from submodules
use bfs::{collect_internal_edges, compute_file_reachability, generate_group_name};
use classify::{infer_file_role, is_config_like_filename, is_top_level_doc};
use merge::{consolidate_small_groups, merge_groups_by_stem};
use rescue::{coalesce_test_impl_pairs, rescue_non_infra_files};
use stem::{is_test_file_name, test_impl_stem};

/// Maximum number of files for a group to be considered "small" and eligible for merging.
pub(crate) const SMALL_GROUP_THRESHOLD: usize = 5;

/// Maximum number of small groups that can merge in a single directory bucket.
/// Prevents collapsing 15+ singletons into one mega-group.
const MAX_MERGE_BUCKET_SIZE: usize = 12;

/// Very large diffs need a coarse partition step before the normal semantic grouping.
/// This is intentionally gated so it does not perturb the normal-sized eval corpus.
const LARGE_DIFF_PARTITION_THRESHOLD: usize = 2000;

pub(super) fn has_semantic_source_extension(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();

    matches!(
        ext.as_str(),
        "go" | "rs"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "java"
            | "kt"
            | "rb"
            | "php"
            | "cs"
            | "swift"
            | "scala"
            | "vue"
            | "svelte"
            | "tmpl"
            | "html"
            | "css"
            | "scss"
            | "md"
            | "mdx"
            | "rst"
            | "c"
            | "h"
            | "cc"
            | "cpp"
            | "cxx"
            | "hh"
            | "hpp"
    )
}

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
    cluster_files_internal(graph, entrypoints, changed_files, true)
}

fn cluster_files_internal(
    graph: &SymbolGraph,
    entrypoints: &[Entrypoint],
    changed_files: &[String],
    allow_large_diff_partitioning: bool,
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

    if allow_large_diff_partitioning && changed_set.len() >= LARGE_DIFF_PARTITION_THRESHOLD {
        return cluster_large_diff_files(graph, entrypoints, &changed_set);
    }

    if entrypoints.is_empty() {
        // No entrypoints detected — but don't dump everything to infra.
        // Classify files directly: source files go to directory groups, infra stays.
        let mut true_infra = Vec::new();
        let mut source_files = Vec::new();
        for file in &changed_set {
            let category = classify_by_convention(file);
            if category == InfraCategory::Infrastructure {
                // Hard infrastructure (CI, Docker, package managers, .changeset/) — always infra
                true_infra.push(file.clone());
            } else if category != InfraCategory::Unclassified
                && category != InfraCategory::DirectoryGroup
            {
                // Soft infrastructure (docs, schemas, migrations, etc.) — rescue if source extension
                if !is_config_like_filename(file) {
                    if has_semantic_source_extension(file) && !is_top_level_doc(file) {
                        source_files.push(file.clone());
                    } else {
                        true_infra.push(file.clone());
                    }
                } else {
                    true_infra.push(file.clone());
                }
            } else if is_config_like_filename(file) {
                true_infra.push(file.clone());
            } else {
                source_files.push(file.clone());
            }
        }
        let _rescued: Vec<(usize, String)> = Vec::new(); // unused but needed for type compatibility

        if source_files.is_empty() {
            // All files are truly infrastructure
            return ClusterResult {
                groups: vec![],
                infrastructure: if true_infra.is_empty() {
                    None
                } else {
                    Some(InfrastructureGroup {
                        files: true_infra,
                        sub_groups: vec![],
                        reason: "Not reachable from any detected entrypoint".to_string(),
                    })
                },
            };
        }

        // Create groups from source files by directory clustering
        let mut dir_groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for file in &source_files {
            let dir = file
                .rsplit_once('/')
                .map(|(d, _)| d.to_string())
                .unwrap_or_default();
            dir_groups.entry(dir).or_default().push(file.clone());
        }

        let mut groups: Vec<FlowGroup> = dir_groups
            .into_iter()
            .enumerate()
            .map(|(idx, (dir, files))| {
                let name = if dir.is_empty() {
                    "root".to_string()
                } else {
                    dir.rsplit('/').next().unwrap_or(&dir).to_string()
                };
                FlowGroup {
                    id: format!("group_{}", idx + 1),
                    name: format!("{} (directory)", name),
                    entrypoint: None,
                    files: files
                        .iter()
                        .enumerate()
                        .map(|(pos, path)| FileChange {
                            path: path.clone(),
                            flow_position: pos as u32,
                            role: infer_file_role(path),
                            changes: ChangeStats {
                                additions: 0,
                                deletions: 0,
                            },
                            symbols_changed: vec![],
                        })
                        .collect(),
                    edges: vec![],
                    risk_score: 0.0,
                    review_order: 0,
                }
            })
            .collect();

        // Consolidate the directory groups
        groups = consolidate_small_groups(groups);

        let infrastructure = if true_infra.is_empty() {
            None
        } else {
            let sub_groups = sub_cluster_infra_files(&true_infra, graph);
            Some(InfrastructureGroup {
                files: true_infra,
                sub_groups,
                reason: "Not reachable from any detected entrypoint".to_string(),
            })
        };

        return ClusterResult {
            groups,
            infrastructure,
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

    // Step 3.5: Coalesce test files with their implementations.
    // If a test file (*.spec.*, *.test.*, *_test.*) is in a different group
    // than its corresponding implementation, move the test to the impl's group.
    coalesce_test_impl_pairs(&mut group_map, &assignments);

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

    // Step 4.5: Rescue non-infrastructure files from the infra bucket.
    // Files unreachable from any entrypoint go to infra by default. But many of these
    // are source/test files that the import graph couldn't connect — not true infrastructure.
    // Assign non-infra-looking files to the nearest group by shared directory prefix.
    let (true_infra, rescued) = rescue_non_infra_files(&infra_files, &groups);
    let mut infra_files = true_infra;
    // Add rescued files to their assigned groups
    for (group_idx, file_path) in rescued {
        if let Some(group) = groups.get_mut(group_idx) {
            let pos = group.files.len() as u32;
            group.files.push(FileChange {
                path: file_path,
                flow_position: pos,
                role: infer_file_role(""),
                changes: ChangeStats {
                    additions: 0,
                    deletions: 0,
                },
                symbols_changed: vec![],
            });
        }
    }

    // Step 5: Consolidate small groups by directory.
    // Merge singleton/small groups (≤ SMALL_GROUP_THRESHOLD files) that share a common
    // directory prefix. This reduces singleton explosion where each entrypoint in the
    // same directory creates its own tiny group.
    groups = consolidate_small_groups(groups);

    // Step 5.5: Coalesce test files from infra into groups.
    // If a test file is in infra (unreachable from entrypoints) but its implementation
    // is in a semantic group, move the test to that group.
    let mut infra_rescued: Vec<(usize, String)> = Vec::new();
    {
        // Build stem->group_idx lookup from all group files
        let mut impl_by_stem: HashMap<String, usize> = HashMap::new();
        for (g_idx, group) in groups.iter().enumerate() {
            for fc in &group.files {
                if !is_test_file_name(&fc.path) {
                    let stem = test_impl_stem(&fc.path);
                    impl_by_stem.insert(stem, g_idx);
                }
            }
        }
        for file in &infra_files {
            if is_test_file_name(file) {
                // Don't rescue test files that are in classified infra categories
                // (migrations, generated, scripts, etc.) — only rescue Unclassified tests
                let cat = classify_by_convention(file);
                if cat != InfraCategory::Unclassified && cat != InfraCategory::DirectoryGroup {
                    continue;
                }
                let stem = test_impl_stem(file);
                if let Some(&g_idx) = impl_by_stem.get(&stem) {
                    infra_rescued.push((g_idx, file.clone()));
                }
            }
        }
    }
    for (g_idx, file_path) in &infra_rescued {
        infra_files.retain(|f| f != file_path);
        if let Some(group) = groups.get_mut(*g_idx) {
            let pos = group.files.len() as u32;
            group.files.push(FileChange {
                path: file_path.clone(),
                flow_position: pos,
                role: FileRole::Test,
                changes: ChangeStats {
                    additions: 0,
                    deletions: 0,
                },
                symbols_changed: vec![],
            });
        }
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

    // Step 6: Merge groups that share test+impl pairs by bare stem.
    // After all prior grouping, test files may end up in different groups than their
    // implementations (e.g., packages/X/src/Foo.ts in group A, packages/X/test/Foo.test.ts
    // in group B). Merge the smaller group into the larger one.
    groups = merge_groups_by_stem(groups);

    ClusterResult {
        groups,
        infrastructure,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LargeDiffPartition {
    files: Vec<String>,
    is_infra: bool,
}

fn cluster_large_diff_files(
    graph: &SymbolGraph,
    entrypoints: &[Entrypoint],
    changed_files: &[String],
) -> ClusterResult {
    let partitions = partition_large_diff_files(changed_files);
    let mut groups = Vec::new();
    let mut infra_files = Vec::new();

    for partition in partitions.into_values() {
        if partition.is_infra {
            infra_files.extend(partition.files);
            continue;
        }

        let partition_paths: HashSet<&str> = partition.files.iter().map(|s| s.as_str()).collect();
        let partition_entrypoints: Vec<Entrypoint> = entrypoints
            .iter()
            .filter(|ep| partition_paths.contains(ep.file.as_str()))
            .cloned()
            .collect();

        let result = cluster_files_internal(graph, &partition_entrypoints, &partition.files, false);
        groups.extend(result.groups);
        if let Some(infra) = result.infrastructure {
            infra_files.extend(infra.files);
        }
    }

    infra_files.sort();
    infra_files.dedup();

    groups.sort_by(|a, b| {
        a.name.cmp(&b.name).then_with(|| {
            a.files
                .first()
                .map(|f| f.path.as_str())
                .cmp(&b.files.first().map(|f| f.path.as_str()))
        })
    });
    for (idx, group) in groups.iter_mut().enumerate() {
        group.id = format!("group_{}", idx + 1);
    }

    let infrastructure = if infra_files.is_empty() {
        None
    } else {
        let sub_groups = sub_cluster_infra_files(&infra_files, graph);
        Some(InfrastructureGroup {
            files: infra_files,
            sub_groups,
            reason: format!(
                "Large diff partitioned into coarse buckets before semantic grouping ({}+ files)",
                LARGE_DIFF_PARTITION_THRESHOLD
            ),
        })
    };

    ClusterResult {
        groups,
        infrastructure,
    }
}

fn partition_large_diff_files(changed_files: &[String]) -> BTreeMap<String, LargeDiffPartition> {
    let mut partitions: BTreeMap<String, LargeDiffPartition> = BTreeMap::new();

    for file in changed_files {
        let (is_infra, key) = large_diff_partition_key(file);
        let bucket_key = format!("{}:{}", if is_infra { "infra" } else { "semantic" }, key);
        partitions
            .entry(bucket_key)
            .or_insert_with(|| LargeDiffPartition {
                files: Vec::new(),
                is_infra,
            })
            .files
            .push(file.clone());
    }

    for partition in partitions.values_mut() {
        partition.files.sort();
        partition.files.dedup();
    }

    partitions
}

fn large_diff_partition_key(file: &str) -> (bool, String) {
    if is_config_like_filename(file) {
        return (true, "config-like".to_string());
    }

    let category = classify_by_convention(file);
    if category != InfraCategory::Unclassified && category != InfraCategory::DirectoryGroup {
        return (true, large_diff_infra_partition_name(category));
    }

    let mut parts = file.split('/');
    let first = parts.next().unwrap_or_default();
    let second = parts.next();
    let third = parts.next();

    if first.is_empty() {
        return (false, "root".to_string());
    }

    if matches!(
        first,
        "apps" | "packages" | "services" | "workers" | "libs" | "modules" | "crates"
    ) {
        if let (Some(second), Some(third)) = (second, third) {
            if !second.contains('.') && !third.contains('.') && is_container_segment(second) {
                return (false, format!("{}/{}/{}", first, second, third));
            }
        }

        if let Some(second) = second.filter(|segment| !segment.contains('.')) {
            return (false, format!("{}/{}", first, second));
        }
    }

    (false, first.to_string())
}

fn large_diff_infra_partition_name(category: InfraCategory) -> String {
    match category {
        InfraCategory::Infrastructure => "infrastructure",
        InfraCategory::Schema => "schema",
        InfraCategory::Script => "script",
        InfraCategory::Migration => "migration",
        InfraCategory::Deployment => "deployment",
        InfraCategory::Documentation => "documentation",
        InfraCategory::Lint => "lint",
        InfraCategory::TestUtil => "test-util",
        InfraCategory::Generated => "generated",
        InfraCategory::DirectoryGroup => "directory-group",
        InfraCategory::Unclassified => "unclassified",
    }
    .to_string()
}

fn is_container_segment(segment: &str) -> bool {
    matches!(
        segment,
        "services"
            | "workers"
            | "libs"
            | "modules"
            | "packages"
            | "features"
            | "domains"
            | "core"
            | "shared"
            | "ui"
    )
}
