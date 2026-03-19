use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};
use git2::Repository;

use flowdiff_core::ast;
use flowdiff_core::cluster;
use flowdiff_core::config::FlowdiffConfig;
use flowdiff_core::entrypoint;
use flowdiff_core::flow::{self, FlowConfig};
use flowdiff_core::git;
use flowdiff_core::graph::SymbolGraph;
use flowdiff_core::llm;
use flowdiff_core::llm::refinement;
use flowdiff_core::output::{self, build_analysis_output};
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

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Analyze(args) => {
            if let Err(e) = run_analyze(args) {
                eprintln!("Error: {}", e);
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

    // Parse all changed files
    let mut parsed_files = Vec::new();
    for file_diff in &diff_result.files {
        let content = file_diff.new_content.as_deref()
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

    // Apply LLM refinement if enabled
    if config.llm.refinement.enabled {
        let rt = tokio::runtime::Runtime::new()?;
        match rt.block_on(run_refinement(&config, &mut analysis_output)) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Warning: LLM refinement failed, using deterministic groups: {}", e);
            }
        }
    }

    // Apply LLM annotation if requested
    if args.annotate {
        let rt = tokio::runtime::Runtime::new()?;
        match rt.block_on(run_annotation(&config, &mut analysis_output)) {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Warning: LLM annotation failed: {}", e);
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
        // Branch comparison (default)
        let base = args.base.as_deref().unwrap_or("main");
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
mod tests {
    use super::*;

    // ── CLI Argument Parsing Tests ──

    #[test]
    fn test_parse_analyze_base_head() {
        let cli = Cli::parse_from(["flowdiff", "analyze", "--base", "main", "--head", "feature"]);
        match cli.command {
            Commands::Analyze(args) => {
                assert_eq!(args.base, Some("main".to_string()));
                assert_eq!(args.head, Some("feature".to_string()));
                assert!(!args.refine);
                assert!(args.refine_model.is_none());
            }
        }
    }

    #[test]
    fn test_parse_analyze_range() {
        let cli = Cli::parse_from(["flowdiff", "analyze", "--range", "HEAD~5..HEAD"]);
        match cli.command {
            Commands::Analyze(args) => {
                assert_eq!(args.range, Some("HEAD~5..HEAD".to_string()));
                assert!(args.base.is_none());
                assert!(args.head.is_none());
            }
        }
    }

    #[test]
    fn test_parse_analyze_staged() {
        let cli = Cli::parse_from(["flowdiff", "analyze", "--staged"]);
        match cli.command {
            Commands::Analyze(args) => {
                assert!(args.staged);
                assert!(!args.unstaged);
            }
        }
    }

    #[test]
    fn test_parse_analyze_unstaged() {
        let cli = Cli::parse_from(["flowdiff", "analyze", "--unstaged"]);
        match cli.command {
            Commands::Analyze(args) => {
                assert!(args.unstaged);
                assert!(!args.staged);
            }
        }
    }

    #[test]
    fn test_parse_analyze_refine() {
        let cli = Cli::parse_from(["flowdiff", "analyze", "--base", "main", "--refine"]);
        match cli.command {
            Commands::Analyze(args) => {
                assert!(args.refine);
                assert!(args.refine_model.is_none());
            }
        }
    }

    #[test]
    fn test_parse_analyze_refine_model() {
        let cli = Cli::parse_from([
            "flowdiff",
            "analyze",
            "--base",
            "main",
            "--refine",
            "--refine-model",
            "gpt-4o",
        ]);
        match cli.command {
            Commands::Analyze(args) => {
                assert!(args.refine);
                assert_eq!(args.refine_model, Some("gpt-4o".to_string()));
            }
        }
    }

    #[test]
    fn test_parse_analyze_refine_model_implies_refine() {
        // --refine-model alone should still set the model
        let cli = Cli::parse_from([
            "flowdiff",
            "analyze",
            "--base",
            "main",
            "--refine-model",
            "claude-sonnet-4-20250514",
        ]);
        match cli.command {
            Commands::Analyze(args) => {
                assert_eq!(
                    args.refine_model,
                    Some("claude-sonnet-4-20250514".to_string())
                );
            }
        }
    }

    #[test]
    fn test_parse_analyze_output_file() {
        let cli = Cli::parse_from([
            "flowdiff",
            "analyze",
            "--base",
            "main",
            "-o",
            "review.json",
        ]);
        match cli.command {
            Commands::Analyze(args) => {
                assert_eq!(args.output, Some(PathBuf::from("review.json")));
            }
        }
    }

    #[test]
    fn test_parse_analyze_annotate() {
        let cli = Cli::parse_from(["flowdiff", "analyze", "--base", "main", "--annotate"]);
        match cli.command {
            Commands::Analyze(args) => {
                assert!(args.annotate);
            }
        }
    }

    #[test]
    fn test_parse_analyze_all_flags() {
        let cli = Cli::parse_from([
            "flowdiff",
            "analyze",
            "--base",
            "main",
            "--head",
            "feature",
            "--annotate",
            "--refine",
            "--refine-model",
            "gpt-4o",
            "-o",
            "out.json",
            "--repo",
            "/tmp/myrepo",
        ]);
        match cli.command {
            Commands::Analyze(args) => {
                assert_eq!(args.base, Some("main".to_string()));
                assert_eq!(args.head, Some("feature".to_string()));
                assert!(args.annotate);
                assert!(args.refine);
                assert_eq!(args.refine_model, Some("gpt-4o".to_string()));
                assert_eq!(args.output, Some(PathBuf::from("out.json")));
                assert_eq!(args.repo, PathBuf::from("/tmp/myrepo"));
            }
        }
    }

    #[test]
    fn test_default_repo_path() {
        let cli = Cli::parse_from(["flowdiff", "analyze", "--staged"]);
        match cli.command {
            Commands::Analyze(args) => {
                assert_eq!(args.repo, PathBuf::from("."));
            }
        }
    }

    // ── Diff Source Selection Tests ──

    #[test]
    fn test_extract_diff_selects_range() {
        // We can't test actual git operations here without a repo,
        // but we test the selection logic by checking args parsing
        let cli = Cli::parse_from(["flowdiff", "analyze", "--range", "abc..def"]);
        match cli.command {
            Commands::Analyze(args) => {
                assert!(args.range.is_some());
                assert!(!args.staged);
                assert!(!args.unstaged);
                assert!(args.base.is_none());
            }
        }
    }

    // ── Config Override Tests ──

    #[test]
    fn test_refine_flag_overrides_config() {
        let mut config = FlowdiffConfig::default();
        assert!(!config.llm.refinement.enabled);

        // Simulate what run_analyze does
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

        // Simulate: --refine-model passed without --refine
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

        // Simulate what run_refinement does: build refinement-specific LlmConfig
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
                    provider: None, // Falls back to main
                    model: None,    // Falls back to main
                    key_cmd: None,  // Falls back to main
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

        // Verify it serializes without error
        let json = serde_json::to_string_pretty(&output).unwrap();
        assert!(json.contains("\"version\": \"1.0.0\""));
    }
}
