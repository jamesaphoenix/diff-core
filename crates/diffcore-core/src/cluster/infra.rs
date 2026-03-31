//! Infrastructure sub-clustering: organizes infra files into semantic sub-groups.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::graph::SymbolGraph;
use crate::types::{InfraCategory, InfraSubGroup};

use super::classify::{category_display_name, classify_by_convention};

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
        if files[1..].iter().all(|f| {
            f.get(..=i)
                .map_or(false, |s| s.ends_with(c) && s == &first[..=i])
        }) {
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
