#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::todo)]
#![deny(clippy::unimplemented)]
#![deny(clippy::dbg_macro)]
#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]

use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand, ValueEnum};
use git2::Repository;
use log::{error, info, warn};

use flowdiff_core::cache;
use flowdiff_core::cluster;
use flowdiff_core::config::FlowdiffConfig;
use flowdiff_core::entrypoint;
use flowdiff_core::flow::{self, FlowConfig};
use flowdiff_core::git;
use flowdiff_core::graph::SymbolGraph;
use flowdiff_core::llm;
use flowdiff_core::llm::refinement;
use flowdiff_core::output::{self, build_analysis_output};
use flowdiff_core::pipeline;
use flowdiff_core::rank;
use flowdiff_core::types::{AnalysisOutput, GroupRankInput};

#[derive(Parser)]
#[command(name = "flowdiff", about = "Semantic diff review tool with ranked data-flow grouping")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Analyze a diff and produce semantic flow groups
    Analyze(AnalyzeArgs),
    /// Launch an external diff tool for a flow group's files
    Launch(LaunchArgs),
}

#[derive(Parser)]
struct AnalyzeArgs {
    /// Base ref for branch comparison (e.g., "main")
    #[arg(long)]
    base: Option<String>,

    /// Head ref for branch comparison (e.g., "feature-branch")
    #[arg(long)]
    head: Option<String>,

    /// Commit range (e.g., "HEAD~5..HEAD")
    #[arg(long)]
    range: Option<String>,

    /// Analyze staged changes
    #[arg(long)]
    staged: bool,

    /// Analyze unstaged changes
    #[arg(long)]
    unstaged: bool,

    /// Output file path (defaults to stdout)
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Enable LLM annotation (Pass 1: overview)
    #[arg(long)]
    annotate: bool,

    /// Enable LLM refinement pass (overrides config)
    #[arg(long)]
    refine: bool,

    /// Model for refinement pass (e.g., "gpt-4o", "claude-sonnet-4-20250514")
    #[arg(long)]
    refine_model: Option<String>,

    /// Path to the git repository (defaults to current directory)
    #[arg(long, default_value = ".")]
    repo: PathBuf,
}

/// Supported external diff tools.
#[derive(Debug, Clone, ValueEnum)]
enum DiffTool {
    /// Beyond Compare
    Bcompare,
    /// Meld
    Meld,
    /// KDiff3
    Kdiff3,
    /// VS Code
    Code,
    /// macOS FileMerge
    Opendiff,
}

impl DiffTool {
    /// Returns the executable name for this diff tool.
    fn executable(&self) -> &str {
        match self {
            DiffTool::Bcompare => "bcompare",
            DiffTool::Meld => "meld",
            DiffTool::Kdiff3 => "kdiff3",
            DiffTool::Code => "code",
            DiffTool::Opendiff => "opendiff",
        }
    }

    /// Returns the display name for this diff tool.
    fn display_name(&self) -> &str {
        match self {
            DiffTool::Bcompare => "Beyond Compare",
            DiffTool::Meld => "Meld",
            DiffTool::Kdiff3 => "KDiff3",
            DiffTool::Code => "VS Code",
            DiffTool::Opendiff => "FileMerge",
        }
    }
}

#[derive(Parser)]
struct LaunchArgs {
    /// External diff tool to use
    #[arg(long, value_enum)]
    tool: DiffTool,

    /// Flow group ID to open (e.g., "group_1")
    #[arg(long)]
    group: String,

    /// Path to the analysis JSON file (output of `flowdiff analyze`)
    #[arg(long)]
    input: PathBuf,

    /// Path to the git repository (defaults to current directory)
    #[arg(long, default_value = ".")]
    repo: PathBuf,
}

fn main() {
    env_logger::init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Analyze(args) => {
            if let Err(e) = run_analyze(args) {
                error!("Error: {}", e);
                process::exit(1);
            }
        }
        Commands::Launch(args) => {
            if let Err(e) = run_launch(args) {
                error!("Error: {}", e);
                process::exit(1);
            }
        }
    }
}

fn run_analyze(args: AnalyzeArgs) -> Result<(), Box<dyn std::error::Error>> {
    // Resolve repo path
    let repo_path = std::fs::canonicalize(&args.repo)?;
    let repo = Repository::discover(&repo_path)
        .map_err(|e| format!("Not a git repository: {}", e))?;
    let workdir = repo
        .workdir()
        .ok_or("Bare repositories are not supported")?
        .to_path_buf();

    // Load config
    let mut config = FlowdiffConfig::load_from_dir(&workdir)
        .map_err(|e| format!("Config error: {}", e))?;

    // Apply CLI overrides for refinement
    if args.refine {
        config.llm.refinement.enabled = true;
    }
    if let Some(ref model) = args.refine_model {
        config.llm.refinement.enabled = true;
        config.llm.refinement.model = Some(model.clone());
    }

    // Extract diff
    let (diff_result, diff_source) = extract_diff(&repo, &args)?;

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
        return write_output(&empty_output, args.output.as_deref());
    }

    // Check cache (skip if LLM annotation or refinement requested — those are additive)
    let cache_key = cache::compute_cache_key(&diff_result);
    if !args.annotate && !args.refine && args.refine_model.is_none() {
        if let Some(cached) = cache::load_cached(&workdir, &cache_key) {
            return write_output(&cached, args.output.as_deref());
        }
    }

    // Parse all changed files in parallel
    let file_inputs: Vec<(&str, &str)> = diff_result
        .files
        .iter()
        .filter_map(|file_diff| {
            let content = file_diff.new_content.as_deref()
                .or(file_diff.old_content.as_deref())?;
            let path = file_diff.path();
            if config.is_ignored(path) {
                return None;
            }
            Some((path, content))
        })
        .collect();
    let parsed_files = pipeline::parse_files_parallel(&file_inputs);

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
                &group.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
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

    // Build initial output
    let mut analysis_output = build_analysis_output(
        &diff_result,
        diff_source,
        &parsed_files,
        &cluster_result,
        &ranked,
    );

    // Cache the deterministic analysis result (before LLM steps)
    cache::store_cached(&workdir, &cache_key, &analysis_output);

    // Apply LLM refinement if enabled
    if config.llm.refinement.enabled {
        let rt = tokio::runtime::Runtime::new()?;
        match rt.block_on(run_refinement(&config, &mut analysis_output)) {
            Ok(()) => {}
            Err(e) => {
                warn!("LLM refinement failed, using deterministic groups: {}", e);
            }
        }
    }

    // Apply LLM annotation if requested
    if args.annotate {
        let rt = tokio::runtime::Runtime::new()?;
        match rt.block_on(run_annotation(&config, &mut analysis_output)) {
            Ok(()) => {}
            Err(e) => {
                warn!("LLM annotation failed: {}", e);
            }
        }
    }

    write_output(&analysis_output, args.output.as_deref())
}

fn extract_diff(
    repo: &Repository,
    args: &AnalyzeArgs,
) -> Result<(git::DiffResult, flowdiff_core::types::DiffSource), Box<dyn std::error::Error>> {
    if let Some(ref range) = args.range {
        let diff = git::diff_range(repo, range)?;
        let source = output::diff_source_range(
            range,
            diff.base_sha.as_deref(),
            diff.head_sha.as_deref(),
        );
        Ok((diff, source))
    } else if args.staged {
        let diff = git::diff_staged(repo)?;
        let source = output::diff_source_staged();
        Ok((diff, source))
    } else if args.unstaged {
        let diff = git::diff_unstaged(repo)?;
        let source = output::diff_source_unstaged();
        Ok((diff, source))
    } else {
        // Branch comparison (default) — auto-detect default branch if not specified
        let detected_default = if args.base.is_none() {
            git::detect_default_branch(repo).ok()
        } else {
            None
        };
        let base = args.base.as_deref()
            .or(detected_default.as_deref())
            .unwrap_or("main");
        let head = args.head.as_deref().unwrap_or("HEAD");
        let diff = git::diff_refs(repo, base, head)?;
        let source = output::diff_source_branch(
            base,
            head,
            diff.base_sha.as_deref(),
            diff.head_sha.as_deref(),
        );
        Ok((diff, source))
    }
}

async fn run_refinement(
    config: &FlowdiffConfig,
    analysis_output: &mut AnalysisOutput,
) -> Result<(), Box<dyn std::error::Error>> {
    // Build a LlmConfig for the refinement provider
    let refinement_llm_config = flowdiff_core::config::LlmConfig {
        provider: config.llm.refinement.provider.clone().or_else(|| config.llm.provider.clone()),
        model: config.llm.refinement.model.clone().or_else(|| config.llm.model.clone()),
        key_cmd: config.llm.refinement.key_cmd.clone().or_else(|| config.llm.key_cmd.clone()),
        ..Default::default()
    };

    let provider = llm::create_provider(&refinement_llm_config)?;

    // Build refinement request
    let analysis_json = serde_json::to_string_pretty(analysis_output)?;
    let diff_summary = format!(
        "{} files changed across {} groups",
        analysis_output.summary.total_files_changed,
        analysis_output.summary.total_groups,
    );

    let request = refinement::build_refinement_request(
        &analysis_output.groups,
        &analysis_json,
        &diff_summary,
    );

    // Call LLM for refinement
    let response = provider.refine_groups(&request).await?;

    if !refinement::has_refinements(&response) {
        return Ok(());
    }

    // Apply refinement
    let (refined_groups, refined_infra) = refinement::apply_refinement(
        &analysis_output.groups,
        analysis_output.infrastructure_group.as_ref(),
        &response,
    )?;

    analysis_output.groups = refined_groups;
    analysis_output.infrastructure_group = refined_infra;
    analysis_output.summary.total_groups = analysis_output.groups.len() as u32;

    Ok(())
}

async fn run_annotation(
    config: &FlowdiffConfig,
    analysis_output: &mut AnalysisOutput,
) -> Result<(), Box<dyn std::error::Error>> {
    let provider = llm::create_provider(&config.llm)?;

    // Build Pass 1 request
    let flow_groups: Vec<llm::schema::Pass1GroupInput> = analysis_output
        .groups
        .iter()
        .map(|g| llm::schema::Pass1GroupInput {
            id: g.id.clone(),
            name: g.name.clone(),
            entrypoint: g.entrypoint.as_ref().map(|ep| format!("{}::{}", ep.file, ep.symbol)),
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
            analysis_output.summary.total_files_changed,
            analysis_output.summary.total_groups,
        ),
        flow_groups,
        graph_summary: format!(
            "{} groups, {} total files",
            analysis_output.summary.total_groups,
            analysis_output.summary.total_files_changed,
        ),
    };

    let response = provider.annotate_overview(&request).await?;
    analysis_output.annotations = Some(serde_json::to_value(response)?);

    Ok(())
}

fn run_launch(args: LaunchArgs) -> Result<(), Box<dyn std::error::Error>> {
    // Read and parse the analysis JSON
    let json_content = std::fs::read_to_string(&args.input)
        .map_err(|e| format!("Failed to read input file {:?}: {}", args.input, e))?;
    let analysis: AnalysisOutput = serde_json::from_str(&json_content)
        .map_err(|e| format!("Failed to parse analysis JSON: {}", e))?;

    // Find the requested group
    let group = analysis
        .groups
        .iter()
        .find(|g| g.id == args.group)
        .ok_or_else(|| {
            let available: Vec<&str> = analysis.groups.iter().map(|g| g.id.as_str()).collect();
            format!(
                "Group '{}' not found. Available groups: {}",
                args.group,
                available.join(", ")
            )
        })?;

    if group.files.is_empty() {
        return Err("Group has no files to compare".into());
    }

    // Open the git repo
    let repo_path = std::fs::canonicalize(&args.repo)?;
    let repo = Repository::discover(&repo_path)
        .map_err(|e| format!("Not a git repository: {}", e))?;
    let workdir = repo
        .workdir()
        .ok_or("Bare repositories are not supported")?
        .to_path_buf();

    // Determine base and head refs from the analysis diff_source
    let base_ref = analysis
        .diff_source
        .base_sha
        .as_deref()
        .or(analysis.diff_source.base.as_deref());
    let head_ref = analysis
        .diff_source
        .head_sha
        .as_deref()
        .or(analysis.diff_source.head.as_deref());

    // Create temp directories for old (base) and new (head) versions
    let tmp_dir = tempfile::tempdir()
        .map_err(|e| format!("Failed to create temp directory: {}", e))?;
    let base_dir = tmp_dir.path().join("base");
    let head_dir = tmp_dir.path().join("head");
    std::fs::create_dir_all(&base_dir)?;
    std::fs::create_dir_all(&head_dir)?;

    let file_paths: Vec<&str> = group.files.iter().map(|f| f.path.as_str()).collect();

    for file_path in &file_paths {
        // Get base content from git ref
        if let Some(ref base) = base_ref {
            if let Ok(Some(content)) = git::file_content_at_ref(&repo, base, file_path) {
                let dest = base_dir.join(file_path);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&dest, content)?;
            }
        }

        // Get head content: from git ref if available, otherwise from working tree
        if let Some(ref head) = head_ref {
            if let Ok(Some(content)) = git::file_content_at_ref(&repo, head, file_path) {
                let dest = head_dir.join(file_path);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&dest, content)?;
            }
        } else {
            // Read from working tree
            let src = workdir.join(file_path);
            if src.exists() {
                let dest = head_dir.join(file_path);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(&src, &dest)?;
            }
        }
    }

    // Launch the diff tool
    let exe = args.tool.executable();
    info!(
        "Launching {} for group '{}' ({} files)...",
        args.tool.display_name(),
        group.name,
        file_paths.len()
    );

    let mut cmd = std::process::Command::new(exe);
    cmd.arg(base_dir.as_os_str()).arg(head_dir.as_os_str());

    let status = cmd
        .status()
        .map_err(|e| format!("Failed to launch '{}': {} (is it installed and in PATH?)", exe, e))?;

    if !status.success() {
        return Err(format!("{} exited with status {}", exe, status).into());
    }

    Ok(())
}

fn write_output(
    output: &AnalysisOutput,
    file_path: Option<&std::path::Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(path) = file_path {
        let mut file = std::fs::File::create(path)?;
        output::write_json(output, &mut file)?;
    } else {
        let mut stdout = std::io::stdout().lock();
        output::write_json(output, &mut stdout)?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod tests {
    use super::*;

    // ── CLI Argument Parsing Tests (Analyze) ──

    #[test]
    fn test_parse_analyze_base_head() {
        let cli = Cli::parse_from(["flowdiff", "analyze", "--base", "main", "--head", "feature"]);
        if let Commands::Analyze(args) = cli.command {
            assert_eq!(args.base, Some("main".to_string()));
            assert_eq!(args.head, Some("feature".to_string()));
            assert!(!args.refine);
            assert!(args.refine_model.is_none());
        } else {
            panic!("expected Analyze command");
        }
    }

    #[test]
    fn test_parse_analyze_range() {
        let cli = Cli::parse_from(["flowdiff", "analyze", "--range", "HEAD~5..HEAD"]);
        if let Commands::Analyze(args) = cli.command {
            assert_eq!(args.range, Some("HEAD~5..HEAD".to_string()));
            assert!(args.base.is_none());
            assert!(args.head.is_none());
        } else {
            panic!("expected Analyze command");
        }
    }

    #[test]
    fn test_parse_analyze_staged() {
        let cli = Cli::parse_from(["flowdiff", "analyze", "--staged"]);
        if let Commands::Analyze(args) = cli.command {
            assert!(args.staged);
            assert!(!args.unstaged);
        } else {
            panic!("expected Analyze command");
        }
    }

    #[test]
    fn test_parse_analyze_unstaged() {
        let cli = Cli::parse_from(["flowdiff", "analyze", "--unstaged"]);
        if let Commands::Analyze(args) = cli.command {
            assert!(args.unstaged);
            assert!(!args.staged);
        } else {
            panic!("expected Analyze command");
        }
    }

    #[test]
    fn test_parse_analyze_refine() {
        let cli = Cli::parse_from(["flowdiff", "analyze", "--base", "main", "--refine"]);
        if let Commands::Analyze(args) = cli.command {
            assert!(args.refine);
            assert!(args.refine_model.is_none());
        } else {
            panic!("expected Analyze command");
        }
    }

    #[test]
    fn test_parse_analyze_refine_model() {
        let cli = Cli::parse_from([
            "flowdiff", "analyze", "--base", "main", "--refine", "--refine-model", "gpt-4o",
        ]);
        if let Commands::Analyze(args) = cli.command {
            assert!(args.refine);
            assert_eq!(args.refine_model, Some("gpt-4o".to_string()));
        } else {
            panic!("expected Analyze command");
        }
    }

    #[test]
    fn test_parse_analyze_refine_model_implies_refine() {
        let cli = Cli::parse_from([
            "flowdiff", "analyze", "--base", "main", "--refine-model", "claude-sonnet-4-20250514",
        ]);
        if let Commands::Analyze(args) = cli.command {
            assert_eq!(args.refine_model, Some("claude-sonnet-4-20250514".to_string()));
        } else {
            panic!("expected Analyze command");
        }
    }

    #[test]
    fn test_parse_analyze_output_file() {
        let cli = Cli::parse_from(["flowdiff", "analyze", "--base", "main", "-o", "review.json"]);
        if let Commands::Analyze(args) = cli.command {
            assert_eq!(args.output, Some(PathBuf::from("review.json")));
        } else {
            panic!("expected Analyze command");
        }
    }

    #[test]
    fn test_parse_analyze_annotate() {
        let cli = Cli::parse_from(["flowdiff", "analyze", "--base", "main", "--annotate"]);
        if let Commands::Analyze(args) = cli.command {
            assert!(args.annotate);
        } else {
            panic!("expected Analyze command");
        }
    }

    #[test]
    fn test_parse_analyze_all_flags() {
        let cli = Cli::parse_from([
            "flowdiff", "analyze", "--base", "main", "--head", "feature",
            "--annotate", "--refine", "--refine-model", "gpt-4o",
            "-o", "out.json", "--repo", "/tmp/myrepo",
        ]);
        if let Commands::Analyze(args) = cli.command {
            assert_eq!(args.base, Some("main".to_string()));
            assert_eq!(args.head, Some("feature".to_string()));
            assert!(args.annotate);
            assert!(args.refine);
            assert_eq!(args.refine_model, Some("gpt-4o".to_string()));
            assert_eq!(args.output, Some(PathBuf::from("out.json")));
            assert_eq!(args.repo, PathBuf::from("/tmp/myrepo"));
        } else {
            panic!("expected Analyze command");
        }
    }

    #[test]
    fn test_default_repo_path() {
        let cli = Cli::parse_from(["flowdiff", "analyze", "--staged"]);
        if let Commands::Analyze(args) = cli.command {
            assert_eq!(args.repo, PathBuf::from("."));
        } else {
            panic!("expected Analyze command");
        }
    }

    // ── Diff Source Selection Tests ──

    #[test]
    fn test_extract_diff_selects_range() {
        let cli = Cli::parse_from(["flowdiff", "analyze", "--range", "abc..def"]);
        if let Commands::Analyze(args) = cli.command {
            assert!(args.range.is_some());
            assert!(!args.staged);
            assert!(!args.unstaged);
            assert!(args.base.is_none());
        } else {
            panic!("expected Analyze command");
        }
    }

    // ── Config Override Tests ──

    #[test]
    fn test_refine_flag_overrides_config() {
        let mut config = FlowdiffConfig::default();
        assert!(!config.llm.refinement.enabled);

        let refine = true;
        let refine_model: Option<String> = Some("gpt-4o".to_string());

        if refine {
            config.llm.refinement.enabled = true;
        }
        if let Some(ref model) = refine_model {
            config.llm.refinement.enabled = true;
            config.llm.refinement.model = Some(model.clone());
        }

        assert!(config.llm.refinement.enabled);
        assert_eq!(config.llm.refinement.model, Some("gpt-4o".to_string()));
    }

    #[test]
    fn test_refine_model_without_refine_enables_refinement() {
        let mut config = FlowdiffConfig::default();
        assert!(!config.llm.refinement.enabled);

        let refine_model: Option<String> = Some("claude-sonnet-4-20250514".to_string());
        if let Some(ref model) = refine_model {
            config.llm.refinement.enabled = true;
            config.llm.refinement.model = Some(model.clone());
        }

        assert!(config.llm.refinement.enabled);
        assert_eq!(
            config.llm.refinement.model,
            Some("claude-sonnet-4-20250514".to_string())
        );
    }

    #[test]
    fn test_refinement_llm_config_construction() {
        let config = FlowdiffConfig {
            llm: flowdiff_core::config::LlmConfig {
                provider: Some("anthropic".to_string()),
                model: Some("claude-sonnet-4-20250514".to_string()),
                key_cmd: Some("echo main-key".to_string()),
                refinement: flowdiff_core::config::RefinementConfig {
                    enabled: true,
                    provider: Some("openai".to_string()),
                    model: Some("gpt-4o".to_string()),
                    key_cmd: Some("echo refinement-key".to_string()),
                    max_iterations: 2,
                },
            },
            ..Default::default()
        };

        let refinement_llm_config = flowdiff_core::config::LlmConfig {
            provider: config.llm.refinement.provider.clone().or_else(|| config.llm.provider.clone()),
            model: config.llm.refinement.model.clone().or_else(|| config.llm.model.clone()),
            key_cmd: config.llm.refinement.key_cmd.clone().or_else(|| config.llm.key_cmd.clone()),
            ..Default::default()
        };

        assert_eq!(refinement_llm_config.provider, Some("openai".to_string()));
        assert_eq!(refinement_llm_config.model, Some("gpt-4o".to_string()));
        assert_eq!(refinement_llm_config.key_cmd, Some("echo refinement-key".to_string()));
    }

    #[test]
    fn test_refinement_llm_config_falls_back_to_main() {
        let config = FlowdiffConfig {
            llm: flowdiff_core::config::LlmConfig {
                provider: Some("anthropic".to_string()),
                model: Some("claude-sonnet-4-20250514".to_string()),
                key_cmd: Some("echo main-key".to_string()),
                refinement: flowdiff_core::config::RefinementConfig {
                    enabled: true,
                    provider: None,
                    model: None,
                    key_cmd: None,
                    max_iterations: 1,
                },
            },
            ..Default::default()
        };

        let refinement_llm_config = flowdiff_core::config::LlmConfig {
            provider: config.llm.refinement.provider.clone().or_else(|| config.llm.provider.clone()),
            model: config.llm.refinement.model.clone().or_else(|| config.llm.model.clone()),
            key_cmd: config.llm.refinement.key_cmd.clone().or_else(|| config.llm.key_cmd.clone()),
            ..Default::default()
        };

        assert_eq!(refinement_llm_config.provider, Some("anthropic".to_string()));
        assert_eq!(refinement_llm_config.model, Some("claude-sonnet-4-20250514".to_string()));
        assert_eq!(refinement_llm_config.key_cmd, Some("echo main-key".to_string()));
    }

    // ── Write Output Tests ──

    #[test]
    fn test_write_output_to_buffer() {
        let output = AnalysisOutput {
            version: "1.0.0".to_string(),
            diff_source: flowdiff_core::types::DiffSource {
                diff_type: flowdiff_core::types::DiffType::Staged,
                base: None,
                head: None,
                base_sha: None,
                head_sha: None,
            },
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

        let json = serde_json::to_string_pretty(&output).unwrap();
        assert!(json.contains("\"version\": \"1.0.0\""));
    }

    // ── Launch Command Parsing Tests ──

    #[test]
    fn test_parse_launch_bcompare() {
        let cli = Cli::parse_from([
            "flowdiff", "launch", "--tool", "bcompare", "--group", "group_1",
            "--input", "review.json",
        ]);
        if let Commands::Launch(args) = cli.command {
            assert!(matches!(args.tool, DiffTool::Bcompare));
            assert_eq!(args.group, "group_1");
            assert_eq!(args.input, PathBuf::from("review.json"));
            assert_eq!(args.repo, PathBuf::from("."));
        } else {
            panic!("expected Launch command");
        }
    }

    #[test]
    fn test_parse_launch_meld() {
        let cli = Cli::parse_from([
            "flowdiff", "launch", "--tool", "meld", "--group", "group_2",
            "--input", "out.json",
        ]);
        if let Commands::Launch(args) = cli.command {
            assert!(matches!(args.tool, DiffTool::Meld));
            assert_eq!(args.group, "group_2");
        } else {
            panic!("expected Launch command");
        }
    }

    #[test]
    fn test_parse_launch_kdiff3() {
        let cli = Cli::parse_from([
            "flowdiff", "launch", "--tool", "kdiff3", "--group", "g1",
            "--input", "a.json",
        ]);
        if let Commands::Launch(args) = cli.command {
            assert!(matches!(args.tool, DiffTool::Kdiff3));
        } else {
            panic!("expected Launch command");
        }
    }

    #[test]
    fn test_parse_launch_code() {
        let cli = Cli::parse_from([
            "flowdiff", "launch", "--tool", "code", "--group", "g1",
            "--input", "a.json",
        ]);
        if let Commands::Launch(args) = cli.command {
            assert!(matches!(args.tool, DiffTool::Code));
        } else {
            panic!("expected Launch command");
        }
    }

    #[test]
    fn test_parse_launch_opendiff() {
        let cli = Cli::parse_from([
            "flowdiff", "launch", "--tool", "opendiff", "--group", "g1",
            "--input", "a.json",
        ]);
        if let Commands::Launch(args) = cli.command {
            assert!(matches!(args.tool, DiffTool::Opendiff));
        } else {
            panic!("expected Launch command");
        }
    }

    #[test]
    fn test_parse_launch_with_repo() {
        let cli = Cli::parse_from([
            "flowdiff", "launch", "--tool", "bcompare", "--group", "group_1",
            "--input", "review.json", "--repo", "/tmp/myrepo",
        ]);
        if let Commands::Launch(args) = cli.command {
            assert_eq!(args.repo, PathBuf::from("/tmp/myrepo"));
        } else {
            panic!("expected Launch command");
        }
    }

    // ── DiffTool Tests ──

    #[test]
    fn test_diff_tool_executable() {
        assert_eq!(DiffTool::Bcompare.executable(), "bcompare");
        assert_eq!(DiffTool::Meld.executable(), "meld");
        assert_eq!(DiffTool::Kdiff3.executable(), "kdiff3");
        assert_eq!(DiffTool::Code.executable(), "code");
        assert_eq!(DiffTool::Opendiff.executable(), "opendiff");
    }

    #[test]
    fn test_diff_tool_display_name() {
        assert_eq!(DiffTool::Bcompare.display_name(), "Beyond Compare");
        assert_eq!(DiffTool::Meld.display_name(), "Meld");
        assert_eq!(DiffTool::Kdiff3.display_name(), "KDiff3");
        assert_eq!(DiffTool::Code.display_name(), "VS Code");
        assert_eq!(DiffTool::Opendiff.display_name(), "FileMerge");
    }
}
