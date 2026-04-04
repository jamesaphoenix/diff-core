//! LLM refinement pass — applies structural improvements to deterministic flow groups.
//!
//! Takes groups v1 (from static analysis) and a `RefinementResponse` (from an LLM),
//! applies split/merge/re-rank/reclassify operations, and produces groups v2.
//!
//! See spec §4 "LLM refinement pass" for design details.

use std::collections::{HashMap, HashSet};

use crate::cluster::{category_display_name, classify_by_convention};
use crate::types::{
    ChangeStats, FileChange, FileRole, FlowGroup, InfraSubGroup, InfrastructureGroup,
};

use super::schema::{RefinementGroupInput, RefinementRequest, RefinementResponse, RefinementSplit};

/// Errors that can occur during refinement application.
#[derive(Debug, thiserror::Error)]
pub enum RefinementError {
    #[error("Split references unknown group: {0}")]
    UnknownSplitSource(String),

    #[error("Merge references unknown group: {0}")]
    UnknownMergeGroup(String),

    #[error("Re-rank references unknown group: {0}")]
    UnknownReRankGroup(String),

    #[error("Reclassify references unknown source group: {0}")]
    UnknownReclassifySource(String),

    #[error("Reclassify references unknown target group: {0}")]
    UnknownReclassifyTarget(String),

    #[error("Split for group '{0}' references file '{1}' not in the original group")]
    SplitFileNotInGroup(String, String),

    #[error("Split for group '{0}' does not account for all files (missing: {1:?})")]
    SplitMissingFiles(String, Vec<String>),

    #[error("Reclassify file '{0}' not found in source group '{1}'")]
    ReclassifyFileNotFound(String, String),
}

/// Validate a refinement response against the current groups.
///
/// Returns Ok(()) if all operations reference valid groups and files.
/// Returns Err with the first validation error found.
pub fn validate_refinement(
    response: &RefinementResponse,
    groups: &[FlowGroup],
    infrastructure: Option<&InfrastructureGroup>,
) -> Result<(), RefinementError> {
    let group_ids: HashSet<&str> = groups.iter().map(|g| g.id.as_str()).collect();

    // Validate splits
    for split in &response.splits {
        if !group_ids.contains(split.source_group_id.as_str()) {
            return Err(RefinementError::UnknownSplitSource(
                split.source_group_id.clone(),
            ));
        }
        // group_ids.contains() above guarantees this find succeeds
        let source_group = match groups.iter().find(|g| g.id == split.source_group_id) {
            Some(g) => g,
            None => {
                return Err(RefinementError::UnknownSplitSource(
                    split.source_group_id.clone(),
                ));
            }
        };
        let source_files: HashSet<&str> =
            source_group.files.iter().map(|f| f.path.as_str()).collect();

        let mut accounted: HashSet<&str> = HashSet::new();
        for new_group in &split.new_groups {
            for file in &new_group.files {
                if !source_files.contains(file.as_str()) {
                    return Err(RefinementError::SplitFileNotInGroup(
                        split.source_group_id.clone(),
                        file.clone(),
                    ));
                }
                accounted.insert(file.as_str());
            }
        }
        let missing: Vec<String> = source_files
            .difference(&accounted)
            .map(|s| s.to_string())
            .collect();
        if !missing.is_empty() {
            return Err(RefinementError::SplitMissingFiles(
                split.source_group_id.clone(),
                missing,
            ));
        }
    }

    // Validate merges
    for merge in &response.merges {
        for gid in &merge.group_ids {
            if !group_ids.contains(gid.as_str()) {
                return Err(RefinementError::UnknownMergeGroup(gid.clone()));
            }
        }
    }

    // Validate re-ranks
    for re_rank in &response.re_ranks {
        if !group_ids.contains(re_rank.group_id.as_str()) {
            return Err(RefinementError::UnknownReRankGroup(
                re_rank.group_id.clone(),
            ));
        }
    }

    // Validate reclassifications
    for reclass in &response.reclassifications {
        // Source must be a valid group or infrastructure
        let source_valid = group_ids.contains(reclass.from_group_id.as_str())
            || reclass.from_group_id == "infrastructure";
        if !source_valid {
            return Err(RefinementError::UnknownReclassifySource(
                reclass.from_group_id.clone(),
            ));
        }

        // Target must be a valid group or infrastructure
        let target_valid = group_ids.contains(reclass.to_group_id.as_str())
            || reclass.to_group_id == "infrastructure";
        if !target_valid {
            return Err(RefinementError::UnknownReclassifyTarget(
                reclass.to_group_id.clone(),
            ));
        }

        // File must exist in source
        if reclass.from_group_id == "infrastructure" {
            if let Some(infra) = infrastructure {
                if !infra.files.contains(&reclass.file) {
                    return Err(RefinementError::ReclassifyFileNotFound(
                        reclass.file.clone(),
                        reclass.from_group_id.clone(),
                    ));
                }
            } else {
                return Err(RefinementError::ReclassifyFileNotFound(
                    reclass.file.clone(),
                    "infrastructure".to_string(),
                ));
            }
        } else if let Some(group) = groups.iter().find(|g| g.id == reclass.from_group_id) {
            if !group.files.iter().any(|f| f.path == reclass.file) {
                return Err(RefinementError::ReclassifyFileNotFound(
                    reclass.file.clone(),
                    reclass.from_group_id.clone(),
                ));
            }
        }
    }

    Ok(())
}

/// Apply refinement operations to produce groups v2.
///
/// Operations are applied in order: reclassifications → splits → merges → re-ranks.
/// This ordering ensures that file moves happen before structural changes, and
/// re-ranking happens last on the final group set.
///
/// Returns the refined groups. Falls back to the original groups on validation error.
pub fn apply_refinement(
    groups: &[FlowGroup],
    infrastructure: Option<&InfrastructureGroup>,
    response: &RefinementResponse,
) -> Result<(Vec<FlowGroup>, Option<InfrastructureGroup>), RefinementError> {
    // Validate first
    validate_refinement(response, groups, infrastructure)?;

    let mut refined_groups: Vec<FlowGroup> = groups.to_vec();
    let mut infra = infrastructure.cloned();

    // 1. Apply reclassifications (move files between groups)
    for reclass in &response.reclassifications {
        let file_change = remove_file_from_group_or_infra(
            &mut refined_groups,
            &mut infra,
            &reclass.from_group_id,
            &reclass.file,
        );

        if let Some(fc) = file_change {
            add_file_to_group_or_infra(&mut refined_groups, &mut infra, &reclass.to_group_id, fc);
        }
    }

    // Remove empty groups after reclassifications
    refined_groups.retain(|g| !g.files.is_empty());

    // 2. Apply splits
    let split_group_ids: HashSet<&str> = response
        .splits
        .iter()
        .map(|s| s.source_group_id.as_str())
        .collect();
    let mut new_groups_from_splits: Vec<FlowGroup> = Vec::new();

    for split in &response.splits {
        if let Some(source) = refined_groups
            .iter()
            .find(|g| g.id == split.source_group_id)
        {
            let split_groups = apply_split(source, split, new_groups_from_splits.len());
            new_groups_from_splits.extend(split_groups);
        }
    }

    // Remove split source groups and add new groups
    refined_groups.retain(|g| !split_group_ids.contains(g.id.as_str()));
    refined_groups.extend(new_groups_from_splits);

    // 3. Apply merges
    for merge in &response.merges {
        let merge_ids: HashSet<&str> = merge.group_ids.iter().map(|s| s.as_str()).collect();
        let mut merged_files: Vec<FileChange> = Vec::new();
        let mut merged_edges = Vec::new();
        let mut first_entrypoint = None;

        for group in refined_groups
            .iter()
            .filter(|g| merge_ids.contains(g.id.as_str()))
        {
            merged_files.extend(group.files.clone());
            merged_edges.extend(group.edges.clone());
            if first_entrypoint.is_none() {
                first_entrypoint = group.entrypoint.clone();
            }
        }

        // Reassign flow positions
        for (i, fc) in merged_files.iter_mut().enumerate() {
            fc.flow_position = i as u32;
        }

        let merged = FlowGroup {
            id: merge
                .group_ids
                .first()
                .cloned()
                .unwrap_or_else(|| "merged".to_string()),
            name: merge.merged_name.clone(),
            entrypoint: first_entrypoint,
            files: merged_files,
            edges: merged_edges,
            risk_score: 0.0, // Will be re-scored
            review_order: 0,
        };

        refined_groups.retain(|g| !merge_ids.contains(g.id.as_str()));
        refined_groups.push(merged);
    }

    // 4. Apply re-ranks
    for re_rank in &response.re_ranks {
        if let Some(group) = refined_groups.iter_mut().find(|g| g.id == re_rank.group_id) {
            group.review_order = re_rank.new_position;
        }
    }

    // Sort by review_order for consistency
    refined_groups.sort_by(|a, b| a.review_order.cmp(&b.review_order).then(a.id.cmp(&b.id)));

    // Renumber group IDs for cleanliness
    for (i, group) in refined_groups.iter_mut().enumerate() {
        group.review_order = (i + 1) as u32;
    }

    Ok((refined_groups, infra))
}

/// Build a `RefinementRequest` from analysis output.
pub fn build_refinement_request(
    groups: &[FlowGroup],
    infrastructure: Option<&InfrastructureGroup>,
    analysis_json: &str,
    diff_summary: &str,
) -> RefinementRequest {
    let group_inputs: Vec<RefinementGroupInput> = groups
        .iter()
        .map(|g| RefinementGroupInput {
            id: g.id.clone(),
            name: g.name.clone(),
            entrypoint: g
                .entrypoint
                .as_ref()
                .map(|ep| format!("{}::{}", ep.file, ep.symbol)),
            files: g.files.iter().map(|f| f.path.clone()).collect(),
            risk_score: g.risk_score,
            review_order: g.review_order,
        })
        .collect();

    let infrastructure_files = infrastructure
        .map(|ig| ig.files.clone())
        .unwrap_or_default();

    RefinementRequest {
        analysis_json: analysis_json.to_string(),
        diff_summary: diff_summary.to_string(),
        groups: group_inputs,
        infrastructure_files,
    }
}

/// Check if a refinement response contains any operations.
pub fn has_refinements(response: &RefinementResponse) -> bool {
    !response.splits.is_empty()
        || !response.merges.is_empty()
        || !response.re_ranks.is_empty()
        || !response.reclassifications.is_empty()
}

// ── Internal helpers ──

fn remove_file_from_group_or_infra(
    groups: &mut [FlowGroup],
    infra: &mut Option<InfrastructureGroup>,
    group_id: &str,
    file_path: &str,
) -> Option<FileChange> {
    if group_id == "infrastructure" {
        if let Some(ref mut ig) = infra {
            ig.files.retain(|f| f != file_path);
            // Also remove from sub-groups
            for sg in ig.sub_groups.iter_mut() {
                sg.files.retain(|f| f != file_path);
            }
            // Remove empty sub-groups
            ig.sub_groups.retain(|sg| !sg.files.is_empty());
        }
        // Create a synthetic FileChange for infrastructure files
        return Some(FileChange {
            path: file_path.to_string(),
            flow_position: 0,
            role: FileRole::Infrastructure,
            changes: ChangeStats {
                additions: 0,
                deletions: 0,
            },
            symbols_changed: vec![],
        });
    }

    for group in groups.iter_mut() {
        if group.id == group_id {
            if let Some(pos) = group.files.iter().position(|f| f.path == file_path) {
                return Some(group.files.remove(pos));
            }
        }
    }
    None
}

fn add_file_to_group_or_infra(
    groups: &mut [FlowGroup],
    infra: &mut Option<InfrastructureGroup>,
    group_id: &str,
    file_change: FileChange,
) {
    if group_id == "infrastructure" {
        let path = file_change.path;
        let category = classify_by_convention(&path);
        let display_name = category_display_name(&category);

        match infra {
            Some(ig) => {
                ig.files.push(path.clone());
                ig.files.sort();
                ig.files.dedup();
                // Add to matching sub-group or create one
                if let Some(sg) = ig.sub_groups.iter_mut().find(|sg| sg.category == category) {
                    sg.files.push(path);
                    sg.files.sort();
                    sg.files.dedup();
                } else {
                    ig.sub_groups.push(InfraSubGroup {
                        name: display_name,
                        category,
                        files: vec![path],
                    });
                    ig.sub_groups.sort_by(|a, b| a.name.cmp(&b.name));
                }
            }
            None => {
                *infra = Some(InfrastructureGroup {
                    files: vec![path.clone()],
                    sub_groups: vec![InfraSubGroup {
                        name: display_name,
                        category,
                        files: vec![path],
                    }],
                    reason: "Moved to infrastructure by LLM refinement".to_string(),
                });
            }
        }
        return;
    }

    for group in groups.iter_mut() {
        if group.id == group_id {
            let pos = group.files.len() as u32;
            let mut fc = file_change;
            fc.flow_position = pos;
            group.files.push(fc);
            return;
        }
    }
}

fn apply_split(source: &FlowGroup, split: &RefinementSplit, offset: usize) -> Vec<FlowGroup> {
    let file_map: HashMap<&str, &FileChange> =
        source.files.iter().map(|f| (f.path.as_str(), f)).collect();

    split
        .new_groups
        .iter()
        .enumerate()
        .map(|(i, new_group)| {
            let files: Vec<FileChange> = new_group
                .files
                .iter()
                .enumerate()
                .map(|(pos, path)| {
                    if let Some(original) = file_map.get(path.as_str()) {
                        let mut fc = (*original).clone();
                        fc.flow_position = pos as u32;
                        fc
                    } else {
                        FileChange {
                            path: path.clone(),
                            flow_position: pos as u32,
                            role: FileRole::Infrastructure,
                            changes: ChangeStats {
                                additions: 0,
                                deletions: 0,
                            },
                            symbols_changed: vec![],
                        }
                    }
                })
                .collect();

            // First sub-group inherits the entrypoint if it contains the entrypoint file
            let entrypoint = source.entrypoint.as_ref().and_then(|ep| {
                if new_group.files.iter().any(|f| *f == ep.file) {
                    Some(ep.clone())
                } else {
                    None
                }
            });

            FlowGroup {
                id: format!("group_refined_{}", offset + i + 1),
                name: new_group.name.clone(),
                entrypoint,
                files,
                edges: vec![], // Edges would need to be recomputed from the graph
                risk_score: source.risk_score, // Inherit risk score; will be re-scored
                review_order: 0,
            }
        })
        .collect()
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
    use crate::llm::schema::{
        RefinementMerge, RefinementNewGroup, RefinementReRank, RefinementReclassify,
        RefinementSplit,
    };
    use crate::types::{EdgeType, Entrypoint, EntrypointType, FlowEdge, InfraCategory};

    fn make_file(path: &str, pos: u32) -> FileChange {
        FileChange {
            path: path.to_string(),
            flow_position: pos,
            role: FileRole::Infrastructure,
            changes: ChangeStats {
                additions: 10,
                deletions: 5,
            },
            symbols_changed: vec![],
        }
    }

    fn make_group(id: &str, name: &str, files: Vec<FileChange>) -> FlowGroup {
        FlowGroup {
            id: id.to_string(),
            name: name.to_string(),
            entrypoint: None,
            files,
            edges: vec![],
            risk_score: 0.5,
            review_order: 0,
        }
    }

    fn empty_refinement() -> RefinementResponse {
        RefinementResponse {
            splits: vec![],
            merges: vec![],
            re_ranks: vec![],
            reclassifications: vec![],
            reasoning: "No refinements needed".to_string(),
        }
    }

    // ── Validation Tests ──

    #[test]
    fn test_validate_empty_refinement() {
        let groups = vec![make_group("g1", "Group 1", vec![make_file("a.ts", 0)])];
        let result = validate_refinement(&empty_refinement(), &groups, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_split_unknown_group() {
        let groups = vec![make_group("g1", "Group 1", vec![make_file("a.ts", 0)])];
        let response = RefinementResponse {
            splits: vec![RefinementSplit {
                source_group_id: "nonexistent".to_string(),
                new_groups: vec![],
                reason: "test".to_string(),
            }],
            ..empty_refinement()
        };
        let result = validate_refinement(&response, &groups, None);
        assert!(matches!(
            result,
            Err(RefinementError::UnknownSplitSource(_))
        ));
    }

    #[test]
    fn test_validate_split_file_not_in_group() {
        let groups = vec![make_group("g1", "Group 1", vec![make_file("a.ts", 0)])];
        let response = RefinementResponse {
            splits: vec![RefinementSplit {
                source_group_id: "g1".to_string(),
                new_groups: vec![RefinementNewGroup {
                    name: "Sub".to_string(),
                    files: vec!["a.ts".to_string(), "nonexistent.ts".to_string()],
                }],
                reason: "test".to_string(),
            }],
            ..empty_refinement()
        };
        let result = validate_refinement(&response, &groups, None);
        assert!(matches!(
            result,
            Err(RefinementError::SplitFileNotInGroup(_, _))
        ));
    }

    #[test]
    fn test_validate_split_missing_files() {
        let groups = vec![make_group(
            "g1",
            "Group 1",
            vec![make_file("a.ts", 0), make_file("b.ts", 1)],
        )];
        let response = RefinementResponse {
            splits: vec![RefinementSplit {
                source_group_id: "g1".to_string(),
                new_groups: vec![RefinementNewGroup {
                    name: "Sub".to_string(),
                    files: vec!["a.ts".to_string()], // Missing b.ts
                }],
                reason: "test".to_string(),
            }],
            ..empty_refinement()
        };
        let result = validate_refinement(&response, &groups, None);
        assert!(matches!(
            result,
            Err(RefinementError::SplitMissingFiles(_, _))
        ));
    }

    #[test]
    fn test_validate_merge_unknown_group() {
        let groups = vec![make_group("g1", "Group 1", vec![make_file("a.ts", 0)])];
        let response = RefinementResponse {
            merges: vec![RefinementMerge {
                group_ids: vec!["g1".to_string(), "nonexistent".to_string()],
                merged_name: "Merged".to_string(),
                reason: "test".to_string(),
            }],
            ..empty_refinement()
        };
        let result = validate_refinement(&response, &groups, None);
        assert!(matches!(result, Err(RefinementError::UnknownMergeGroup(_))));
    }

    #[test]
    fn test_validate_rerank_unknown_group() {
        let groups = vec![make_group("g1", "Group 1", vec![make_file("a.ts", 0)])];
        let response = RefinementResponse {
            re_ranks: vec![RefinementReRank {
                group_id: "nonexistent".to_string(),
                new_position: 1,
                reason: "test".to_string(),
            }],
            ..empty_refinement()
        };
        let result = validate_refinement(&response, &groups, None);
        assert!(matches!(
            result,
            Err(RefinementError::UnknownReRankGroup(_))
        ));
    }

    #[test]
    fn test_validate_reclassify_unknown_source() {
        let groups = vec![make_group("g1", "Group 1", vec![make_file("a.ts", 0)])];
        let response = RefinementResponse {
            reclassifications: vec![RefinementReclassify {
                file: "a.ts".to_string(),
                from_group_id: "nonexistent".to_string(),
                to_group_id: "g1".to_string(),
                reason: "test".to_string(),
            }],
            ..empty_refinement()
        };
        let result = validate_refinement(&response, &groups, None);
        assert!(matches!(
            result,
            Err(RefinementError::UnknownReclassifySource(_))
        ));
    }

    #[test]
    fn test_validate_reclassify_unknown_target() {
        let groups = vec![make_group("g1", "Group 1", vec![make_file("a.ts", 0)])];
        let response = RefinementResponse {
            reclassifications: vec![RefinementReclassify {
                file: "a.ts".to_string(),
                from_group_id: "g1".to_string(),
                to_group_id: "nonexistent".to_string(),
                reason: "test".to_string(),
            }],
            ..empty_refinement()
        };
        let result = validate_refinement(&response, &groups, None);
        assert!(matches!(
            result,
            Err(RefinementError::UnknownReclassifyTarget(_))
        ));
    }

    #[test]
    fn test_validate_reclassify_file_not_found() {
        let groups = vec![make_group("g1", "Group 1", vec![make_file("a.ts", 0)])];
        let response = RefinementResponse {
            reclassifications: vec![RefinementReclassify {
                file: "nonexistent.ts".to_string(),
                from_group_id: "g1".to_string(),
                to_group_id: "infrastructure".to_string(),
                reason: "test".to_string(),
            }],
            ..empty_refinement()
        };
        let result = validate_refinement(&response, &groups, None);
        assert!(matches!(
            result,
            Err(RefinementError::ReclassifyFileNotFound(_, _))
        ));
    }

    #[test]
    fn test_validate_reclassify_from_infrastructure() {
        let groups = vec![make_group("g1", "Group 1", vec![make_file("a.ts", 0)])];
        let infra = InfrastructureGroup {
            files: vec!["config.ts".to_string()],
            sub_groups: vec![],
            reason: "test".to_string(),
        };
        let response = RefinementResponse {
            reclassifications: vec![RefinementReclassify {
                file: "config.ts".to_string(),
                from_group_id: "infrastructure".to_string(),
                to_group_id: "g1".to_string(),
                reason: "test".to_string(),
            }],
            ..empty_refinement()
        };
        let result = validate_refinement(&response, &groups, Some(&infra));
        assert!(result.is_ok());
    }

    // ── Apply Refinement Tests ──

    #[test]
    fn test_apply_empty_refinement() {
        let groups = vec![make_group(
            "g1",
            "Group 1",
            vec![make_file("a.ts", 0), make_file("b.ts", 1)],
        )];
        let (result, infra) = apply_refinement(&groups, None, &empty_refinement()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].files.len(), 2);
        assert!(infra.is_none());
    }

    #[test]
    fn test_apply_split() {
        let groups = vec![make_group(
            "g1",
            "Mixed group",
            vec![
                make_file("auth.ts", 0),
                make_file("db.ts", 1),
                make_file("config.ts", 2),
            ],
        )];
        let response = RefinementResponse {
            splits: vec![RefinementSplit {
                source_group_id: "g1".to_string(),
                new_groups: vec![
                    RefinementNewGroup {
                        name: "Auth flow".to_string(),
                        files: vec!["auth.ts".to_string()],
                    },
                    RefinementNewGroup {
                        name: "DB + Config".to_string(),
                        files: vec!["db.ts".to_string(), "config.ts".to_string()],
                    },
                ],
                reason: "Auth is unrelated to DB config".to_string(),
            }],
            ..empty_refinement()
        };

        let (result, _) = apply_refinement(&groups, None, &response).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].name, "Auth flow");
        assert_eq!(result[0].files.len(), 1);
        assert_eq!(result[0].files[0].path, "auth.ts");
        assert_eq!(result[1].name, "DB + Config");
        assert_eq!(result[1].files.len(), 2);
    }

    #[test]
    fn test_apply_merge() {
        let groups = vec![
            make_group("g1", "Part A", vec![make_file("a.ts", 0)]),
            make_group("g2", "Part B", vec![make_file("b.ts", 0)]),
            make_group("g3", "Unrelated", vec![make_file("c.ts", 0)]),
        ];
        let response = RefinementResponse {
            merges: vec![RefinementMerge {
                group_ids: vec!["g1".to_string(), "g2".to_string()],
                merged_name: "Combined refactor".to_string(),
                reason: "Same logical change".to_string(),
            }],
            ..empty_refinement()
        };

        let (result, _) = apply_refinement(&groups, None, &response).unwrap();
        assert_eq!(result.len(), 2); // Merged group + unrelated
        let merged = result
            .iter()
            .find(|g| g.name == "Combined refactor")
            .unwrap();
        assert_eq!(merged.files.len(), 2);
        let unrelated = result.iter().find(|g| g.name == "Unrelated").unwrap();
        assert_eq!(unrelated.files.len(), 1);
    }

    #[test]
    fn test_apply_rerank() {
        let mut groups = vec![
            make_group("g1", "Group 1", vec![make_file("a.ts", 0)]),
            make_group("g2", "Group 2", vec![make_file("b.ts", 0)]),
        ];
        groups[0].review_order = 1;
        groups[1].review_order = 2;

        let response = RefinementResponse {
            re_ranks: vec![
                RefinementReRank {
                    group_id: "g2".to_string(),
                    new_position: 1,
                    reason: "Review schema first".to_string(),
                },
                RefinementReRank {
                    group_id: "g1".to_string(),
                    new_position: 2,
                    reason: "Handler depends on schema".to_string(),
                },
            ],
            ..empty_refinement()
        };

        let (result, _) = apply_refinement(&groups, None, &response).unwrap();
        // After re-ranking and sorting, g2 should come first
        assert_eq!(result[0].id, "g2");
        assert_eq!(result[1].id, "g1");
    }

    #[test]
    fn test_apply_reclassify_to_infrastructure() {
        let groups = vec![make_group(
            "g1",
            "Group 1",
            vec![make_file("a.ts", 0), make_file("config.ts", 1)],
        )];
        let response = RefinementResponse {
            reclassifications: vec![RefinementReclassify {
                file: "config.ts".to_string(),
                from_group_id: "g1".to_string(),
                to_group_id: "infrastructure".to_string(),
                reason: "Config is shared infra".to_string(),
            }],
            ..empty_refinement()
        };

        let (result, infra) = apply_refinement(&groups, None, &response).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].files.len(), 1);
        assert_eq!(result[0].files[0].path, "a.ts");
        let infra = infra.unwrap();
        assert!(infra.files.contains(&"config.ts".to_string()));
    }

    #[test]
    fn test_apply_reclassify_from_infrastructure() {
        let groups = vec![make_group("g1", "Auth", vec![make_file("auth.ts", 0)])];
        let infra = InfrastructureGroup {
            files: vec!["token.ts".to_string()],
            sub_groups: vec![],
            reason: "test".to_string(),
        };
        let response = RefinementResponse {
            reclassifications: vec![RefinementReclassify {
                file: "token.ts".to_string(),
                from_group_id: "infrastructure".to_string(),
                to_group_id: "g1".to_string(),
                reason: "Token is part of auth".to_string(),
            }],
            ..empty_refinement()
        };

        let (result, infra_out) = apply_refinement(&groups, Some(&infra), &response).unwrap();
        assert_eq!(result[0].files.len(), 2);
        let file_paths: Vec<&str> = result[0].files.iter().map(|f| f.path.as_str()).collect();
        assert!(file_paths.contains(&"token.ts"));
        // Infrastructure should be empty after moving the file out
        if let Some(ig) = &infra_out {
            assert!(!ig.files.contains(&"token.ts".to_string()));
        }
    }

    #[test]
    fn test_apply_reclassify_between_groups() {
        let groups = vec![
            make_group(
                "g1",
                "Group 1",
                vec![make_file("a.ts", 0), make_file("shared.ts", 1)],
            ),
            make_group("g2", "Group 2", vec![make_file("b.ts", 0)]),
        ];
        let response = RefinementResponse {
            reclassifications: vec![RefinementReclassify {
                file: "shared.ts".to_string(),
                from_group_id: "g1".to_string(),
                to_group_id: "g2".to_string(),
                reason: "shared.ts is primarily used by g2".to_string(),
            }],
            ..empty_refinement()
        };

        let (result, _) = apply_refinement(&groups, None, &response).unwrap();
        let g1 = result.iter().find(|g| g.id == "g1").unwrap();
        let g2 = result.iter().find(|g| g.id == "g2").unwrap();
        assert_eq!(g1.files.len(), 1);
        assert_eq!(g1.files[0].path, "a.ts");
        assert_eq!(g2.files.len(), 2);
        assert!(g2.files.iter().any(|f| f.path == "shared.ts"));
    }

    #[test]
    fn test_apply_combined_operations() {
        let groups = vec![
            make_group(
                "g1",
                "Mixed",
                vec![make_file("auth.ts", 0), make_file("db.ts", 1)],
            ),
            make_group("g2", "Part A", vec![make_file("a.ts", 0)]),
            make_group("g3", "Part B", vec![make_file("b.ts", 0)]),
        ];
        let response = RefinementResponse {
            splits: vec![RefinementSplit {
                source_group_id: "g1".to_string(),
                new_groups: vec![
                    RefinementNewGroup {
                        name: "Auth".to_string(),
                        files: vec!["auth.ts".to_string()],
                    },
                    RefinementNewGroup {
                        name: "DB".to_string(),
                        files: vec!["db.ts".to_string()],
                    },
                ],
                reason: "Unrelated".to_string(),
            }],
            merges: vec![RefinementMerge {
                group_ids: vec!["g2".to_string(), "g3".to_string()],
                merged_name: "Combined".to_string(),
                reason: "Same change".to_string(),
            }],
            re_ranks: vec![],
            reclassifications: vec![],
            reasoning: "Split mixed group, merge related groups".to_string(),
        };

        let (result, _) = apply_refinement(&groups, None, &response).unwrap();
        // Should have 3 groups: Auth, DB, Combined
        assert_eq!(result.len(), 3);
        let names: Vec<&str> = result.iter().map(|g| g.name.as_str()).collect();
        assert!(names.contains(&"Auth"));
        assert!(names.contains(&"DB"));
        assert!(names.contains(&"Combined"));
    }

    #[test]
    fn test_apply_split_preserves_entrypoint() {
        let mut group = make_group(
            "g1",
            "API",
            vec![make_file("src/route.ts", 0), make_file("src/util.ts", 1)],
        );
        group.entrypoint = Some(Entrypoint {
            file: "src/route.ts".to_string(),
            symbol: "POST".to_string(),
            entrypoint_type: EntrypointType::HttpRoute,
        });

        let response = RefinementResponse {
            splits: vec![RefinementSplit {
                source_group_id: "g1".to_string(),
                new_groups: vec![
                    RefinementNewGroup {
                        name: "Route".to_string(),
                        files: vec!["src/route.ts".to_string()],
                    },
                    RefinementNewGroup {
                        name: "Util".to_string(),
                        files: vec!["src/util.ts".to_string()],
                    },
                ],
                reason: "test".to_string(),
            }],
            ..empty_refinement()
        };

        let (result, _) = apply_refinement(&[group], None, &response).unwrap();
        let route_group = result.iter().find(|g| g.name == "Route").unwrap();
        let util_group = result.iter().find(|g| g.name == "Util").unwrap();
        assert!(route_group.entrypoint.is_some());
        assert!(util_group.entrypoint.is_none());
    }

    #[test]
    fn test_has_refinements_empty() {
        assert!(!has_refinements(&empty_refinement()));
    }

    #[test]
    fn test_has_refinements_with_split() {
        let response = RefinementResponse {
            splits: vec![RefinementSplit {
                source_group_id: "g1".to_string(),
                new_groups: vec![],
                reason: "test".to_string(),
            }],
            ..empty_refinement()
        };
        assert!(has_refinements(&response));
    }

    #[test]
    fn test_has_refinements_with_merge() {
        let response = RefinementResponse {
            merges: vec![RefinementMerge {
                group_ids: vec!["g1".to_string()],
                merged_name: "test".to_string(),
                reason: "test".to_string(),
            }],
            ..empty_refinement()
        };
        assert!(has_refinements(&response));
    }

    #[test]
    fn test_build_refinement_request() {
        let groups = vec![FlowGroup {
            id: "g1".to_string(),
            name: "Auth flow".to_string(),
            entrypoint: Some(Entrypoint {
                file: "src/auth.ts".to_string(),
                symbol: "login".to_string(),
                entrypoint_type: EntrypointType::HttpRoute,
            }),
            files: vec![make_file("src/auth.ts", 0), make_file("src/token.ts", 1)],
            edges: vec![FlowEdge {
                from: "auth".to_string(),
                to: "token".to_string(),
                edge_type: EdgeType::Calls,
            }],
            risk_score: 0.82,
            review_order: 1,
        }];

        let request = build_refinement_request(&groups, None, "{}", "10 files changed");
        assert_eq!(request.groups.len(), 1);
        assert_eq!(request.groups[0].id, "g1");
        assert_eq!(
            request.groups[0].entrypoint,
            Some("src/auth.ts::login".to_string())
        );
        assert_eq!(request.groups[0].files.len(), 2);
        assert_eq!(request.diff_summary, "10 files changed");
    }

    #[test]
    fn test_apply_removes_empty_groups_after_reclassify() {
        let groups = vec![
            make_group("g1", "Group 1", vec![make_file("a.ts", 0)]),
            make_group("g2", "Group 2", vec![make_file("b.ts", 0)]),
        ];
        let response = RefinementResponse {
            reclassifications: vec![RefinementReclassify {
                file: "a.ts".to_string(),
                from_group_id: "g1".to_string(),
                to_group_id: "g2".to_string(),
                reason: "test".to_string(),
            }],
            ..empty_refinement()
        };

        let (result, _) = apply_refinement(&groups, None, &response).unwrap();
        // g1 should be removed since it's now empty
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "g2");
        assert_eq!(result[0].files.len(), 2);
    }

    // ── Sub-Group Maintenance Tests ──

    #[test]
    fn test_reclassify_from_infra_removes_from_sub_group() {
        let groups = vec![make_group("g1", "Auth", vec![make_file("auth.ts", 0)])];
        let infra = InfrastructureGroup {
            files: vec!["Dockerfile".to_string(), "token.ts".to_string()],
            sub_groups: vec![
                InfraSubGroup {
                    name: "Infrastructure".to_string(),
                    category: InfraCategory::Infrastructure,
                    files: vec!["Dockerfile".to_string()],
                },
                InfraSubGroup {
                    name: "Unclassified".to_string(),
                    category: InfraCategory::Unclassified,
                    files: vec!["token.ts".to_string()],
                },
            ],
            reason: "test".to_string(),
        };
        let response = RefinementResponse {
            reclassifications: vec![RefinementReclassify {
                file: "token.ts".to_string(),
                from_group_id: "infrastructure".to_string(),
                to_group_id: "g1".to_string(),
                reason: "Token belongs to auth".to_string(),
            }],
            ..empty_refinement()
        };

        let (result, infra_out) = apply_refinement(&groups, Some(&infra), &response).unwrap();
        // File moved to group
        assert!(result[0].files.iter().any(|f| f.path == "token.ts"));
        // File removed from infra files
        let ig = infra_out.unwrap();
        assert!(!ig.files.contains(&"token.ts".to_string()));
        // File removed from sub_groups — Unclassified sub-group should be gone (was its only file)
        assert!(
            !ig.sub_groups
                .iter()
                .any(|sg| sg.category == InfraCategory::Unclassified),
            "Empty Unclassified sub-group should be removed"
        );
        // Dockerfile sub-group still exists
        assert!(ig
            .sub_groups
            .iter()
            .any(|sg| sg.files.contains(&"Dockerfile".to_string())));
    }

    #[test]
    fn test_reclassify_to_infra_adds_to_correct_sub_group() {
        let groups = vec![make_group(
            "g1",
            "Group 1",
            vec![make_file("a.ts", 0), make_file("Dockerfile", 1)],
        )];
        let response = RefinementResponse {
            reclassifications: vec![RefinementReclassify {
                file: "Dockerfile".to_string(),
                from_group_id: "g1".to_string(),
                to_group_id: "infrastructure".to_string(),
                reason: "Dockerfile is infra".to_string(),
            }],
            ..empty_refinement()
        };

        let (_, infra_out) = apply_refinement(&groups, None, &response).unwrap();
        let ig = infra_out.unwrap();
        assert!(ig.files.contains(&"Dockerfile".to_string()));
        // Should be classified as Infrastructure category
        let infra_sg = ig
            .sub_groups
            .iter()
            .find(|sg| sg.category == InfraCategory::Infrastructure)
            .expect("Should have Infrastructure sub-group");
        assert!(infra_sg.files.contains(&"Dockerfile".to_string()));
    }

    #[test]
    fn test_reclassify_to_infra_adds_to_existing_sub_group() {
        let groups = vec![make_group(
            "g1",
            "Group 1",
            vec![make_file("a.ts", 0), make_file("docker-compose.yml", 1)],
        )];
        let existing_infra = InfrastructureGroup {
            files: vec!["Dockerfile".to_string()],
            sub_groups: vec![InfraSubGroup {
                name: "Infrastructure".to_string(),
                category: InfraCategory::Infrastructure,
                files: vec!["Dockerfile".to_string()],
            }],
            reason: "test".to_string(),
        };
        let response = RefinementResponse {
            reclassifications: vec![RefinementReclassify {
                file: "docker-compose.yml".to_string(),
                from_group_id: "g1".to_string(),
                to_group_id: "infrastructure".to_string(),
                reason: "Docker compose is infra".to_string(),
            }],
            ..empty_refinement()
        };

        let (_, infra_out) = apply_refinement(&groups, Some(&existing_infra), &response).unwrap();
        let ig = infra_out.unwrap();
        // Should be added to the existing Infrastructure sub-group, not create a new one
        let infra_sgs: Vec<_> = ig
            .sub_groups
            .iter()
            .filter(|sg| sg.category == InfraCategory::Infrastructure)
            .collect();
        assert_eq!(
            infra_sgs.len(),
            1,
            "Should have exactly one Infrastructure sub-group"
        );
        assert_eq!(infra_sgs[0].files.len(), 2);
        assert!(infra_sgs[0].files.contains(&"Dockerfile".to_string()));
        assert!(infra_sgs[0]
            .files
            .contains(&"docker-compose.yml".to_string()));
    }

    #[test]
    fn test_reclassify_to_infra_schema_file_categorized() {
        let groups = vec![make_group(
            "g1",
            "Group 1",
            vec![make_file("a.ts", 0), make_file("schemas/user.schema.ts", 1)],
        )];
        let response = RefinementResponse {
            reclassifications: vec![RefinementReclassify {
                file: "schemas/user.schema.ts".to_string(),
                from_group_id: "g1".to_string(),
                to_group_id: "infrastructure".to_string(),
                reason: "Schema is shared".to_string(),
            }],
            ..empty_refinement()
        };

        let (_, infra_out) = apply_refinement(&groups, None, &response).unwrap();
        let ig = infra_out.unwrap();
        let schema_sg = ig
            .sub_groups
            .iter()
            .find(|sg| sg.category == InfraCategory::Schema)
            .expect("Should have Schema sub-group");
        assert!(schema_sg
            .files
            .contains(&"schemas/user.schema.ts".to_string()));
    }

    #[test]
    fn test_reclassify_from_infra_sub_groups_consistent_with_files() {
        let groups = vec![make_group("g1", "Auth", vec![make_file("auth.ts", 0)])];
        let infra = InfrastructureGroup {
            files: vec![
                "Dockerfile".to_string(),
                "scripts/deploy.sh".to_string(),
                "README.md".to_string(),
            ],
            sub_groups: vec![
                InfraSubGroup {
                    name: "Infrastructure".to_string(),
                    category: InfraCategory::Infrastructure,
                    files: vec!["Dockerfile".to_string()],
                },
                InfraSubGroup {
                    name: "Scripts".to_string(),
                    category: InfraCategory::Script,
                    files: vec!["scripts/deploy.sh".to_string()],
                },
                InfraSubGroup {
                    name: "Documentation".to_string(),
                    category: InfraCategory::Documentation,
                    files: vec!["README.md".to_string()],
                },
            ],
            reason: "test".to_string(),
        };
        let response = RefinementResponse {
            reclassifications: vec![
                RefinementReclassify {
                    file: "scripts/deploy.sh".to_string(),
                    from_group_id: "infrastructure".to_string(),
                    to_group_id: "g1".to_string(),
                    reason: "Deploy script belongs to auth flow".to_string(),
                },
                RefinementReclassify {
                    file: "README.md".to_string(),
                    from_group_id: "infrastructure".to_string(),
                    to_group_id: "g1".to_string(),
                    reason: "Docs for auth".to_string(),
                },
            ],
            ..empty_refinement()
        };

        let (_, infra_out) = apply_refinement(&groups, Some(&infra), &response).unwrap();
        let ig = infra_out.unwrap();
        // Only Dockerfile should remain
        assert_eq!(ig.files.len(), 1);
        assert_eq!(ig.files[0], "Dockerfile");
        // Only Infrastructure sub-group should remain
        assert_eq!(ig.sub_groups.len(), 1);
        assert_eq!(ig.sub_groups[0].category, InfraCategory::Infrastructure);
        // All sub_group files should be a subset of ig.files
        let all_sg_files: Vec<&String> = ig.sub_groups.iter().flat_map(|sg| &sg.files).collect();
        for f in &all_sg_files {
            assert!(
                ig.files.contains(f),
                "Sub-group file {:?} not in infra.files",
                f
            );
        }
    }

    // ── Property-Based Tests ──

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        fn file_path_strategy() -> impl Strategy<Value = String> {
            "[a-z]{1,5}".prop_map(|name| format!("src/{}.ts", name))
        }

        /// Strategy that generates classifiable infra file paths
        fn infra_file_path_strategy() -> impl Strategy<Value = String> {
            prop::sample::select(vec![
                "Dockerfile".to_string(),
                "docker-compose.yml".to_string(),
                ".env.dev".to_string(),
                "tsconfig.json".to_string(),
                "package.json".to_string(),
                "schemas/user.ts".to_string(),
                "schemas/billing.schema.ts".to_string(),
                "scripts/deploy.sh".to_string(),
                "scripts/setup.sh".to_string(),
                "migrations/001.sql".to_string(),
                "docs/README.md".to_string(),
                "docs/setup.md".to_string(),
                "src/random-file.ts".to_string(),
                ".eslintrc.json".to_string(),
                "src/generated/api.ts".to_string(),
            ])
        }

        fn group_strategy() -> impl Strategy<Value = FlowGroup> {
            (
                "[a-z]{2,5}",
                prop::collection::vec(file_path_strategy(), 1..5),
            )
                .prop_map(|(name, file_paths)| {
                    let files: Vec<FileChange> = file_paths
                        .into_iter()
                        .enumerate()
                        .map(|(i, path)| make_file(&path, i as u32))
                        .collect();
                    make_group(&format!("g_{}", name), &name, files)
                })
        }

        proptest! {
            /// Empty refinement never changes group count
            #[test]
            fn prop_empty_refinement_preserves_groups(
                groups in prop::collection::vec(group_strategy(), 1..5)
            ) {
                let (result, _) = apply_refinement(&groups, None, &empty_refinement()).unwrap();
                prop_assert_eq!(result.len(), groups.len());
            }

            /// Empty refinement preserves total file count
            #[test]
            fn prop_empty_refinement_preserves_files(
                groups in prop::collection::vec(group_strategy(), 1..5)
            ) {
                let total_before: usize = groups.iter().map(|g| g.files.len()).sum();
                let (result, _) = apply_refinement(&groups, None, &empty_refinement()).unwrap();
                let total_after: usize = result.iter().map(|g| g.files.len()).sum();
                prop_assert_eq!(total_before, total_after);
            }

            /// Validation of empty refinement never fails
            #[test]
            fn prop_validate_empty_never_fails(
                groups in prop::collection::vec(group_strategy(), 1..5)
            ) {
                let result = validate_refinement(&empty_refinement(), &groups, None);
                prop_assert!(result.is_ok());
            }

            /// has_refinements is false for empty response
            #[test]
            fn prop_has_refinements_empty(_seed in 0u32..100) {
                prop_assert!(!has_refinements(&empty_refinement()));
            }

            /// build_refinement_request produces correct group count
            #[test]
            fn prop_build_request_group_count(
                groups in prop::collection::vec(group_strategy(), 1..5)
            ) {
                let request = build_refinement_request(&groups, None, "{}", "diff");
                prop_assert_eq!(request.groups.len(), groups.len());
            }

            /// Review order is always sequential after apply
            #[test]
            fn prop_review_order_sequential(
                groups in prop::collection::vec(group_strategy(), 1..5)
            ) {
                let (result, _) = apply_refinement(&groups, None, &empty_refinement()).unwrap();
                for (i, group) in result.iter().enumerate() {
                    prop_assert_eq!(group.review_order, (i + 1) as u32);
                }
            }

            /// Reclassifying to infrastructure always places the file in a sub-group
            /// whose category matches classify_by_convention
            #[test]
            fn prop_reclassify_to_infra_categorized_correctly(
                file_path in infra_file_path_strategy()
            ) {
                let groups = vec![make_group(
                    "g1",
                    "Group 1",
                    vec![make_file(&file_path, 0), make_file("src/keep.ts", 1)],
                )];
                let response = RefinementResponse {
                    reclassifications: vec![RefinementReclassify {
                        file: file_path.clone(),
                        from_group_id: "g1".to_string(),
                        to_group_id: "infrastructure".to_string(),
                        reason: "test".to_string(),
                    }],
                    ..empty_refinement()
                };

                let (_, infra_out) = apply_refinement(&groups, None, &response).unwrap();
                let ig = infra_out.unwrap();
                let expected_category = crate::cluster::classify_by_convention(&file_path);
                // File must appear in exactly one sub-group with the correct category
                let matching: Vec<_> = ig
                    .sub_groups
                    .iter()
                    .filter(|sg| sg.files.contains(&file_path))
                    .collect();
                prop_assert_eq!(
                    matching.len(),
                    1,
                    "File should be in exactly one sub-group, found {}",
                    matching.len()
                );
                prop_assert_eq!(matching[0].category.clone(), expected_category);
            }

            /// After reclassifying a file FROM infrastructure, it appears in no sub-group
            #[test]
            fn prop_reclassify_from_infra_removes_from_all_sub_groups(
                file_path in infra_file_path_strategy()
            ) {
                use crate::cluster::classify_by_convention;
                use crate::cluster::category_display_name;
                let category = classify_by_convention(&file_path);
                let display = category_display_name(&category);

                let groups = vec![make_group("g1", "Target", vec![make_file("src/a.ts", 0)])];
                let infra = InfrastructureGroup {
                    files: vec![file_path.clone()],
                    sub_groups: vec![InfraSubGroup {
                        name: display,
                        category,
                        files: vec![file_path.clone()],
                    }],
                    reason: "test".to_string(),
                };
                let response = RefinementResponse {
                    reclassifications: vec![RefinementReclassify {
                        file: file_path.clone(),
                        from_group_id: "infrastructure".to_string(),
                        to_group_id: "g1".to_string(),
                        reason: "test".to_string(),
                    }],
                    ..empty_refinement()
                };

                let (_, infra_out) = apply_refinement(&groups, Some(&infra), &response).unwrap();
                if let Some(ig) = infra_out {
                    for sg in &ig.sub_groups {
                        prop_assert!(
                            !sg.files.contains(&file_path),
                            "File {:?} still in sub-group {:?}",
                            file_path,
                            sg.name
                        );
                    }
                    prop_assert!(
                        !ig.files.contains(&file_path),
                        "File {:?} still in infra.files",
                        file_path
                    );
                }
            }

            /// After any reclassification involving infrastructure, every file in
            /// infra.files appears in exactly one sub-group (consistency invariant)
            #[test]
            fn prop_infra_sub_groups_consistent_after_reclassify(
                file_path in infra_file_path_strategy()
            ) {
                // Start with a group containing the file, reclassify to infra
                let groups = vec![make_group(
                    "g1",
                    "Group 1",
                    vec![make_file(&file_path, 0), make_file("src/keep.ts", 1)],
                )];
                let response = RefinementResponse {
                    reclassifications: vec![RefinementReclassify {
                        file: file_path.clone(),
                        from_group_id: "g1".to_string(),
                        to_group_id: "infrastructure".to_string(),
                        reason: "test".to_string(),
                    }],
                    ..empty_refinement()
                };

                let (_, infra_out) = apply_refinement(&groups, None, &response).unwrap();
                let ig = infra_out.unwrap();

                // Every file in ig.files must appear in exactly one sub-group
                for f in &ig.files {
                    let count = ig.sub_groups.iter().filter(|sg| sg.files.contains(f)).count();
                    prop_assert_eq!(
                        count,
                        1,
                        "File {:?} appears in {} sub-groups, expected 1",
                        f,
                        count
                    );
                }
                // Every file in sub-groups must appear in ig.files
                for sg in &ig.sub_groups {
                    for f in &sg.files {
                        prop_assert!(
                            ig.files.contains(f),
                            "Sub-group file {:?} not in infra.files",
                            f
                        );
                    }
                }
            }
        }
    }
}
