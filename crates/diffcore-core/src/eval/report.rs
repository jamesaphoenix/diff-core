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
    let _ = writeln!(
        buf,
        "\u{2551}                    EVAL SUITE AGGREGATE REPORT                   \u{2551}"
    );
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
    let _ = writeln!(
        buf,
        "\u{2551}  Group coherence:    {:.2}                 \u{2551}",
        scores.group_coherence
    );
    let _ = writeln!(
        buf,
        "\u{2551}  Entrypoint accuracy:{:.2}                 \u{2551}",
        scores.entrypoint_accuracy
    );
    let _ = writeln!(
        buf,
        "\u{2551}  Review ordering:    {:.2}                 \u{2551}",
        scores.review_ordering
    );
    let _ = writeln!(
        buf,
        "\u{2551}  Risk reasonableness:{:.2}                 \u{2551}",
        scores.risk_reasonableness
    );
    let _ = writeln!(
        buf,
        "\u{2551}  Language detection:  {:.2}                \u{2551}",
        scores.language_detection
    );
    let _ = writeln!(
        buf,
        "\u{2551}  File accounting:    {:.2}                 \u{2551}",
        scores.file_accounting
    );
    let _ = writeln!(buf, "\u{2560}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2550}\u{2563}");
    let _ = writeln!(
        buf,
        "\u{2551}  OVERALL:            {:.2}                 \u{2551}",
        scores.overall
    );
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

// ═══════════════════════════════════════════════════════════════════════════
// HTML Dashboard Report
// ═══════════════════════════════════════════════════════════════════════════

/// Escape a string for safe inclusion in HTML content.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// Build a self-contained HTML dashboard report.
///
/// The report includes:
/// - Summary header with pass/fail status and average score
/// - Per-fixture scores table with all 6 criteria
/// - Per-criterion bar chart (inline SVG)
/// - Historical trend chart (inline SVG, if history provided)
/// - Diff against last run with delta indicators
pub fn build_html_report(
    fixture_results: &[(String, String, EvalScores)],
    avg_overall: f64,
    min_score: f64,
    timestamp: &str,
    history: &[ScoreHistoryEntry],
) -> String {
    let passed = avg_overall >= min_score;
    let mut buf = String::new();

    // ── HTML head ──────────────────────────────────────────────────────────
    let _ = write!(
        buf,
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>diffcore Eval Dashboard</title>
<style>
:root {{
  --bg: #1e1e2e;
  --surface: #313244;
  --overlay: #45475a;
  --text: #cdd6f4;
  --subtext: #a6adc8;
  --green: #a6e3a1;
  --red: #f38ba8;
  --peach: #fab387;
  --blue: #89b4fa;
  --lavender: #b4befe;
  --yellow: #f9e2af;
  --teal: #94e2d5;
  --sky: #89dceb;
}}
*{{ margin:0; padding:0; box-sizing:border-box; }}
body {{
  font-family: 'SF Mono', 'Fira Code', 'JetBrains Mono', monospace;
  background: var(--bg);
  color: var(--text);
  padding: 2rem;
  max-width: 1200px;
  margin: 0 auto;
}}
h1 {{ font-size: 1.5rem; margin-bottom: 0.25rem; }}
h2 {{
  font-size: 1.1rem;
  color: var(--lavender);
  margin: 2rem 0 0.75rem;
  border-bottom: 1px solid var(--overlay);
  padding-bottom: 0.5rem;
}}
.header {{
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 1.5rem;
  padding-bottom: 1rem;
  border-bottom: 2px solid var(--overlay);
}}
.header-left h1 {{ color: var(--lavender); }}
.header-left .timestamp {{ color: var(--subtext); font-size: 0.8rem; }}
.badge {{
  display: inline-block;
  padding: 0.35rem 1rem;
  border-radius: 6px;
  font-weight: bold;
  font-size: 1rem;
}}
.badge-pass {{ background: var(--green); color: #1e1e2e; }}
.badge-fail {{ background: var(--red); color: #1e1e2e; }}
.score-big {{
  font-size: 2.5rem;
  font-weight: bold;
  text-align: center;
  margin: 0.5rem 0;
}}
.summary-cards {{
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
  gap: 1rem;
  margin-bottom: 1.5rem;
}}
.card {{
  background: var(--surface);
  border-radius: 8px;
  padding: 1rem;
  text-align: center;
}}
.card .label {{ color: var(--subtext); font-size: 0.75rem; text-transform: uppercase; letter-spacing: 0.05em; }}
.card .value {{ font-size: 1.5rem; font-weight: bold; margin-top: 0.25rem; }}
table {{
  width: 100%;
  border-collapse: collapse;
  margin-bottom: 1rem;
}}
th, td {{
  padding: 0.5rem 0.75rem;
  text-align: right;
  border-bottom: 1px solid var(--overlay);
}}
th {{ color: var(--subtext); font-size: 0.75rem; text-transform: uppercase; letter-spacing: 0.05em; }}
td:first-child, th:first-child {{ text-align: left; }}
tr:hover td {{ background: var(--surface); }}
.bar-container {{
  display: flex;
  align-items: center;
  gap: 0.5rem;
}}
.bar-track {{
  flex: 1;
  height: 16px;
  background: var(--overlay);
  border-radius: 4px;
  overflow: hidden;
}}
.bar-fill {{
  height: 100%;
  border-radius: 4px;
  transition: width 0.3s;
}}
.bar-label {{
  min-width: 3rem;
  text-align: right;
  font-size: 0.85rem;
}}
.delta {{ font-size: 0.8rem; margin-left: 0.5rem; }}
.delta-pos {{ color: var(--green); }}
.delta-neg {{ color: var(--red); }}
.delta-zero {{ color: var(--subtext); }}
svg {{ display: block; }}
.chart-container {{
  background: var(--surface);
  border-radius: 8px;
  padding: 1.5rem;
  margin-bottom: 1.5rem;
  overflow-x: auto;
}}
.criterion-grid {{
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(300px, 1fr));
  gap: 1rem;
}}
.criterion-card {{
  background: var(--surface);
  border-radius: 8px;
  padding: 1rem;
}}
.criterion-card .name {{ color: var(--subtext); font-size: 0.75rem; text-transform: uppercase; margin-bottom: 0.5rem; }}
.no-data {{ color: var(--subtext); font-style: italic; padding: 1rem; }}
</style>
</head>
<body>
"#
    );

    // ── Header ─────────────────────────────────────────────────────────────
    let _ = write!(
        buf,
        r#"<div class="header">
<div class="header-left">
<h1>diffcore Eval Dashboard</h1>
<div class="timestamp">{}</div>
</div>
<span class="badge {}">{}</span>
</div>
"#,
        html_escape(timestamp),
        if passed { "badge-pass" } else { "badge-fail" },
        if passed { "PASS" } else { "FAIL" },
    );

    // ── Summary cards ──────────────────────────────────────────────────────
    let _ = write!(
        buf,
        r#"<div class="summary-cards">
<div class="card">
<div class="label">Average Score</div>
<div class="value" style="color: {}">{:.2}</div>
</div>
<div class="card">
<div class="label">Threshold</div>
<div class="value">{:.2}</div>
</div>
<div class="card">
<div class="label">Fixtures</div>
<div class="value">{}</div>
</div>
<div class="card">
<div class="label">History Runs</div>
<div class="value">{}</div>
</div>
</div>
"#,
        score_color(avg_overall),
        avg_overall,
        min_score,
        fixture_results.len(),
        history.len(),
    );

    // ── Diff against last run ──────────────────────────────────────────────
    if let Some(last) = history.last() {
        let _ = write!(buf, "<h2>Diff vs Last Run</h2>");
        let _ = write!(
            buf,
            r#"<table>
<thead><tr>
<th>Fixture</th><th>Previous</th><th>Current</th><th>Delta</th>
</tr></thead><tbody>
"#
        );

        for (name, display_name, scores) in fixture_results {
            let prev_score = last
                .fixtures
                .iter()
                .find(|f| f.name == *name)
                .map(|f| f.overall);
            let delta_html = match prev_score {
                Some(prev) => {
                    let delta = scores.overall - prev;
                    let (class, sign) = if delta > 0.005 {
                        ("delta-pos", "+")
                    } else if delta < -0.005 {
                        ("delta-neg", "")
                    } else {
                        ("delta-zero", "")
                    };
                    format!(
                        r#"<td>{:.2}</td><td>{:.2}</td><td><span class="delta {}">{}{:.2}</span></td>"#,
                        prev, scores.overall, class, sign, delta
                    )
                }
                None => format!(
                    r#"<td>--</td><td>{:.2}</td><td><span class="delta delta-zero">new</span></td>"#,
                    scores.overall
                ),
            };
            let _ = write!(
                buf,
                "<tr><td>{}</td>{}</tr>\n",
                html_escape(display_name),
                delta_html
            );
        }

        // Average row
        let avg_delta = avg_overall - last.avg_overall;
        let (avg_class, avg_sign) = if avg_delta > 0.005 {
            ("delta-pos", "+")
        } else if avg_delta < -0.005 {
            ("delta-neg", "")
        } else {
            ("delta-zero", "")
        };
        let _ = write!(
            buf,
            r#"<tr style="font-weight:bold"><td>AVERAGE</td><td>{:.2}</td><td>{:.2}</td><td><span class="delta {}">{}{:.2}</span></td></tr>
"#,
            last.avg_overall, avg_overall, avg_class, avg_sign, avg_delta
        );

        let _ = write!(buf, "</tbody></table>\n");
    }

    // ── Per-fixture scores table ───────────────────────────────────────────
    let _ = write!(buf, "<h2>Per-Fixture Scores</h2>");
    let _ = write!(
        buf,
        r#"<table>
<thead><tr>
<th>Fixture</th><th>GrpCo</th><th>EntPt</th><th>Order</th><th>Risk</th><th>Lang</th><th>Files</th><th>Overall</th>
</tr></thead><tbody>
"#
    );

    for (_, display_name, scores) in fixture_results {
        let _ = write!(
            buf,
            r#"<tr>
<td>{}</td>
<td style="color:{}">{:.2}</td>
<td style="color:{}">{:.2}</td>
<td style="color:{}">{:.2}</td>
<td style="color:{}">{:.2}</td>
<td style="color:{}">{:.2}</td>
<td style="color:{}">{:.2}</td>
<td style="color:{}; font-weight:bold">{:.2}</td>
</tr>
"#,
            html_escape(display_name),
            score_color(scores.group_coherence),
            scores.group_coherence,
            score_color(scores.entrypoint_accuracy),
            scores.entrypoint_accuracy,
            score_color(scores.review_ordering),
            scores.review_ordering,
            score_color(scores.risk_reasonableness),
            scores.risk_reasonableness,
            score_color(scores.language_detection),
            scores.language_detection,
            score_color(scores.file_accounting),
            scores.file_accounting,
            score_color(scores.overall),
            scores.overall,
        );
    }

    // Average row
    if !fixture_results.is_empty() {
        let n = fixture_results.len() as f64;
        let avg = |f: fn(&EvalScores) -> f64| -> f64 {
            fixture_results.iter().map(|(_, _, s)| f(s)).sum::<f64>() / n
        };
        let avg_gc = avg(|s| s.group_coherence);
        let avg_ep = avg(|s| s.entrypoint_accuracy);
        let avg_or = avg(|s| s.review_ordering);
        let avg_ri = avg(|s| s.risk_reasonableness);
        let avg_la = avg(|s| s.language_detection);
        let avg_fa = avg(|s| s.file_accounting);
        let _ = write!(
            buf,
            r#"<tr style="font-weight:bold; border-top:2px solid var(--lavender)">
<td>AVERAGE</td>
<td>{:.2}</td><td>{:.2}</td><td>{:.2}</td><td>{:.2}</td><td>{:.2}</td><td>{:.2}</td>
<td style="color:{}">{:.2}</td>
</tr>
"#,
            avg_gc,
            avg_ep,
            avg_or,
            avg_ri,
            avg_la,
            avg_fa,
            score_color(avg_overall),
            avg_overall,
        );
    }

    let _ = write!(buf, "</tbody></table>\n");

    // ── Per-criterion breakdown (bar chart) ────────────────────────────────
    let _ = write!(buf, "<h2>Per-Criterion Breakdown</h2>");
    let _ = write!(buf, r#"<div class="criterion-grid">"#);

    let criteria: &[(&str, fn(&EvalScores) -> f64)] = &[
        ("Group Coherence", |s: &EvalScores| s.group_coherence),
        ("Entrypoint Accuracy", |s: &EvalScores| {
            s.entrypoint_accuracy
        }),
        ("Review Ordering", |s: &EvalScores| s.review_ordering),
        ("Risk Reasonableness", |s: &EvalScores| {
            s.risk_reasonableness
        }),
        ("Language Detection", |s: &EvalScores| s.language_detection),
        ("File Accounting", |s: &EvalScores| s.file_accounting),
    ];

    for (criterion_name, extractor) in criteria {
        let _ = write!(
            buf,
            r#"<div class="criterion-card">
<div class="name">{}</div>"#,
            criterion_name
        );

        for (_, display_name, scores) in fixture_results {
            let val = extractor(scores);
            let pct = (val * 100.0).min(100.0);
            let _ = write!(
                buf,
                r#"<div class="bar-container">
<span style="min-width:120px; font-size:0.8rem; overflow:hidden; text-overflow:ellipsis; white-space:nowrap">{}</span>
<div class="bar-track"><div class="bar-fill" style="width:{:.0}%; background:{}"></div></div>
<span class="bar-label">{:.2}</span>
</div>
"#,
                html_escape(display_name),
                pct,
                score_color(val),
                val,
            );
        }

        let _ = write!(buf, "</div>\n");
    }

    let _ = write!(buf, "</div>\n");

    // ── Historical trend chart (SVG) ───────────────────────────────────────
    let _ = write!(buf, "<h2>Historical Trend</h2>");

    // Include current run in the trend data
    let mut trend_points: Vec<(String, f64)> = history
        .iter()
        .map(|h| (h.timestamp.clone(), h.avg_overall))
        .collect();
    trend_points.push((timestamp.to_string(), avg_overall));

    if trend_points.len() < 2 {
        let _ = write!(buf, "<div class=\"no-data\">Not enough history data for trend chart. Run eval with --history-file to track scores over time.</div>");
    } else {
        let chart_w: f64 = 800.0;
        let chart_h: f64 = 300.0;
        let pad_l: f64 = 50.0;
        let pad_r: f64 = 20.0;
        let pad_t: f64 = 20.0;
        let pad_b: f64 = 60.0;
        let plot_w = chart_w - pad_l - pad_r;
        let plot_h = chart_h - pad_t - pad_b;

        let n = trend_points.len();
        let x_step = if n > 1 {
            plot_w / (n - 1) as f64
        } else {
            plot_w
        };

        // Y axis: 0.0 to 1.0
        let y_scale = |v: f64| -> f64 { pad_t + plot_h * (1.0 - v) };
        let x_pos = |i: usize| -> f64 { pad_l + i as f64 * x_step };

        let _ = write!(buf,
            "<div class=\"chart-container\">\n\
             <svg width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\" xmlns=\"http://www.w3.org/2000/svg\">\n",
            chart_w, chart_h, chart_w, chart_h);

        // SVG color constants
        let grid_color = "#45475a";
        let label_color = "#a6adc8";
        let line_color = "#89b4fa";
        let red_color = "#f38ba8";
        let bg_color = "#1e1e2e";
        let text_color = "#cdd6f4";

        // Grid lines
        for &tick in &[0.0, 0.25, 0.5, 0.75, 1.0] {
            let y = y_scale(tick);
            let _ = write!(
                buf,
                "<line x1=\"{}\" y1=\"{:.1}\" x2=\"{}\" y2=\"{:.1}\" stroke=\"{}\" stroke-dasharray=\"4,4\"/>\n\
                 <text x=\"{}\" y=\"{:.1}\" fill=\"{}\" font-size=\"11\" text-anchor=\"end\" dominant-baseline=\"middle\">{:.2}</text>\n",
                pad_l, y, chart_w - pad_r, y, grid_color,
                pad_l - 6.0, y, label_color, tick,
            );
        }

        // Threshold line
        let th_y = y_scale(min_score);
        let _ = write!(
            buf,
            "<line x1=\"{}\" y1=\"{:.1}\" x2=\"{}\" y2=\"{:.1}\" stroke=\"{}\" stroke-width=\"1.5\" stroke-dasharray=\"8,4\"/>\n\
             <text x=\"{}\" y=\"{:.1}\" fill=\"{}\" font-size=\"10\" dominant-baseline=\"auto\">threshold {:.2}</text>\n",
            pad_l, th_y, chart_w - pad_r, th_y, red_color,
            chart_w - pad_r + 2.0, th_y - 4.0, red_color, min_score,
        );

        // Line path
        let mut path = String::new();
        for (i, (_, score)) in trend_points.iter().enumerate() {
            let x = x_pos(i);
            let y = y_scale(*score);
            if i == 0 {
                let _ = write!(path, "M{:.1},{:.1}", x, y);
            } else {
                let _ = write!(path, " L{:.1},{:.1}", x, y);
            }
        }
        let _ = write!(
            buf,
            "<path d=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"2.5\" stroke-linejoin=\"round\"/>\n",
            path, line_color
        );

        // Points
        for (i, (_, score)) in trend_points.iter().enumerate() {
            let x = x_pos(i);
            let y = y_scale(*score);
            let color = score_color(*score);
            let _ = write!(
                buf,
                "<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"5\" fill=\"{}\" stroke=\"{}\" stroke-width=\"2\"/>\n\
                 <text x=\"{:.1}\" y=\"{:.1}\" fill=\"{}\" font-size=\"10\" text-anchor=\"middle\">{:.2}</text>\n",
                x, y, color, bg_color,
                x, y - 12.0, text_color, score,
            );
        }

        // X axis labels (run numbers)
        for (i, (ts, _)) in trend_points.iter().enumerate() {
            let x = x_pos(i);
            // Show short label: run number or truncated timestamp
            let label = if i == n - 1 {
                "current".to_string()
            } else {
                // Show just date portion if available, else run number
                let fallback = format!("#{}", i + 1);
                ts.get(..10).unwrap_or(&fallback).to_string()
            };
            let label_y = chart_h - pad_b + 20.0;
            let _ =
                write!(
                buf,
                "<text x=\"{:.1}\" y=\"{:.1}\" fill=\"{}\" font-size=\"9\" text-anchor=\"middle\" \
                 transform=\"rotate(-30,{:.1},{:.1})\">{}</text>\n",
                x, label_y, label_color,
                x, label_y,
                html_escape(&label),
            );
        }

        let _ = write!(buf, "</svg>\n</div>\n");
    }

    // ── Per-fixture historical trend (if history has per-fixture data) ──────
    if history.len() >= 2 {
        let _ = write!(buf, "<h2>Per-Fixture Trend</h2>");
        let _ = write!(buf, "<div class=\"chart-container\">");

        let chart_w: f64 = 800.0;
        let chart_h: f64 = 300.0;
        let pad_l: f64 = 50.0;
        let pad_r: f64 = 120.0; // extra room for legend
        let pad_t: f64 = 20.0;
        let pad_b: f64 = 60.0;
        let plot_w = chart_w - pad_l - pad_r;
        let plot_h = chart_h - pad_t - pad_b;

        // Collect unique fixture names from history
        let mut fixture_names_set: Vec<String> = Vec::new();
        for entry in history {
            for f in &entry.fixtures {
                if !fixture_names_set.contains(&f.name) {
                    fixture_names_set.push(f.name.clone());
                }
            }
        }
        // Add current fixtures
        for (name, _, _) in fixture_results {
            if !fixture_names_set.contains(name) {
                fixture_names_set.push(name.clone());
            }
        }

        let colors = [
            "#89b4fa", "#a6e3a1", "#fab387", "#f38ba8", "#b4befe", "#94e2d5", "#f9e2af", "#89dceb",
        ];

        let grid_color = "#45475a";
        let label_color = "#a6adc8";

        // Build data: for each fixture, collect (index, score) across history + current
        let total_runs = history.len() + 1;
        let x_step = if total_runs > 1 {
            plot_w / (total_runs - 1) as f64
        } else {
            plot_w
        };
        let y_scale = |v: f64| -> f64 { pad_t + plot_h * (1.0 - v) };
        let x_pos = |i: usize| -> f64 { pad_l + i as f64 * x_step };

        let _ = write!(buf,
            "<svg width=\"{}\" height=\"{}\" viewBox=\"0 0 {} {}\" xmlns=\"http://www.w3.org/2000/svg\">\n",
            chart_w, chart_h, chart_w, chart_h);

        // Grid
        for &tick in &[0.0, 0.25, 0.5, 0.75, 1.0] {
            let y = y_scale(tick);
            let _ = write!(
                buf,
                "<line x1=\"{}\" y1=\"{:.1}\" x2=\"{}\" y2=\"{:.1}\" stroke=\"{}\" stroke-dasharray=\"4,4\"/>\n\
                 <text x=\"{}\" y=\"{:.1}\" fill=\"{}\" font-size=\"11\" text-anchor=\"end\" dominant-baseline=\"middle\">{:.2}</text>\n",
                pad_l, y, chart_w - pad_r, y, grid_color,
                pad_l - 6.0, y, label_color, tick,
            );
        }

        // One line per fixture
        for (fi, fixture_name) in fixture_names_set.iter().enumerate() {
            let color = colors[fi % colors.len()];
            let mut points: Vec<(f64, f64)> = Vec::new();

            for (hi, entry) in history.iter().enumerate() {
                if let Some(f) = entry.fixtures.iter().find(|f| f.name == *fixture_name) {
                    points.push((x_pos(hi), y_scale(f.overall)));
                }
            }
            // Current run
            if let Some((_, _, scores)) = fixture_results.iter().find(|(n, _, _)| n == fixture_name)
            {
                points.push((x_pos(history.len()), y_scale(scores.overall)));
            }

            if points.len() >= 2 {
                let mut path = String::new();
                for (i, (x, y)) in points.iter().enumerate() {
                    if i == 0 {
                        let _ = write!(path, "M{:.1},{:.1}", x, y);
                    } else {
                        let _ = write!(path, " L{:.1},{:.1}", x, y);
                    }
                }
                let _ = write!(
                    buf,
                    "<path d=\"{}\" fill=\"none\" stroke=\"{}\" stroke-width=\"1.5\" stroke-linejoin=\"round\" opacity=\"0.8\"/>\n",
                    path, color
                );
            }

            // Legend
            let legend_y = pad_t + 14.0 * fi as f64 + 10.0;
            let _ = write!(
                buf,
                "<rect x=\"{}\" y=\"{:.1}\" width=\"12\" height=\"3\" fill=\"{}\" rx=\"1.5\"/>\n\
                 <text x=\"{}\" y=\"{:.1}\" fill=\"{}\" font-size=\"9\" dominant-baseline=\"middle\">{}</text>\n",
                chart_w - pad_r + 8.0, legend_y, color,
                chart_w - pad_r + 24.0, legend_y + 1.5, label_color, html_escape(fixture_name),
            );
        }

        let _ = write!(buf, "</svg>\n</div>\n");
    }

    // ── Footer ─────────────────────────────────────────────────────────────
    let _ = write!(
        buf,
        r#"<div style="margin-top:2rem; padding-top:1rem; border-top:1px solid var(--overlay); color:var(--subtext); font-size:0.75rem">
Generated by <strong>diffcore eval --format html</strong>
</div>
</body>
</html>"#
    );

    buf
}

/// Return a CSS color string based on a [0.0, 1.0] score.
fn score_color(score: f64) -> &'static str {
    if score >= 0.9 {
        "#a6e3a1" // green
    } else if score >= 0.7 {
        "#f9e2af" // yellow
    } else if score >= 0.5 {
        "#fab387" // peach
    } else {
        "#f38ba8" // red
    }
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
            fixtures: vec![FixtureHistoryEntry {
                name: "ts-express".to_string(),
                overall: 0.89,
            }],
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

    // ─── HTML Report Tests ─────────────────────────────────────────────────

    #[test]
    fn test_html_report_basic_structure() {
        let results = sample_results();
        let html = build_html_report(&results, 0.90, 0.50, "2026-03-19T12:00:00Z", &[]);
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("</html>"));
        assert!(html.contains("diffcore Eval Dashboard"));
        assert!(html.contains("2026-03-19T12:00:00Z"));
    }

    #[test]
    fn test_html_report_pass_badge() {
        let results = sample_results();
        let html = build_html_report(&results, 0.90, 0.50, "2026-03-19T12:00:00Z", &[]);
        assert!(html.contains("badge-pass"));
        assert!(html.contains(">PASS<"));
    }

    #[test]
    fn test_html_report_fail_badge() {
        let results = sample_results();
        let html = build_html_report(&results, 0.10, 0.50, "2026-03-19T12:00:00Z", &[]);
        assert!(html.contains("badge-fail"));
        assert!(html.contains(">FAIL<"));
    }

    #[test]
    fn test_html_report_contains_fixture_names() {
        let results = sample_results();
        let html = build_html_report(&results, 0.90, 0.50, "2026-03-19T12:00:00Z", &[]);
        assert!(html.contains("TS Express API"));
        assert!(html.contains("Python FastAPI"));
    }

    #[test]
    fn test_html_report_contains_scores() {
        let results = sample_results();
        let html = build_html_report(&results, 0.90, 0.50, "2026-03-19T12:00:00Z", &[]);
        assert!(html.contains("0.85")); // group_coherence
        assert!(html.contains("0.89")); // ts-express overall
        assert!(html.contains("0.91")); // python-fastapi overall
    }

    #[test]
    fn test_html_report_contains_criterion_sections() {
        let results = sample_results();
        let html = build_html_report(&results, 0.90, 0.50, "2026-03-19T12:00:00Z", &[]);
        assert!(html.contains("Group Coherence"));
        assert!(html.contains("Entrypoint Accuracy"));
        assert!(html.contains("Review Ordering"));
        assert!(html.contains("Risk Reasonableness"));
        assert!(html.contains("Language Detection"));
        assert!(html.contains("File Accounting"));
    }

    #[test]
    fn test_html_report_contains_bar_charts() {
        let results = sample_results();
        let html = build_html_report(&results, 0.90, 0.50, "2026-03-19T12:00:00Z", &[]);
        assert!(html.contains("bar-fill"));
        assert!(html.contains("bar-track"));
    }

    #[test]
    fn test_html_report_no_history_shows_message() {
        let results = sample_results();
        let html = build_html_report(&results, 0.90, 0.50, "2026-03-19T12:00:00Z", &[]);
        assert!(html.contains("Not enough history data"));
        // Should NOT contain diff table (no last run)
        assert!(!html.contains("Diff vs Last Run"));
    }

    #[test]
    fn test_html_report_with_history_shows_trend() {
        let results = sample_results();
        let history = vec![ScoreHistoryEntry {
            timestamp: "2026-03-18T12:00:00Z".to_string(),
            avg_overall: 0.85,
            passed: true,
            fixture_count: 2,
            fixtures: vec![
                FixtureHistoryEntry {
                    name: "ts-express".to_string(),
                    overall: 0.84,
                },
                FixtureHistoryEntry {
                    name: "python-fastapi".to_string(),
                    overall: 0.86,
                },
            ],
        }];
        let html = build_html_report(&results, 0.90, 0.50, "2026-03-19T12:00:00Z", &history);
        // Should have SVG trend chart (2 points: 1 history + 1 current)
        assert!(html.contains("<svg"));
        assert!(html.contains("Historical Trend"));
        // Should have diff table
        assert!(html.contains("Diff vs Last Run"));
    }

    #[test]
    fn test_html_report_diff_positive_delta() {
        let results = sample_results();
        let history = vec![ScoreHistoryEntry {
            timestamp: "2026-03-18T12:00:00Z".to_string(),
            avg_overall: 0.80,
            passed: true,
            fixture_count: 2,
            fixtures: vec![
                FixtureHistoryEntry {
                    name: "ts-express".to_string(),
                    overall: 0.80,
                },
                FixtureHistoryEntry {
                    name: "python-fastapi".to_string(),
                    overall: 0.80,
                },
            ],
        }];
        let html = build_html_report(&results, 0.90, 0.50, "2026-03-19T12:00:00Z", &history);
        assert!(html.contains("delta-pos"));
        assert!(html.contains("+"));
    }

    #[test]
    fn test_html_report_diff_negative_delta() {
        let results = sample_results();
        let history = vec![ScoreHistoryEntry {
            timestamp: "2026-03-18T12:00:00Z".to_string(),
            avg_overall: 0.95,
            passed: true,
            fixture_count: 2,
            fixtures: vec![
                FixtureHistoryEntry {
                    name: "ts-express".to_string(),
                    overall: 0.95,
                },
                FixtureHistoryEntry {
                    name: "python-fastapi".to_string(),
                    overall: 0.95,
                },
            ],
        }];
        let html = build_html_report(&results, 0.90, 0.50, "2026-03-19T12:00:00Z", &history);
        assert!(html.contains("delta-neg"));
    }

    #[test]
    fn test_html_report_diff_new_fixture() {
        let results = sample_results();
        let history = vec![ScoreHistoryEntry {
            timestamp: "2026-03-18T12:00:00Z".to_string(),
            avg_overall: 0.85,
            passed: true,
            fixture_count: 1,
            fixtures: vec![
                // Only ts-express in history — python-fastapi is new
                FixtureHistoryEntry {
                    name: "ts-express".to_string(),
                    overall: 0.84,
                },
            ],
        }];
        let html = build_html_report(&results, 0.90, 0.50, "2026-03-19T12:00:00Z", &history);
        assert!(html.contains("new")); // "new" label for python-fastapi
    }

    #[test]
    fn test_html_report_summary_cards() {
        let results = sample_results();
        let html = build_html_report(&results, 0.90, 0.50, "2026-03-19T12:00:00Z", &[]);
        assert!(html.contains("Average Score"));
        assert!(html.contains("Threshold"));
        assert!(html.contains("Fixtures"));
        assert!(html.contains("History Runs"));
    }

    #[test]
    fn test_html_report_empty_fixtures() {
        let results: Vec<(String, String, EvalScores)> = vec![];
        let html = build_html_report(&results, 0.0, 0.50, "2026-03-19T12:00:00Z", &[]);
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("FAIL")); // 0.0 < 0.50
    }

    #[test]
    fn test_html_report_xss_prevention() {
        let results = vec![(
            "xss".to_string(),
            "<script>alert('xss')</script>".to_string(),
            EvalScores {
                group_coherence: 0.5,
                entrypoint_accuracy: 0.5,
                review_ordering: 0.5,
                risk_reasonableness: 0.5,
                language_detection: 0.5,
                file_accounting: 0.5,
                overall: 0.5,
            },
        )];
        let html = build_html_report(&results, 0.50, 0.50, "2026-03-19T12:00:00Z", &[]);
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn test_html_report_per_fixture_trend_with_multiple_history() {
        let results = sample_results();
        let history = vec![
            ScoreHistoryEntry {
                timestamp: "2026-03-17T12:00:00Z".to_string(),
                avg_overall: 0.80,
                passed: true,
                fixture_count: 2,
                fixtures: vec![
                    FixtureHistoryEntry {
                        name: "ts-express".to_string(),
                        overall: 0.78,
                    },
                    FixtureHistoryEntry {
                        name: "python-fastapi".to_string(),
                        overall: 0.82,
                    },
                ],
            },
            ScoreHistoryEntry {
                timestamp: "2026-03-18T12:00:00Z".to_string(),
                avg_overall: 0.85,
                passed: true,
                fixture_count: 2,
                fixtures: vec![
                    FixtureHistoryEntry {
                        name: "ts-express".to_string(),
                        overall: 0.84,
                    },
                    FixtureHistoryEntry {
                        name: "python-fastapi".to_string(),
                        overall: 0.86,
                    },
                ],
            },
        ];
        let html = build_html_report(&results, 0.90, 0.50, "2026-03-19T12:00:00Z", &history);
        assert!(html.contains("Per-Fixture Trend"));
        // Should have legend entries for both fixtures
        assert!(html.contains("ts-express"));
        assert!(html.contains("python-fastapi"));
    }

    #[test]
    fn test_html_report_threshold_line_in_chart() {
        let results = sample_results();
        let history = vec![ScoreHistoryEntry {
            timestamp: "2026-03-18T12:00:00Z".to_string(),
            avg_overall: 0.85,
            passed: true,
            fixture_count: 2,
            fixtures: vec![
                FixtureHistoryEntry {
                    name: "ts-express".to_string(),
                    overall: 0.84,
                },
                FixtureHistoryEntry {
                    name: "python-fastapi".to_string(),
                    overall: 0.86,
                },
            ],
        }];
        let html = build_html_report(&results, 0.90, 0.50, "2026-03-19T12:00:00Z", &history);
        assert!(html.contains("threshold 0.50"));
    }

    #[test]
    fn test_score_color_ranges() {
        assert_eq!(score_color(1.0), "#a6e3a1"); // green
        assert_eq!(score_color(0.95), "#a6e3a1"); // green
        assert_eq!(score_color(0.90), "#a6e3a1"); // green
        assert_eq!(score_color(0.89), "#f9e2af"); // yellow
        assert_eq!(score_color(0.70), "#f9e2af"); // yellow
        assert_eq!(score_color(0.69), "#fab387"); // peach
        assert_eq!(score_color(0.50), "#fab387"); // peach
        assert_eq!(score_color(0.49), "#f38ba8"); // red
        assert_eq!(score_color(0.0), "#f38ba8"); // red
    }

    #[test]
    fn test_html_escape() {
        assert_eq!(html_escape("<b>bold</b>"), "&lt;b&gt;bold&lt;/b&gt;");
        assert_eq!(html_escape("a&b"), "a&amp;b");
        assert_eq!(html_escape("\"quoted\""), "&quot;quoted&quot;");
        assert_eq!(html_escape("it's"), "it&#x27;s");
        assert_eq!(html_escape("normal text"), "normal text");
    }

    #[test]
    fn test_html_report_average_row() {
        let results = sample_results();
        let html = build_html_report(&results, 0.90, 0.50, "2026-03-19T12:00:00Z", &[]);
        assert!(html.contains("AVERAGE"));
    }

    #[test]
    fn test_html_report_footer() {
        let results = sample_results();
        let html = build_html_report(&results, 0.90, 0.50, "2026-03-19T12:00:00Z", &[]);
        assert!(html.contains("diffcore eval --format html"));
    }
}
