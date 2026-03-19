//! Tauri IPC commands — bridge between the React frontend and flowdiff-core.
//!
//! Each `#[tauri::command]` function is callable from the frontend via `invoke()`.

use std::path::PathBuf;
use std::sync::Mutex;

use flowdiff_core::ast;
use flowdiff_core::cluster;
use flowdiff_core::config::FlowdiffConfig;
use flowdiff_core::entrypoint;
use flowdiff_core::flow::{self, FlowConfig};
use flowdiff_core::git;
use flowdiff_core::graph::SymbolGraph;
use flowdiff_core::output::{self, build_analysis_output};
use flowdiff_core::rank;
use flowdiff_core::types::{AnalysisOutput, GroupRankInput};

/// Application state shared across commands.
pub struct AppState {
    /// The most recent analysis result, available for subsequent queries.
    pub last_analysis: Mutex<Option<AnalysisOutput>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            last_analysis: Mutex::new(None),
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
#[tauri::command]
pub fn analyze(
    repo_path: String,
    base: Option<String>,
    head: Option<String>,
    range: Option<String>,
    staged: bool,
    unstaged: bool,
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
    let (diff_result, diff_source) = extract_diff(&repo, base, head, range, staged, unstaged)?;

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
        if let Ok(mut last) = state.last_analysis.lock() {
            *last = Some(empty_output.clone());
        }
        return Ok(empty_output);
    }

    // Parse all changed files
    let mut parsed_files = Vec::new();
    for file_diff in &diff_result.files {
        let content = file_diff
            .new_content
            .as_deref()
            .or(file_diff.old_content.as_deref());
        if let Some(content) = content {
            let path = file_diff.path();
            if config.is_ignored(path) {
                continue;
            }
            if let Ok(parsed) = ast::parse_file(path, content) {
                parsed_files.push(parsed);
            }
        }
    }

    // Build symbol graph
    let mut graph = SymbolGraph::build(&parsed_files);

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

    // Store for subsequent queries
    if let Ok(mut last) = state.last_analysis.lock() {
        *last = Some(analysis_output.clone());
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
#[tauri::command]
pub fn get_file_diff(
    repo_path: String,
    file_path: String,
    base: Option<String>,
    head: Option<String>,
    range: Option<String>,
    staged: bool,
    unstaged: bool,
) -> Result<FileDiffContent, CommandError> {
    let repo_path = PathBuf::from(&repo_path);
    let repo_path = std::fs::canonicalize(&repo_path)
        .map_err(|e| CommandError::Io(format!("Invalid repo path: {}", e)))?;

    let repo = git2::Repository::discover(&repo_path)
        .map_err(|e| CommandError::Git(format!("Not a git repository: {}", e)))?;

    let (diff_result, _) = extract_diff(&repo, base, head, range, staged, unstaged)?;

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
    }

    #[test]
    fn test_command_error_serialize() {
        let err = CommandError::Git("test error".to_string());
        let json = serde_json::to_string(&err).unwrap();
        assert_eq!(json, "\"Git error: test error\"");
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
}
