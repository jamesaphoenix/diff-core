//! Multi-repository evaluation harness for real-world codebases.
//!
//! Unlike the synthetic fixture suite, this mode scores analysis quality against
//! threshold-based heuristics: group count cap, infra ratio, singleton ratio,
//! and file accounting. It is intended for benchmarking grouping behavior across
//! 10-20 local repositories listed in a manifest file.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use git2::Repository;
use serde::{Deserialize, Serialize};

use crate::cluster;
use crate::config::FlowdiffConfig;
use crate::entrypoint;
use crate::flow::{self, FlowConfig};
use crate::graph::SymbolGraph;
use crate::output::{self, build_analysis_output};
use crate::pipeline;
use crate::rank::{self, compute_risk_score, compute_surface_area};
use crate::types::{AnalysisOutput, DiffSource, GroupRankInput, RankWeights};

use super::EvalFormat;

/// A manifest of real repositories to benchmark.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoEvalManifest {
    #[serde(default)]
    pub defaults: RepoEvalThresholds,
    #[serde(default)]
    pub corpus: RepoEvalCorpusConfig,
    /// Directory of per-repo .toml files to include. Relative to the manifest file.
    #[serde(default)]
    pub include_dir: Option<String>,
    #[serde(default)]
    pub repos: Vec<RepoEvalTarget>,
}

impl RepoEvalManifest {
    pub fn from_file(path: &Path) -> Result<Self, RepoEvalError> {
        let contents = std::fs::read_to_string(path)?;
        let mut manifest: RepoEvalManifest = toml::from_str(&contents)?;

        // If include_dir is set, load all .toml files from that directory
        // and merge their [[repos]] entries into the manifest.
        if let Some(ref include_dir) = manifest.include_dir {
            let base_dir = path.parent().unwrap_or(Path::new("."));
            let repo_dir = base_dir.join(include_dir);
            if repo_dir.is_dir() {
                let mut entries: Vec<_> = std::fs::read_dir(&repo_dir)?
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().map_or(false, |ext| ext == "toml"))
                    .collect();
                entries.sort_by_key(|e| e.file_name());
                for entry in entries {
                    let repo_contents = std::fs::read_to_string(entry.path())?;
                    let repo_manifest: RepoEvalManifest =
                        toml::from_str(&repo_contents).map_err(|e| RepoEvalError::Parse(e))?;
                    manifest.repos.extend(repo_manifest.repos);
                }
            }
        }

        if manifest.repos.is_empty() {
            return Err(RepoEvalError::Validation(
                "Repo eval manifest must contain at least one [[repos]] entry (inline or via include_dir)".to_string(),
            ));
        }
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<(), RepoEvalError> {
        if let Some(min_repos) = self.corpus.min_repos_per_language {
            let mut counts = std::collections::BTreeMap::<String, usize>::new();

            for repo in &self.repos {
                let language = repo.language.as_deref().ok_or_else(|| {
                    RepoEvalError::Validation(
                        "All [[repos]] entries must set language when corpus.min_repos_per_language is enabled".to_string(),
                    )
                })?;
                *counts.entry(language.to_ascii_lowercase()).or_default() += 1;
            }

            let missing: Vec<String> = counts
                .into_iter()
                .filter(|(_, count)| *count < min_repos)
                .map(|(language, count)| format!("{} ({}/{})", language, count, min_repos))
                .collect();

            if !missing.is_empty() {
                return Err(RepoEvalError::Validation(format!(
                    "Repo eval manifest does not satisfy language balance: {}",
                    missing.join(", ")
                )));
            }
        }

        for repo in &self.repos {
            if let Some(expectations) = &repo.expectations {
                expectations.validate()?;
            }
        }

        Ok(())
    }
}

/// Thresholds used to score a real repository analysis run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoEvalThresholds {
    #[serde(default = "default_repo_eval_max_groups")]
    pub max_groups: usize,
    #[serde(default = "default_repo_eval_max_infra_ratio")]
    pub max_infra_ratio: f64,
    #[serde(default = "default_repo_eval_max_singleton_ratio")]
    pub max_singleton_ratio: f64,
    /// Optional density-style cap used by the separate large-diff eval track.
    /// Example: 55.0 means "no more than 55 semantic groups per 1000 changed files".
    #[serde(default)]
    pub max_groups_per_1000_files: Option<f64>,
}

fn default_repo_eval_max_groups() -> usize {
    200
}

fn default_repo_eval_max_infra_ratio() -> f64 {
    0.35
}

fn default_repo_eval_max_singleton_ratio() -> f64 {
    0.50
}

impl Default for RepoEvalThresholds {
    fn default() -> Self {
        Self {
            max_groups: default_repo_eval_max_groups(),
            max_infra_ratio: default_repo_eval_max_infra_ratio(),
            max_singleton_ratio: default_repo_eval_max_singleton_ratio(),
            max_groups_per_1000_files: None,
        }
    }
}

/// Optional manifest-level corpus validation settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RepoEvalCorpusConfig {
    /// When set, every language present in the manifest must have at least this
    /// many repositories.
    #[serde(default)]
    pub min_repos_per_language: Option<usize>,
}

/// Golden expectations for a repository analysis run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RepoEvalExpectations {
    /// Optional lower bound to prevent over-collapsing all changes into a tiny
    /// number of giant groups.
    #[serde(default)]
    pub group_count_min: Option<usize>,
    /// Optional upper bound that is more specific than the global thresholds.
    #[serde(default)]
    pub group_count_max: Option<usize>,
    /// Each set must resolve to the same semantic group (not infrastructure).
    #[serde(default)]
    pub same_group: Vec<Vec<String>>,
    /// Each set must resolve to distinct destinations.
    #[serde(default)]
    pub separate_group: Vec<Vec<String>>,
    /// These paths must land in the infrastructure bucket.
    #[serde(default)]
    pub infrastructure: Vec<String>,
    /// These paths must remain in semantic groups.
    #[serde(default)]
    pub non_infrastructure: Vec<String>,
}

impl RepoEvalExpectations {
    fn validate(&self) -> Result<(), RepoEvalError> {
        if let (Some(min), Some(max)) = (self.group_count_min, self.group_count_max) {
            if min > max {
                return Err(RepoEvalError::Validation(format!(
                    "Repo eval expectation group_count_min ({}) cannot exceed group_count_max ({})",
                    min, max
                )));
            }
        }

        validate_constraint_sets("same_group", &self.same_group)?;
        validate_constraint_sets("separate_group", &self.separate_group)?;

        Ok(())
    }
}

fn validate_constraint_sets(
    name: &str,
    constraint_sets: &[Vec<String>],
) -> Result<(), RepoEvalError> {
    for files in constraint_sets {
        if files.len() < 2 {
            return Err(RepoEvalError::Validation(format!(
                "Repo eval expectation '{}' must contain at least two files",
                name
            )));
        }
    }

    Ok(())
}

/// A single repository benchmark target.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoEvalTarget {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub repo_url: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub head: Option<String>,
    #[serde(default)]
    pub range: Option<String>,
    #[serde(default)]
    pub staged: bool,
    #[serde(default)]
    pub unstaged: bool,
    #[serde(default)]
    pub thresholds: Option<RepoEvalThresholds>,
    #[serde(default)]
    pub expectations: Option<RepoEvalExpectations>,
}

/// Raw quality metrics derived from a repository analysis output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoEvalMetrics {
    pub raw_total_files_changed: usize,
    pub total_files_changed: usize,
    pub ignored_files: usize,
    pub duplicate_file_entries: usize,
    pub total_groups: usize,
    pub groups_per_1000_files: f64,
    pub infra_files: usize,
    pub infra_ratio: f64,
    pub singleton_groups: usize,
    pub singleton_ratio: f64,
    pub max_group_size: usize,
    pub avg_group_size: f64,
    pub files_accounted: bool,
    pub golden_checks: usize,
    pub golden_satisfied: usize,
    pub golden_score: f64,
}

/// Scores derived from the raw metrics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoEvalScores {
    pub group_count: f64,
    pub group_density: Option<f64>,
    pub infra_ratio: f64,
    pub singleton_ratio: f64,
    pub file_accounting: f64,
    pub golden: f64,
    pub overall: f64,
}

/// Result of evaluating repo-specific golden expectations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoEvalGoldenResult {
    pub total_checks: usize,
    pub satisfied_checks: usize,
    pub score: f64,
    pub failures: Vec<String>,
    /// File classification coverage: fraction of changed files that appear in
    /// either `infrastructure` or `non_infrastructure` expectations.
    pub file_coverage: f64,
    /// Number of files in the diff that have a classification.
    pub classified_files: usize,
    /// Number of files in the diff that are NOT classified.
    pub unclassified_files: usize,
    /// Paths of files that are not classified (for linting).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unclassified_paths: Vec<String>,
}

impl Default for RepoEvalGoldenResult {
    fn default() -> Self {
        Self {
            total_checks: 0,
            satisfied_checks: 0,
            score: 1.0,
            failures: Vec::new(),
            file_coverage: 0.0,
            classified_files: 0,
            unclassified_files: 0,
            unclassified_paths: Vec::new(),
        }
    }
}

/// Result for a single repository benchmark target.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoEvalRun {
    pub name: String,
    pub path: String,
    pub diff_spec: String,
    pub thresholds: RepoEvalThresholds,
    pub metrics: RepoEvalMetrics,
    pub scores: RepoEvalScores,
    pub golden: RepoEvalGoldenResult,
    pub passed: bool,
}

/// Aggregate result for a repo manifest evaluation run.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoEvalResult {
    pub manifest_path: String,
    pub avg_overall: f64,
    pub passed: bool,
    pub repo_count: usize,
    pub repos: Vec<RepoEvalRun>,
    pub report: String,
}

/// Errors from repo-manifest evaluation.
#[derive(Debug, thiserror::Error)]
pub enum RepoEvalError {
    #[error("Failed to read repo eval manifest: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to parse repo eval manifest: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("Repo eval manifest validation failed: {0}")]
    Validation(String),

    #[error("Git error for '{0}': {1}")]
    Git(String, String),
}

/// Run a manifest-driven real-repo evaluation.
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
pub fn run_repo_eval(
    manifest_path: &Path,
    min_score: f64,
    format: EvalFormat,
) -> Result<RepoEvalResult, RepoEvalError> {
    let manifest = RepoEvalManifest::from_file(manifest_path)?;

    let mut repo_results = Vec::new();
    for repo in &manifest.repos {
        let thresholds = repo
            .thresholds
            .clone()
            .unwrap_or_else(|| manifest.defaults.clone());
        repo_results.push(run_repo_target(repo, thresholds)?);
    }

    let avg_overall = if repo_results.is_empty() {
        0.0
    } else {
        repo_results.iter().map(|r| r.scores.overall).sum::<f64>() / repo_results.len() as f64
    };

    let passed = avg_overall >= min_score && repo_results.iter().all(|r| r.passed);
    let report = match format {
        EvalFormat::Json => serde_json::to_string_pretty(&serde_json::json!({
            "manifest_path": manifest_path.display().to_string(),
            "avg_overall": avg_overall,
            "min_score": min_score,
            "passed": passed,
            "repo_count": repo_results.len(),
            "repos": repo_results,
        }))
        .unwrap_or_default(),
        EvalFormat::Html => {
            format_repo_eval_html(manifest_path, &repo_results, avg_overall, min_score)
        }
        EvalFormat::Text => {
            format_repo_eval_text(manifest_path, &repo_results, avg_overall, min_score)
        }
    };

    Ok(RepoEvalResult {
        manifest_path: manifest_path.display().to_string(),
        avg_overall,
        passed,
        repo_count: repo_results.len(),
        repos: repo_results,
        report,
    })
}

fn run_repo_target(
    target: &RepoEvalTarget,
    thresholds: RepoEvalThresholds,
) -> Result<RepoEvalRun, RepoEvalError> {
    let repo_root = std::fs::canonicalize(PathBuf::from(&target.path))?;
    let repo = Repository::discover(&repo_root)
        .map_err(|e| RepoEvalError::Git(target.name.clone(), e.to_string()))?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| {
            RepoEvalError::Git(
                target.name.clone(),
                "Bare repositories are not supported".to_string(),
            )
        })?
        .to_path_buf();

    let config = FlowdiffConfig::load_with_global_llm_from_dir(&workdir).unwrap_or_default();
    let (diff_result, diff_source, diff_spec) = extract_diff_for_target(&repo, target)?;

    let file_inputs: Vec<(&str, &str)> = diff_result
        .files
        .iter()
        .filter_map(|file_diff| {
            let content = file_diff
                .new_content
                .as_deref()
                .or(file_diff.old_content.as_deref())?;
            let path = file_diff.path();
            if config.is_ignored(path) {
                return None;
            }
            Some((path, content))
        })
        .collect();
    let parsed_files = pipeline::parse_files_parallel(&file_inputs);

    let workspace_map = crate::graph::build_workspace_map(&workdir);
    let mut graph = SymbolGraph::build_with_workspace(&parsed_files, &workspace_map);
    let entrypoints = entrypoint::detect_entrypoints(&parsed_files);
    let flow_analysis = flow::analyze_data_flow(&parsed_files, &FlowConfig::default());
    flow::enrich_graph(&mut graph, &flow_analysis);

    let changed_files: Vec<String> = diff_result
        .files
        .iter()
        .filter(|f| !config.is_ignored(f.path()))
        .map(|f| f.path().to_string())
        .collect();
    let mut unique_changed_files = changed_files.clone();
    unique_changed_files.sort();
    unique_changed_files.dedup();
    let ignored_files = diff_result
        .files
        .iter()
        .filter(|f| config.is_ignored(f.path()))
        .count();
    let duplicate_file_entries = changed_files
        .len()
        .saturating_sub(unique_changed_files.len());

    let cluster_result = cluster::cluster_files(&graph, &entrypoints, &changed_files);

    // Optionally refine clustering with embedding similarity (requires `embeddings` feature).
    // Uses diff-based embeddings: embeds what CHANGED (added/removed lines) rather than
    // full file content. This captures change semantics — files with similar changes
    // (both adding auth logic) cluster together even if their content is very different.
    #[cfg(feature = "embeddings")]
    let cluster_result = {
        let file_diffs_for_embed: Vec<(String, String)> = diff_result
            .files
            .iter()
            .filter_map(|fd| {
                let path = fd.new_path.as_deref().or(fd.old_path.as_deref())?;
                let diff_text = compute_diff_text(fd);
                if diff_text.is_empty() {
                    return None;
                }
                Some((path.to_string(), diff_text))
            })
            .collect();
        cluster::refine_with_embeddings(cluster_result, &file_diffs_for_embed)
    };

    let rank_inputs: Vec<GroupRankInput> = cluster_result
        .groups
        .iter()
        .map(|group| {
            let risk_flags = output::compute_group_risk_flags(
                &group
                    .files
                    .iter()
                    .map(|f| f.path.as_str())
                    .collect::<Vec<_>>(),
            );
            let total_add: u32 = group.files.iter().map(|f| f.changes.additions).sum();
            let total_del: u32 = group.files.iter().map(|f| f.changes.deletions).sum();

            GroupRankInput {
                group_id: group.id.clone(),
                risk: compute_risk_score(
                    risk_flags.has_schema_change,
                    risk_flags.has_api_change,
                    risk_flags.has_auth_change,
                    false,
                ),
                centrality: 0.5,
                surface_area: compute_surface_area(total_add, total_del, 1000),
                uncertainty: if risk_flags.has_test_only { 0.1 } else { 0.5 },
            }
        })
        .collect();

    let ranked = rank::rank_groups(&rank_inputs, &RankWeights::default());
    let output = build_analysis_output(
        &diff_result,
        diff_source,
        &parsed_files,
        &cluster_result,
        &ranked,
    );
    let golden = evaluate_repo_expectations(&output, target.expectations.as_ref());
    let metrics = compute_repo_metrics(
        &output,
        diff_result.files.len(),
        unique_changed_files.len(),
        ignored_files,
        duplicate_file_entries,
        &golden,
    );
    let scores = score_repo_metrics(&metrics, &thresholds, &golden);
    let passed = metrics.files_accounted
        && metrics.total_groups <= thresholds.max_groups
        && match thresholds.max_groups_per_1000_files {
            Some(limit) => metrics.groups_per_1000_files <= limit,
            None => true,
        }
        && metrics.infra_ratio <= thresholds.max_infra_ratio
        && metrics.singleton_ratio <= thresholds.max_singleton_ratio
        && golden.failures.is_empty();

    Ok(RepoEvalRun {
        name: target.name.clone(),
        path: workdir.display().to_string(),
        diff_spec,
        thresholds,
        metrics,
        scores,
        golden,
        passed,
    })
}

/// Compute a diff-text representation for embedding: only the lines that changed.
/// For new files, uses the full new content. For modified files, extracts added lines
/// (lines in new but not old). This captures change semantics rather than file identity.
#[cfg(feature = "embeddings")]
fn compute_diff_text(fd: &crate::git::FileDiff) -> String {
    use std::collections::HashSet;

    let path = fd
        .new_path
        .as_deref()
        .or(fd.old_path.as_deref())
        .unwrap_or("unknown");

    match (&fd.old_content, &fd.new_content) {
        (None, Some(new)) => {
            // New file — use full content (the entire file is "the change")
            format!("// NEW FILE: {}\n{}", path, new)
        }
        (Some(_old), None) => {
            // Deleted file — the change is the deletion
            format!("// DELETED FILE: {}", path)
        }
        (Some(old), Some(new)) => {
            // Modified file — extract added lines (in new but not old)
            let old_lines: HashSet<&str> = old.lines().collect();
            let added: Vec<&str> = new.lines().filter(|l| !old_lines.contains(l)).collect();
            let removed_count = old
                .lines()
                .filter(|l| !new.lines().collect::<HashSet<_>>().contains(l))
                .count();
            if added.is_empty() {
                format!("// MODIFIED: {} (-{} lines)", path, removed_count)
            } else {
                format!(
                    "// MODIFIED: {} (+{} -{} lines)\n{}",
                    path,
                    added.len(),
                    removed_count,
                    added.join("\n")
                )
            }
        }
        (None, None) => String::new(),
    }
}

fn extract_diff_for_target(
    repo: &Repository,
    target: &RepoEvalTarget,
) -> Result<(crate::git::DiffResult, DiffSource, String), RepoEvalError> {
    if let Some(range) = target.range.as_deref() {
        let diff = crate::git::diff_range(repo, range)
            .map_err(|e| RepoEvalError::Git(target.name.clone(), e.to_string()))?;
        let source =
            output::diff_source_range(range, diff.base_sha.as_deref(), diff.head_sha.as_deref());
        return Ok((diff, source, format!("range {}", range)));
    }

    if target.staged {
        let diff = crate::git::diff_staged(repo)
            .map_err(|e| RepoEvalError::Git(target.name.clone(), e.to_string()))?;
        return Ok((diff, output::diff_source_staged(), "staged".to_string()));
    }

    if target.unstaged {
        let diff = crate::git::diff_unstaged(repo)
            .map_err(|e| RepoEvalError::Git(target.name.clone(), e.to_string()))?;
        return Ok((diff, output::diff_source_unstaged(), "unstaged".to_string()));
    }

    let base = target.base.as_deref().unwrap_or("main");
    let head = target.head.as_deref().unwrap_or("HEAD");
    let diff = crate::git::diff_refs(repo, base, head)
        .map_err(|e| RepoEvalError::Git(target.name.clone(), e.to_string()))?;
    let source = output::diff_source_branch(
        base,
        head,
        diff.base_sha.as_deref(),
        diff.head_sha.as_deref(),
    );
    Ok((diff, source, format!("{}...{}", base, head)))
}

fn compute_repo_metrics(
    output: &AnalysisOutput,
    raw_total_files_changed: usize,
    analyzed_total_files_changed: usize,
    ignored_files: usize,
    duplicate_file_entries: usize,
    golden: &RepoEvalGoldenResult,
) -> RepoEvalMetrics {
    let total_files_changed = analyzed_total_files_changed;
    let total_groups = output.groups.len();
    let infra_files = output
        .infrastructure_group
        .as_ref()
        .map(|infra| infra.files.len())
        .unwrap_or(0);
    let singleton_groups = output
        .groups
        .iter()
        .filter(|group| group.files.len() == 1)
        .count();
    let max_group_size = output
        .groups
        .iter()
        .map(|group| group.files.len())
        .max()
        .unwrap_or(0);
    let total_grouped_files: usize = output.groups.iter().map(|group| group.files.len()).sum();
    let avg_group_size = if total_groups == 0 {
        0.0
    } else {
        total_grouped_files as f64 / total_groups as f64
    };

    let mut seen = HashSet::new();
    let mut duplicates = false;
    for group in &output.groups {
        for file in &group.files {
            if !seen.insert(file.path.clone()) {
                duplicates = true;
            }
        }
    }
    if let Some(infra) = &output.infrastructure_group {
        for file in &infra.files {
            if !seen.insert(file.clone()) {
                duplicates = true;
            }
        }
    }

    let accounted =
        total_grouped_files + infra_files == analyzed_total_files_changed && !duplicates;

    RepoEvalMetrics {
        raw_total_files_changed,
        total_files_changed,
        ignored_files,
        duplicate_file_entries,
        total_groups,
        groups_per_1000_files: ratio(total_groups, total_files_changed) * 1000.0,
        infra_files,
        infra_ratio: ratio(infra_files, total_files_changed),
        singleton_groups,
        singleton_ratio: ratio(singleton_groups, total_groups),
        max_group_size,
        avg_group_size,
        files_accounted: accounted,
        golden_checks: golden.total_checks,
        golden_satisfied: golden.satisfied_checks,
        golden_score: golden.score,
    }
}

fn score_repo_metrics(
    metrics: &RepoEvalMetrics,
    thresholds: &RepoEvalThresholds,
    golden: &RepoEvalGoldenResult,
) -> RepoEvalScores {
    let group_count = if metrics.total_groups <= thresholds.max_groups {
        1.0
    } else if metrics.total_groups == 0 {
        0.0
    } else {
        (thresholds.max_groups as f64 / metrics.total_groups as f64).clamp(0.0, 1.0)
    };

    let group_density = thresholds
        .max_groups_per_1000_files
        .map(|limit| threshold_score(metrics.groups_per_1000_files, limit));
    let infra_ratio = threshold_score(metrics.infra_ratio, thresholds.max_infra_ratio);
    let singleton_ratio = threshold_score(metrics.singleton_ratio, thresholds.max_singleton_ratio);
    let file_accounting = if metrics.files_accounted { 1.0 } else { 0.0 };
    let golden_score = golden.score;
    let overall = if let Some(group_density_score) = group_density {
        if golden.total_checks == 0 {
            0.20 * group_count
                + 0.20 * group_density_score
                + 0.20 * infra_ratio
                + 0.20 * singleton_ratio
                + 0.20 * file_accounting
        } else {
            0.15 * group_count
                + 0.15 * group_density_score
                + 0.10 * infra_ratio
                + 0.10 * singleton_ratio
                + 0.10 * file_accounting
                + 0.40 * golden_score
        }
    } else if golden.total_checks == 0 {
        0.35 * group_count + 0.25 * infra_ratio + 0.20 * singleton_ratio + 0.20 * file_accounting
    } else {
        0.20 * group_count
            + 0.15 * infra_ratio
            + 0.15 * singleton_ratio
            + 0.10 * file_accounting
            + 0.40 * golden_score
    };

    RepoEvalScores {
        group_count,
        group_density,
        infra_ratio,
        singleton_ratio,
        file_accounting,
        golden: golden_score,
        overall,
    }
}

fn evaluate_repo_expectations(
    output: &AnalysisOutput,
    expectations: Option<&RepoEvalExpectations>,
) -> RepoEvalGoldenResult {
    let Some(expectations) = expectations else {
        return RepoEvalGoldenResult::default();
    };

    let mut file_locations = HashMap::<String, String>::new();
    for group in &output.groups {
        for file in &group.files {
            file_locations.insert(file.path.clone(), group.id.clone());
        }
    }
    let infra_files: HashSet<String> = output
        .infrastructure_group
        .as_ref()
        .map(|infra| infra.files.iter().cloned().collect())
        .unwrap_or_default();
    for file in &infra_files {
        file_locations.insert(file.clone(), "infra".to_string());
    }

    let mut total_checks = 0usize;
    let mut satisfied_checks = 0usize;
    let mut failures = Vec::new();

    for (label, expected, actual) in [
        (
            "group_count_min",
            expectations.group_count_min,
            Some(output.groups.len()),
        ),
        (
            "group_count_max",
            expectations.group_count_max,
            Some(output.groups.len()),
        ),
    ] {
        let Some(boundary) = expected else {
            continue;
        };
        total_checks += 1;
        let matches = match label {
            "group_count_min" => actual.is_some_and(|value| value >= boundary),
            "group_count_max" => actual.is_some_and(|value| value <= boundary),
            _ => false,
        };

        if matches {
            satisfied_checks += 1;
        } else {
            failures.push(format!(
                "{} failed: expected {} {}, got {}",
                label,
                if label.ends_with("_min") { ">=" } else { "<=" },
                boundary,
                output.groups.len()
            ));
        }
    }

    for files in &expectations.same_group {
        total_checks += 1;
        match shared_semantic_group(files, &file_locations) {
            Ok(true) => satisfied_checks += 1,
            Ok(false) => failures.push(format!(
                "same_group failed: expected [{}] to share a semantic group",
                files.join(", ")
            )),
            Err(missing) => failures.push(format!(
                "same_group failed: missing '{}' from analysis output",
                missing
            )),
        }
    }

    for files in &expectations.separate_group {
        total_checks += 1;
        match distinct_destinations(files, &file_locations) {
            Ok(true) => satisfied_checks += 1,
            Ok(false) => failures.push(format!(
                "separate_group failed: expected [{}] to land in distinct destinations",
                files.join(", ")
            )),
            Err(missing) => failures.push(format!(
                "separate_group failed: missing '{}' from analysis output",
                missing
            )),
        }
    }

    for path in &expectations.infrastructure {
        total_checks += 1;
        if infra_files.contains(path) {
            satisfied_checks += 1;
        } else {
            failures.push(format!(
                "infrastructure failed: expected '{}' to be classified as infrastructure",
                path
            ));
        }
    }

    for path in &expectations.non_infrastructure {
        total_checks += 1;
        match file_locations.get(path) {
            Some(location) if location != "infra" => satisfied_checks += 1,
            Some(_) => failures.push(format!(
                "non_infrastructure failed: expected '{}' to remain in a semantic group",
                path
            )),
            None => failures.push(format!(
                "non_infrastructure failed: missing '{}' from analysis output",
                path
            )),
        }
    }

    // Compute file classification coverage
    let classified_set: HashSet<&str> = expectations
        .infrastructure
        .iter()
        .chain(expectations.non_infrastructure.iter())
        .map(|s| s.as_str())
        .collect();

    let all_changed_files: HashSet<&str> = output
        .groups
        .iter()
        .flat_map(|g| g.files.iter().map(|f| f.path.as_str()))
        .chain(
            output
                .infrastructure_group
                .as_ref()
                .into_iter()
                .flat_map(|ig| ig.files.iter().map(|s| s.as_str())),
        )
        .collect();

    let mut unclassified_paths: Vec<String> = all_changed_files
        .iter()
        .filter(|f| !classified_set.contains(**f))
        .map(|f| f.to_string())
        .collect();
    unclassified_paths.sort();

    let classified_files = all_changed_files.len() - unclassified_paths.len();
    let file_coverage = if all_changed_files.is_empty() {
        1.0
    } else {
        classified_files as f64 / all_changed_files.len() as f64
    };

    RepoEvalGoldenResult {
        total_checks,
        satisfied_checks,
        score: if total_checks == 0 {
            1.0
        } else {
            satisfied_checks as f64 / total_checks as f64
        },
        failures,
        file_coverage,
        classified_files,
        unclassified_files: unclassified_paths.len(),
        unclassified_paths,
    }
}

fn shared_semantic_group(
    files: &[String],
    file_locations: &HashMap<String, String>,
) -> Result<bool, String> {
    let mut locations = HashSet::new();
    for path in files {
        let Some(location) = file_locations.get(path) else {
            return Err(path.clone());
        };
        if location == "infra" {
            return Ok(false);
        }
        locations.insert(location.as_str());
    }

    Ok(locations.len() == 1)
}

fn distinct_destinations(
    files: &[String],
    file_locations: &HashMap<String, String>,
) -> Result<bool, String> {
    let mut locations = HashSet::new();
    for path in files {
        let Some(location) = file_locations.get(path) else {
            return Err(path.clone());
        };
        locations.insert(location.as_str());
    }

    Ok(locations.len() == files.len())
}

fn threshold_score(actual: f64, max_allowed: f64) -> f64 {
    if actual <= max_allowed {
        1.0
    } else {
        let span = (1.0 - max_allowed).max(f64::EPSILON);
        (1.0 - ((actual - max_allowed) / span)).clamp(0.0, 1.0)
    }
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn format_repo_eval_text(
    manifest_path: &Path,
    results: &[RepoEvalRun],
    avg_overall: f64,
    min_score: f64,
) -> String {
    let mut lines = Vec::new();
    let show_group_density = results
        .iter()
        .any(|result| result.thresholds.max_groups_per_1000_files.is_some());
    lines.push(format!("Repo eval manifest: {}", manifest_path.display()));
    lines.push(String::new());
    if show_group_density {
        lines.push(
            "name | files(analyzed) | groups | groups/1k | infra% | singletons% | golden | score | status"
                .to_string(),
        );
        lines.push("--- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---".to_string());
    } else {
        lines.push(
            "name | files(analyzed) | groups | infra% | singletons% | golden | score | status"
                .to_string(),
        );
        lines.push("--- | ---: | ---: | ---: | ---: | ---: | ---: | ---".to_string());
    }

    for result in results {
        if show_group_density {
            lines.push(format!(
                "{} | {} | {} | {:.1} | {:.1}% | {:.1}% | {:.2} | {:.2} | {}",
                result.name,
                result.metrics.total_files_changed,
                result.metrics.total_groups,
                result.metrics.groups_per_1000_files,
                result.metrics.infra_ratio * 100.0,
                result.metrics.singleton_ratio * 100.0,
                result.golden.score,
                result.scores.overall,
                if result.passed { "PASS" } else { "FAIL" }
            ));
        } else {
            lines.push(format!(
                "{} | {} | {} | {:.1}% | {:.1}% | {:.2} | {:.2} | {}",
                result.name,
                result.metrics.total_files_changed,
                result.metrics.total_groups,
                result.metrics.infra_ratio * 100.0,
                result.metrics.singleton_ratio * 100.0,
                result.golden.score,
                result.scores.overall,
                if result.passed { "PASS" } else { "FAIL" }
            ));
        }
    }

    let failures: Vec<&RepoEvalRun> = results
        .iter()
        .filter(|result| !result.golden.failures.is_empty())
        .collect();
    if !failures.is_empty() {
        lines.push(String::new());
        lines.push("Golden failures:".to_string());
        for result in failures {
            lines.push(format!("{}:", result.name));
            for failure in &result.golden.failures {
                lines.push(format!("  - {}", failure));
            }
        }
    }

    lines.push(String::new());
    lines.push(format!(
        "Average overall score: {:.2} {} {:.2}",
        avg_overall,
        if avg_overall >= min_score { ">=" } else { "<" },
        min_score
    ));

    lines.join("\n")
}

fn format_repo_eval_html(
    manifest_path: &Path,
    results: &[RepoEvalRun],
    avg_overall: f64,
    min_score: f64,
) -> String {
    let show_group_density = results
        .iter()
        .any(|result| result.thresholds.max_groups_per_1000_files.is_some());
    let mut rows = String::new();
    for result in results {
        if show_group_density {
            rows.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{:.1}</td><td>{:.1}%</td><td>{:.1}%</td><td>{:.2}</td><td>{:.2}</td><td>{}</td></tr>",
                html_escape(&result.name),
                result.metrics.total_files_changed,
                result.metrics.total_groups,
                result.metrics.groups_per_1000_files,
                result.metrics.infra_ratio * 100.0,
                result.metrics.singleton_ratio * 100.0,
                result.golden.score,
                result.scores.overall,
                if result.passed { "PASS" } else { "FAIL" }
            ));
        } else {
            rows.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{:.1}%</td><td>{:.1}%</td><td>{:.2}</td><td>{:.2}</td><td>{}</td></tr>",
                html_escape(&result.name),
                result.metrics.total_files_changed,
                result.metrics.total_groups,
                result.metrics.infra_ratio * 100.0,
                result.metrics.singleton_ratio * 100.0,
                result.golden.score,
                result.scores.overall,
                if result.passed { "PASS" } else { "FAIL" }
            ));
        }
    }

    format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>flowdiff repo eval</title><style>body{{font-family:ui-monospace,monospace;padding:24px;background:#111827;color:#e5e7eb}}table{{border-collapse:collapse;width:100%}}td,th{{border:1px solid #374151;padding:8px;text-align:left;vertical-align:top}}th{{background:#1f2937}}ul{{margin:12px 0 0 18px}}</style></head><body><h1>flowdiff repo eval</h1><p>Manifest: {}</p><p>Average overall score: {:.2} {} {:.2}</p><table><thead><tr>{}</tr></thead><tbody>{}</tbody></table>{}</body></html>",
        html_escape(&manifest_path.display().to_string()),
        avg_overall,
        if avg_overall >= min_score { "&gt;=" } else { "&lt;" },
        min_score,
        if show_group_density {
            "<th>Name</th><th>Files (analyzed)</th><th>Groups</th><th>Groups / 1k files</th><th>Infra %</th><th>Singleton %</th><th>Golden</th><th>Score</th><th>Status</th>"
        } else {
            "<th>Name</th><th>Files (analyzed)</th><th>Groups</th><th>Infra %</th><th>Singleton %</th><th>Golden</th><th>Score</th><th>Status</th>"
        },
        rows,
        format_golden_failures_html(results)
    )
}

fn format_golden_failures_html(results: &[RepoEvalRun]) -> String {
    let failures: Vec<&RepoEvalRun> = results
        .iter()
        .filter(|result| !result.golden.failures.is_empty())
        .collect();
    if failures.is_empty() {
        return String::new();
    }

    let mut sections = String::from("<h2>Golden failures</h2>");
    for result in failures {
        sections.push_str(&format!("<h3>{}</h3><ul>", html_escape(&result.name)));
        for failure in &result.golden.failures {
            sections.push_str(&format!("<li>{}</li>", html_escape(failure)));
        }
        sections.push_str("</ul>");
    }

    sections
}

fn html_escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
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
    use crate::types::{AnalysisSummary, DiffSource, DiffType, FlowGroup, InfrastructureGroup};

    fn make_output(
        groups: Vec<FlowGroup>,
        infra_files: Vec<&str>,
        total_files: u32,
    ) -> AnalysisOutput {
        AnalysisOutput {
            version: "1.0.0".to_string(),
            diff_source: DiffSource {
                diff_type: DiffType::BranchComparison,
                base: Some("main".to_string()),
                head: Some("HEAD".to_string()),
                base_sha: None,
                head_sha: None,
            },
            summary: AnalysisSummary {
                total_files_changed: total_files,
                total_groups: groups.len() as u32,
                languages_detected: vec![],
                frameworks_detected: vec![],
            },
            groups,
            infrastructure_group: if infra_files.is_empty() {
                None
            } else {
                Some(InfrastructureGroup {
                    files: infra_files.into_iter().map(|s| s.to_string()).collect(),
                    sub_groups: vec![],
                    reason: "test".to_string(),
                })
            },
            annotations: None,
        }
    }

    fn make_group(id: &str, files: &[&str]) -> FlowGroup {
        FlowGroup {
            id: id.to_string(),
            name: id.to_string(),
            entrypoint: None,
            files: files
                .iter()
                .enumerate()
                .map(|(idx, path)| crate::types::FileChange {
                    path: (*path).to_string(),
                    flow_position: idx as u32,
                    role: crate::types::FileRole::Infrastructure,
                    changes: crate::types::ChangeStats {
                        additions: 0,
                        deletions: 0,
                    },
                    symbols_changed: vec![],
                })
                .collect(),
            edges: vec![],
            risk_score: 0.0,
            review_order: 1,
        }
    }

    #[test]
    fn test_compute_repo_metrics_detects_duplicates() {
        let output = make_output(
            vec![
                make_group("g1", &["src/a.ts"]),
                make_group("g2", &["src/a.ts"]),
            ],
            vec![],
            2,
        );
        let metrics = compute_repo_metrics(&output, 2, 2, 0, 0, &RepoEvalGoldenResult::default());
        assert!(!metrics.files_accounted);
    }

    #[test]
    fn test_score_repo_metrics_rewards_threshold_compliance() {
        let output = make_output(
            vec![
                make_group("g1", &["src/a.ts", "src/b.ts"]),
                make_group("g2", &["src/c.ts"]),
            ],
            vec!["docs/readme.md"],
            4,
        );
        let golden = RepoEvalGoldenResult::default();
        let metrics = compute_repo_metrics(&output, 4, 4, 0, 0, &golden);
        let scores = score_repo_metrics(&metrics, &RepoEvalThresholds::default(), &golden);
        assert!(scores.overall > 0.8);
    }

    #[test]
    fn test_compute_repo_metrics_uses_analyzed_file_count_after_ignores() {
        let output = make_output(vec![make_group("g1", &["src/a.ts"])], vec!["src/b.ts"], 5);
        let metrics = compute_repo_metrics(&output, 5, 2, 3, 0, &RepoEvalGoldenResult::default());

        assert_eq!(metrics.raw_total_files_changed, 5);
        assert_eq!(metrics.total_files_changed, 2);
        assert_eq!(metrics.ignored_files, 3);
        assert_eq!(metrics.groups_per_1000_files, 500.0);
        assert!(metrics.files_accounted);
    }

    #[test]
    fn test_compute_repo_metrics_tracks_duplicate_diff_entries() {
        let output = make_output(vec![make_group("g1", &["src/a.ts"])], vec!["src/b.ts"], 3);
        let metrics = compute_repo_metrics(&output, 3, 2, 0, 1, &RepoEvalGoldenResult::default());

        assert_eq!(metrics.duplicate_file_entries, 1);
        assert!(metrics.files_accounted);
    }

    #[test]
    fn test_evaluate_repo_expectations_scores_same_group_and_infra() {
        let output = make_output(
            vec![make_group(
                "g1",
                &[
                    "src/vcr/cassette.test.ts",
                    "src/vcr/cassette-invalidation.test.ts",
                ],
            )],
            vec!["package.json"],
            3,
        );
        let expectations = RepoEvalExpectations {
            group_count_min: Some(1),
            group_count_max: Some(2),
            same_group: vec![vec![
                "src/vcr/cassette.test.ts".to_string(),
                "src/vcr/cassette-invalidation.test.ts".to_string(),
            ]],
            separate_group: vec![],
            infrastructure: vec!["package.json".to_string()],
            non_infrastructure: vec!["src/vcr/cassette.test.ts".to_string()],
        };

        let golden = evaluate_repo_expectations(&output, Some(&expectations));
        assert_eq!(golden.total_checks, 5);
        assert_eq!(golden.satisfied_checks, 5);
        assert!(golden.failures.is_empty());
    }

    #[test]
    fn test_score_repo_metrics_uses_group_density_when_configured() {
        let metrics = RepoEvalMetrics {
            raw_total_files_changed: 3000,
            total_files_changed: 3000,
            ignored_files: 0,
            duplicate_file_entries: 0,
            total_groups: 180,
            groups_per_1000_files: 60.0,
            infra_files: 300,
            infra_ratio: 0.1,
            singleton_groups: 10,
            singleton_ratio: 10.0 / 180.0,
            max_group_size: 40,
            avg_group_size: 15.0,
            files_accounted: true,
            golden_checks: 0,
            golden_satisfied: 0,
            golden_score: 1.0,
        };
        let thresholds = RepoEvalThresholds {
            max_groups: 300,
            max_infra_ratio: 0.5,
            max_singleton_ratio: 0.6,
            max_groups_per_1000_files: Some(55.0),
        };

        let scores = score_repo_metrics(&metrics, &thresholds, &RepoEvalGoldenResult::default());

        assert!(scores.group_density.is_some());
        assert!(scores.group_density.unwrap() < 1.0);
        assert!(scores.overall < 1.0);
    }

    #[test]
    fn test_format_repo_eval_text_adds_group_density_column_when_needed() {
        let run = RepoEvalRun {
            name: "large-diff".to_string(),
            path: "/tmp/repo".to_string(),
            diff_spec: "main...HEAD".to_string(),
            thresholds: RepoEvalThresholds {
                max_groups: 300,
                max_infra_ratio: 0.5,
                max_singleton_ratio: 0.6,
                max_groups_per_1000_files: Some(55.0),
            },
            metrics: RepoEvalMetrics {
                raw_total_files_changed: 3000,
                total_files_changed: 3000,
                ignored_files: 0,
                duplicate_file_entries: 0,
                total_groups: 120,
                groups_per_1000_files: 40.0,
                infra_files: 200,
                infra_ratio: 0.066,
                singleton_groups: 4,
                singleton_ratio: 0.033,
                max_group_size: 25,
                avg_group_size: 10.0,
                files_accounted: true,
                golden_checks: 0,
                golden_satisfied: 0,
                golden_score: 1.0,
            },
            scores: RepoEvalScores {
                group_count: 1.0,
                group_density: Some(1.0),
                infra_ratio: 1.0,
                singleton_ratio: 1.0,
                file_accounting: 1.0,
                golden: 1.0,
                overall: 1.0,
            },
            golden: RepoEvalGoldenResult::default(),
            passed: true,
        };

        let text = format_repo_eval_text(
            Path::new("eval/repositories.large-diff.toml"),
            &[run],
            1.0,
            0.9,
        );

        assert!(text.contains("groups/1k"));
        assert!(text.contains("40.0"));
    }

    #[test]
    fn test_manifest_from_file_requires_targets() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repos.toml");
        std::fs::write(&path, "").unwrap();
        let err = RepoEvalManifest::from_file(&path).unwrap_err();
        assert!(err.to_string().contains("at least one [[repos]] entry"));
    }

    #[test]
    fn test_manifest_language_balance_validation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("repos.toml");
        std::fs::write(
            &path,
            r#"
[corpus]
min_repos_per_language = 2

[[repos]]
name = "one"
path = "/tmp/one"
language = "typescript"
"#,
        )
        .unwrap();

        let err = RepoEvalManifest::from_file(&path).unwrap_err();
        assert!(err
            .to_string()
            .contains("does not satisfy language balance"));
    }

    #[test]
    fn test_expectations_validation_rejects_single_file_constraint() {
        let expectations = RepoEvalExpectations {
            group_count_min: None,
            group_count_max: None,
            same_group: vec![vec!["src/a.ts".to_string()]],
            separate_group: vec![],
            infrastructure: vec![],
            non_infrastructure: vec![],
        };

        let err = expectations.validate().unwrap_err();
        assert!(err.to_string().contains("same_group"));
    }
}
