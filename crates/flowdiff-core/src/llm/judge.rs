//! LLM-as-judge evaluator for flowdiff analysis quality.
//!
//! Takes flowdiff's JSON output + fixture source code and scores quality
//! using structured LLM outputs. Evaluation criteria:
//! 1. Flow group coherence
//! 2. Review ordering logic
//! 3. Entrypoint identification accuracy
//! 4. Risk score reasonableness
//! 5. Mermaid graph accuracy
//!
//! Returns per-criterion scores (1-5) + overall score + failure explanations.
//! Supports VCR caching for deterministic CI replay.

use std::fmt::Write as FmtWrite;
use std::path::Path;

use crate::types::AnalysisOutput;

use super::schema::{JudgeCriterionScore, JudgeRequest, JudgeResponse, JudgeSourceFile};
use super::{LlmError, LlmProvider};

/// The 5 evaluation criteria used by the LLM judge.
pub const JUDGE_CRITERIA: &[&str] = &[
    "group_coherence",
    "review_ordering",
    "entrypoint_identification",
    "risk_reasonableness",
    "mermaid_accuracy",
];

/// Build a JudgeRequest from analysis output and fixture source files.
///
/// `source_files` should be a list of (relative_path, content) pairs from the fixture codebase.
/// `diff_text` is the unified diff that was analyzed.
/// `fixture_name` is a descriptive name for the fixture.
pub fn build_judge_request(
    output: &AnalysisOutput,
    source_files: &[(String, String)],
    diff_text: &str,
    fixture_name: &str,
) -> Result<JudgeRequest, LlmError> {
    let analysis_json = serde_json::to_string_pretty(output).map_err(|e| {
        LlmError::ParseResponse(format!("Failed to serialize analysis output: {}", e))
    })?;

    let judge_files: Vec<JudgeSourceFile> = source_files
        .iter()
        .map(|(path, content)| JudgeSourceFile {
            path: path.clone(),
            content: content.clone(),
        })
        .collect();

    Ok(JudgeRequest {
        analysis_json,
        source_files: judge_files,
        diff_text: diff_text.to_string(),
        fixture_name: fixture_name.to_string(),
    })
}

/// Collect source files from a directory, returning (relative_path, content) pairs.
///
/// Reads all text files from the given directory tree, skipping binary files
/// and hidden directories (starting with `.`).
pub fn collect_source_files(repo_path: &Path) -> Vec<(String, String)> {
    let mut files = Vec::new();
    collect_recursive(repo_path, repo_path, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

fn collect_recursive(root: &Path, dir: &Path, files: &mut Vec<(String, String)>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();

        // Skip hidden dirs and .git
        if name_str.starts_with('.') {
            continue;
        }

        if path.is_dir() {
            collect_recursive(root, &path, files);
        } else if path.is_file() {
            // Skip binary-looking files
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            if matches!(ext, "png" | "jpg" | "jpeg" | "gif" | "ico" | "woff" | "woff2" | "ttf" | "eot" | "exe" | "dll" | "so" | "dylib") {
                continue;
            }

            if let Ok(content) = std::fs::read_to_string(&path) {
                let rel_path = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                files.push((rel_path, content));
            }
        }
    }
}

/// Validate a JudgeResponse for completeness and correctness.
///
/// Returns a list of validation errors (empty if valid).
pub fn validate_judge_response(response: &JudgeResponse) -> Vec<String> {
    let mut errors = Vec::new();

    // Check all 5 criteria are present
    let criteria_names: Vec<&str> = response.criteria.iter().map(|c| c.criterion.as_str()).collect();
    for expected in JUDGE_CRITERIA {
        if !criteria_names.contains(expected) {
            errors.push(format!("Missing criterion: {}", expected));
        }
    }

    // Check score bounds
    for criterion in &response.criteria {
        if criterion.score < 1 || criterion.score > 5 {
            errors.push(format!(
                "Criterion '{}' score {} out of bounds [1, 5]",
                criterion.criterion, criterion.score
            ));
        }
    }

    // Check overall score bounds
    if response.overall_score < 1.0 || response.overall_score > 5.0 {
        errors.push(format!(
            "Overall score {} out of bounds [1.0, 5.0]",
            response.overall_score
        ));
    }

    // Check overall score is approximately the average of criteria scores
    if !response.criteria.is_empty() {
        let expected_avg =
            response.criteria.iter().map(|c| c.score as f64).sum::<f64>() / response.criteria.len() as f64;
        if (response.overall_score - expected_avg).abs() > 0.5 {
            errors.push(format!(
                "Overall score {:.1} differs significantly from criteria average {:.1}",
                response.overall_score, expected_avg
            ));
        }
    }

    // Check that low scores have failure explanations
    let low_scores: Vec<&JudgeCriterionScore> = response
        .criteria
        .iter()
        .filter(|c| c.score < 3)
        .collect();
    if !low_scores.is_empty() && response.failure_explanations.is_empty() {
        errors.push("Criteria with scores below 3 but no failure_explanations provided".to_string());
    }

    errors
}

/// Normalize the judge's 1-5 scores to 0.0-1.0 range for compatibility with
/// the existing deterministic eval scoring.
pub fn normalize_judge_scores(response: &JudgeResponse) -> JudgeNormalizedScores {
    let mut scores = JudgeNormalizedScores::default();

    for criterion in &response.criteria {
        let normalized = (criterion.score as f64 - 1.0) / 4.0; // Map 1-5 to 0.0-1.0
        match criterion.criterion.as_str() {
            "group_coherence" => scores.group_coherence = normalized,
            "review_ordering" => scores.review_ordering = normalized,
            "entrypoint_identification" => scores.entrypoint_identification = normalized,
            "risk_reasonableness" => scores.risk_reasonableness = normalized,
            "mermaid_accuracy" => scores.mermaid_accuracy = normalized,
            _ => {}
        }
    }

    scores.overall = (response.overall_score - 1.0) / 4.0;
    scores
}

/// Normalized judge scores in [0.0, 1.0] range.
#[derive(Debug, Clone, Default)]
pub struct JudgeNormalizedScores {
    pub group_coherence: f64,
    pub review_ordering: f64,
    pub entrypoint_identification: f64,
    pub risk_reasonableness: f64,
    pub mermaid_accuracy: f64,
    pub overall: f64,
}

/// Run the full LLM-as-judge evaluation pipeline.
///
/// 1. Builds the judge request from analysis output + source files
/// 2. Calls the LLM provider (with VCR caching if configured)
/// 3. Validates the response
/// 4. Returns the validated response
pub async fn run_judge_evaluation(
    provider: &dyn LlmProvider,
    output: &AnalysisOutput,
    source_files: &[(String, String)],
    diff_text: &str,
    fixture_name: &str,
) -> Result<JudgeResponse, LlmError> {
    let request = build_judge_request(output, source_files, diff_text, fixture_name)?;
    let response = provider.evaluate_quality(&request).await?;

    let validation_errors = validate_judge_response(&response);
    if !validation_errors.is_empty() {
        return Err(LlmError::ParseResponse(format!(
            "Judge response validation failed: {}",
            validation_errors.join("; ")
        )));
    }

    Ok(response)
}

/// Format a judge evaluation report as a string.
///
/// Returns the formatted report text. Callers decide how to display it
/// (e.g. `eprintln!` in CLI, `log::info!` in library code).
pub fn format_judge_report(fixture_name: &str, response: &JudgeResponse) -> String {
    let normalized = normalize_judge_scores(response);
    let mut buf = String::new();

    let _ = writeln!(buf, "\n╔══════════════════════════════════════════════╗");
    let _ = writeln!(buf, "║  LLM Judge: {:<33}║", fixture_name);
    let _ = writeln!(buf, "╠══════════════════════════════════════════════╣");
    for criterion in &response.criteria {
        let _ = writeln!(
            buf,
            "║  {:<25} {}/5  ({:.2})       ║",
            criterion.criterion,
            criterion.score,
            (criterion.score as f64 - 1.0) / 4.0
        );
    }
    let _ = writeln!(buf, "╠══════════════════════════════════════════════╣");
    let _ = writeln!(
        buf,
        "║  OVERALL:             {:.1}/5  ({:.2})       ║",
        response.overall_score, normalized.overall
    );
    let _ = writeln!(buf, "╚══════════════════════════════════════════════╝");

    if !response.strengths.is_empty() {
        let _ = writeln!(buf, "  Strengths:");
        for s in &response.strengths {
            let _ = writeln!(buf, "    + {}", s);
        }
    }
    if !response.failure_explanations.is_empty() {
        let _ = writeln!(buf, "  Issues:");
        for f in &response.failure_explanations {
            let _ = writeln!(buf, "    - {}", f);
        }
    }
    buf
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod tests {
    use super::*;
    use crate::types::*;

    fn sample_output() -> AnalysisOutput {
        AnalysisOutput {
            version: "1.0.0".to_string(),
            diff_source: DiffSource {
                diff_type: DiffType::BranchComparison,
                base: Some("main".to_string()),
                head: Some("feature".to_string()),
                base_sha: None,
                head_sha: None,
            },
            summary: AnalysisSummary {
                total_files_changed: 3,
                total_groups: 1,
                languages_detected: vec!["typescript".to_string()],
                frameworks_detected: vec!["express".to_string()],
            },
            groups: vec![FlowGroup {
                id: "group_1".to_string(),
                name: "User API flow".to_string(),
                entrypoint: Some(Entrypoint {
                    file: "src/routes/users.ts".to_string(),
                    symbol: "POST".to_string(),
                    entrypoint_type: EntrypointType::HttpRoute,
                }),
                files: vec![
                    FileChange {
                        path: "src/routes/users.ts".to_string(),
                        flow_position: 0,
                        role: FileRole::Entrypoint,
                        changes: ChangeStats {
                            additions: 20,
                            deletions: 0,
                        },
                        symbols_changed: vec!["POST".to_string()],
                    },
                    FileChange {
                        path: "src/services/userService.ts".to_string(),
                        flow_position: 1,
                        role: FileRole::Service,
                        changes: ChangeStats {
                            additions: 15,
                            deletions: 0,
                        },
                        symbols_changed: vec!["createUser".to_string()],
                    },
                ],
                edges: vec![FlowEdge {
                    from: "src/routes/users.ts::POST".to_string(),
                    to: "src/services/userService.ts::createUser".to_string(),
                    edge_type: EdgeType::Calls,
                }],
                risk_score: 0.65,
                review_order: 1,
            }],
            infrastructure_group: Some(InfrastructureGroup {
                files: vec!["package.json".to_string()],
                sub_groups: vec![],
                reason: "Not reachable from entrypoints".to_string(),
            }),
            annotations: None,
        }
    }

    fn sample_source_files() -> Vec<(String, String)> {
        vec![
            (
                "src/routes/users.ts".to_string(),
                "import { createUser } from '../services/userService';\nexport function POST() {}".to_string(),
            ),
            (
                "src/services/userService.ts".to_string(),
                "export function createUser() {}".to_string(),
            ),
        ]
    }

    // ── build_judge_request Tests ──

    #[test]
    fn test_build_judge_request() {
        let output = sample_output();
        let files = sample_source_files();
        let request =
            build_judge_request(&output, &files, "+ new line", "TS Express API").unwrap();

        assert_eq!(request.fixture_name, "TS Express API");
        assert_eq!(request.source_files.len(), 2);
        assert_eq!(request.diff_text, "+ new line");
        assert!(request.analysis_json.contains("group_1"));
        assert!(request.analysis_json.contains("User API flow"));
    }

    #[test]
    fn test_build_judge_request_empty_files() {
        let output = sample_output();
        let request = build_judge_request(&output, &[], "diff", "empty").unwrap();
        assert!(request.source_files.is_empty());
    }

    // ── validate_judge_response Tests ──

    #[test]
    fn test_validate_valid_response() {
        let response = JudgeResponse {
            criteria: JUDGE_CRITERIA
                .iter()
                .map(|c| JudgeCriterionScore {
                    criterion: c.to_string(),
                    score: 4,
                    explanation: "Good".to_string(),
                })
                .collect(),
            overall_score: 4.0,
            failure_explanations: vec![],
            strengths: vec!["Good analysis".to_string()],
        };
        let errors = validate_judge_response(&response);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn test_validate_missing_criterion() {
        let response = JudgeResponse {
            criteria: vec![JudgeCriterionScore {
                criterion: "group_coherence".to_string(),
                score: 4,
                explanation: "Good".to_string(),
            }],
            overall_score: 4.0,
            failure_explanations: vec![],
            strengths: vec![],
        };
        let errors = validate_judge_response(&response);
        assert!(errors.len() >= 4, "Should report missing criteria");
        assert!(errors.iter().any(|e| e.contains("review_ordering")));
    }

    #[test]
    fn test_validate_score_out_of_bounds() {
        let response = JudgeResponse {
            criteria: vec![JudgeCriterionScore {
                criterion: "group_coherence".to_string(),
                score: 6,
                explanation: "Too high".to_string(),
            }],
            overall_score: 6.0,
            failure_explanations: vec![],
            strengths: vec![],
        };
        let errors = validate_judge_response(&response);
        assert!(errors.iter().any(|e| e.contains("out of bounds")));
    }

    #[test]
    fn test_validate_score_zero() {
        let response = JudgeResponse {
            criteria: vec![JudgeCriterionScore {
                criterion: "group_coherence".to_string(),
                score: 0,
                explanation: "Too low".to_string(),
            }],
            overall_score: 0.0,
            failure_explanations: vec![],
            strengths: vec![],
        };
        let errors = validate_judge_response(&response);
        assert!(errors.iter().any(|e| e.contains("out of bounds")));
    }

    #[test]
    fn test_validate_low_scores_without_explanations() {
        let response = JudgeResponse {
            criteria: JUDGE_CRITERIA
                .iter()
                .map(|c| JudgeCriterionScore {
                    criterion: c.to_string(),
                    score: 2,
                    explanation: "Below average".to_string(),
                })
                .collect(),
            overall_score: 2.0,
            failure_explanations: vec![], // Missing!
            strengths: vec![],
        };
        let errors = validate_judge_response(&response);
        assert!(errors
            .iter()
            .any(|e| e.contains("failure_explanations")));
    }

    #[test]
    fn test_validate_overall_mismatch() {
        let response = JudgeResponse {
            criteria: JUDGE_CRITERIA
                .iter()
                .map(|c| JudgeCriterionScore {
                    criterion: c.to_string(),
                    score: 4,
                    explanation: "Good".to_string(),
                })
                .collect(),
            overall_score: 1.0, // Way off from average of 4.0
            failure_explanations: vec![],
            strengths: vec![],
        };
        let errors = validate_judge_response(&response);
        assert!(errors.iter().any(|e| e.contains("differs significantly")));
    }

    // ── normalize_judge_scores Tests ──

    #[test]
    fn test_normalize_all_fives() {
        let response = JudgeResponse {
            criteria: JUDGE_CRITERIA
                .iter()
                .map(|c| JudgeCriterionScore {
                    criterion: c.to_string(),
                    score: 5,
                    explanation: "Excellent".to_string(),
                })
                .collect(),
            overall_score: 5.0,
            failure_explanations: vec![],
            strengths: vec![],
        };
        let normalized = normalize_judge_scores(&response);
        assert!((normalized.overall - 1.0).abs() < f64::EPSILON);
        assert!((normalized.group_coherence - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_normalize_all_ones() {
        let response = JudgeResponse {
            criteria: JUDGE_CRITERIA
                .iter()
                .map(|c| JudgeCriterionScore {
                    criterion: c.to_string(),
                    score: 1,
                    explanation: "Poor".to_string(),
                })
                .collect(),
            overall_score: 1.0,
            failure_explanations: vec!["Everything is bad".to_string()],
            strengths: vec![],
        };
        let normalized = normalize_judge_scores(&response);
        assert!(normalized.overall.abs() < f64::EPSILON);
        assert!(normalized.group_coherence.abs() < f64::EPSILON);
    }

    #[test]
    fn test_normalize_mixed_scores() {
        let response = JudgeResponse {
            criteria: vec![
                JudgeCriterionScore {
                    criterion: "group_coherence".to_string(),
                    score: 5,
                    explanation: "Excellent".to_string(),
                },
                JudgeCriterionScore {
                    criterion: "review_ordering".to_string(),
                    score: 1,
                    explanation: "Poor".to_string(),
                },
                JudgeCriterionScore {
                    criterion: "entrypoint_identification".to_string(),
                    score: 3,
                    explanation: "OK".to_string(),
                },
                JudgeCriterionScore {
                    criterion: "risk_reasonableness".to_string(),
                    score: 4,
                    explanation: "Good".to_string(),
                },
                JudgeCriterionScore {
                    criterion: "mermaid_accuracy".to_string(),
                    score: 2,
                    explanation: "Below average".to_string(),
                },
            ],
            overall_score: 3.0,
            failure_explanations: vec!["Review ordering is bad".to_string()],
            strengths: vec!["Good grouping".to_string()],
        };
        let normalized = normalize_judge_scores(&response);
        assert!((normalized.group_coherence - 1.0).abs() < f64::EPSILON);
        assert!(normalized.review_ordering.abs() < f64::EPSILON);
        assert!((normalized.entrypoint_identification - 0.5).abs() < f64::EPSILON);
        assert!((normalized.risk_reasonableness - 0.75).abs() < f64::EPSILON);
        assert!((normalized.mermaid_accuracy - 0.25).abs() < f64::EPSILON);
        assert!((normalized.overall - 0.5).abs() < f64::EPSILON);
    }

    // ── collect_source_files Tests ──

    #[test]
    fn test_collect_source_files_from_temp_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src/a.ts"), "export function a() {}").unwrap();
        std::fs::write(tmp.path().join("src/b.py"), "def b(): pass").unwrap();
        std::fs::write(tmp.path().join("package.json"), "{}").unwrap();

        let files = collect_source_files(tmp.path());
        assert_eq!(files.len(), 3);
        assert!(files.iter().any(|(p, _)| p == "src/a.ts"));
        assert!(files.iter().any(|(p, _)| p == "src/b.py"));
        assert!(files.iter().any(|(p, _)| p == "package.json"));
    }

    #[test]
    fn test_collect_skips_hidden_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
        std::fs::write(tmp.path().join(".git/config"), "hidden").unwrap();
        std::fs::write(tmp.path().join("visible.ts"), "export {}").unwrap();

        let files = collect_source_files(tmp.path());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "visible.ts");
    }

    #[test]
    fn test_collect_skips_binary_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("icon.png"), &[0u8; 10]).unwrap();
        std::fs::write(tmp.path().join("code.ts"), "export {}").unwrap();

        let files = collect_source_files(tmp.path());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "code.ts");
    }

    #[test]
    fn test_collect_empty_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let files = collect_source_files(tmp.path());
        assert!(files.is_empty());
    }

    #[test]
    fn test_collect_files_sorted() {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("z.ts"), "z").unwrap();
        std::fs::write(tmp.path().join("a.ts"), "a").unwrap();
        std::fs::write(tmp.path().join("m.ts"), "m").unwrap();

        let files = collect_source_files(tmp.path());
        assert_eq!(files[0].0, "a.ts");
        assert_eq!(files[1].0, "m.ts");
        assert_eq!(files[2].0, "z.ts");
    }

    // ── JUDGE_CRITERIA Tests ──

    #[test]
    fn test_judge_criteria_count() {
        assert_eq!(JUDGE_CRITERIA.len(), 5);
    }

    #[test]
    fn test_judge_criteria_unique() {
        let mut set = std::collections::HashSet::new();
        for c in JUDGE_CRITERIA {
            assert!(set.insert(c), "Duplicate criterion: {}", c);
        }
    }

    // ── Property-Based Tests ──

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// normalize_judge_scores always produces values in [0.0, 1.0].
            #[test]
            fn normalize_bounds(
                scores in prop::collection::vec(1u8..=5, 5..=5),
                overall in 1.0f64..=5.0,
            ) {
                let criteria: Vec<JudgeCriterionScore> = JUDGE_CRITERIA
                    .iter()
                    .zip(scores.iter())
                    .map(|(name, &score)| JudgeCriterionScore {
                        criterion: name.to_string(),
                        score,
                        explanation: "test".to_string(),
                    })
                    .collect();
                let response = JudgeResponse {
                    criteria,
                    overall_score: overall,
                    failure_explanations: vec![],
                    strengths: vec![],
                };
                let normalized = normalize_judge_scores(&response);
                prop_assert!(normalized.overall >= 0.0 && normalized.overall <= 1.0);
                prop_assert!(normalized.group_coherence >= 0.0 && normalized.group_coherence <= 1.0);
                prop_assert!(normalized.review_ordering >= 0.0 && normalized.review_ordering <= 1.0);
                prop_assert!(normalized.entrypoint_identification >= 0.0 && normalized.entrypoint_identification <= 1.0);
                prop_assert!(normalized.risk_reasonableness >= 0.0 && normalized.risk_reasonableness <= 1.0);
                prop_assert!(normalized.mermaid_accuracy >= 0.0 && normalized.mermaid_accuracy <= 1.0);
            }

            /// validate_judge_response never panics on valid-looking inputs.
            #[test]
            fn validate_never_panics(
                scores in prop::collection::vec(0u8..=10, 0..=10),
                overall in -1.0f64..=10.0,
            ) {
                let criteria: Vec<JudgeCriterionScore> = scores
                    .iter()
                    .enumerate()
                    .map(|(i, &score)| JudgeCriterionScore {
                        criterion: format!("criterion_{}", i),
                        score,
                        explanation: "test".to_string(),
                    })
                    .collect();
                let response = JudgeResponse {
                    criteria,
                    overall_score: overall,
                    failure_explanations: vec![],
                    strengths: vec![],
                };
                let _ = validate_judge_response(&response);
            }

            /// build_judge_request never panics for valid AnalysisOutput.
            #[test]
            fn build_request_never_panics(fixture_name in "[a-z ]{1,30}") {
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
                let result = build_judge_request(&output, &[], "", &fixture_name);
                prop_assert!(result.is_ok());
            }

            /// Normalization is deterministic.
            #[test]
            fn normalize_deterministic(
                scores in prop::collection::vec(1u8..=5, 5..=5),
                overall in 1.0f64..=5.0,
            ) {
                let criteria: Vec<JudgeCriterionScore> = JUDGE_CRITERIA
                    .iter()
                    .zip(scores.iter())
                    .map(|(name, &score)| JudgeCriterionScore {
                        criterion: name.to_string(),
                        score,
                        explanation: "test".to_string(),
                    })
                    .collect();
                let response = JudgeResponse {
                    criteria,
                    overall_score: overall,
                    failure_explanations: vec![],
                    strengths: vec![],
                };
                let n1 = normalize_judge_scores(&response);
                let n2 = normalize_judge_scores(&response);
                prop_assert!((n1.overall - n2.overall).abs() < f64::EPSILON);
                prop_assert!((n1.group_coherence - n2.group_coherence).abs() < f64::EPSILON);
            }
        }
    }
}
