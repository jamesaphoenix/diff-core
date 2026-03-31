#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
//! Synthetic Eval Suite (Phase 7) — validates the full diffcore pipeline against
//! known-good baselines for realistic fixture codebases.
//!
//! Uses the shared `diffcore_core::eval` module for fixture definitions,
//! scoring functions, and pipeline runner.
//!
//! Run with:
//!   cargo test --test eval_suite
//!
//! Architecture references:
//! - Evaluator-Optimizer pattern: score = f(output), never changes between runs
//! - Golden references: human-curated baselines for what "good" looks like
//! - Regression detection: minimum score thresholds catch pipeline degradation
#![allow(dead_code)]

use diffcore_core::eval::fixtures::{
    build_fixture, find_feature_branch, run_pipeline, FIXTURE_NAMES,
};
use diffcore_core::eval::report;
use diffcore_core::eval::scoring::score_output;
use diffcore_core::eval::{self, EvalConfig, EvalFormat};
use diffcore_core::output;
use diffcore_core::types::{
    AnalysisOutput, AnalysisSummary, ChangeStats, DiffSource, DiffType, FileChange, FileRole,
    FlowGroup, InfrastructureGroup,
};

// ═══════════════════════════════════════════════════════════════════════════
// Eval Tests
// ═══════════════════════════════════════════════════════════════════════════

/// Minimum acceptable overall score for any fixture.
const MIN_OVERALL_SCORE: f64 = 0.50;

// --- Individual fixture eval tests ---

#[test]
fn test_eval_ts_express_api() {
    let (rb, baseline) = build_fixture("ts-express").unwrap();
    let branch = find_feature_branch(rb.path());
    let output = run_pipeline(rb.path(), "main", &branch);

    let scores = score_output(&output, &baseline);
    let report = report::format_fixture_report(&baseline.name, &scores);
    eprintln!("{}", report);

    assert!(
        scores.overall >= MIN_OVERALL_SCORE,
        "[{}] overall {:.2} < {:.2}",
        baseline.name,
        scores.overall,
        MIN_OVERALL_SCORE,
    );
    assert!(scores.language_detection >= 1.0, "Should detect TypeScript");
    assert!(scores.risk_reasonableness >= 0.5);
    assert!(scores.file_accounting >= 0.25);
}

#[test]
fn test_eval_python_fastapi() {
    let (rb, baseline) = build_fixture("python-fastapi").unwrap();
    let branch = find_feature_branch(rb.path());
    let output = run_pipeline(rb.path(), "main", &branch);

    let scores = score_output(&output, &baseline);
    let report = report::format_fixture_report(&baseline.name, &scores);
    eprintln!("{}", report);

    assert!(
        scores.overall >= MIN_OVERALL_SCORE,
        "[{}] overall {:.2} < {:.2}",
        baseline.name,
        scores.overall,
        MIN_OVERALL_SCORE,
    );
    assert!(scores.language_detection >= 1.0, "Should detect Python");
    assert!(scores.risk_reasonableness >= 0.5);
}

#[test]
fn test_eval_nextjs_fullstack() {
    let (rb, baseline) = build_fixture("nextjs-fullstack").unwrap();
    let branch = find_feature_branch(rb.path());
    let output = run_pipeline(rb.path(), "main", &branch);

    let scores = score_output(&output, &baseline);
    let report = report::format_fixture_report(&baseline.name, &scores);
    eprintln!("{}", report);

    assert!(
        scores.overall >= MIN_OVERALL_SCORE,
        "[{}] overall {:.2} < {:.2}",
        baseline.name,
        scores.overall,
        MIN_OVERALL_SCORE,
    );
    assert!(scores.language_detection >= 1.0, "Should detect TypeScript");
    assert!(scores.risk_reasonableness >= 0.5);
}

#[test]
fn test_eval_rust_cli() {
    let (rb, baseline) = build_fixture("rust-cli").unwrap();
    let branch = find_feature_branch(rb.path());
    let output = run_pipeline(rb.path(), "main", &branch);

    let scores = score_output(&output, &baseline);
    let report = report::format_fixture_report(&baseline.name, &scores);
    eprintln!("{}", report);

    // Rust has no tree-sitter grammar in deps, so scores are naturally lower
    assert!(
        scores.overall >= 0.30,
        "[{}] overall {:.2} < 0.30",
        baseline.name,
        scores.overall
    );
    assert!(scores.risk_reasonableness >= 0.5);
}

#[test]
fn test_eval_multi_language_monorepo() {
    let (rb, baseline) = build_fixture("multi-language").unwrap();
    let branch = find_feature_branch(rb.path());
    let output = run_pipeline(rb.path(), "main", &branch);

    let scores = score_output(&output, &baseline);
    let report = report::format_fixture_report(&baseline.name, &scores);
    eprintln!("{}", report);

    assert!(
        scores.overall >= MIN_OVERALL_SCORE,
        "[{}] overall {:.2} < {:.2}",
        baseline.name,
        scores.overall,
        MIN_OVERALL_SCORE,
    );
    assert!(
        scores.language_detection >= 1.0,
        "Should detect both TS and Python"
    );
    assert!(scores.risk_reasonableness >= 0.5);
}

// --- Cross-fixture consistency tests ---

/// All fixtures should produce deterministic results.
#[test]
fn test_eval_all_fixtures_deterministic() {
    let fixture_names = &["ts-express", "python-fastapi", "nextjs-fullstack"];

    for &name in fixture_names {
        let (rb, baseline) = build_fixture(name).unwrap();
        let branch = find_feature_branch(rb.path());

        let output1 = run_pipeline(rb.path(), "main", &branch);
        let output2 = run_pipeline(rb.path(), "main", &branch);

        let json1 = output::to_json(&output1).unwrap();
        let json2 = output::to_json(&output2).unwrap();

        assert_eq!(
            json1, json2,
            "[{}] pipeline output not deterministic",
            baseline.name,
        );
    }
}

/// All fixtures should produce valid JSON that roundtrips cleanly.
#[test]
fn test_eval_all_fixtures_json_roundtrip() {
    for &name in FIXTURE_NAMES {
        let (rb, baseline) = build_fixture(name).unwrap();
        let branch = find_feature_branch(rb.path());
        let output = run_pipeline(rb.path(), "main", &branch);

        let json = output::to_json(&output).unwrap();
        let parsed: AnalysisOutput = serde_json::from_str(&json).unwrap();
        let json2 = output::to_json(&parsed).unwrap();

        assert_eq!(json, json2, "[{}] JSON roundtrip not stable", baseline.name);
    }
}

/// Every fixture's output must account for all files (no files lost).
#[test]
fn test_eval_all_fixtures_file_accounting() {
    for &name in FIXTURE_NAMES {
        let (rb, baseline) = build_fixture(name).unwrap();
        let branch = find_feature_branch(rb.path());
        let output = run_pipeline(rb.path(), "main", &branch);

        let total_grouped: usize = output.groups.iter().map(|g| g.files.len()).sum();
        let infra: usize = output
            .infrastructure_group
            .as_ref()
            .map(|i| i.files.len())
            .unwrap_or(0);

        assert_eq!(
            total_grouped + infra,
            output.summary.total_files_changed as usize,
            "[{}] file accounting: grouped({}) + infra({}) != total({})",
            baseline.name,
            total_grouped,
            infra,
            output.summary.total_files_changed,
        );
    }
}

/// All risk scores must be in [0.0, 1.0].
#[test]
fn test_eval_all_fixtures_risk_bounds() {
    for &name in FIXTURE_NAMES {
        let (rb, baseline) = build_fixture(name).unwrap();
        let branch = find_feature_branch(rb.path());
        let output = run_pipeline(rb.path(), "main", &branch);

        for group in &output.groups {
            assert!(
                group.risk_score >= 0.0 && group.risk_score <= 1.0,
                "[{}] group '{}' risk_score {} out of bounds",
                baseline.name,
                group.name,
                group.risk_score,
            );
            assert!(
                group.review_order >= 1,
                "[{}] group '{}' review_order {} < 1",
                baseline.name,
                group.name,
                group.review_order,
            );
        }
    }
}

/// Mermaid diagrams should be generated for all groups across all fixtures.
#[test]
fn test_eval_all_fixtures_mermaid() {
    let fixture_names = &[
        "ts-express",
        "python-fastapi",
        "nextjs-fullstack",
        "multi-language",
    ];

    for &name in fixture_names {
        let (rb, baseline) = build_fixture(name).unwrap();
        let branch = find_feature_branch(rb.path());
        let output = run_pipeline(rb.path(), "main", &branch);

        for group in &output.groups {
            let mermaid = output::generate_mermaid(group);
            assert!(
                mermaid.starts_with("graph TD"),
                "[{}] group '{}' Mermaid should start with 'graph TD', got: {}",
                baseline.name,
                group.name,
                &mermaid[..mermaid.len().min(50)],
            );
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Eval Harness Integration Tests
// ═══════════════════════════════════════════════════════════════════════════

/// Test that run_eval produces valid results for all fixtures.
#[test]
fn test_eval_harness_all_fixtures() {
    let result = eval::run_eval(&EvalConfig::default());
    assert_eq!(result.fixture_results.len(), 5);
    assert!(result.passed);
    assert!(result.avg_overall >= 0.50);
}

/// Test eval harness with JSON output format.
#[test]
fn test_eval_harness_json_output() {
    let config = EvalConfig {
        fixture_filter: Some("ts-express".to_string()),
        format: EvalFormat::Json,
        ..Default::default()
    };
    let result = eval::run_eval(&config);
    let parsed: serde_json::Value = serde_json::from_str(&result.report).unwrap();
    assert!(parsed.get("fixtures").is_some());
    assert!(parsed.get("passed").is_some());
}

/// Test eval harness with score history tracking.
#[test]
fn test_eval_harness_history_tracking() {
    let tmp = tempfile::TempDir::new().unwrap();
    let history_path = tmp.path().join("eval-history.jsonl");

    let config = EvalConfig {
        fixture_filter: Some("rust-cli".to_string()),
        history_file: Some(history_path.clone()),
        ..Default::default()
    };

    // Run eval twice
    let _result1 = eval::run_eval(&config);
    let _result2 = eval::run_eval(&config);

    let history = report::load_history(&history_path);
    assert_eq!(history.len(), 2);
}

// ═══════════════════════════════════════════════════════════════════════════
// Property-Based Tests for Scoring Functions
// ═══════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod scoring_properties {
    use super::*;
    use diffcore_core::eval::scoring;
    use proptest::prelude::*;

    fn arb_analysis_output() -> impl Strategy<Value = AnalysisOutput> {
        let arb_file_change = (
            "[a-z]{1,5}/[a-z]{1,5}\\.[a-z]{2,3}",
            0u32..10,
            0u32..100,
            0u32..100,
        )
            .prop_map(|(path, pos, adds, dels)| FileChange {
                path,
                flow_position: pos,
                role: FileRole::Utility,
                changes: ChangeStats {
                    additions: adds,
                    deletions: dels,
                },
                symbols_changed: vec![],
            });

        let arb_group = (
            prop::collection::vec(arb_file_change.clone(), 1..6),
            prop::num::f64::POSITIVE | prop::num::f64::ZERO,
            1u32..20,
        )
            .prop_map(|(files, risk_raw, order)| FlowGroup {
                id: format!("group_{}", order),
                name: format!("Group {}", order),
                entrypoint: None,
                files,
                edges: vec![],
                risk_score: risk_raw.min(1.0),
                review_order: order,
            });

        (prop::collection::vec(arb_group, 0..5), 0u32..50).prop_map(|(groups, extra_files)| {
            let total_in_groups: u32 = groups.iter().map(|g| g.files.len() as u32).sum();
            let total = total_in_groups + extra_files;
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
                    total_files_changed: total,
                    total_groups: groups.len() as u32,
                    languages_detected: vec!["typescript".to_string()],
                    frameworks_detected: vec![],
                },
                groups,
                infrastructure_group: if extra_files > 0 {
                    Some(InfrastructureGroup {
                        files: (0..extra_files)
                            .map(|i| format!("infra_{}.ts", i))
                            .collect(),
                        sub_groups: vec![],
                        reason: "Not reachable".to_string(),
                    })
                } else {
                    None
                },
                annotations: None,
            }
        })
    }

    fn arb_baseline() -> impl Strategy<Value = scoring::EvalBaseline> {
        Just(scoring::EvalBaseline {
            name: "test".to_string(),
            expected_languages: vec!["typescript".to_string()],
            min_groups: 0,
            max_groups: 10,
            expected_file_count: 5,
            expected_entrypoints: vec![],
            expected_groups: vec![],
            risk_ordering: vec![],
            expected_infrastructure: vec![],
        })
    }

    proptest! {
        /// All scoring functions must return values in [0.0, 1.0].
        #[test]
        fn score_bounds(output in arb_analysis_output(), baseline in arb_baseline()) {
            let scores = scoring::score_output(&output, &baseline);
            prop_assert!(scores.group_coherence >= 0.0 && scores.group_coherence <= 1.0,
                "group_coherence out of bounds: {}", scores.group_coherence);
            prop_assert!(scores.entrypoint_accuracy >= 0.0 && scores.entrypoint_accuracy <= 1.0,
                "entrypoint_accuracy out of bounds: {}", scores.entrypoint_accuracy);
            prop_assert!(scores.review_ordering >= 0.0 && scores.review_ordering <= 1.0,
                "review_ordering out of bounds: {}", scores.review_ordering);
            prop_assert!(scores.risk_reasonableness >= 0.0 && scores.risk_reasonableness <= 1.0,
                "risk_reasonableness out of bounds: {}", scores.risk_reasonableness);
            prop_assert!(scores.language_detection >= 0.0 && scores.language_detection <= 1.0,
                "language_detection out of bounds: {}", scores.language_detection);
            prop_assert!(scores.file_accounting >= 0.0 && scores.file_accounting <= 1.0,
                "file_accounting out of bounds: {}", scores.file_accounting);
            prop_assert!(scores.overall >= 0.0 && scores.overall <= 1.0,
                "overall out of bounds: {}", scores.overall);
        }

        /// Overall score is a weighted average, so it should be between min and max individual scores.
        #[test]
        fn overall_between_min_max(output in arb_analysis_output(), baseline in arb_baseline()) {
            let scores = scoring::score_output(&output, &baseline);
            let all = vec![
                scores.group_coherence,
                scores.entrypoint_accuracy,
                scores.review_ordering,
                scores.risk_reasonableness,
                scores.language_detection,
                scores.file_accounting,
            ];
            let min_score = all.iter().cloned().fold(f64::INFINITY, f64::min);
            let max_score = all.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            prop_assert!(scores.overall >= min_score - 0.01,
                "overall {} < min individual {}", scores.overall, min_score);
            prop_assert!(scores.overall <= max_score + 0.01,
                "overall {} > max individual {}", scores.overall, max_score);
        }

        /// Empty output (no groups, no files) should not cause panics.
        #[test]
        fn empty_output_no_panic(baseline in arb_baseline()) {
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
            let scores = scoring::score_output(&output, &baseline);
            prop_assert!(scores.overall >= 0.0 && scores.overall <= 1.0);
        }

        /// Scoring the same output twice must give the same result (determinism).
        #[test]
        fn scoring_deterministic(output in arb_analysis_output(), baseline in arb_baseline()) {
            let scores1 = scoring::score_output(&output, &baseline);
            let scores2 = scoring::score_output(&output, &baseline);
            prop_assert!((scores1.overall - scores2.overall).abs() < f64::EPSILON,
                "scoring not deterministic: {} vs {}", scores1.overall, scores2.overall);
        }

        /// Perfect baseline match should score >= 0.8.
        #[test]
        fn perfect_match_high_score(n_groups in 1usize..4) {
            let groups: Vec<FlowGroup> = (0..n_groups).map(|i| {
                FlowGroup {
                    id: format!("group_{}", i),
                    name: format!("Group {}", i),
                    entrypoint: None,
                    files: vec![FileChange {
                        path: format!("file_{}.ts", i),
                        flow_position: 0,
                        role: FileRole::Utility,
                        changes: ChangeStats { additions: 10, deletions: 5 },
                        symbols_changed: vec![],
                    }],
                    edges: vec![],
                    risk_score: 0.5,
                    review_order: (i + 1) as u32,
                }
            }).collect();

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
                    total_files_changed: n_groups as u32,
                    total_groups: n_groups as u32,
                    languages_detected: vec!["typescript".to_string()],
                    frameworks_detected: vec![],
                },
                groups,
                infrastructure_group: None,
                annotations: None,
            };

            let baseline = scoring::EvalBaseline {
                name: "perfect".to_string(),
                expected_languages: vec!["typescript".to_string()],
                min_groups: 1,
                max_groups: n_groups + 2,
                expected_file_count: n_groups,
                expected_entrypoints: vec![],
                expected_groups: vec![],
                risk_ordering: vec![],
                expected_infrastructure: vec![],
            };

            let scores = scoring::score_output(&output, &baseline);
            prop_assert!(scores.overall >= 0.8,
                "perfect match scored only {:.2}", scores.overall);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Aggregate Eval Report
// ═══════════════════════════════════════════════════════════════════════════

/// Run all fixture evals and print an aggregate report.
/// This test always passes — it's for observing score trends.
#[test]
fn test_eval_aggregate_report() {
    let result = eval::run_eval(&EvalConfig::default());
    let report_text = report::format_text_report(&result.fixture_results, result.avg_overall, 0.50);
    eprintln!("{}", report_text);
}
