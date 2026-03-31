//! Rescue non-infrastructure files from the infra bucket and coalesce test/impl pairs.

use std::collections::{BTreeMap, HashMap};

use crate::types::FlowGroup;

use super::classify::{classify_by_convention, is_config_like_filename, is_top_level_doc};
use super::stem::{bare_stem, is_test_file_name, test_impl_stem};
use super::{
    is_semantic_fixture_project_config_candidate, is_semantic_fixture_source_candidate,
    semantic_fixture_root,
};
use crate::types::InfraCategory;

/// Separate truly infrastructure files from source files that just couldn't be reached
/// by the import graph. Returns (true_infra_files, rescued_files_with_group_assignment).
pub(super) fn rescue_non_infra_files(
    infra_files: &[String],
    groups: &[FlowGroup],
) -> (Vec<String>, Vec<(usize, String)>) {
    let mut true_infra = Vec::new();
    let mut rescued: Vec<(usize, String)> = Vec::new();
    let mut fixture_group_assignments: HashMap<String, usize> = HashMap::new();
    let root_docs_batch_count = infra_files
        .iter()
        .filter(|file| is_root_nonlocalized_docs_page(file))
        .count();

    for file in infra_files {
        if !is_semantic_fixture_source_candidate(file) {
            continue;
        }

        let target_group = semantic_fixture_root(file)
            .and_then(|root| fixture_group_assignments.get(root).copied())
            .or_else(|| find_group_by_fixture_root(file, groups))
            .or_else(|| find_nearest_group_by_directory(file, groups))
            .or_else(|| {
                groups
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, g)| g.files.len())
                    .map(|(idx, _)| idx)
            });

        if let Some(group_idx) = target_group {
            rescued.push((group_idx, file.clone()));
            if let Some(root) = semantic_fixture_root(file) {
                fixture_group_assignments.insert(root.to_string(), group_idx);
            }
        } else {
            true_infra.push(file.clone());
        }
    }

    for file in infra_files {
        if is_semantic_fixture_source_candidate(file) {
            continue;
        }

        if is_semantic_fixture_project_config_candidate(file) {
            if let Some(group_idx) = semantic_fixture_root(file)
                .and_then(|root| fixture_group_assignments.get(root).copied())
                .or_else(|| find_group_by_fixture_root(file, groups))
            {
                rescued.push((group_idx, file.clone()));
                continue;
            }
        }

        let category = classify_by_convention(file);
        if category == InfraCategory::Documentation {
            if let Some(group_idx) = find_group_by_topic_affinity(file, groups) {
                rescued.push((group_idx, file.clone()));
            } else if !is_semantic_doc_candidate(file, root_docs_batch_count) {
                true_infra.push(file.clone());
            } else if is_semantic_doc_fallback_candidate(file, root_docs_batch_count) {
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
            continue;
        }

        if matches!(category, InfraCategory::Schema | InfraCategory::Generated) {
            if let Some(group_idx) = find_group_by_topic_affinity(file, groups) {
                rescued.push((group_idx, file.clone()));
            } else {
                true_infra.push(file.clone());
            }
            continue;
        }

        // Only rescue files that are Unclassified (source code) or DirectoryGroup
        // Everything else (Infrastructure, Schema, Migration, etc.) stays in infra
        if category != InfraCategory::Unclassified && category != InfraCategory::DirectoryGroup {
            true_infra.push(file.clone());
        } else if is_config_like_filename(file) {
            // Config-like filenames stay in infra even if classify_by_convention says Unclassified
            true_infra.push(file.clone());
        } else {
            // This looks like source code — assign to nearest group by directory
            match find_nearest_group_by_directory(file, groups)
                .or_else(|| find_group_by_topic_affinity(file, groups))
            {
                Some(group_idx) => rescued.push((group_idx, file.clone())),
                None => {
                    if is_native_source_or_header(file) {
                        if let Some(group_idx) = find_group_by_exact_directory(file, groups) {
                            rescued.push((group_idx, file.clone()));
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

fn find_group_by_fixture_root(file: &str, groups: &[FlowGroup]) -> Option<usize> {
    let root = semantic_fixture_root(file)?;
    let mut best: Option<(usize, usize)> = None;
    let mut tied = false;

    for (idx, group) in groups.iter().enumerate() {
        let same_root_source_count = group
            .files
            .iter()
            .filter(|group_file| {
                semantic_fixture_root(&group_file.path) == Some(root)
                    && is_semantic_fixture_source_candidate(&group_file.path)
            })
            .count();

        if same_root_source_count == 0 {
            continue;
        }

        match best {
            None => {
                best = Some((idx, same_root_source_count));
                tied = false;
            }
            Some((_, best_count)) if same_root_source_count > best_count => {
                best = Some((idx, same_root_source_count));
                tied = false;
            }
            Some((_, best_count)) if same_root_source_count == best_count => {
                tied = true;
            }
            _ => {}
        }
    }

    match best {
        Some((idx, _)) if !tied => Some(idx),
        _ => None,
    }
}

fn is_native_source_or_header(path: &str) -> bool {
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "c" | "h" | "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" | "h++"
    )
}

fn find_group_by_exact_directory(file: &str, groups: &[FlowGroup]) -> Option<usize> {
    let file_parent = parent_directory(file)?;
    let mut best: Option<(usize, usize)> = None;
    let mut tied = false;

    for (idx, group) in groups.iter().enumerate() {
        let same_dir_count = group
            .files
            .iter()
            .filter(|group_file| parent_directory(&group_file.path) == Some(file_parent))
            .count();

        if same_dir_count == 0 {
            continue;
        }

        match best {
            None => {
                best = Some((idx, same_dir_count));
                tied = false;
            }
            Some((_, best_count)) if same_dir_count > best_count => {
                best = Some((idx, same_dir_count));
                tied = false;
            }
            Some((_, best_count)) if same_dir_count == best_count => {
                tied = true;
            }
            _ => {}
        }
    }

    match best {
        Some((idx, _)) if !tied => Some(idx),
        _ => None,
    }
}

fn parent_directory(path: &str) -> Option<&str> {
    path.rsplit_once('/').map(|(dir, _)| dir)
}

fn is_semantic_doc_candidate(path: &str, root_docs_batch_count: usize) -> bool {
    let lower = path.to_lowercase();
    let ext = lower.rsplit('.').next().unwrap_or("");
    if !matches!(ext, "md" | "mdx" | "rst" | "txt") || is_top_level_doc(path) {
        return false;
    }

    if is_localized_docs_tree(&lower) {
        return false;
    }

    lower.contains("/src/docs/")
        || lower.contains("/resources/mdtest/")
        || lower.starts_with("docs_src/")
        || lower.contains("/docs_src/")
        || lower.starts_with("docs/tutorial/")
        || lower.starts_with("docs/advanced/")
        || lower.starts_with("docs/how-to/")
        || lower.starts_with("docs/guide/")
        || lower.starts_with("docs/guides/")
        || lower.starts_with("docs/reference/")
        || is_semantic_doc_fallback_candidate(path, root_docs_batch_count)
}

fn is_semantic_doc_fallback_candidate(path: &str, root_docs_batch_count: usize) -> bool {
    let lower = path.to_lowercase();
    lower.starts_with("documentation/docs/")
        || lower.contains("/documentation/docs/")
        || lower.starts_with("www/docs/")
        || lower.contains("/www/docs/")
        || lower.starts_with("content/docs/")
        || lower.contains("/content/docs/")
        || lower.starts_with("site/content/")
        || lower.contains("/site/content/")
        || lower.starts_with("docs/tutorial/")
        || lower.starts_with("docs/advanced/")
        || (is_root_nonlocalized_docs_page(path) && root_docs_batch_count >= 3)
}

fn is_root_nonlocalized_docs_page(path: &str) -> bool {
    let lower = path.to_lowercase();
    if is_top_level_doc(path)
        || is_localized_docs_tree(&lower)
        || !lower.starts_with("docs/")
        || lower.contains("/docs/")
    {
        return false;
    }

    let Some(rest) = lower.strip_prefix("docs/") else {
        return false;
    };
    if rest.contains('/') {
        return false;
    }

    let ext = lower.rsplit('.').next().unwrap_or("");
    matches!(ext, "md" | "mdx" | "rst" | "txt")
}

fn is_localized_docs_tree(lower: &str) -> bool {
    let Some(rest) = lower.strip_prefix("docs/") else {
        return false;
    };
    let Some((locale, remainder)) = rest.split_once('/') else {
        return false;
    };

    remainder.starts_with("docs/")
        && (2..=8).contains(&locale.len())
        && locale
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch == '-')
}

fn find_group_by_topic_affinity(file: &str, groups: &[FlowGroup]) -> Option<usize> {
    let doc_tokens = path_topic_tokens(file);
    if doc_tokens.is_empty() {
        return None;
    }

    let mut best: Option<(usize, usize)> = None;
    let mut tied = false;

    for (idx, group) in groups.iter().enumerate() {
        let mut score = 0;
        for group_file in &group.files {
            let overlap = path_topic_tokens(&group_file.path)
                .into_iter()
                .filter(|token| doc_tokens.contains(token))
                .count();
            score = score.max(overlap);
        }

        if score == 0 {
            continue;
        }

        match best {
            None => {
                best = Some((idx, score));
                tied = false;
            }
            Some((_, best_score)) if score > best_score => {
                best = Some((idx, score));
                tied = false;
            }
            Some((_, best_score)) if score == best_score => {
                tied = true;
            }
            _ => {}
        }
    }

    match best {
        Some((idx, score)) if score >= 2 || (score == 1 && !tied) => Some(idx),
        _ => None,
    }
}

fn path_topic_tokens(path: &str) -> Vec<String> {
    let lower = path.to_lowercase();
    let mut tokens = Vec::new();

    let stem = bare_stem(path);
    if !stem.is_empty() {
        push_topic_pieces(&mut tokens, &stem);
    }

    if let Some(parent) = lower.rsplit_once('/').map(|(dir, _)| dir) {
        for segment in parent.rsplit('/').take(3) {
            push_topic_pieces(&mut tokens, segment);
        }
    }

    tokens
}

fn push_topic_pieces(tokens: &mut Vec<String>, segment: &str) {
    let normalized: String = segment
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect();

    for raw in normalized.split_whitespace() {
        let token = raw.trim_start_matches(|ch: char| ch.is_ascii_digit());
        if token.len() < 3 || is_generic_topic_token(token) || tokens.iter().any(|t| t == token) {
            continue;
        }
        tokens.push(token.to_string());
    }
}

fn is_generic_topic_token(token: &str) -> bool {
    matches!(
        token,
        "docs"
            | "doc"
            | "documentation"
            | "content"
            | "tutorial"
            | "tutorials"
            | "guide"
            | "guides"
            | "reference"
            | "references"
            | "advanced"
            | "example"
            | "examples"
            | "sample"
            | "samples"
            | "site"
            | "www"
            | "src"
            | "test"
            | "tests"
            | "resources"
            | "mdtest"
            | "how"
            | "with"
            | "and"
    )
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

    best_match.and_then(|(idx, shared_depth)| (shared_depth >= 2).then_some(idx))
}

#[cfg(test)]
mod tests {
    use crate::types::{ChangeStats, FileChange};

    use super::*;

    fn group_with_file(path: &str) -> FlowGroup {
        FlowGroup {
            id: "group_1".to_string(),
            name: "test".to_string(),
            entrypoint: None,
            files: vec![FileChange {
                path: path.to_string(),
                flow_position: 0,
                role: crate::types::FileRole::Infrastructure,
                changes: ChangeStats {
                    additions: 0,
                    deletions: 0,
                },
                symbols_changed: vec![],
            }],
            edges: vec![],
            risk_score: 0.0,
            review_order: 0,
        }
    }

    #[test]
    fn semantic_doc_candidates_exclude_localized_trees() {
        assert!(is_semantic_doc_candidate(
            "documentation/docs/02-runes/03-$derived.md",
            0
        ));
        assert!(!is_semantic_doc_candidate(
            "docs/zh-hant/docs/tutorial/first-steps.md",
            0
        ));
    }

    #[test]
    fn semantic_doc_affinity_matches_topic_directory() {
        let groups = vec![group_with_file(
            "docs_src/tutorial/where/tutorial006b_py310.py",
        )];

        assert_eq!(
            find_group_by_topic_affinity("docs/tutorial/where.md", &groups),
            Some(0)
        );
    }

    #[test]
    fn root_docs_pages_need_batch_context() {
        assert!(!is_semantic_doc_candidate("docs/quickstart.md", 1));
        assert!(is_semantic_doc_candidate("docs/quickstart.md", 3));
    }

    #[test]
    fn nested_docs_trees_are_not_treated_as_root_docs_pages() {
        assert!(!is_root_nonlocalized_docs_page("docs/user/advanced.rst"));
        assert!(!is_root_nonlocalized_docs_page("docs/features/caching.md"));
        assert!(is_root_nonlocalized_docs_page("docs/quickstart.md"));
    }

    #[test]
    fn tutorial_and_advanced_docs_keep_semantic_fallback() {
        assert!(is_semantic_doc_fallback_candidate(
            "docs/tutorial/where.md",
            0
        ));
        assert!(is_semantic_doc_fallback_candidate(
            "docs/advanced/ssl.md",
            0
        ));
        assert!(!is_semantic_doc_fallback_candidate(
            "docs/user/advanced.rst",
            3
        ));
    }

    #[test]
    fn rescue_non_infra_files_treats_c_and_headers_as_source_like() {
        let groups = vec![group_with_file("src/module.c")];
        let infra_files = vec!["src/cluster.c".to_string(), "src/stream.h".to_string()];

        let (true_infra, rescued) = rescue_non_infra_files(&infra_files, &groups);

        assert!(true_infra.is_empty());
        assert_eq!(
            rescued,
            vec![
                (0, "src/cluster.c".to_string()),
                (0, "src/stream.h".to_string())
            ]
        );
    }

    #[test]
    fn topic_affinity_rescues_source_adjacent_text_artifacts() {
        let groups = vec![group_with_file("src/blib2to3/pgen2/parse.py")];
        let infra_files = vec!["src/blib2to3/Grammar.txt".to_string()];

        let (true_infra, rescued) = rescue_non_infra_files(&infra_files, &groups);

        assert!(true_infra.is_empty());
        assert_eq!(rescued, vec![(0, "src/blib2to3/Grammar.txt".to_string())]);
    }

    #[test]
    fn topic_affinity_rescues_schema_and_generated_artifacts() {
        let groups = vec![group_with_file("internal/tfplugin6/plugin.go")];
        let infra_files = vec![
            "docs/plugin-protocol/tfplugin6.proto".to_string(),
            "internal/tfplugin6/tfplugin6.pb.go".to_string(),
        ];

        let (true_infra, rescued) = rescue_non_infra_files(&infra_files, &groups);

        assert!(true_infra.is_empty());
        assert_eq!(
            rescued,
            vec![
                (0, "docs/plugin-protocol/tfplugin6.proto".to_string()),
                (0, "internal/tfplugin6/tfplugin6.pb.go".to_string())
            ]
        );
    }
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
            let impl_group = impl_lookup.get(&stem).or_else(|| {
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
