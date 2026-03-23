//! Tauri IPC commands — bridge between the React frontend and flowdiff-core.
//!
//! Each `#[tauri::command]` function is callable from the frontend via `invoke()`.

use std::path::PathBuf;
use std::sync::Mutex;

use log::warn;

use flowdiff_core::cache;
use flowdiff_core::cluster;
use flowdiff_core::config::FlowdiffConfig;
use flowdiff_core::entrypoint;
use flowdiff_core::flow::{self, FlowConfig};
use flowdiff_core::git;
use flowdiff_core::graph::SymbolGraph;
use flowdiff_core::llm;
use flowdiff_core::llm::refinement;
use flowdiff_core::llm::schema::{Pass1Response, Pass2Response, RefinementResponse};
use flowdiff_core::output::{self, build_analysis_output};
use flowdiff_core::pipeline;
use flowdiff_core::rank;
use flowdiff_core::types::{AnalysisOutput, GroupRankInput};

/// Application state shared across commands.
pub struct AppState {
    /// The most recent analysis result, available for subsequent queries.
    pub last_analysis: Mutex<Option<AnalysisOutput>>,
    /// Cached diff result from the most recent analysis, for instant file diff lookups.
    pub last_diff: Mutex<Option<CachedDiff>>,
}

/// Cached diff result with the parameters that produced it, for cache invalidation.
pub struct CachedDiff {
    pub repo_path: PathBuf,
    pub base: Option<String>,
    pub diff_result: git::DiffResult,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            last_analysis: Mutex::new(None),
            last_diff: Mutex::new(None),
        }
    }
}

/// Error type for Tauri commands — must implement `Into<tauri::InvokeError>`.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("Git error: {0}")]
    Git(String),
    #[error("Analysis error: {0}")]
    Analysis(String),
    #[error("Config error: {0}")]
    Config(String),
    #[error("IO error: {0}")]
    Io(String),
    #[error("LLM error: {0}")]
    Llm(String),
}

impl serde::Serialize for CommandError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// Analyze a git diff and return semantic flow groups.
///
/// This is the primary IPC command — equivalent to `flowdiff analyze` in the CLI.
/// When `pr_preview` is true, uses merge-base diff (shows what the branch introduces
/// relative to where it diverged from the base).
#[tauri::command]
pub fn analyze(
    repo_path: String,
    base: Option<String>,
    head: Option<String>,
    range: Option<String>,
    staged: bool,
    unstaged: bool,
    pr_preview: Option<bool>,
    state: tauri::State<'_, AppState>,
) -> Result<AnalysisOutput, CommandError> {
    let repo_path = PathBuf::from(&repo_path);
    let repo_path = std::fs::canonicalize(&repo_path)
        .map_err(|e| CommandError::Io(format!("Invalid repo path: {}", e)))?;

    let repo = git2::Repository::discover(&repo_path)
        .map_err(|e| CommandError::Git(format!("Not a git repository: {}", e)))?;

    let workdir = repo
        .workdir()
        .ok_or_else(|| CommandError::Git("Bare repositories are not supported".to_string()))?
        .to_path_buf();

    // Load config
    let config = FlowdiffConfig::load_from_dir(&workdir)
        .map_err(|e| CommandError::Config(format!("{}", e)))?;

    // Extract diff
    let (diff_result, diff_source) = extract_diff(
        &repo,
        base.clone(),
        head,
        range,
        staged,
        unstaged,
        pr_preview.unwrap_or(false),
    )?;

    // Cache the diff result for subsequent get_file_diff() calls
    match state.last_diff.lock() {
        Ok(mut cached) => {
            *cached = Some(CachedDiff {
                repo_path: repo_path.clone(),
                base: base,
                diff_result: diff_result.clone(),
            });
        }
        Err(e) => warn!("Failed to update last_diff state (lock poisoned): {}", e),
    }

    if diff_result.files.is_empty() {
        let empty_output = AnalysisOutput {
            version: "1.0.0".to_string(),
            diff_source,
            summary: flowdiff_core::types::AnalysisSummary {
                total_files_changed: 0,
                total_groups: 0,
                languages_detected: vec![],
                frameworks_detected: vec![],
            },
            groups: vec![],
            infrastructure_group: None,
            annotations: None,
        };
        match state.last_analysis.lock() {
            Ok(mut last) => *last = Some(empty_output.clone()),
            Err(e) => warn!("Failed to update last_analysis state (lock poisoned): {}", e),
        }
        return Ok(empty_output);
    }

    // Check cache for previously computed results
    let cache_key = cache::compute_cache_key(&diff_result);
    if let Some(cached) = cache::load_cached(&workdir, &cache_key) {
        match state.last_analysis.lock() {
            Ok(mut last) => *last = Some(cached.clone()),
            Err(e) => warn!("Failed to update last_analysis state (lock poisoned): {}", e),
        }
        return Ok(cached);
    }

    // Parse all changed files in parallel
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

    // Build workspace map for monorepo cross-package import resolution
    let workspace_map = flowdiff_core::graph::build_workspace_map(&workdir);

    // Build symbol graph
    let mut graph = SymbolGraph::build_with_workspace(&parsed_files, &workspace_map);

    // Detect entrypoints
    let entrypoints = entrypoint::detect_entrypoints(&parsed_files);

    // Run data flow analysis and enrich graph
    let flow_analysis = flow::analyze_data_flow(&parsed_files, &FlowConfig::default());
    flow::enrich_graph(&mut graph, &flow_analysis);

    // Cluster changed files
    let changed_files: Vec<String> = diff_result
        .files
        .iter()
        .filter(|f| !config.is_ignored(f.path()))
        .map(|f| f.path().to_string())
        .collect();
    let cluster_result = cluster::cluster_files(&graph, &entrypoints, &changed_files);

    // Rank groups
    let weights = config.ranking.clone();
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
                risk: rank::compute_risk_score(
                    risk_flags.has_schema_change,
                    risk_flags.has_api_change,
                    risk_flags.has_auth_change,
                    false,
                ),
                centrality: 0.5,
                surface_area: rank::compute_surface_area(total_add, total_del, 1000),
                uncertainty: if risk_flags.has_test_only { 0.1 } else { 0.5 },
            }
        })
        .collect();

    let ranked = rank::rank_groups(&rank_inputs, &weights);

    // Build output
    let analysis_output =
        build_analysis_output(&diff_result, diff_source, &parsed_files, &cluster_result, &ranked);

    // Cache the deterministic analysis result
    cache::store_cached(&workdir, &cache_key, &analysis_output);

    // Store for subsequent queries
    match state.last_analysis.lock() {
        Ok(mut last) => *last = Some(analysis_output.clone()),
        Err(e) => warn!("Failed to update last_analysis state (lock poisoned): {}", e),
    }

    Ok(analysis_output)
}

/// Get the most recent analysis result without re-running.
#[tauri::command]
pub fn get_last_analysis(
    state: tauri::State<'_, AppState>,
) -> Result<Option<AnalysisOutput>, CommandError> {
    let last = state
        .last_analysis
        .lock()
        .map_err(|e| CommandError::Analysis(format!("Lock poisoned: {}", e)))?;
    Ok(last.clone())
}

/// Generate a Mermaid diagram for a specific group by ID.
#[tauri::command]
pub fn get_mermaid(
    group_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<String, CommandError> {
    let last = state
        .last_analysis
        .lock()
        .map_err(|e| CommandError::Analysis(format!("Lock poisoned: {}", e)))?;

    let analysis = last
        .as_ref()
        .ok_or_else(|| CommandError::Analysis("No analysis available. Run analyze first.".into()))?;

    let group = analysis
        .groups
        .iter()
        .find(|g| g.id == group_id)
        .ok_or_else(|| CommandError::Analysis(format!("Group '{}' not found", group_id)))?;

    Ok(output::generate_mermaid(group))
}

/// Get the diff content (old + new) for a specific file.
/// Returns the raw old and new content for the Monaco diff viewer.
/// Uses the cached DiffResult from the last `analyze()` call when parameters match,
/// avoiding redundant git diff extraction for every file navigation.
#[tauri::command]
pub fn get_file_diff(
    repo_path: String,
    file_path: String,
    base: Option<String>,
    head: Option<String>,
    range: Option<String>,
    staged: bool,
    unstaged: bool,
    state: tauri::State<'_, AppState>,
) -> Result<FileDiffContent, CommandError> {
    // Try to use cached diff from the last analyze() call
    let cached_file = {
        let repo_path_buf = PathBuf::from(&repo_path);
        let repo_path_buf = std::fs::canonicalize(&repo_path_buf).ok();
        let cached = state.last_diff.lock().ok();
        cached.and_then(|guard| {
            let c = guard.as_ref()?;
            let rp = repo_path_buf.as_ref()?;
            if &c.repo_path == rp && c.base == base {
                c.diff_result
                    .files
                    .iter()
                    .find(|f| f.path() == file_path)
                    .map(|f| FileDiffContent {
                        path: file_path.clone(),
                        old_content: f.old_content.clone().unwrap_or_default(),
                        new_content: f.new_content.clone().unwrap_or_default(),
                        language: detect_language(&f.path()),
                    })
            } else {
                None
            }
        })
    };

    if let Some(content) = cached_file {
        return Ok(content);
    }

    // Cache miss — fall back to extracting from git
    get_file_diff_uncached(repo_path, file_path, base, head, range, staged, unstaged)
}

/// Core file diff logic without caching — also callable from integration tests.
pub fn get_file_diff_uncached(
    repo_path: String,
    file_path: String,
    base: Option<String>,
    head: Option<String>,
    range: Option<String>,
    staged: bool,
    unstaged: bool,
) -> Result<FileDiffContent, CommandError> {
    // Security: reject paths with traversal components or absolute paths
    // to prevent path traversal via IPC from a compromised frontend.
    let fp = std::path::Path::new(&file_path);
    if fp.is_absolute()
        || fp
            .components()
            .any(|c| c == std::path::Component::ParentDir)
    {
        return Err(CommandError::Io(format!(
            "Invalid file path (path traversal rejected): {}",
            file_path
        )));
    }

    let repo_path_buf = PathBuf::from(&repo_path);
    let repo_path_buf = std::fs::canonicalize(&repo_path_buf)
        .map_err(|e| CommandError::Io(format!("Invalid repo path: {}", e)))?;

    let repo = git2::Repository::discover(&repo_path_buf)
        .map_err(|e| CommandError::Git(format!("Not a git repository: {}", e)))?;

    let (diff_result, _) = extract_diff(&repo, base, head, range, staged, unstaged, false)?;

    let file_diff = diff_result
        .files
        .iter()
        .find(|f| f.path() == file_path)
        .ok_or_else(|| CommandError::Analysis(format!("File '{}' not found in diff", file_path)))?;

    Ok(FileDiffContent {
        path: file_path,
        old_content: file_diff.old_content.clone().unwrap_or_default(),
        new_content: file_diff.new_content.clone().unwrap_or_default(),
        language: detect_language(&file_diff.path()),
    })
}

/// Run LLM Pass 1 (overview annotation) on the cached analysis.
///
/// Returns structured overview with per-group summaries, risk flags,
/// and suggested review order. The result is also stored in the cached
/// analysis output's `annotations` field.
#[tauri::command]
pub async fn annotate_overview(
    repo_path: Option<String>,
    llm_provider: Option<String>,
    llm_model: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<Pass1Response, CommandError> {
    // Get the cached analysis to build the request
    let analysis = {
        let last = state
            .last_analysis
            .lock()
            .map_err(|e| CommandError::Analysis(format!("Lock poisoned: {}", e)))?;
        last.clone()
            .ok_or_else(|| CommandError::Analysis("No analysis available. Run analyze first.".into()))?
    };

    // Load config from the repo directory (not default)
    let (mut config, _) = load_config_from_path(repo_path.as_deref());

    // Apply frontend overrides if provided
    if let Some(p) = llm_provider {
        config.llm.provider = Some(p);
    }
    if let Some(m) = llm_model {
        config.llm.model = Some(m);
    }

    // Create LLM provider
    let provider = llm::create_provider(&config.llm)
        .map_err(|e| CommandError::Llm(format!("{}", e)))?;

    // Build Pass 1 request
    let flow_groups: Vec<llm::schema::Pass1GroupInput> = analysis
        .groups
        .iter()
        .map(|g| llm::schema::Pass1GroupInput {
            id: g.id.clone(),
            name: g.name.clone(),
            entrypoint: g
                .entrypoint
                .as_ref()
                .map(|ep| format!("{}::{}", ep.file, ep.symbol)),
            files: g.files.iter().map(|f| f.path.clone()).collect(),
            risk_score: g.risk_score,
            edge_summary: g
                .edges
                .iter()
                .map(|e| format!("{} -> {}", e.from, e.to))
                .collect::<Vec<_>>()
                .join(", "),
        })
        .collect();

    let request = llm::schema::Pass1Request {
        diff_summary: format!(
            "{} files changed across {} groups",
            analysis.summary.total_files_changed, analysis.summary.total_groups,
        ),
        flow_groups,
        graph_summary: format!(
            "{} groups, {} total files",
            analysis.summary.total_groups, analysis.summary.total_files_changed,
        ),
    };

    let response = provider
        .annotate_overview(&request)
        .await
        .map_err(|e| CommandError::Llm(format!("{}", e)))?;

    // Store the annotations in the cached analysis
    match state.last_analysis.lock() {
        Ok(mut last) => {
            if let Some(ref mut a) = *last {
                a.annotations = Some(
                    serde_json::to_value(&response)
                        .map_err(|e| CommandError::Llm(format!("Failed to serialize response: {}", e)))?,
                );
            }
        }
        Err(e) => warn!("Failed to update last_analysis annotations (lock poisoned): {}", e),
    }

    Ok(response)
}

/// Run LLM Pass 2 (deep analysis) on a specific group.
///
/// Returns per-file annotations, flow narrative, and cross-cutting concerns.
#[tauri::command]
pub async fn annotate_group(
    group_id: String,
    repo_path: String,
    base: Option<String>,
    head: Option<String>,
    range: Option<String>,
    staged: bool,
    unstaged: bool,
    llm_provider: Option<String>,
    llm_model: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<Pass2Response, CommandError> {
    // Get the cached analysis to find the group
    let analysis = {
        let last = state
            .last_analysis
            .lock()
            .map_err(|e| CommandError::Analysis(format!("Lock poisoned: {}", e)))?;
        last.clone()
            .ok_or_else(|| CommandError::Analysis("No analysis available. Run analyze first.".into()))?
    };

    let group = analysis
        .groups
        .iter()
        .find(|g| g.id == group_id)
        .ok_or_else(|| CommandError::Analysis(format!("Group '{}' not found", group_id)))?
        .clone();

    // Get file diffs for Pass 2 context
    let repo_path_buf = PathBuf::from(&repo_path);
    let repo_path_buf = std::fs::canonicalize(&repo_path_buf)
        .map_err(|e| CommandError::Io(format!("Invalid repo path: {}", e)))?;
    let repo = git2::Repository::discover(&repo_path_buf)
        .map_err(|e| CommandError::Git(format!("Not a git repository: {}", e)))?;

    let (diff_result, _) = extract_diff(&repo, base, head, range, staged, unstaged, false)?;

    // Build Pass 2 file inputs with diffs
    let files: Vec<llm::schema::Pass2FileInput> = group
        .files
        .iter()
        .map(|f| {
            let file_diff = diff_result.files.iter().find(|d| d.path() == f.path);
            let diff_text = file_diff
                .map(|d| {
                    // Build a simple unified diff representation
                    let old = d.old_content.as_deref().unwrap_or("");
                    let new = d.new_content.as_deref().unwrap_or("");
                    format!(
                        "--- a/{}\n+++ b/{}\n{}",
                        f.path,
                        f.path,
                        simple_unified_diff(old, new)
                    )
                })
                .unwrap_or_default();
            let new_content = file_diff.and_then(|d| d.new_content.clone());

            llm::schema::Pass2FileInput {
                path: f.path.clone(),
                diff: diff_text,
                new_content,
                role: format!("{:?}", f.role),
            }
        })
        .collect();

    // Build graph context
    let graph_context = group
        .edges
        .iter()
        .map(|e| format!("{} --{:?}--> {}", e.from, e.edge_type, e.to))
        .collect::<Vec<_>>()
        .join("\n");

    let (mut config, _) = load_config_from_path(Some(&repo_path));
    if let Some(p) = llm_provider {
        config.llm.provider = Some(p);
    }
    if let Some(m) = llm_model {
        config.llm.model = Some(m);
    }
    let provider = llm::create_provider(&config.llm)
        .map_err(|e| CommandError::Llm(format!("{}", e)))?;

    let request = llm::schema::Pass2Request {
        group_id: group.id.clone(),
        group_name: group.name.clone(),
        files,
        graph_context,
    };

    let response = provider
        .annotate_group(&request)
        .await
        .map_err(|e| CommandError::Llm(format!("{}", e)))?;

    Ok(response)
}

/// Run LLM refinement pass on the cached analysis groups.
///
/// Takes the deterministic groups (v1) and asks an LLM to suggest structural
/// improvements: splits, merges, re-ranks, and reclassifications. Applies the
/// refinement operations and returns the result containing both the refined
/// groups and the raw refinement response (for change indicators in the UI).
///
/// Falls back to returning the original groups if refinement produces no changes
/// or validation fails.
#[tauri::command]
pub async fn refine_groups(
    repo_path: Option<String>,
    llm_provider: Option<String>,
    llm_model: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<RefinementResult, CommandError> {
    // Get the cached analysis
    let analysis = {
        let last = state
            .last_analysis
            .lock()
            .map_err(|e| CommandError::Analysis(format!("Lock poisoned: {}", e)))?;
        last.clone()
            .ok_or_else(|| CommandError::Analysis("No analysis available. Run analyze first.".into()))?
    };

    // Load config, applying frontend overrides
    let (mut config, _) = load_config_from_path(repo_path.as_deref());
    // Use refinement-specific provider/model if set, otherwise fall back to overrides
    if let Some(p) = llm_provider {
        config.llm.refinement.provider = Some(p.clone());
        if config.llm.provider.is_none() {
            config.llm.provider = Some(p);
        }
    }
    if let Some(m) = llm_model {
        config.llm.refinement.model = Some(m.clone());
        if config.llm.model.is_none() {
            config.llm.model = Some(m);
        }
    }

    // Build LLM config for the refinement provider
    let refinement_llm_config = flowdiff_core::config::LlmConfig {
        provider: config.llm.refinement.provider.clone().or(config.llm.provider.clone()),
        model: config.llm.refinement.model.clone().or(config.llm.model.clone()),
        key_cmd: config.llm.key_cmd.clone(),
        key: config.llm.key.clone(),
        refinement: config.llm.refinement.clone(),
    };

    let provider = llm::create_provider(&refinement_llm_config)
        .map_err(|e| CommandError::Llm(format!("{}", e)))?;

    // Serialize analysis for the refinement request
    let analysis_json = serde_json::to_string_pretty(&analysis)
        .map_err(|e| CommandError::Llm(format!("Failed to serialize analysis: {}", e)))?;

    let diff_summary = format!(
        "{} files changed across {} groups",
        analysis.summary.total_files_changed, analysis.summary.total_groups,
    );

    let request = refinement::build_refinement_request(
        &analysis.groups,
        &analysis_json,
        &diff_summary,
    );

    let response = provider
        .refine_groups(&request)
        .await
        .map_err(|e| CommandError::Llm(format!("{}", e)))?;

    let provider_name = refinement_llm_config
        .provider
        .unwrap_or_else(|| "anthropic".to_string());
    let model_name = refinement_llm_config
        .model
        .unwrap_or_else(|| default_model_for_provider(&provider_name).to_string());

    if !refinement::has_refinements(&response) {
        return Ok(RefinementResult {
            refined_groups: analysis.groups.clone(),
            infrastructure_group: analysis.infrastructure_group.clone(),
            refinement_response: response,
            provider: provider_name,
            model: model_name,
            had_changes: false,
        });
    }

    // Apply the refinement
    match refinement::apply_refinement(
        &analysis.groups,
        analysis.infrastructure_group.as_ref(),
        &response,
    ) {
        Ok((refined_groups, infra)) => {
            // Update cached analysis with refined groups
            match state.last_analysis.lock() {
                Ok(mut last) => {
                    if let Some(ref mut a) = *last {
                        a.groups = refined_groups.clone();
                        a.infrastructure_group = infra.clone();
                    }
                }
                Err(e) => warn!("Failed to update last_analysis with refinement (lock poisoned): {}", e),
            }

            Ok(RefinementResult {
                refined_groups,
                infrastructure_group: infra,
                refinement_response: response,
                provider: provider_name,
                model: model_name,
                had_changes: true,
            })
        }
        Err(e) => {
            // Validation failed — return original groups with error info
            Err(CommandError::Llm(format!("Refinement validation failed: {}", e)))
        }
    }
}

/// Result of a refinement pass, including both the refined groups and
/// the raw refinement operations (for UI change indicators).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RefinementResult {
    /// The refined flow groups (v2) — or original groups if no changes.
    pub refined_groups: Vec<flowdiff_core::types::FlowGroup>,
    /// The refined infrastructure group.
    pub infrastructure_group: Option<flowdiff_core::types::InfrastructureGroup>,
    /// The raw refinement response with split/merge/re-rank/reclassify operations.
    pub refinement_response: RefinementResponse,
    /// Which provider performed the refinement.
    pub provider: String,
    /// Which model performed the refinement.
    pub model: String,
    /// Whether the refinement actually produced changes.
    pub had_changes: bool,
}

/// List all local branches in the repository.
///
/// Returns branches sorted with current branch first, then alphabetically.
#[tauri::command]
pub fn list_branches(
    repo_path: String,
) -> Result<Vec<git::BranchInfo>, CommandError> {
    let repo = open_repo(&repo_path)?;
    git::list_branches(&repo).map_err(|e| CommandError::Git(format!("{}", e)))
}

/// List all git worktrees for the repository.
#[tauri::command]
pub fn list_worktrees(
    repo_path: String,
) -> Result<Vec<git::WorktreeInfo>, CommandError> {
    let repo = open_repo(&repo_path)?;
    git::list_worktrees(&repo).map_err(|e| CommandError::Git(format!("{}", e)))
}

/// Get the current branch's tracking status (ahead/behind upstream).
#[tauri::command]
pub fn get_branch_status(
    repo_path: String,
) -> Result<git::BranchStatus, CommandError> {
    let repo = open_repo(&repo_path)?;
    git::get_branch_status(&repo).map_err(|e| CommandError::Git(format!("{}", e)))
}

/// Auto-detect the default branch and current branch for a repository.
///
/// Returns a summary useful for the UI to set up initial state.
#[tauri::command]
pub fn get_repo_info(
    repo_path: String,
) -> Result<RepoInfo, CommandError> {
    let repo = open_repo(&repo_path)?;

    let current = git::current_branch(&repo);
    let default_branch = git::detect_default_branch(&repo)
        .unwrap_or_else(|_| "main".to_string());
    let branches = git::list_branches(&repo)
        .map_err(|e| CommandError::Git(format!("{}", e)))?;
    let worktrees = git::list_worktrees(&repo)
        .map_err(|e| CommandError::Git(format!("{}", e)))?;
    let status = git::get_branch_status(&repo).ok();

    Ok(RepoInfo {
        current_branch: current,
        default_branch,
        branches,
        worktrees,
        status,
    })
}

/// Check whether an LLM API key is configured and available.
///
/// Attempts to resolve the API key using the same logic as `create_provider`:
/// key_cmd > FLOWDIFF_API_KEY > provider-specific env var. Returns true if
/// a key is available, false otherwise.
#[tauri::command]
pub fn check_api_key(repo_path: Option<String>) -> Result<bool, CommandError> {
    let config = if let Some(ref path) = repo_path {
        let repo_path = PathBuf::from(path);
        if let Ok(canonical) = std::fs::canonicalize(&repo_path) {
            if let Ok(repo) = git2::Repository::discover(&canonical) {
                if let Some(workdir) = repo.workdir() {
                    FlowdiffConfig::load_from_dir(workdir).unwrap_or_default()
                } else {
                    FlowdiffConfig::default()
                }
            } else {
                FlowdiffConfig::default()
            }
        } else {
            FlowdiffConfig::default()
        }
    } else {
        FlowdiffConfig::default()
    };

    let provider_name = config
        .llm
        .provider
        .as_deref()
        .unwrap_or("anthropic");

    Ok(llm::resolve_api_key(&config.llm, provider_name).is_ok())
}

/// Get LLM settings from the project's `.flowdiff.toml` and environment.
///
/// Reads the config file, resolves API key availability, and returns a
/// unified `LlmSettings` struct for the settings panel.
#[tauri::command]
pub fn get_llm_settings(repo_path: Option<String>) -> Result<LlmSettings, CommandError> {
    let (config, workdir) = load_config_from_path(repo_path.as_deref());

    let provider = config
        .llm
        .provider
        .clone()
        .unwrap_or_else(|| "anthropic".to_string());
    let model = config
        .llm
        .model
        .clone()
        .unwrap_or_else(|| default_model_for_provider(&provider).to_string());

    let has_api_key = llm::resolve_api_key(&config.llm, &provider).is_ok();

    let api_key_source = if config.llm.key_cmd.is_some() {
        "key_cmd".to_string()
    } else if config.llm.key.as_ref().is_some_and(|k| !k.is_empty()) {
        "config file".to_string()
    } else if std::env::var("FLOWDIFF_API_KEY").is_ok() {
        "FLOWDIFF_API_KEY".to_string()
    } else {
        let env_var = match provider.as_str() {
            "anthropic" => "ANTHROPIC_API_KEY",
            "openai" => "OPENAI_API_KEY",
            "gemini" => "GEMINI_API_KEY",
            _ => "none",
        };
        if std::env::var(env_var).is_ok() {
            env_var.to_string()
        } else if workdir.is_some() {
            "none (configure in .flowdiff.toml or env)".to_string()
        } else {
            "none".to_string()
        }
    };

    let refinement_provider = config
        .llm
        .refinement
        .provider
        .clone()
        .unwrap_or_else(|| provider.clone());
    let refinement_model = config
        .llm
        .refinement
        .model
        .clone()
        .unwrap_or_else(|| default_model_for_provider(&refinement_provider).to_string());

    Ok(LlmSettings {
        annotations_enabled: has_api_key,
        refinement_enabled: config.llm.refinement.enabled,
        provider,
        model,
        api_key_source,
        has_api_key,
        refinement_provider,
        refinement_model,
        refinement_max_iterations: config.llm.refinement.max_iterations,
    })
}

/// Save LLM settings to the project's `.flowdiff.toml`.
///
/// Loads the existing config (preserving non-LLM sections), updates the LLM
/// section with the provided settings, and writes back.
#[tauri::command]
pub fn save_llm_settings(
    repo_path: String,
    settings: LlmSettings,
) -> Result<(), CommandError> {
    let repo_path_buf = PathBuf::from(&repo_path);
    let repo_path_buf = std::fs::canonicalize(&repo_path_buf)
        .map_err(|e| CommandError::Io(format!("Invalid repo path: {}", e)))?;
    let repo = git2::Repository::discover(&repo_path_buf)
        .map_err(|e| CommandError::Git(format!("Not a git repository: {}", e)))?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| CommandError::Git("Bare repositories are not supported".to_string()))?;

    let mut config = FlowdiffConfig::load_from_dir(workdir)
        .map_err(|e| CommandError::Config(format!("{}", e)))?;

    // Update LLM section
    config.llm.provider = Some(settings.provider);
    config.llm.model = Some(settings.model);
    // Don't overwrite key_cmd — that's managed manually
    config.llm.refinement.enabled = settings.refinement_enabled;
    config.llm.refinement.provider = Some(settings.refinement_provider);
    config.llm.refinement.model = Some(settings.refinement_model);
    config.llm.refinement.max_iterations = settings.refinement_max_iterations;

    config
        .save_to_dir(workdir)
        .map_err(|e| CommandError::Config(format!("Failed to save config: {}", e)))?;

    Ok(())
}

/// Save an API key to `.flowdiff.toml` under `[llm] key = "..."`.
///
/// The key is stored directly in the config file. Precedence is maintained:
/// `key_cmd` > `key` (config) > env vars.
#[tauri::command]
pub fn save_api_key(repo_path: String, api_key: String) -> Result<(), CommandError> {
    let repo_path_buf = PathBuf::from(&repo_path);
    let repo_path_buf = std::fs::canonicalize(&repo_path_buf)
        .map_err(|e| CommandError::Io(format!("Invalid repo path: {}", e)))?;
    let repo = git2::Repository::discover(&repo_path_buf)
        .map_err(|e| CommandError::Git(format!("Not a git repository: {}", e)))?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| CommandError::Git("Bare repositories are not supported".to_string()))?;

    let mut config = FlowdiffConfig::load_from_dir(workdir)
        .map_err(|e| CommandError::Config(format!("{}", e)))?;

    config.llm.key = Some(api_key);

    config
        .save_to_dir(workdir)
        .map_err(|e| CommandError::Config(format!("Failed to save config: {}", e)))?;

    Ok(())
}

/// Remove the stored API key from `.flowdiff.toml`.
#[tauri::command]
pub fn clear_api_key(repo_path: String) -> Result<(), CommandError> {
    let repo_path_buf = PathBuf::from(&repo_path);
    let repo_path_buf = std::fs::canonicalize(&repo_path_buf)
        .map_err(|e| CommandError::Io(format!("Invalid repo path: {}", e)))?;
    let repo = git2::Repository::discover(&repo_path_buf)
        .map_err(|e| CommandError::Git(format!("Not a git repository: {}", e)))?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| CommandError::Git("Bare repositories are not supported".to_string()))?;

    let mut config = FlowdiffConfig::load_from_dir(workdir)
        .map_err(|e| CommandError::Config(format!("{}", e)))?;

    config.llm.key = None;

    config
        .save_to_dir(workdir)
        .map_err(|e| CommandError::Config(format!("Failed to save config: {}", e)))?;

    Ok(())
}

/// Get the current ignore paths from `.flowdiff.toml`.
#[tauri::command]
pub fn get_ignore_paths(repo_path: Option<String>) -> Result<Vec<String>, CommandError> {
    let (config, _workdir) = load_config_from_path(repo_path.as_deref());
    Ok(config.ignore.paths)
}

/// Save ignore paths to `.flowdiff.toml`.
///
/// Loads the existing config (preserving other sections), updates the ignore
/// paths, and writes back.
#[tauri::command]
pub fn save_ignore_paths(repo_path: String, paths: Vec<String>) -> Result<(), CommandError> {
    let repo_path_buf = PathBuf::from(&repo_path);
    let repo_path_buf = std::fs::canonicalize(&repo_path_buf)
        .map_err(|e| CommandError::Io(format!("Invalid repo path: {}", e)))?;
    let repo = git2::Repository::discover(&repo_path_buf)
        .map_err(|e| CommandError::Git(format!("Not a git repository: {}", e)))?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| CommandError::Git("Bare repositories are not supported".to_string()))?;

    let mut config = FlowdiffConfig::load_from_dir(workdir)
        .map_err(|e| CommandError::Config(format!("{}", e)))?;

    config.ignore.paths = paths;

    config
        .save_to_dir(workdir)
        .map_err(|e| CommandError::Config(format!("Failed to save config: {}", e)))?;

    Ok(())
}

/// macOS app bundle name for each editor.
#[cfg(target_os = "macos")]
fn macos_app_name(editor: &str) -> Option<&'static str> {
    match editor {
        "vscode" => Some("Visual Studio Code"),
        "cursor" => Some("Cursor"),
        "zed" => Some("Zed"),
        _ => None,
    }
}

/// Check if a macOS .app bundle exists in /Applications or ~/Applications.
#[cfg(target_os = "macos")]
fn macos_app_exists(app_name: &str) -> bool {
    let global = format!("/Applications/{}.app", app_name);
    if PathBuf::from(&global).exists() {
        return true;
    }
    if let Ok(home) = std::env::var("HOME") {
        let user = format!("{}/Applications/{}.app", home, app_name);
        if PathBuf::from(&user).exists() {
            return true;
        }
    }
    false
}

/// Open a file in an external editor.
///
/// On macOS, uses `open -a "App Name"` for GUI editors (works without PATH).
/// Falls back to CLI binary for non-macOS or terminal-based editors.
#[tauri::command]
pub fn open_in_editor(editor: String, file_path: String) -> Result<(), CommandError> {
    let path = PathBuf::from(&file_path);
    if !path.exists() {
        return Err(CommandError::Io(format!("File not found: {}", file_path)));
    }

    let result = match editor.as_str() {
        "vscode" | "cursor" | "zed" => {
            #[cfg(target_os = "macos")]
            {
                // Use the CLI binary via the app bundle's bin/ path for proper workspace trust.
                // `open -a` opens files as untrusted; the CLI opens in the existing workspace.
                let cli_path = match editor.as_str() {
                    "vscode" => "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code",
                    "cursor" => "/Applications/Cursor.app/Contents/Resources/app/bin/cursor",
                    "zed" => "/Applications/Zed.app/Contents/MacOS/cli",
                    _ => unreachable!(),
                };
                if std::path::Path::new(cli_path).exists() {
                    std::process::Command::new(cli_path)
                        .args(["--reuse-window", "--goto", &file_path])
                        .spawn()
                } else {
                    // Fallback to `open -a` if CLI path not found
                    let app_name = macos_app_name(&editor).unwrap();
                    std::process::Command::new("open")
                        .args(["-a", app_name, &file_path])
                        .spawn()
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                let bin = match editor.as_str() {
                    "vscode" => "code",
                    "cursor" => "cursor",
                    "zed" => "zed",
                    _ => unreachable!(),
                };
                std::process::Command::new(bin).args(["--reuse-window", "--goto", &file_path]).spawn()
            }
        }
        "vim" => {
            #[cfg(target_os = "macos")]
            {
                // Open vim in a NEW Terminal window via AppleScript
                let escaped = file_path.replace('\\', "\\\\").replace('"', "\\\"");
                std::process::Command::new("osascript")
                    .args([
                        "-e",
                        &format!(
                            "tell application \"Terminal\"\n\
                                activate\n\
                                do script \"vim \\\"{}\\\"\" \n\
                            end tell",
                            escaped
                        ),
                    ])
                    .spawn()
            }
            #[cfg(not(target_os = "macos"))]
            {
                std::process::Command::new("vim").arg(&file_path).spawn()
            }
        }
        "terminal" => {
            let dir = if path.is_dir() {
                file_path.clone()
            } else {
                path.parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| file_path.clone())
            };
            #[cfg(target_os = "macos")]
            {
                // Use AppleScript to open Terminal and cd to the directory
                let escaped = dir.replace('\\', "\\\\").replace('"', "\\\"");
                std::process::Command::new("osascript")
                    .args([
                        "-e",
                        &format!(
                            "tell application \"Terminal\"\n\
                                activate\n\
                                do script \"cd \\\"{}\\\"\" \n\
                            end tell",
                            escaped
                        ),
                    ])
                    .spawn()
            }
            #[cfg(target_os = "linux")]
            {
                std::process::Command::new("xdg-open").arg(&dir).spawn()
            }
            #[cfg(target_os = "windows")]
            {
                std::process::Command::new("cmd").args(["/c", "start", "cmd", "/k", &format!("cd /d {}", dir)]).spawn()
            }
        }
        other => {
            return Err(CommandError::Io(format!("Unknown editor: {}", other)));
        }
    };

    match result {
        Ok(_) => Ok(()),
        Err(e) => {
            let label = match editor.as_str() {
                "vscode" => "VS Code",
                "cursor" => "Cursor",
                "zed" => "Zed",
                "vim" => "Vim",
                "terminal" => "Terminal",
                _ => &editor,
            };
            Err(CommandError::Io(format!(
                "Failed to open {} — is it installed? ({})",
                label, e
            )))
        }
    }
}

/// Check which editors are available on the system.
///
/// On macOS, checks for .app bundles in /Applications (works without PATH).
/// On other platforms, uses `which`/`where` to find CLI binaries.
#[tauri::command]
pub fn check_editors_available() -> std::collections::HashMap<String, bool> {
    let mut result = std::collections::HashMap::new();

    // GUI editors
    for id in &["vscode", "cursor", "zed"] {
        let available = {
            #[cfg(target_os = "macos")]
            {
                macos_app_name(id)
                    .map(|name| macos_app_exists(name))
                    .unwrap_or(false)
            }
            #[cfg(not(target_os = "macos"))]
            {
                let bin = match *id {
                    "vscode" => "code",
                    "cursor" => "cursor",
                    "zed" => "zed",
                    _ => id,
                };
                #[cfg(unix)]
                {
                    std::process::Command::new("which")
                        .arg(bin)
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false)
                }
                #[cfg(windows)]
                {
                    std::process::Command::new("where")
                        .arg(bin)
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false)
                }
            }
        };
        result.insert(id.to_string(), available);
    }

    // vim — check binary in PATH (available on most systems)
    let vim_available = {
        #[cfg(unix)]
        {
            std::process::Command::new("which")
                .arg("vim")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
        #[cfg(windows)]
        {
            std::process::Command::new("where")
                .arg("vim")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
    };
    result.insert("vim".to_string(), vim_available);

    // Terminal is always available
    result.insert("terminal".to_string(), true);

    result
}

/// Load config from a repo path, returning both config and optional workdir.
fn load_config_from_path(repo_path: Option<&str>) -> (FlowdiffConfig, Option<PathBuf>) {
    if let Some(path) = repo_path {
        let repo_path = PathBuf::from(path);
        if let Ok(canonical) = std::fs::canonicalize(&repo_path) {
            if let Ok(repo) = git2::Repository::discover(&canonical) {
                if let Some(workdir) = repo.workdir() {
                    let config = FlowdiffConfig::load_from_dir(workdir).unwrap_or_default();
                    return (config, Some(workdir.to_path_buf()));
                }
            }
        }
    }
    (FlowdiffConfig::default(), None)
}

/// Get the default model for a provider.
fn default_model_for_provider(provider: &str) -> &str {
    match provider {
        "anthropic" => "claude-sonnet-4-6",
        "openai" => "gpt-4.1",
        "gemini" => "gemini-2.5-flash",
        _ => "claude-sonnet-4-6",
    }
}

/// LLM settings for the UI — surface for the settings panel.
///
/// Contains the current provider/model configuration, API key status,
/// and annotation/refinement toggle states. Returned by `get_llm_settings`
/// and accepted by `save_llm_settings`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LlmSettings {
    /// Whether LLM annotations are enabled (controls visibility of Summarize PR / Analyze buttons).
    pub annotations_enabled: bool,
    /// Whether LLM refinement is enabled.
    pub refinement_enabled: bool,
    /// Selected LLM provider: "anthropic", "openai", or "gemini".
    pub provider: String,
    /// Selected model identifier.
    pub model: String,
    /// How the API key is configured.
    pub api_key_source: String,
    /// Whether an API key is actually available (resolvable).
    pub has_api_key: bool,
    /// Refinement provider (can differ from annotation provider).
    pub refinement_provider: String,
    /// Refinement model.
    pub refinement_model: String,
    /// Maximum refinement iterations.
    pub refinement_max_iterations: u32,
}

/// Summary of repository state for the UI.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RepoInfo {
    pub current_branch: Option<String>,
    pub default_branch: String,
    pub branches: Vec<git::BranchInfo>,
    pub worktrees: Vec<git::WorktreeInfo>,
    pub status: Option<git::BranchStatus>,
}

/// Open a repository from a path, with canonicalization and error handling.
fn open_repo(repo_path: &str) -> Result<git2::Repository, CommandError> {
    let path = PathBuf::from(repo_path);
    let path = std::fs::canonicalize(&path)
        .map_err(|e| CommandError::Io(format!("Invalid repo path: {}", e)))?;
    git2::Repository::discover(&path)
        .map_err(|e| CommandError::Git(format!("Not a git repository: {}", e)))
}

/// Build a simple unified diff from old and new content.
fn simple_unified_diff(old: &str, new: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let mut result = String::new();
    // Simple approach: show all old lines as removed, all new lines as added
    // For a real implementation, use a proper diff algorithm
    for line in &old_lines {
        result.push_str(&format!("-{}\n", line));
    }
    for line in &new_lines {
        result.push_str(&format!("+{}\n", line));
    }
    result
}

/// File diff content for the Monaco diff viewer.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FileDiffContent {
    pub path: String,
    pub old_content: String,
    pub new_content: String,
    pub language: String,
}

// ── Internal helpers ──

fn extract_diff(
    repo: &git2::Repository,
    base: Option<String>,
    head: Option<String>,
    range: Option<String>,
    staged: bool,
    unstaged: bool,
    pr_preview: bool,
) -> Result<(git::DiffResult, flowdiff_core::types::DiffSource), CommandError> {
    if let Some(ref range) = range {
        let diff = git::diff_range(repo, range)
            .map_err(|e| CommandError::Git(format!("{}", e)))?;
        let source = output::diff_source_range(
            range,
            diff.base_sha.as_deref(),
            diff.head_sha.as_deref(),
        );
        Ok((diff, source))
    } else if staged {
        let diff = git::diff_staged(repo)
            .map_err(|e| CommandError::Git(format!("{}", e)))?;
        let source = output::diff_source_staged();
        Ok((diff, source))
    } else if unstaged {
        let diff = git::diff_unstaged(repo)
            .map_err(|e| CommandError::Git(format!("{}", e)))?;
        let source = output::diff_source_unstaged();
        Ok((diff, source))
    } else if pr_preview {
        // PR preview mode: use merge-base diff
        // Auto-detect default branch if no base ref provided
        let detected_default = if base.is_none() {
            git::detect_default_branch(repo).ok()
        } else {
            None
        };
        let base_ref = base.as_deref()
            .or(detected_default.as_deref())
            .unwrap_or("main");
        let head_ref = head.as_deref().unwrap_or("HEAD");
        let diff = git::diff_merge_base(repo, base_ref, head_ref)
            .map_err(|e| CommandError::Git(format!(
                "Failed to compute merge-base diff between '{}' and '{}': {}",
                base_ref, head_ref, e
            )))?;
        let source = output::diff_source_branch(
            base_ref,
            head_ref,
            diff.base_sha.as_deref(),
            diff.head_sha.as_deref(),
        );
        Ok((diff, source))
    } else {
        let base_ref = base.as_deref().unwrap_or("main");
        let head_ref = head.as_deref().unwrap_or("HEAD");
        let diff = git::diff_refs(repo, base_ref, head_ref)
            .map_err(|e| CommandError::Git(format!("{}", e)))?;
        let source = output::diff_source_branch(
            base_ref,
            head_ref,
            diff.base_sha.as_deref(),
            diff.head_sha.as_deref(),
        );
        Ok((diff, source))
    }
}

// ── Review Comments ──────────────────────────────────────────────────

/// A single review comment — can be scoped to a group, file, or code range.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReviewComment {
    /// Unique identifier for the comment.
    pub id: String,
    /// Comment scope: "code", "file", or "group".
    #[serde(rename = "type")]
    pub comment_type: String,
    /// The flow group this comment belongs to.
    pub group_id: String,
    /// File path (null for group-level comments).
    pub file_path: Option<String>,
    /// Start line (null for file/group-level comments).
    pub start_line: Option<u32>,
    /// End line (null for file/group-level comments).
    pub end_line: Option<u32>,
    /// The selected code snippet (for code-level comments).
    pub selected_code: Option<String>,
    /// The comment text.
    pub text: String,
    /// ISO 8601 timestamp when the comment was created.
    pub created_at: String,
}

/// Container for persisted comments, keyed by analysis hash.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommentsFile {
    /// Hash of the analysis run these comments belong to.
    pub analysis_hash: String,
    /// All comments for this analysis.
    pub comments: Vec<ReviewComment>,
}

/// Get the `.flowdiff/comments.json` path for a repo.
fn comments_file_path(repo_path: &str) -> Result<PathBuf, CommandError> {
    let repo_path = PathBuf::from(repo_path);
    let repo_path = std::fs::canonicalize(&repo_path)
        .map_err(|e| CommandError::Io(format!("Invalid repo path: {}", e)))?;
    let repo = git2::Repository::discover(&repo_path)
        .map_err(|e| CommandError::Git(format!("Not a git repository: {}", e)))?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| CommandError::Git("Bare repositories are not supported".to_string()))?;
    Ok(workdir.join(".flowdiff").join("comments.json"))
}

/// Save a comment to `.flowdiff/comments.json`.
///
/// Creates the `.flowdiff/` directory if it doesn't exist. Appends to existing
/// comments if the analysis hash matches, otherwise starts fresh.
#[tauri::command]
pub fn save_comment(
    repo_path: String,
    analysis_hash: String,
    comment: ReviewComment,
) -> Result<(), CommandError> {
    let path = comments_file_path(&repo_path)?;

    // Ensure .flowdiff directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CommandError::Io(format!("Failed to create .flowdiff directory: {}", e)))?;
    }

    // Load existing comments or start fresh
    let mut comments_file = load_comments_from_file(&path, &analysis_hash);
    comments_file.comments.push(comment);

    // Write back
    let json = serde_json::to_string_pretty(&comments_file)
        .map_err(|e| CommandError::Io(format!("Failed to serialize comments: {}", e)))?;
    std::fs::write(&path, json)
        .map_err(|e| CommandError::Io(format!("Failed to write comments file: {}", e)))?;

    Ok(())
}

/// Delete a comment by ID from `.flowdiff/comments.json`.
#[tauri::command]
pub fn delete_comment(
    repo_path: String,
    analysis_hash: String,
    comment_id: String,
) -> Result<(), CommandError> {
    let path = comments_file_path(&repo_path)?;
    let mut comments_file = load_comments_from_file(&path, &analysis_hash);
    comments_file.comments.retain(|c| c.id != comment_id);

    let json = serde_json::to_string_pretty(&comments_file)
        .map_err(|e| CommandError::Io(format!("Failed to serialize comments: {}", e)))?;
    std::fs::write(&path, json)
        .map_err(|e| CommandError::Io(format!("Failed to write comments file: {}", e)))?;

    Ok(())
}

/// Load all comments for a given analysis hash from `.flowdiff/comments.json`.
#[tauri::command]
pub fn load_comments(
    repo_path: String,
    analysis_hash: String,
) -> Result<Vec<ReviewComment>, CommandError> {
    let path = comments_file_path(&repo_path)?;
    let comments_file = load_comments_from_file(&path, &analysis_hash);
    Ok(comments_file.comments)
}

/// Export all comments as a formatted string ready for pasting to an AI agent.
///
/// Includes absolute file paths, code snippets for code-level comments,
/// and group context.
#[tauri::command]
pub fn export_comments(
    repo_path: String,
    analysis_hash: String,
) -> Result<String, CommandError> {
    let path = comments_file_path(&repo_path)?;
    let comments_file = load_comments_from_file(&path, &analysis_hash);

    let repo_path_buf = PathBuf::from(&repo_path);
    let repo_path_buf = std::fs::canonicalize(&repo_path_buf)
        .map_err(|e| CommandError::Io(format!("Invalid repo path: {}", e)))?;
    let repo = git2::Repository::discover(&repo_path_buf)
        .map_err(|e| CommandError::Git(format!("Not a git repository: {}", e)))?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| CommandError::Git("Bare repositories are not supported".to_string()))?
        .to_string_lossy()
        .to_string();
    let workdir = if workdir.ends_with('/') {
        workdir[..workdir.len() - 1].to_string()
    } else {
        workdir
    };

    let mut output = String::new();

    for comment in &comments_file.comments {
        match comment.comment_type.as_str() {
            "code" => {
                if let Some(ref fp) = comment.file_path {
                    let abs_path = format!("{}/{}", workdir, fp);
                    if let (Some(start), Some(end)) = (comment.start_line, comment.end_line) {
                        output.push_str(&format!("{}:{}-{}\n", abs_path, start, end));
                    } else {
                        output.push_str(&format!("{}\n", abs_path));
                    }
                    if let Some(ref code) = comment.selected_code {
                        output.push_str("```\n");
                        output.push_str(code);
                        if !code.ends_with('\n') {
                            output.push('\n');
                        }
                        output.push_str("```\n");
                    }
                    output.push_str(&format!("> {}\n\n", comment.text));
                }
            }
            "file" => {
                if let Some(ref fp) = comment.file_path {
                    let abs_path = format!("{}/{}", workdir, fp);
                    output.push_str(&format!("{}\n", abs_path));
                    output.push_str(&format!("> {}\n\n", comment.text));
                }
            }
            "group" => {
                output.push_str(&format!("Flow: \"{}\"\n", comment.group_id));
                output.push_str(&format!("> {}\n\n", comment.text));
            }
            _ => {}
        }
    }

    Ok(output)
}

/// Load comments from a file, returning empty if file doesn't exist or hash doesn't match.
fn load_comments_from_file(path: &PathBuf, analysis_hash: &str) -> CommentsFile {
    if let Ok(data) = std::fs::read_to_string(path) {
        if let Ok(existing) = serde_json::from_str::<CommentsFile>(&data) {
            if existing.analysis_hash == analysis_hash {
                return existing;
            }
        }
    }
    CommentsFile {
        analysis_hash: analysis_hash.to_string(),
        comments: vec![],
    }
}

fn detect_language(path: &str) -> String {
    match path.rsplit('.').next() {
        Some("ts" | "tsx") => "typescript".to_string(),
        Some("js" | "jsx") => "javascript".to_string(),
        Some("py") => "python".to_string(),
        Some("rs") => "rust".to_string(),
        Some("json") => "json".to_string(),
        Some("toml") => "toml".to_string(),
        Some("yaml" | "yml") => "yaml".to_string(),
        Some("md") => "markdown".to_string(),
        Some("css") => "css".to_string(),
        Some("html") => "html".to_string(),
        Some("sql") => "sql".to_string(),
        Some("sh" | "bash" | "zsh") => "shell".to_string(),
        Some("go") => "go".to_string(),
        Some("java") => "java".to_string(),
        Some("rb") => "ruby".to_string(),
        Some("prisma") => "prisma".to_string(),
        _ => "plaintext".to_string(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_language_typescript() {
        assert_eq!(detect_language("src/app.ts"), "typescript");
        assert_eq!(detect_language("src/App.tsx"), "typescript");
    }

    #[test]
    fn test_detect_language_javascript() {
        assert_eq!(detect_language("index.js"), "javascript");
        assert_eq!(detect_language("App.jsx"), "javascript");
    }

    #[test]
    fn test_detect_language_python() {
        assert_eq!(detect_language("main.py"), "python");
    }

    #[test]
    fn test_detect_language_rust() {
        assert_eq!(detect_language("lib.rs"), "rust");
    }

    #[test]
    fn test_detect_language_json() {
        assert_eq!(detect_language("package.json"), "json");
    }

    #[test]
    fn test_detect_language_unknown() {
        assert_eq!(detect_language("Makefile"), "plaintext");
        assert_eq!(detect_language("noext"), "plaintext");
    }

    #[test]
    fn test_detect_language_yaml() {
        assert_eq!(detect_language("config.yaml"), "yaml");
        assert_eq!(detect_language("ci.yml"), "yaml");
    }

    #[test]
    fn test_detect_language_shell() {
        assert_eq!(detect_language("run.sh"), "shell");
        assert_eq!(detect_language("init.bash"), "shell");
    }

    #[test]
    fn test_detect_language_various() {
        assert_eq!(detect_language("main.go"), "go");
        assert_eq!(detect_language("App.java"), "java");
        assert_eq!(detect_language("app.rb"), "ruby");
        assert_eq!(detect_language("schema.prisma"), "prisma");
        assert_eq!(detect_language("query.sql"), "sql");
        assert_eq!(detect_language("style.css"), "css");
        assert_eq!(detect_language("page.html"), "html");
        assert_eq!(detect_language("README.md"), "markdown");
        assert_eq!(detect_language("config.toml"), "toml");
    }

    #[test]
    fn test_app_state_new() {
        let state = AppState::new();
        let last = state.last_analysis.lock().unwrap();
        assert!(last.is_none());
    }

    #[test]
    fn test_command_error_display() {
        let err = CommandError::Git("not found".to_string());
        assert_eq!(err.to_string(), "Git error: not found");

        let err = CommandError::Analysis("no data".to_string());
        assert_eq!(err.to_string(), "Analysis error: no data");

        let err = CommandError::Config("invalid".to_string());
        assert_eq!(err.to_string(), "Config error: invalid");

        let err = CommandError::Io("permission denied".to_string());
        assert_eq!(err.to_string(), "IO error: permission denied");

        let err = CommandError::Llm("no api key".to_string());
        assert_eq!(err.to_string(), "LLM error: no api key");
    }

    #[test]
    fn test_command_error_serialize() {
        let err = CommandError::Git("test error".to_string());
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, "\"Git error: test error\"");

        let err = CommandError::Llm("rate limited".to_string());
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, "\"LLM error: rate limited\"");
    }

    #[test]
    fn test_simple_unified_diff_basic() {
        let diff = simple_unified_diff("old line", "new line");
        assert!(diff.contains("-old line"));
        assert!(diff.contains("+new line"));
    }

    #[test]
    fn test_simple_unified_diff_empty() {
        let diff = simple_unified_diff("", "");
        assert!(diff.is_empty());
    }

    #[test]
    fn test_simple_unified_diff_multiline() {
        let diff = simple_unified_diff("a\nb", "c\nd\ne");
        assert!(diff.contains("-a\n"));
        assert!(diff.contains("-b\n"));
        assert!(diff.contains("+c\n"));
        assert!(diff.contains("+d\n"));
        assert!(diff.contains("+e\n"));
    }

    #[test]
    fn test_repo_info_serde_roundtrip() {
        let info = RepoInfo {
            current_branch: Some("feature-branch".to_string()),
            default_branch: "main".to_string(),
            branches: vec![
                git::BranchInfo {
                    name: "main".to_string(),
                    is_current: false,
                    has_upstream: true,
                },
                git::BranchInfo {
                    name: "feature-branch".to_string(),
                    is_current: true,
                    has_upstream: false,
                },
            ],
            worktrees: vec![git::WorktreeInfo {
                path: "/tmp/repo".to_string(),
                branch: Some("main".to_string()),
                is_main: true,
            }],
            status: Some(git::BranchStatus {
                branch: "feature-branch".to_string(),
                upstream: None,
                ahead: 0,
                behind: 0,
            }),
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: RepoInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.current_branch, Some("feature-branch".to_string()));
        assert_eq!(back.default_branch, "main");
        assert_eq!(back.branches.len(), 2);
        assert_eq!(back.worktrees.len(), 1);
        assert!(back.status.is_some());
    }

    #[test]
    fn test_repo_info_no_status() {
        let info = RepoInfo {
            current_branch: None,
            default_branch: "main".to_string(),
            branches: vec![],
            worktrees: vec![],
            status: None,
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: RepoInfo = serde_json::from_str(&json).unwrap();
        assert!(back.current_branch.is_none());
        assert!(back.status.is_none());
    }

    #[test]
    fn test_check_api_key_no_repo() {
        // Without any env vars or config, should return false (no key configured)
        // Note: this test may pass or fail depending on whether env vars are set,
        // but it should never panic.
        let result = check_api_key(None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_check_api_key_invalid_path() {
        // Invalid path should not panic, should return Ok(bool)
        let result = check_api_key(Some("/nonexistent/path/to/repo".to_string()));
        assert!(result.is_ok());
    }

    #[test]
    fn test_llm_settings_serde_roundtrip() {
        let settings = LlmSettings {
            annotations_enabled: true,
            refinement_enabled: false,
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            api_key_source: "ANTHROPIC_API_KEY".to_string(),
            has_api_key: true,
            refinement_provider: "openai".to_string(),
            refinement_model: "gpt-4.1".to_string(),
            refinement_max_iterations: 2,
        };
        let json = serde_json::to_string(&settings).unwrap();
        let back: LlmSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.provider, "anthropic");
        assert_eq!(back.model, "claude-sonnet-4-6");
        assert!(back.annotations_enabled);
        assert!(!back.refinement_enabled);
        assert!(back.has_api_key);
        assert_eq!(back.refinement_provider, "openai");
        assert_eq!(back.refinement_model, "gpt-4.1");
        assert_eq!(back.refinement_max_iterations, 2);
    }

    #[test]
    fn test_llm_settings_all_providers() {
        for provider in &["anthropic", "openai", "gemini"] {
            let expected = default_model_for_provider(provider);
            assert!(!expected.is_empty(), "Provider '{}' should have a default model", provider);
        }
    }

    #[test]
    fn test_default_model_for_provider() {
        assert_eq!(default_model_for_provider("anthropic"), "claude-sonnet-4-6");
        assert_eq!(default_model_for_provider("openai"), "gpt-4.1");
        assert_eq!(default_model_for_provider("gemini"), "gemini-2.5-flash");
        // Unknown provider falls back to anthropic default
        assert_eq!(default_model_for_provider("unknown"), "claude-sonnet-4-6");
    }

    #[test]
    fn test_get_llm_settings_no_repo() {
        let result = get_llm_settings(None);
        assert!(result.is_ok());
        let settings = result.unwrap();
        assert_eq!(settings.provider, "anthropic");
        assert_eq!(settings.model, "claude-sonnet-4-6");
    }

    #[test]
    fn test_get_llm_settings_invalid_path() {
        let result = get_llm_settings(Some("/nonexistent/path".to_string()));
        assert!(result.is_ok());
        let settings = result.unwrap();
        // Falls back to defaults
        assert_eq!(settings.provider, "anthropic");
    }

    #[test]
    fn test_load_config_from_path_none() {
        let (config, workdir) = load_config_from_path(None);
        assert_eq!(config, FlowdiffConfig::default());
        assert!(workdir.is_none());
    }

    #[test]
    fn test_load_config_from_path_invalid() {
        let (config, workdir) = load_config_from_path(Some("/nonexistent/path"));
        assert_eq!(config, FlowdiffConfig::default());
        assert!(workdir.is_none());
    }

    #[test]
    fn test_refinement_result_serde_roundtrip() {
        use flowdiff_core::llm::schema::RefinementResponse;

        let result = RefinementResult {
            refined_groups: vec![],
            infrastructure_group: None,
            refinement_response: RefinementResponse {
                splits: vec![],
                merges: vec![],
                re_ranks: vec![],
                reclassifications: vec![],
                reasoning: "No changes needed".to_string(),
            },
            provider: "anthropic".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            had_changes: false,
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: RefinementResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.provider, "anthropic");
        assert_eq!(back.model, "claude-sonnet-4-6");
        assert!(!back.had_changes);
        assert!(back.refined_groups.is_empty());
        assert!(back.infrastructure_group.is_none());
    }

    #[test]
    fn test_refinement_result_with_changes() {
        use flowdiff_core::llm::schema::{RefinementResponse, RefinementSplit, RefinementNewGroup};
        use flowdiff_core::types::{FlowGroup, FileChange, FileRole, ChangeStats};

        let result = RefinementResult {
            refined_groups: vec![FlowGroup {
                id: "g1".to_string(),
                name: "Refined group".to_string(),
                entrypoint: None,
                files: vec![FileChange {
                    path: "test.ts".to_string(),
                    flow_position: 0,
                    role: FileRole::Entrypoint,
                    changes: ChangeStats { additions: 10, deletions: 5 },
                    symbols_changed: vec![],
                }],
                edges: vec![],
                risk_score: 0.5,
                review_order: 1,
            }],
            infrastructure_group: None,
            refinement_response: RefinementResponse {
                splits: vec![RefinementSplit {
                    source_group_id: "g1".to_string(),
                    new_groups: vec![RefinementNewGroup {
                        name: "Sub A".to_string(),
                        files: vec!["test.ts".to_string()],
                    }],
                    reason: "test split".to_string(),
                }],
                merges: vec![],
                re_ranks: vec![],
                reclassifications: vec![],
                reasoning: "Split for clarity".to_string(),
            },
            provider: "openai".to_string(),
            model: "gpt-4.1".to_string(),
            had_changes: true,
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: RefinementResult = serde_json::from_str(&json).unwrap();
        assert!(back.had_changes);
        assert_eq!(back.refined_groups.len(), 1);
        assert_eq!(back.refinement_response.splits.len(), 1);
    }

    #[test]
    fn test_file_diff_content_serde_roundtrip() {
        let content = FileDiffContent {
            path: "src/main.ts".to_string(),
            old_content: "const x = 1;".to_string(),
            new_content: "const x = 2;".to_string(),
            language: "typescript".to_string(),
        };
        let json = serde_json::to_string(&content).unwrap();
        let back: FileDiffContent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.path, "src/main.ts");
        assert_eq!(back.old_content, "const x = 1;");
        assert_eq!(back.new_content, "const x = 2;");
        assert_eq!(back.language, "typescript");
    }

    // ── Error handling edge case tests ────────────────────────────────

    #[test]
    fn test_command_error_all_variants_display() {
        let variants = vec![
            CommandError::Git("git error".into()),
            CommandError::Analysis("analysis error".into()),
            CommandError::Config("config error".into()),
            CommandError::Io("io error".into()),
            CommandError::Llm("llm error".into()),
        ];
        for err in &variants {
            let msg = err.to_string();
            assert!(!msg.is_empty());
            // Verify serialization works for all variants (sent to frontend)
            let json = serde_json::to_string(err).unwrap();
            assert!(!json.is_empty());
        }
    }

    #[test]
    fn test_detect_language_edge_cases() {
        // Path with multiple dots
        assert_eq!(detect_language("my.file.test.ts"), "typescript");
        // Hidden file
        assert_eq!(detect_language(".hidden.js"), "javascript");
        // No extension
        assert_eq!(detect_language("Makefile"), "plaintext");
        // Empty string
        assert_eq!(detect_language(""), "plaintext");
        // Path with spaces
        assert_eq!(detect_language("path with spaces/file.ts"), "typescript");
    }

    #[test]
    fn test_simple_unified_diff_only_additions() {
        let diff = simple_unified_diff("", "new line 1\nnew line 2");
        assert!(diff.contains("+new line 1"));
        assert!(diff.contains("+new line 2"));
        assert!(!diff.contains("-"));
    }

    #[test]
    fn test_simple_unified_diff_only_deletions() {
        let diff = simple_unified_diff("old line 1\nold line 2", "");
        assert!(diff.contains("-old line 1"));
        assert!(diff.contains("-old line 2"));
        assert!(!diff.contains("+"));
    }

    #[test]
    fn test_app_state_mutex_not_poisoned() {
        let state = AppState::new();
        // Lock, set, release
        {
            let mut last = state.last_analysis.lock().unwrap();
            *last = None;
        }
        // Lock again should succeed
        let last = state.last_analysis.lock().unwrap();
        assert!(last.is_none());
    }

    #[test]
    fn test_default_model_for_unknown_provider() {
        // Unknown providers should get a reasonable default
        let model = default_model_for_provider("nonexistent");
        assert!(!model.is_empty());
    }

    #[test]
    fn test_open_in_editor_nonexistent_file() {
        let result = open_in_editor("vscode".to_string(), "/tmp/__nonexistent_file_12345__".to_string());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("File not found"), "Expected file-not-found error, got: {}", err);
    }

    #[test]
    fn test_open_in_editor_unknown_editor() {
        // Create a temporary file to pass the file-exists check
        let tmp = std::env::temp_dir().join("flowdiff_test_open_editor");
        std::fs::write(&tmp, "test").unwrap();
        let result = open_in_editor("unknown_editor".to_string(), tmp.to_str().unwrap().to_string());
        std::fs::remove_file(&tmp).ok();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Unknown editor"), "Expected unknown-editor error, got: {}", err);
    }

    // ── Review comment tests ────────────────────────────────────────

    #[test]
    fn test_review_comment_serde_roundtrip() {
        let comment = ReviewComment {
            id: "c1".to_string(),
            comment_type: "code".to_string(),
            group_id: "group_1".to_string(),
            file_path: Some("src/auth.ts".to_string()),
            start_line: Some(42),
            end_line: Some(58),
            selected_code: Some("function validate() {}".to_string()),
            text: "Missing validation".to_string(),
            created_at: "2026-03-20T14:30:00Z".to_string(),
        };
        let json = serde_json::to_string(&comment).unwrap();
        let back: ReviewComment = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "c1");
        assert_eq!(back.comment_type, "code");
        assert_eq!(back.group_id, "group_1");
        assert_eq!(back.file_path, Some("src/auth.ts".to_string()));
        assert_eq!(back.start_line, Some(42));
        assert_eq!(back.end_line, Some(58));
        assert_eq!(back.selected_code, Some("function validate() {}".to_string()));
        assert_eq!(back.text, "Missing validation");
    }

    #[test]
    fn test_review_comment_file_level() {
        let comment = ReviewComment {
            id: "c2".to_string(),
            comment_type: "file".to_string(),
            group_id: "group_1".to_string(),
            file_path: Some("src/auth.ts".to_string()),
            start_line: None,
            end_line: None,
            selected_code: None,
            text: "Should we add rate limiting?".to_string(),
            created_at: "2026-03-20T14:30:00Z".to_string(),
        };
        let json = serde_json::to_string(&comment).unwrap();
        let back: ReviewComment = serde_json::from_str(&json).unwrap();
        assert_eq!(back.comment_type, "file");
        assert!(back.start_line.is_none());
        assert!(back.selected_code.is_none());
    }

    #[test]
    fn test_review_comment_group_level() {
        let comment = ReviewComment {
            id: "c3".to_string(),
            comment_type: "group".to_string(),
            group_id: "group_1".to_string(),
            file_path: None,
            start_line: None,
            end_line: None,
            selected_code: None,
            text: "Overall looks good".to_string(),
            created_at: "2026-03-20T14:31:00Z".to_string(),
        };
        let json = serde_json::to_string(&comment).unwrap();
        let back: ReviewComment = serde_json::from_str(&json).unwrap();
        assert_eq!(back.comment_type, "group");
        assert!(back.file_path.is_none());
    }

    #[test]
    fn test_comments_file_serde_roundtrip() {
        let comments_file = CommentsFile {
            analysis_hash: "abc123".to_string(),
            comments: vec![
                ReviewComment {
                    id: "c1".to_string(),
                    comment_type: "code".to_string(),
                    group_id: "group_1".to_string(),
                    file_path: Some("src/auth.ts".to_string()),
                    start_line: Some(42),
                    end_line: Some(58),
                    selected_code: Some("fn validate()".to_string()),
                    text: "Missing validation".to_string(),
                    created_at: "2026-03-20T14:30:00Z".to_string(),
                },
                ReviewComment {
                    id: "c2".to_string(),
                    comment_type: "group".to_string(),
                    group_id: "group_1".to_string(),
                    file_path: None,
                    start_line: None,
                    end_line: None,
                    selected_code: None,
                    text: "Needs review".to_string(),
                    created_at: "2026-03-20T14:31:00Z".to_string(),
                },
            ],
        };
        let json = serde_json::to_string_pretty(&comments_file).unwrap();
        let back: CommentsFile = serde_json::from_str(&json).unwrap();
        assert_eq!(back.analysis_hash, "abc123");
        assert_eq!(back.comments.len(), 2);
        assert_eq!(back.comments[0].comment_type, "code");
        assert_eq!(back.comments[1].comment_type, "group");
    }

    #[test]
    fn test_load_comments_from_file_missing() {
        let path = std::env::temp_dir().join("flowdiff_test_no_such_file.json");
        let result = load_comments_from_file(&path, "test_hash");
        assert_eq!(result.analysis_hash, "test_hash");
        assert!(result.comments.is_empty());
    }

    #[test]
    fn test_load_comments_from_file_wrong_hash() {
        let path = std::env::temp_dir().join("flowdiff_test_wrong_hash.json");
        let data = CommentsFile {
            analysis_hash: "old_hash".to_string(),
            comments: vec![ReviewComment {
                id: "c1".to_string(),
                comment_type: "group".to_string(),
                group_id: "g1".to_string(),
                file_path: None,
                start_line: None,
                end_line: None,
                selected_code: None,
                text: "old comment".to_string(),
                created_at: "2026-03-20T14:30:00Z".to_string(),
            }],
        };
        std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
        let result = load_comments_from_file(&path, "new_hash");
        assert_eq!(result.analysis_hash, "new_hash");
        assert!(result.comments.is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_load_comments_from_file_matching_hash() {
        let path = std::env::temp_dir().join("flowdiff_test_matching_hash.json");
        let data = CommentsFile {
            analysis_hash: "matching_hash".to_string(),
            comments: vec![ReviewComment {
                id: "c1".to_string(),
                comment_type: "file".to_string(),
                group_id: "g1".to_string(),
                file_path: Some("test.ts".to_string()),
                start_line: None,
                end_line: None,
                selected_code: None,
                text: "test comment".to_string(),
                created_at: "2026-03-20T14:30:00Z".to_string(),
            }],
        };
        std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
        let result = load_comments_from_file(&path, "matching_hash");
        assert_eq!(result.analysis_hash, "matching_hash");
        assert_eq!(result.comments.len(), 1);
        assert_eq!(result.comments[0].text, "test comment");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn test_review_comment_json_type_field() {
        // Verify the "type" field is correctly renamed from comment_type
        let comment = ReviewComment {
            id: "c1".to_string(),
            comment_type: "code".to_string(),
            group_id: "g1".to_string(),
            file_path: None,
            start_line: None,
            end_line: None,
            selected_code: None,
            text: "test".to_string(),
            created_at: "2026-03-20T14:30:00Z".to_string(),
        };
        let json = serde_json::to_string(&comment).unwrap();
        assert!(json.contains("\"type\":\"code\""), "JSON should use 'type' not 'comment_type': {}", json);
        // Verify deserialization from "type" field
        let back: ReviewComment = serde_json::from_str(&json).unwrap();
        assert_eq!(back.comment_type, "code");
    }

    // ── Open-in-editor / editor detection tests ─────────────────────

    #[test]
    fn test_check_editors_available_returns_all_editor_ids() {
        let result = check_editors_available();
        // Should always contain all 5 editor IDs
        for id in &["vscode", "cursor", "zed", "vim", "terminal"] {
            assert!(result.contains_key(*id), "Missing editor id: {}", id);
        }
        // Terminal should always be available
        assert_eq!(result["terminal"], true);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_macos_app_name_mapping() {
        assert_eq!(macos_app_name("vscode"), Some("Visual Studio Code"));
        assert_eq!(macos_app_name("cursor"), Some("Cursor"));
        assert_eq!(macos_app_name("zed"), Some("Zed"));
        assert_eq!(macos_app_name("vim"), None);
        assert_eq!(macos_app_name("terminal"), None);
        assert_eq!(macos_app_name("unknown"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_macos_app_exists_nonexistent() {
        // An app that definitely doesn't exist
        assert!(!macos_app_exists("Flowdiff Nonexistent App 12345"));
    }

    /// Gated behind `FLOWDIFF_RUN_EDITOR_TESTS=1` because it actually spawns editor processes.
    #[test]
    fn test_open_in_editor_all_known_editors_accept_temp_file() {
        if std::env::var("FLOWDIFF_RUN_EDITOR_TESTS").is_err() {
            eprintln!("Skipped: set FLOWDIFF_RUN_EDITOR_TESTS=1 to run (launches real editors)");
            return;
        }

        // All known editor IDs should not return "Unknown editor" for a valid file
        let tmp = std::env::temp_dir().join("flowdiff_test_known_editors");
        std::fs::write(&tmp, "test").unwrap();
        let path = tmp.to_str().unwrap().to_string();

        for editor in &["vscode", "cursor", "zed", "vim", "terminal"] {
            let result = open_in_editor(editor.to_string(), path.clone());
            // Result may be Ok (if editor is installed) or Err (not installed),
            // but should never be "Unknown editor"
            if let Err(e) = &result {
                let msg = e.to_string();
                assert!(
                    !msg.contains("Unknown editor"),
                    "Editor '{}' treated as unknown: {}",
                    editor,
                    msg
                );
            }
        }

        std::fs::remove_file(&tmp).ok();
    }
}
