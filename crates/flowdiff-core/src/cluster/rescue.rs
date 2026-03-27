//! Rescue non-infrastructure files from the infra bucket and coalesce test/impl pairs.

use std::collections::{BTreeMap, HashMap};

use crate::types::FlowGroup;

use super::classify::{classify_by_convention, is_config_like_filename, is_top_level_doc};
use super::stem::{is_test_file_name, test_impl_stem};
use crate::types::InfraCategory;

/// Separate truly infrastructure files from source files that just couldn't be reached
/// by the import graph. Returns (true_infra_files, rescued_files_with_group_assignment).
pub(super) fn rescue_non_infra_files(
    infra_files: &[String],
    groups: &[FlowGroup],
) -> (Vec<String>, Vec<(usize, String)>) {
    let mut true_infra = Vec::new();
    let mut rescued: Vec<(usize, String)> = Vec::new();

    for file in infra_files {
        let category = classify_by_convention(file);
        // Only rescue files that are Unclassified (source code) or DirectoryGroup
        // Everything else (Infrastructure, Schema, Migration, etc.) stays in infra
        if category != InfraCategory::Unclassified && category != InfraCategory::DirectoryGroup {
            true_infra.push(file.clone());
        } else if is_config_like_filename(file) {
            // Config-like filenames stay in infra even if classify_by_convention says Unclassified
            true_infra.push(file.clone());
        } else {
            // This looks like source code — assign to nearest group by directory
            match find_nearest_group_by_directory(file, groups) {
                Some(group_idx) => rescued.push((group_idx, file.clone())),
                None => {
                    // No directory match. Only use fallback (largest group) for files
                    // with clear source code extensions — not for config-like files.
                    let ext = file.rsplit('.').next().unwrap_or("");
                    let is_source = matches!(
                        ext,
                        "go" | "rs" | "ts" | "tsx" | "js" | "jsx" | "py" | "java" | "kt"
                            | "rb" | "php" | "cs" | "swift" | "scala" | "vue" | "svelte"
                            | "tmpl" | "html" | "css" | "scss" | "md" | "mdx" | "rst"
                    );
                    if is_source && !is_top_level_doc(file) {
                        if let Some(largest_idx) = groups
                            .iter()
                            .enumerate()
                            .max_by_key(|(_, g)| g.files.len())
                            .map(|(idx, _)| idx)
                        {
                            rescued.push((largest_idx, file.clone()));
                        } else {
                            true_infra.push(file.clone());
                        }
                    } else {
                        true_infra.push(file.clone());
                    }
                }
            }
        }
    }

    (true_infra, rescued)
}

/// Find the group that shares the longest directory prefix with the given file.
pub(super) fn find_nearest_group_by_directory(file: &str, groups: &[FlowGroup]) -> Option<usize> {
    let file_parts: Vec<&str> = file.split('/').collect();
    let mut best_match: Option<(usize, usize)> = None; // (group_idx, shared_depth)

    for (idx, group) in groups.iter().enumerate() {
        for group_file in &group.files {
            let group_parts: Vec<&str> = group_file.path.split('/').collect();
            let shared = file_parts
                .iter()
                .zip(group_parts.iter())
                .take_while(|(a, b)| a == b)
                .count();
            if shared > 0 {
                match best_match {
                    None => best_match = Some((idx, shared)),
                    Some((_, best_depth)) if shared > best_depth => {
                        best_match = Some((idx, shared));
                    }
                    _ => {}
                }
            }
        }
    }

    best_match.map(|(idx, _)| idx)
}

/// Move test files to the same group as their corresponding implementation files.
///
/// For each test file (matching *.spec.*, *.test.*, *_test.*), find the corresponding
/// implementation file (without the test suffix) in a different group and move the test
/// to that group. This ensures test+impl pairs always end up together regardless of
/// BFS assignment.
pub(super) fn coalesce_test_impl_pairs(
    group_map: &mut BTreeMap<usize, Vec<(String, usize)>>,
    assignments: &BTreeMap<String, (usize, usize)>,
) {
    // Build lookups: stem → (file_path, group_idx)
    // Use both full context stem and bare filename stem for flexible matching
    let mut impl_lookup: HashMap<String, (String, usize)> = HashMap::new();
    let mut impl_bare_lookup: HashMap<String, (String, usize)> = HashMap::new();
    for (file, (ep_idx, _)) in assignments {
        if !is_test_file_name(file) {
            let stem = test_impl_stem(file);
            impl_lookup.insert(stem, (file.clone(), *ep_idx));
            // Also index by bare filename stem (no directory context)
            let bare = file
                .rsplit('/')
                .next()
                .unwrap_or(file)
                .rsplit_once('.')
                .map(|(s, _)| s)
                .unwrap_or(file)
                .to_string();
            impl_bare_lookup.insert(bare, (file.clone(), *ep_idx));
        }
    }

    // Find test files whose impl is in a different group
    let mut moves: Vec<(String, usize, usize)> = Vec::new(); // (file, from_group, to_group)
    for (file, (ep_idx, _)) in assignments.iter() {
        if is_test_file_name(file) {
            let stem = test_impl_stem(file);
            // Try full context match first, then bare stem match
            let impl_group = impl_lookup
                .get(&stem)
                .or_else(|| {
                    let bare = file
                        .rsplit('/')
                        .next()
                        .unwrap_or(file)
                        .rsplit_once('.')
                        .map(|(s, _)| s)
                        .unwrap_or(file)
                        .replace(".test", "")
                        .replace(".spec", "")
                        .replace("_test", "")
                        .replace("test_", "");
                    impl_bare_lookup.get(&bare)
                });
            if let Some((_, impl_grp)) = impl_group {
                if impl_grp != ep_idx {
                    moves.push((file.clone(), *ep_idx, *impl_grp));
                }
            }
        }
    }

    // Apply moves
    for (file, from_group, to_group) in moves {
        if let Some(from_files) = group_map.get_mut(&from_group) {
            if let Some(pos) = from_files.iter().position(|(f, _)| *f == file) {
                let (f, dist) = from_files.remove(pos);
                group_map.entry(to_group).or_default().push((f, dist));
            }
        }
    }
}
