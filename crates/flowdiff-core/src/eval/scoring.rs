//! Eval scoring types and functions.
//!
//! Provides baseline definitions and 6 scoring functions that produce
//! per-criterion scores in [0.0, 1.0] for evaluating flowdiff pipeline output.

use std::collections::HashSet;

use crate::types::{AnalysisOutput, EntrypointType};

// ═══════════════════════════════════════════════════════════════════════════
// Baseline Types
// ═══════════════════════════════════════════════════════════════════════════

/// Expected entrypoint in a fixture baseline.
#[derive(Debug, Clone)]
pub struct ExpectedEntrypoint {
    /// Substring that must appear in the entrypoint's file path
    pub file_contains: String,
    /// Expected entrypoint type
    pub ep_type: EntrypointType,
}

/// Expected flow group in a fixture baseline.
#[derive(Debug, Clone)]
pub struct ExpectedGroup {
    /// Descriptive label for the expected group (for error messages)
    pub label: String,
    /// File path substrings that must all appear in the same group
    pub must_contain: Vec<String>,
    /// File path substrings that must NOT appear in this group
    pub must_not_contain: Vec<String>,
}

/// Expected risk ordering constraint.
/// "The group containing `higher_risk_file` should be reviewed before `lower_risk_file`."
#[derive(Debug, Clone)]
pub struct RiskOrderingConstraint {
    pub higher_risk_file: String,
    pub lower_risk_file: String,
}

/// Complete baseline for a synthetic fixture.
#[derive(Debug, Clone)]
pub struct EvalBaseline {
    /// Human-readable fixture name
    pub name: String,
    /// Expected languages detected
    pub expected_languages: Vec<String>,
    /// Bounds on number of flow groups (not counting infrastructure)
    pub min_groups: usize,
    pub max_groups: usize,
    /// Expected total files changed
    pub expected_file_count: usize,
    /// Expected entrypoints
    pub expected_entrypoints: Vec<ExpectedEntrypoint>,
    /// Expected group compositions
    pub expected_groups: Vec<ExpectedGroup>,
    /// Risk ordering constraints
    pub risk_ordering: Vec<RiskOrderingConstraint>,
    /// Files expected in infrastructure group (not reachable from entrypoints)
    pub expected_infrastructure: Vec<String>,
}

// ═══════════════════════════════════════════════════════════════════════════
// Scoring
// ═══════════════════════════════════════════════════════════════════════════

/// Per-criterion eval scores, all in [0.0, 1.0].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EvalScores {
    /// Are flow groups semantically coherent? (right files grouped together)
    pub group_coherence: f64,
    /// Are entrypoints correctly identified?
    pub entrypoint_accuracy: f64,
    /// Is review ordering logical? (risk ordering constraints satisfied)
    pub review_ordering: f64,
    /// Are risk scores reasonable? (in valid range, auth > utils, etc.)
    pub risk_reasonableness: f64,
    /// Are languages correctly detected?
    pub language_detection: f64,
    /// Is the file count correct?
    pub file_accounting: f64,
    /// Overall weighted score
    pub overall: f64,
}

impl EvalScores {
    /// Compute the weighted overall score from individual criteria.
    pub fn compute_overall(&mut self) {
        self.overall = 0.25 * self.group_coherence
            + 0.20 * self.entrypoint_accuracy
            + 0.15 * self.review_ordering
            + 0.15 * self.risk_reasonableness
            + 0.15 * self.language_detection
            + 0.10 * self.file_accounting;
    }
}

/// Score the pipeline output against a baseline.
pub fn score_output(output: &AnalysisOutput, baseline: &EvalBaseline) -> EvalScores {
    let group_coherence = score_group_coherence(output, baseline);
    let entrypoint_accuracy = score_entrypoint_accuracy(output, baseline);
    let review_ordering = score_review_ordering(output, baseline);
    let risk_reasonableness = score_risk_reasonableness(output);
    let language_detection = score_language_detection(output, baseline);
    let file_accounting = score_file_accounting(output, baseline);

    let mut scores = EvalScores {
        group_coherence,
        entrypoint_accuracy,
        review_ordering,
        risk_reasonableness,
        language_detection,
        file_accounting,
        overall: 0.0,
    };
    scores.compute_overall();
    scores
}

/// Score group coherence: do the right files end up in the same groups?
pub fn score_group_coherence(output: &AnalysisOutput, baseline: &EvalBaseline) -> f64 {
    if baseline.expected_groups.is_empty() {
        return 1.0;
    }

    let mut total_score = 0.0;
    let mut total_weight = 0.0;

    for expected in &baseline.expected_groups {
        let weight = expected.must_contain.len().max(1) as f64;
        total_weight += weight;

        let mut best_match_score: f64 = 0.0;

        for group in &output.groups {
            let group_paths: Vec<&str> = group.files.iter().map(|f| f.path.as_str()).collect();

            let contained = expected
                .must_contain
                .iter()
                .filter(|mc| group_paths.iter().any(|p| p.contains(mc.as_str())))
                .count();

            let violations = expected
                .must_not_contain
                .iter()
                .filter(|mnc| group_paths.iter().any(|p| p.contains(mnc.as_str())))
                .count();

            if expected.must_contain.is_empty() {
                continue;
            }

            let contain_score = contained as f64 / expected.must_contain.len() as f64;
            let violation_penalty = if expected.must_not_contain.is_empty() {
                0.0
            } else {
                violations as f64 / expected.must_not_contain.len() as f64
            };

            let group_score = (contain_score - 0.5 * violation_penalty).max(0.0);
            best_match_score = best_match_score.max(group_score);
        }

        if let Some(ref infra) = output.infrastructure_group {
            let contained_in_infra = expected
                .must_contain
                .iter()
                .filter(|mc| infra.files.iter().any(|f| f.contains(mc.as_str())))
                .count();
            if contained_in_infra > 0 && !expected.must_contain.is_empty() {
                let infra_score =
                    1.0 - (contained_in_infra as f64 / expected.must_contain.len() as f64);
                best_match_score = best_match_score.max(0.0).min(infra_score);
            }
        }

        total_score += best_match_score * weight;
    }

    if total_weight == 0.0 {
        1.0
    } else {
        total_score / total_weight
    }
}

/// Score entrypoint detection accuracy.
pub fn score_entrypoint_accuracy(output: &AnalysisOutput, baseline: &EvalBaseline) -> f64 {
    if baseline.expected_entrypoints.is_empty() {
        return 1.0;
    }

    let detected_entrypoints: Vec<_> = output
        .groups
        .iter()
        .filter_map(|g| g.entrypoint.as_ref())
        .collect();

    let mut matched = 0;
    for expected in &baseline.expected_entrypoints {
        let found = detected_entrypoints.iter().any(|ep| {
            ep.file.contains(&expected.file_contains) && ep.entrypoint_type == expected.ep_type
        });
        if found {
            matched += 1;
        }
    }

    matched as f64 / baseline.expected_entrypoints.len() as f64
}

/// Score review ordering: are risk ordering constraints satisfied?
pub fn score_review_ordering(output: &AnalysisOutput, baseline: &EvalBaseline) -> f64 {
    if baseline.risk_ordering.is_empty() {
        return 1.0;
    }

    let mut satisfied = 0;
    for constraint in &baseline.risk_ordering {
        let higher_group = output.groups.iter().find(|g| {
            g.files
                .iter()
                .any(|f| f.path.contains(&constraint.higher_risk_file))
        });
        let lower_group = output.groups.iter().find(|g| {
            g.files
                .iter()
                .any(|f| f.path.contains(&constraint.lower_risk_file))
        });

        match (higher_group, lower_group) {
            (Some(h), Some(l)) => {
                if h.review_order <= l.review_order {
                    satisfied += 1;
                }
            }
            (Some(_), None) => {
                satisfied += 1;
            }
            _ => {}
        }
    }

    satisfied as f64 / baseline.risk_ordering.len() as f64
}

/// Score risk reasonableness: are scores in valid range and sensible?
pub fn score_risk_reasonableness(output: &AnalysisOutput) -> f64 {
    if output.groups.is_empty() {
        return 1.0;
    }

    let mut score: f64 = 1.0;
    for group in &output.groups {
        if group.risk_score < 0.0 || group.risk_score > 1.0 {
            score -= 0.25;
        }
        if group.review_order < 1 {
            score -= 0.25;
        }
        if group.files.is_empty() {
            score -= 0.25;
        }
    }

    let mut orders: Vec<u32> = output.groups.iter().map(|g| g.review_order).collect();
    orders.sort();
    orders.dedup();
    if orders.len() != output.groups.len() {
        score -= 0.1;
    }

    score.max(0.0)
}

/// Score language detection accuracy.
pub fn score_language_detection(output: &AnalysisOutput, baseline: &EvalBaseline) -> f64 {
    if baseline.expected_languages.is_empty() {
        return 1.0;
    }

    let detected: HashSet<&str> = output
        .summary
        .languages_detected
        .iter()
        .map(|s| s.as_str())
        .collect();

    let mut matched = 0;
    for lang in &baseline.expected_languages {
        if detected.contains(lang.as_str()) {
            matched += 1;
        }
    }

    matched as f64 / baseline.expected_languages.len() as f64
}

/// Score file accounting: correct number of files, all files accounted for.
pub fn score_file_accounting(output: &AnalysisOutput, baseline: &EvalBaseline) -> f64 {
    let mut score: f64 = 1.0;

    if output.summary.total_files_changed as usize != baseline.expected_file_count {
        score -= 0.5;
    }

    let total_grouped: usize = output.groups.iter().map(|g| g.files.len()).sum();
    let infra: usize = output
        .infrastructure_group
        .as_ref()
        .map(|i| i.files.len())
        .unwrap_or(0);

    if total_grouped + infra != output.summary.total_files_changed as usize {
        score -= 0.5;
    }

    let group_count = output.groups.len();
    if group_count < baseline.min_groups || group_count > baseline.max_groups {
        score -= 0.25;
    }

    score.max(0.0)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::types::*;

    #[test]
    fn test_score_output_empty_baseline() {
        let output = AnalysisOutput {
            version: "1.0.0".to_string(),
            diff_source: DiffSource {
                diff_type: DiffType::BranchComparison,
                base: Some("main".to_string()),
                head: Some("feature".to_string()),
                base_sha: None,
                head_sha: None,
            },
            summary: AnalysisSummary {
                total_files_changed: 0,
                total_groups: 0,
                languages_detected: vec![],
                frameworks_detected: vec![],
            },
            groups: vec![],
            infrastructure_group: None,
            annotations: None,
        };
        let baseline = EvalBaseline {
            name: "empty".to_string(),
            expected_languages: vec![],
            min_groups: 0,
            max_groups: 0,
            expected_file_count: 0,
            expected_entrypoints: vec![],
            expected_groups: vec![],
            risk_ordering: vec![],
            expected_infrastructure: vec![],
        };
        let scores = score_output(&output, &baseline);
        assert!(scores.overall >= 0.0 && scores.overall <= 1.0);
    }

    #[test]
    fn test_score_risk_reasonableness_valid() {
        let output = AnalysisOutput {
            version: "1.0.0".to_string(),
            diff_source: DiffSource {
                diff_type: DiffType::BranchComparison,
                base: Some("main".to_string()),
                head: Some("feature".to_string()),
                base_sha: None,
                head_sha: None,
            },
            summary: AnalysisSummary {
                total_files_changed: 1,
                total_groups: 1,
                languages_detected: vec![],
                frameworks_detected: vec![],
            },
            groups: vec![FlowGroup {
                id: "g1".to_string(),
                name: "Group 1".to_string(),
                entrypoint: None,
                files: vec![FileChange {
                    path: "a.ts".to_string(),
                    flow_position: 0,
                    role: FileRole::Utility,
                    changes: ChangeStats {
                        additions: 5,
                        deletions: 2,
                    },
                    symbols_changed: vec![],
                }],
                edges: vec![],
                risk_score: 0.5,
                review_order: 1,
            }],
            infrastructure_group: None,
            annotations: None,
        };
        let score = score_risk_reasonableness(&output);
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_score_risk_out_of_bounds() {
        let output = AnalysisOutput {
            version: "1.0.0".to_string(),
            diff_source: DiffSource {
                diff_type: DiffType::BranchComparison,
                base: Some("main".to_string()),
                head: Some("feature".to_string()),
                base_sha: None,
                head_sha: None,
            },
            summary: AnalysisSummary {
                total_files_changed: 1,
                total_groups: 1,
                languages_detected: vec![],
                frameworks_detected: vec![],
            },
            groups: vec![FlowGroup {
                id: "g1".to_string(),
                name: "Group 1".to_string(),
                entrypoint: None,
                files: vec![FileChange {
                    path: "a.ts".to_string(),
                    flow_position: 0,
                    role: FileRole::Utility,
                    changes: ChangeStats {
                        additions: 5,
                        deletions: 2,
                    },
                    symbols_changed: vec![],
                }],
                edges: vec![],
                risk_score: 1.5, // Out of bounds
                review_order: 1,
            }],
            infrastructure_group: None,
            annotations: None,
        };
        let score = score_risk_reasonableness(&output);
        assert!(score < 1.0);
    }

    #[test]
    fn test_score_language_detection_full_match() {
        let output = AnalysisOutput {
            version: "1.0.0".to_string(),
            diff_source: DiffSource {
                diff_type: DiffType::BranchComparison,
                base: None,
                head: None,
                base_sha: None,
                head_sha: None,
            },
            summary: AnalysisSummary {
                total_files_changed: 0,
                total_groups: 0,
                languages_detected: vec!["typescript".to_string(), "python".to_string()],
                frameworks_detected: vec![],
            },
            groups: vec![],
            infrastructure_group: None,
            annotations: None,
        };
        let baseline = EvalBaseline {
            name: "test".to_string(),
            expected_languages: vec!["typescript".to_string(), "python".to_string()],
            min_groups: 0,
            max_groups: 0,
            expected_file_count: 0,
            expected_entrypoints: vec![],
            expected_groups: vec![],
            risk_ordering: vec![],
            expected_infrastructure: vec![],
        };
        let score = score_language_detection(&output, &baseline);
        assert!((score - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_score_language_detection_partial() {
        let output = AnalysisOutput {
            version: "1.0.0".to_string(),
            diff_source: DiffSource {
                diff_type: DiffType::BranchComparison,
                base: None,
                head: None,
                base_sha: None,
                head_sha: None,
            },
            summary: AnalysisSummary {
                total_files_changed: 0,
                total_groups: 0,
                languages_detected: vec!["typescript".to_string()],
                frameworks_detected: vec![],
            },
            groups: vec![],
            infrastructure_group: None,
            annotations: None,
        };
        let baseline = EvalBaseline {
            name: "test".to_string(),
            expected_languages: vec!["typescript".to_string(), "python".to_string()],
            min_groups: 0,
            max_groups: 0,
            expected_file_count: 0,
            expected_entrypoints: vec![],
            expected_groups: vec![],
            risk_ordering: vec![],
            expected_infrastructure: vec![],
        };
        let score = score_language_detection(&output, &baseline);
        assert!((score - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_eval_scores_serde_roundtrip() {
        let scores = EvalScores {
            group_coherence: 0.85,
            entrypoint_accuracy: 1.0,
            review_ordering: 0.75,
            risk_reasonableness: 1.0,
            language_detection: 1.0,
            file_accounting: 0.75,
            overall: 0.89,
        };
        let json = serde_json::to_string(&scores).unwrap();
        let parsed: EvalScores = serde_json::from_str(&json).unwrap();
        assert!((parsed.overall - scores.overall).abs() < f64::EPSILON);
    }

    #[test]
    fn test_compute_overall_weights_sum_to_one() {
        // Weights: 0.25 + 0.20 + 0.15 + 0.15 + 0.15 + 0.10 = 1.00
        let mut scores = EvalScores {
            group_coherence: 1.0,
            entrypoint_accuracy: 1.0,
            review_ordering: 1.0,
            risk_reasonableness: 1.0,
            language_detection: 1.0,
            file_accounting: 1.0,
            overall: 0.0,
        };
        scores.compute_overall();
        assert!((scores.overall - 1.0).abs() < f64::EPSILON);
    }
}
