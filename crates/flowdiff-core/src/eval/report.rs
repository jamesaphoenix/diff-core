//! Eval report formatting (text + JSON) and score history tracking.
//!
//! Produces formatted reports for CLI output and JSON for CI integration.
//! Tracks eval scores over time in a JSON Lines file for regression detection.

use std::fmt::Write as FmtWrite;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::scoring::EvalScores;

// ═══════════════════════════════════════════════════════════════════════════
// Report Types
// ═══════════════════════════════════════════════════════════════════════════

/// Result for a single fixture in the eval report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureReport {
    pub name: String,
    pub display_name: String,
    pub scores: EvalScores,
}

/// Complete eval report (JSON-serializable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalJsonReport {
    pub timestamp: String,
    pub min_score: f64,
    pub avg_overall: f64,
    pub passed: bool,
    pub fixture_count: usize,
    pub fixtures: Vec<FixtureReport>,
}

// ═══════════════════════════════════════════════════════════════════════════
// History Types
// ═══════════════════════════════════════════════════════════════════════════

/// A single entry in the score history file (JSON Lines format).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreHistoryEntry {
    pub timestamp: String,
    pub avg_overall: f64,
    pub passed: bool,
    pub fixture_count: usize,
    pub fixtures: Vec<FixtureHistoryEntry>,
}

/// Per-fixture entry in the score history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureHistoryEntry {
    pub name: String,
    pub overall: f64,
}

// ═══════════════════════════════════════════════════════════════════════════
// Text Report Formatting
// ═══════════════════════════════════════════════════════════════════════════

/// Format the eval report as a human-readable text table.
///
/// Returns the formatted string. Callers decide how to display it.
pub fn format_text_report(
    fixture_results: &[(String, String, EvalScores)],
    avg_overall: f64,
    min_score: f64,
) -> String {
    let mut buf = String::new();

    let _ = writeln!(buf);
    let _ = writeln!(buf, "\u{2554}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2557}");
    let _ = writeln!(buf, "\u{2551}                    EVAL SUITE AGGREGATE REPORT                   \u{2551}");
    let _ = writeln!(buf, "\u{2560}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2563}");
    let _ = writeln!(
        buf,
        "\u{2551} {:.<25} {:>6} {:>6} {:>6} {:>6} {:>6} {:>7} \u{2551}",
        "Fixture", "GrpCo", "EntPt", "Order", "Risk", "Lang", "TOTAL"
    );
    let _ = writeln!(buf, "\u{2560}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2563}");

    for (_, display_name, scores) in fixture_results {
        let _ = writeln!(
            buf,
            "\u{2551} {:.<25} {:>5.2} {:>6.2} {:>6.2} {:>6.2} {:>6.2} {:>7.2} \u{2551}",
            display_name,
            scores.group_coherence,
            scores.entrypoint_accuracy,
            scores.review_ordering,
            scores.risk_reasonableness,
            scores.language_detection,
            scores.overall,
        );
    }

    let _ = writeln!(buf, "\u{2560}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2563}");
    let _ = writeln!(
        buf,
        "\u{2551} {:.<25} {:>39.2} \u{2551}",
        "AVERAGE", avg_overall
    );
    let _ = writeln!(buf, "\u{255a}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{255d}");

    let passed = avg_overall >= min_score;
    let _ = writeln!(buf);
    let _ = writeln!(
        buf,
        "Overall: {:.2} {} {:.2} (threshold) -- {}",
        avg_overall,
        if passed { ">=" } else { "<" },
        min_score,
        if passed { "PASS" } else { "FAIL" },
    );

    buf
}

/// Format a single fixture's eval report as a box.
pub fn format_fixture_report(fixture_name: &str, scores: &EvalScores) -> String {
    let mut buf = String::new();

    let _ = writeln!(buf, "\n\u{2554}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2557}");
    let _ = writeln!(buf, "\u{2551}  Eval: {:<33}\u{2551}", fixture_name);
    let _ = writeln!(buf, "\u{2560}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2563}");
    let _ = writeln!(buf, "\u{2551}  Group coherence:    {:.2}                 \u{2551}", scores.group_coherence);
    let _ = writeln!(buf, "\u{2551}  Entrypoint accuracy:{:.2}                 \u{2551}", scores.entrypoint_accuracy);
    let _ = writeln!(buf, "\u{2551}  Review ordering:    {:.2}                 \u{2551}", scores.review_ordering);
    let _ = writeln!(buf, "\u{2551}  Risk reasonableness:{:.2}                 \u{2551}", scores.risk_reasonableness);
    let _ = writeln!(buf, "\u{2551}  Language detection:  {:.2}                \u{2551}", scores.language_detection);
    let _ = writeln!(buf, "\u{2551}  File accounting:    {:.2}                 \u{2551}", scores.file_accounting);
    let _ = writeln!(buf, "\u{2560}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2563}");
    let _ = writeln!(buf, "\u{2551}  OVERALL:            {:.2}                 \u{2551}", scores.overall);
    let _ = writeln!(buf, "\u{255a}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{255d}");

    buf
}

// ═══════════════════════════════════════════════════════════════════════════
// JSON Report
// ═══════════════════════════════════════════════════════════════════════════

/// Build a JSON-serializable eval report.
pub fn build_json_report(
    fixture_results: &[(String, String, EvalScores)],
    avg_overall: f64,
    min_score: f64,
    timestamp: &str,
) -> EvalJsonReport {
    let fixtures: Vec<FixtureReport> = fixture_results
        .iter()
        .map(|(name, display_name, scores)| FixtureReport {
            name: name.clone(),
            display_name: display_name.clone(),
            scores: scores.clone(),
        })
        .collect();

    EvalJsonReport {
        timestamp: timestamp.to_string(),
        min_score,
        avg_overall,
        passed: avg_overall >= min_score,
        fixture_count: fixtures.len(),
        fixtures,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Score History
// ═══════════════════════════════════════════════════════════════════════════

/// Append a score history entry to the JSONL history file.
///
/// Creates the file and parent directories if they don't exist.
pub fn append_history(
    history_path: &Path,
    fixture_results: &[(String, String, EvalScores)],
    avg_overall: f64,
    passed: bool,
    timestamp: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let entry = ScoreHistoryEntry {
        timestamp: timestamp.to_string(),
        avg_overall,
        passed,
        fixture_count: fixture_results.len(),
        fixtures: fixture_results
            .iter()
            .map(|(name, _, scores)| FixtureHistoryEntry {
                name: name.clone(),
                overall: scores.overall,
            })
            .collect(),
    };

    if let Some(parent) = history_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(history_path)?;
    let json = serde_json::to_string(&entry)?;
    writeln!(file, "{}", json)?;

    Ok(())
}

/// Load score history entries from the JSONL history file.
///
/// Returns an empty vec if the file doesn't exist or is empty.
pub fn load_history(history_path: &Path) -> Vec<ScoreHistoryEntry> {
    let content = match std::fs::read_to_string(history_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Check for regression against the last history entry.
///
/// Returns `Some((previous_score, delta))` if there is a regression,
/// `None` if no regression or no history.
pub fn check_regression(
    history_path: &Path,
    current_avg: f64,
    regression_threshold: f64,
) -> Option<(f64, f64)> {
    let history = load_history(history_path);
    let last = history.last()?;
    let delta = current_avg - last.avg_overall;
    if delta < -regression_threshold {
        Some((last.avg_overall, delta))
    } else {
        None
    }
}

/// Format a regression warning message.
pub fn format_regression_warning(previous: f64, current: f64, delta: f64) -> String {
    format!(
        "REGRESSION DETECTED: score dropped {:.2} -> {:.2} (delta: {:.2})",
        previous, current, delta
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn sample_results() -> Vec<(String, String, EvalScores)> {
        vec![
            (
                "ts-express".to_string(),
                "TS Express API".to_string(),
                EvalScores {
                    group_coherence: 0.85,
                    entrypoint_accuracy: 1.0,
                    review_ordering: 0.75,
                    risk_reasonableness: 1.0,
                    language_detection: 1.0,
                    file_accounting: 0.75,
                    overall: 0.89,
                },
            ),
            (
                "python-fastapi".to_string(),
                "Python FastAPI".to_string(),
                EvalScores {
                    group_coherence: 0.80,
                    entrypoint_accuracy: 1.0,
                    review_ordering: 1.0,
                    risk_reasonableness: 1.0,
                    language_detection: 1.0,
                    file_accounting: 0.75,
                    overall: 0.91,
                },
            ),
        ]
    }

    #[test]
    fn test_format_text_report_contains_fixture_names() {
        let results = sample_results();
        let report = format_text_report(&results, 0.90, 0.50);
        assert!(report.contains("TS Express API"));
        assert!(report.contains("Python FastAPI"));
        assert!(report.contains("PASS"));
    }

    #[test]
    fn test_format_text_report_fail() {
        let results = sample_results();
        let report = format_text_report(&results, 0.10, 0.50);
        assert!(report.contains("FAIL"));
    }

    #[test]
    fn test_build_json_report() {
        let results = sample_results();
        let report = build_json_report(&results, 0.90, 0.50, "2026-03-19T12:00:00Z");
        assert!(report.passed);
        assert_eq!(report.fixture_count, 2);
        assert_eq!(report.fixtures.len(), 2);
        assert_eq!(report.fixtures[0].name, "ts-express");
    }

    #[test]
    fn test_json_report_serde_roundtrip() {
        let results = sample_results();
        let report = build_json_report(&results, 0.90, 0.50, "2026-03-19T12:00:00Z");
        let json = serde_json::to_string(&report).unwrap();
        let parsed: EvalJsonReport = serde_json::from_str(&json).unwrap();
        assert!((parsed.avg_overall - report.avg_overall).abs() < f64::EPSILON);
        assert_eq!(parsed.fixtures.len(), report.fixtures.len());
    }

    #[test]
    fn test_append_and_load_history() {
        let tmp = tempfile::TempDir::new().unwrap();
        let history_path = tmp.path().join("eval-history.jsonl");

        let results = sample_results();
        append_history(&history_path, &results, 0.90, true, "2026-03-19T12:00:00Z").unwrap();

        let history = load_history(&history_path);
        assert_eq!(history.len(), 1);
        assert!((history[0].avg_overall - 0.90).abs() < f64::EPSILON);
        assert_eq!(history[0].fixtures.len(), 2);
    }

    #[test]
    fn test_append_history_multiple() {
        let tmp = tempfile::TempDir::new().unwrap();
        let history_path = tmp.path().join("eval-history.jsonl");

        let results = sample_results();
        append_history(&history_path, &results, 0.90, true, "2026-03-19T12:00:00Z").unwrap();
        append_history(&history_path, &results, 0.85, true, "2026-03-19T13:00:00Z").unwrap();

        let history = load_history(&history_path);
        assert_eq!(history.len(), 2);
        assert!((history[1].avg_overall - 0.85).abs() < f64::EPSILON);
    }

    #[test]
    fn test_load_history_missing_file() {
        let history = load_history(Path::new("/nonexistent/path/history.jsonl"));
        assert!(history.is_empty());
    }

    #[test]
    fn test_check_regression_detected() {
        let tmp = tempfile::TempDir::new().unwrap();
        let history_path = tmp.path().join("eval-history.jsonl");

        let results = sample_results();
        append_history(&history_path, &results, 0.90, true, "2026-03-19T12:00:00Z").unwrap();

        let regression = check_regression(&history_path, 0.70, 0.05);
        assert!(regression.is_some());
        let (prev, delta) = regression.unwrap();
        assert!((prev - 0.90).abs() < f64::EPSILON);
        assert!(delta < 0.0);
    }

    #[test]
    fn test_check_regression_none() {
        let tmp = tempfile::TempDir::new().unwrap();
        let history_path = tmp.path().join("eval-history.jsonl");

        let results = sample_results();
        append_history(&history_path, &results, 0.90, true, "2026-03-19T12:00:00Z").unwrap();

        let regression = check_regression(&history_path, 0.89, 0.05);
        assert!(regression.is_none());
    }

    #[test]
    fn test_check_regression_no_history() {
        let regression = check_regression(Path::new("/nonexistent/path"), 0.90, 0.05);
        assert!(regression.is_none());
    }

    #[test]
    fn test_format_regression_warning() {
        let msg = format_regression_warning(0.90, 0.70, -0.20);
        assert!(msg.contains("REGRESSION DETECTED"));
        assert!(msg.contains("0.90"));
        assert!(msg.contains("0.70"));
    }

    #[test]
    fn test_history_entry_serde_roundtrip() {
        let entry = ScoreHistoryEntry {
            timestamp: "2026-03-19T12:00:00Z".to_string(),
            avg_overall: 0.89,
            passed: true,
            fixture_count: 5,
            fixtures: vec![
                FixtureHistoryEntry {
                    name: "ts-express".to_string(),
                    overall: 0.89,
                },
            ],
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: ScoreHistoryEntry = serde_json::from_str(&json).unwrap();
        assert!((parsed.avg_overall - 0.89).abs() < f64::EPSILON);
    }

    #[test]
    fn test_format_fixture_report() {
        let scores = EvalScores {
            group_coherence: 0.85,
            entrypoint_accuracy: 1.0,
            review_ordering: 0.75,
            risk_reasonableness: 1.0,
            language_detection: 1.0,
            file_accounting: 0.75,
            overall: 0.89,
        };
        let report = format_fixture_report("TS Express API", &scores);
        assert!(report.contains("TS Express API"));
        assert!(report.contains("0.89"));
    }
}
