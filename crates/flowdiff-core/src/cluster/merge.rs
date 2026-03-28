//! Group merging and consolidation logic.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::types::{FileChange, FlowGroup};

use super::stem::{bare_stem, is_test_file_name};
use super::{MAX_MERGE_BUCKET_SIZE, SMALL_GROUP_THRESHOLD};

/// Merge small groups that share a common directory prefix.
///
/// For each directory depth (from deepest to shallowest), groups whose files all
/// share that directory prefix and have ≤ SMALL_GROUP_THRESHOLD files get merged
/// into one group. The merged group takes the name of the first group.
pub(super) fn consolidate_small_groups(mut groups: Vec<FlowGroup>) -> Vec<FlowGroup> {
    // Try merging at progressively shallower depths: 6, 5, 4, 3, 2, 1, 0
    // depth=0 catches root-level files (no directory) that share no prefix at depth≥1
    for depth in (0..=6).rev() {
        groups = merge_at_depth(groups, depth);
    }
    groups
}

/// Get the directory prefix of a path at a given depth.
/// depth=0: "" (root level — all files with no directory match here)
/// depth=1: "cmd/", depth=2: "cmd/admin/", etc.
fn dir_prefix(path: &str, depth: usize) -> Option<String> {
    if depth == 0 {
        // All root-level files (no directory) share the empty prefix
        if !path.contains('/') {
            return Some(String::new());
        }
        return None; // Files with directories don't match at depth=0
    }
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() <= depth {
        return None;
    }
    Some(parts[..depth].join("/"))
}

/// Get the common directory prefix for all files in a group at a given depth.
fn group_dir_prefix(group: &FlowGroup, depth: usize) -> Option<String> {
    let mut common: Option<String> = None;
    for file in &group.files {
        match dir_prefix(&file.path, depth) {
            Some(prefix) => {
                if let Some(ref c) = common {
                    if *c != prefix {
                        return None; // Files don't share the same prefix
                    }
                } else {
                    common = Some(prefix);
                }
            }
            None => return None,
        }
    }
    common
}

/// Merge small groups sharing a directory prefix at a specific depth.
fn merge_at_depth(groups: Vec<FlowGroup>, depth: usize) -> Vec<FlowGroup> {
    let mut buckets: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut no_merge: Vec<usize> = Vec::new();

    for (idx, group) in groups.iter().enumerate() {
        if group.entrypoint.is_none() && group.files.len() <= SMALL_GROUP_THRESHOLD {
            if let Some(prefix) = group_dir_prefix(group, depth) {
                buckets.entry(prefix).or_default().push(idx);
            } else {
                no_merge.push(idx);
            }
        } else {
            no_merge.push(idx);
        }
    }

    let mut result: Vec<FlowGroup> = Vec::new();

    // Keep non-mergeable groups as-is
    for idx in &no_merge {
        result.push(groups[*idx].clone());
    }

    // Merge buckets — sub-bucket by next directory level to keep siblings together
    for (_prefix, indices) in &buckets {
        if indices.len() <= 1 {
            for idx in indices {
                result.push(groups[*idx].clone());
            }
        } else if indices.len() <= MAX_MERGE_BUCKET_SIZE {
            // Small enough to merge directly
            merge_group_indices(&groups, indices, &mut result);
        } else {
            // Large bucket: sub-divide by the next directory level (depth+1)
            // so files in the same immediate directory always stay together
            let mut sub_buckets: BTreeMap<String, Vec<usize>> = BTreeMap::new();
            for &idx in indices.iter() {
                // Use the entrypoint file's parent directory as sub-key
                let sub_key = if let Some(ref ep) = groups[idx].entrypoint {
                    ep.file
                        .rsplit_once('/')
                        .map(|(dir, _)| dir.to_string())
                        .unwrap_or_else(|| ep.file.clone())
                } else if let Some(first) = groups[idx].files.first() {
                    first
                        .path
                        .rsplit_once('/')
                        .map(|(dir, _)| dir.to_string())
                        .unwrap_or_else(|| first.path.clone())
                } else {
                    format!("_unknown_{}", idx)
                };
                sub_buckets.entry(sub_key).or_default().push(idx);
            }

            // Merge within each sub-bucket.
            // Files in the same immediate directory always merge together — no cap.
            // Only apply the bucket cap when splitting ACROSS different sub-directories.
            for (_sub_key, sub_indices) in &sub_buckets {
                merge_group_indices(&groups, sub_indices, &mut result);
            }
        }
    }

    // Re-number group IDs
    result.sort_by(|a, b| a.name.cmp(&b.name));
    for (i, group) in result.iter_mut().enumerate() {
        group.id = format!("group_{}", i + 1);
    }

    result
}

/// Merge a set of group indices into one group and push to result.
fn merge_group_indices(groups: &[FlowGroup], indices: &[usize], result: &mut Vec<FlowGroup>) {
    if indices.is_empty() {
        return;
    }
    if indices.len() == 1 {
        result.push(groups[indices[0]].clone());
        return;
    }

    let first = &groups[indices[0]];
    let mut merged_files: Vec<FileChange> = Vec::new();
    let mut merged_edges: Vec<crate::types::FlowEdge> = Vec::new();

    for &idx in indices {
        merged_files.extend(groups[idx].files.clone());
        merged_edges.extend(groups[idx].edges.clone());
    }

    merged_files.sort_by(|a, b| a.path.cmp(&b.path));
    for (i, f) in merged_files.iter_mut().enumerate() {
        f.flow_position = i as u32;
    }

    result.push(FlowGroup {
        id: first.id.clone(),
        name: first.name.clone(),
        entrypoint: first.entrypoint.clone(),
        files: merged_files,
        edges: merged_edges,
        risk_score: 0.0,
        review_order: 0,
    });
}

/// Merge groups that contain test+impl pairs sharing a bare filename stem.
/// If a test file in group A has a matching impl in group B (by bare stem),
/// merge the smaller group into the larger one.
pub(super) fn merge_groups_by_stem(mut groups: Vec<FlowGroup>) -> Vec<FlowGroup> {
    let mut merged = true;
    while merged {
        merged = false;
        // Build: bare_stem → list of (group_idx, is_test)
        let mut stem_locations: HashMap<String, Vec<(usize, bool)>> = HashMap::new();
        for (g_idx, group) in groups.iter().enumerate() {
            for fc in &group.files {
                let is_test = is_test_file_name(&fc.path);
                let stem = bare_stem(&fc.path);
                if !stem.is_empty() {
                    stem_locations
                        .entry(stem)
                        .or_default()
                        .push((g_idx, is_test));
                }
            }
        }

        // Find stems that appear in multiple groups with both test and impl
        let mut best_merge: Option<(usize, usize)> = None;
        for (_stem, locations) in &stem_locations {
            let group_indices: HashSet<usize> = locations.iter().map(|(g, _)| *g).collect();
            // Only merge if stem appears in exactly 2 groups.
            // Stems in 3+ groups are too common and cause cascade-merging.
            if group_indices.len() != 2 {
                continue;
            }
            let has_test = locations.iter().any(|(_, t)| *t);
            let has_impl = locations.iter().any(|(_, t)| !*t);
            if has_test && has_impl {
                // Find the test group and impl group
                let impl_group = locations.iter().find(|(_, t)| !*t).map(|(g, _)| *g);
                let test_group = locations.iter().find(|(_, t)| *t).map(|(g, _)| *g);
                if let (Some(ig), Some(tg)) = (impl_group, test_group) {
                    if ig != tg {
                        // Merge smaller into larger
                        let (keep, donor) = if groups[ig].files.len() >= groups[tg].files.len() {
                            (ig, tg)
                        } else {
                            (tg, ig)
                        };
                        best_merge = Some((keep, donor));
                        break;
                    }
                }
            }
        }

        // Also try merging small groups that share a full directory path.
        // E.g., modules/httplib/serve.go and modules/httplib/content_disposition.go
        // should be in the same group even without matching stems.
        if best_merge.is_none() {
            let mut dir_locations: HashMap<String, Vec<usize>> = HashMap::new();
            for (g_idx, group) in groups.iter().enumerate() {
                if group.entrypoint.is_some() {
                    continue;
                }
                if group.files.len() > SMALL_GROUP_THRESHOLD {
                    continue;
                }
                for fc in &group.files {
                    if let Some(dir) = fc.path.rsplit_once('/').map(|(d, _)| d.to_string()) {
                        dir_locations.entry(dir).or_default().push(g_idx);
                    }
                }
            }
            for (_dir, indices) in &dir_locations {
                let unique: HashSet<&usize> = indices.iter().collect();
                if unique.len() >= 2 {
                    let mut sorted: Vec<usize> = unique.into_iter().copied().collect();
                    sorted.sort();
                    best_merge = Some((sorted[0], sorted[1]));
                    break;
                }
            }
        }

        if let Some((keep_idx, donor_idx)) = best_merge {
            let donor = groups.remove(donor_idx);
            let keep_idx = if donor_idx < keep_idx {
                keep_idx - 1
            } else {
                keep_idx
            };
            let receiver = &mut groups[keep_idx];
            for fc in donor.files {
                let pos = receiver.files.len() as u32;
                receiver.files.push(FileChange {
                    flow_position: pos,
                    ..fc
                });
            }
            receiver.edges.extend(donor.edges);
            merged = true;
        }
    }
    groups
}
