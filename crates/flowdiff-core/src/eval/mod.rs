//! Eval harness for the flowdiff analysis pipeline.
//!
//! Runs synthetic fixture codebases through the pipeline, compares against
//! known-good baselines, and produces scored reports. Supports:
//! - Per-fixture and aggregate scoring across 6 criteria
//! - Text and JSON report output
//! - Score history tracking for regression detection
//! - CI integration via minimum score thresholds

pub mod fixtures;
pub mod report;
pub mod repos;
pub mod scoring;

use fixtures::{
    build_fixture, find_feature_branch, fixture_display_name, run_pipeline, FIXTURE_NAMES,
};
use report::{
    append_history, build_html_report, build_json_report, check_regression,
    format_regression_warning, format_text_report, load_history,
};
use scoring::{score_output, EvalScores};

// ═══════════════════════════════════════════════════════════════════════════
// Config
// ═══════════════════════════════════════════════════════════════════════════

/// Configuration for an eval run.
pub struct EvalConfig {
    /// Run only this fixture (None = run all).
    pub fixture_filter: Option<String>,
    /// Minimum acceptable overall score. Exit with error if below.
    pub min_score: f64,
    /// Path to score history file (JSONL). None = don't track.
    pub history_file: Option<std::path::PathBuf>,
    /// Output format: "text" or "json".
    pub format: EvalFormat,
}

/// Output format for the eval report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalFormat {
    Text,
    Json,
    Html,
}

impl Default for EvalConfig {
    fn default() -> Self {
        Self {
            fixture_filter: None,
            min_score: 0.50,
            history_file: None,
            format: EvalFormat::Text,
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Result
// ═══════════════════════════════════════════════════════════════════════════

/// Complete result of an eval run.
pub struct EvalResult {
    /// Per-fixture results: (short_name, display_name, scores)
    pub fixture_results: Vec<(String, String, EvalScores)>,
    /// Average overall score across all fixtures.
    pub avg_overall: f64,
    /// Whether the eval passed (avg_overall >= min_score).
    pub passed: bool,
    /// Formatted report string (text or JSON).
    pub report: String,
    /// Regression warning message, if any.
    pub regression_warning: Option<String>,
}

// ═══════════════════════════════════════════════════════════════════════════
// Runner
// ═══════════════════════════════════════════════════════════════════════════

/// Run the eval suite with the given configuration.
///
/// Returns an `EvalResult` containing per-fixture scores, aggregate results,
/// and a formatted report. The caller is responsible for writing the report
/// to stdout and setting the exit code based on `passed`.
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
pub fn run_eval(config: &EvalConfig) -> EvalResult {
    let fixture_names: Vec<&str> = if let Some(ref filter) = config.fixture_filter {
        let names: Vec<&str> = FIXTURE_NAMES
            .iter()
            .copied()
            .filter(|n| *n == filter.as_str())
            .collect();
        if names.is_empty() {
            return EvalResult {
                fixture_results: vec![],
                avg_overall: 0.0,
                passed: false,
                report: format!(
                    "Unknown fixture: '{}'. Available: {}",
                    filter,
                    FIXTURE_NAMES.join(", ")
                ),
                regression_warning: None,
            };
        }
        names
    } else {
        FIXTURE_NAMES.to_vec()
    };

    let mut fixture_results: Vec<(String, String, EvalScores)> = Vec::new();

    for &name in &fixture_names {
        let (rb, baseline) = build_fixture(name).expect("fixture should be buildable");
        let branch = find_feature_branch(rb.path());
        let output = run_pipeline(rb.path(), "main", &branch);
        let scores = score_output(&output, &baseline);

        fixture_results.push((
            name.to_string(),
            fixture_display_name(name).to_string(),
            scores,
        ));
    }

    let avg_overall = if fixture_results.is_empty() {
        0.0
    } else {
        fixture_results.iter().map(|(_, _, s)| s.overall).sum::<f64>()
            / fixture_results.len() as f64
    };

    let passed = avg_overall >= config.min_score;

    let timestamp = chrono::Utc::now().to_rfc3339();

    // Build report
    let report = match config.format {
        EvalFormat::Text => format_text_report(&fixture_results, avg_overall, config.min_score),
        EvalFormat::Json => {
            let json_report =
                build_json_report(&fixture_results, avg_overall, config.min_score, &timestamp);
            serde_json::to_string_pretty(&json_report).unwrap_or_default()
        }
        EvalFormat::Html => {
            let history = config
                .history_file
                .as_ref()
                .map(|p| load_history(p))
                .unwrap_or_default();
            build_html_report(
                &fixture_results,
                avg_overall,
                config.min_score,
                &timestamp,
                &history,
            )
        }
    };

    // Check for regression
    let regression_warning = if let Some(ref history_path) = config.history_file {
        // Check regression before appending new entry
        let warning = check_regression(history_path, avg_overall, 0.05).map(|(prev, delta)| {
            format_regression_warning(prev, avg_overall, delta)
        });

        // Append to history
        if let Err(e) = append_history(
            history_path,
            &fixture_results,
            avg_overall,
            passed,
            &timestamp,
        ) {
            log::warn!("Failed to append eval history: {}", e);
        }

        warning
    } else {
        None
    };

    EvalResult {
        fixture_results,
        avg_overall,
        passed,
        report,
        regression_warning,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn test_run_eval_all_fixtures() {
        let config = EvalConfig::default();
        let result = run_eval(&config);
        assert_eq!(result.fixture_results.len(), 5);
        assert!(result.avg_overall > 0.0);
        assert!(result.passed); // Default threshold is 0.50
    }

    #[test]
    fn test_run_eval_single_fixture() {
        let config = EvalConfig {
            fixture_filter: Some("ts-express".to_string()),
            ..Default::default()
        };
        let result = run_eval(&config);
        assert_eq!(result.fixture_results.len(), 1);
        assert_eq!(result.fixture_results[0].0, "ts-express");
    }

    #[test]
    fn test_run_eval_unknown_fixture() {
        let config = EvalConfig {
            fixture_filter: Some("nonexistent".to_string()),
            ..Default::default()
        };
        let result = run_eval(&config);
        assert!(result.fixture_results.is_empty());
        assert!(!result.passed);
        assert!(result.report.contains("Unknown fixture"));
    }

    #[test]
    fn test_run_eval_json_format() {
        let config = EvalConfig {
            fixture_filter: Some("ts-express".to_string()),
            format: EvalFormat::Json,
            ..Default::default()
        };
        let result = run_eval(&config);
        // Should be valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&result.report).unwrap();
        assert!(parsed.get("avg_overall").is_some());
        assert!(parsed.get("fixtures").is_some());
    }

    #[test]
    fn test_run_eval_html_format() {
        let config = EvalConfig {
            fixture_filter: Some("ts-express".to_string()),
            format: EvalFormat::Html,
            ..Default::default()
        };
        let result = run_eval(&config);
        assert!(result.report.contains("<!DOCTYPE html>"));
        assert!(result.report.contains("flowdiff Eval Dashboard"));
        assert!(result.report.contains("TS Express API"));
    }

    #[test]
    fn test_run_eval_with_history() {
        let tmp = tempfile::TempDir::new().unwrap();
        let history_path = tmp.path().join(".flowdiff/eval-history.jsonl");

        let config = EvalConfig {
            fixture_filter: Some("rust-cli".to_string()),
            history_file: Some(history_path.clone()),
            ..Default::default()
        };
        let result = run_eval(&config);
        assert!(result.passed);

        // History file should have been created
        let history = report::load_history(&history_path);
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn test_run_eval_regression_detection() {
        let tmp = tempfile::TempDir::new().unwrap();
        let history_path = tmp.path().join("eval-history.jsonl");

        // Write a fake high-score history entry
        let fake_results = vec![(
            "ts-express".to_string(),
            "TS Express API".to_string(),
            EvalScores {
                group_coherence: 1.0,
                entrypoint_accuracy: 1.0,
                review_ordering: 1.0,
                risk_reasonableness: 1.0,
                language_detection: 1.0,
                file_accounting: 1.0,
                overall: 1.0,
            },
        )];
        report::append_history(&history_path, &fake_results, 1.0, true, "2026-03-18T12:00:00Z")
            .unwrap();

        // Run eval — score will be lower than 1.0, triggering regression
        let config = EvalConfig {
            fixture_filter: Some("ts-express".to_string()),
            history_file: Some(history_path),
            ..Default::default()
        };
        let result = run_eval(&config);
        // Regression should be detected since real score < 1.0
        assert!(result.regression_warning.is_some());
        assert!(result.regression_warning.as_ref().unwrap().contains("REGRESSION"));
    }

    #[test]
    fn test_run_eval_high_threshold_fails() {
        let config = EvalConfig {
            fixture_filter: Some("ts-express".to_string()),
            min_score: 2.0, // Impossible threshold — score can never exceed 1.0
            ..Default::default()
        };
        let result = run_eval(&config);
        assert!(!result.passed);
    }

    #[test]
    fn test_eval_config_default() {
        let config = EvalConfig::default();
        assert!(config.fixture_filter.is_none());
        assert!((config.min_score - 0.50).abs() < f64::EPSILON);
        assert!(config.history_file.is_none());
        assert_eq!(config.format, EvalFormat::Text);
    }
}
