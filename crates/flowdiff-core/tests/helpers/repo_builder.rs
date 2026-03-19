//! Programmatic git repo builder and pipeline runner for integration tests.
//!
//! Provides `RepoBuilder` for creating temporary git repos with known file structures,
//! and `run_pipeline` for executing the full flowdiff analysis pipeline.

use std::path::Path;

use git2::{Repository, Signature};
use tempfile::TempDir;

use flowdiff_core::ast;
use flowdiff_core::cluster;
use flowdiff_core::entrypoint;
use flowdiff_core::flow::{self, FlowConfig};
use flowdiff_core::git;
use flowdiff_core::graph::SymbolGraph;
use flowdiff_core::output::{self, build_analysis_output};
use flowdiff_core::rank::{self, compute_risk_score, compute_surface_area};
use flowdiff_core::types::{AnalysisOutput, GroupRankInput, RankWeights};

/// Create a git repo, commit initial files, apply changes on a branch, and return the repo + dir.
pub struct RepoBuilder {
    dir: TempDir,
    repo: Repository,
}

impl RepoBuilder {
    pub fn new() -> Self {
        let dir = TempDir::new().expect("failed to create temp dir");
        let repo = Repository::init(dir.path()).expect("failed to init repo");

        // Configure user for commits
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "Test User").unwrap();
        config.set_str("user.email", "test@example.com").unwrap();

        Self { dir, repo }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Write a file relative to the repo root.
    pub fn write_file(&self, rel_path: &str, content: &str) {
        let full = self.dir.path().join(rel_path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full, content).unwrap();
    }

    /// Stage all changes and commit with a message. Returns the commit OID.
    pub fn commit(&self, message: &str) -> git2::Oid {
        let mut index = self.repo.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();

        let tree_oid = index.write_tree().unwrap();
        let tree = self.repo.find_tree(tree_oid).unwrap();
        let sig = Signature::now("Test User", "test@example.com").unwrap();

        let parent = self
            .repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<&git2::Commit> = parent.iter().collect();

        self.repo
            .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
            .unwrap()
    }

    /// Create a branch at the current HEAD. No-op if it already exists.
    pub fn create_branch(&self, name: &str) {
        let head = self.repo.head().unwrap().peel_to_commit().unwrap();
        // force=false; ignore AlreadyExists errors
        let _ = self.repo.branch(name, &head, false);
    }

    /// Checkout a branch by name.
    pub fn checkout(&self, name: &str) {
        let ref_name = format!("refs/heads/{}", name);
        let obj = self.repo.revparse_single(&ref_name).unwrap();
        self.repo.checkout_tree(&obj, None).unwrap();
        self.repo.set_head(&ref_name).unwrap();
    }

    /// Get a reference to the underlying git2 Repository.
    pub fn repo(&self) -> &Repository {
        &self.repo
    }

    /// Get a reference to the underlying TempDir (for keeping it alive).
    pub fn temp_dir(&self) -> &TempDir {
        &self.dir
    }
}

/// Run the full pipeline on a repo diff between two refs.
pub fn run_pipeline(repo_path: &Path, base_ref: &str, head_ref: &str) -> AnalysisOutput {
    let repo = Repository::open(repo_path).expect("failed to open repo");
    let diff_result = git::diff_refs(&repo, base_ref, head_ref).expect("diff_refs failed");

    // Parse all changed files (using new content for adds/modifies, old for deletes).
    let mut parsed_files = Vec::new();
    for file_diff in &diff_result.files {
        if let Some(ref content) = file_diff.new_content {
            let path = file_diff.path();
            if let Ok(parsed) = ast::parse_file(path, content) {
                parsed_files.push(parsed);
            }
        }
    }

    // Build symbol graph.
    let mut graph = SymbolGraph::build(&parsed_files);

    // Detect entrypoints.
    let entrypoints = entrypoint::detect_entrypoints(&parsed_files);

    // Run data flow analysis and enrich graph.
    let flow_analysis = flow::analyze_data_flow(&parsed_files, &FlowConfig::default());
    flow::enrich_graph(&mut graph, &flow_analysis);

    // Cluster changed files.
    let changed_files: Vec<String> = diff_result
        .files
        .iter()
        .map(|f| f.path().to_string())
        .collect();
    let cluster_result = cluster::cluster_files(&graph, &entrypoints, &changed_files);

    // Rank groups.
    let weights = RankWeights::default();
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
                centrality: 0.5, // Simplified — no PageRank for now
                surface_area: compute_surface_area(total_add, total_del, 1000),
                uncertainty: if risk_flags.has_test_only {
                    0.1
                } else {
                    0.5
                },
            }
        })
        .collect();

    let ranked = rank::rank_groups(&rank_inputs, &weights);

    let diff_source = output::diff_source_branch(
        base_ref,
        head_ref,
        diff_result.base_sha.as_deref(),
        diff_result.head_sha.as_deref(),
    );

    build_analysis_output(&diff_result, diff_source, &parsed_files, &cluster_result, &ranked)
}

/// Find the feature branch name in a repo (first non-main branch).
pub fn find_feature_branch(repo_path: &Path) -> String {
    let repo = Repository::open(repo_path).unwrap();
    let branches = repo
        .branches(Some(git2::BranchType::Local))
        .unwrap();
    for branch in branches {
        let (branch, _) = branch.unwrap();
        let name = branch.name().unwrap().unwrap().to_string();
        if name != "main" {
            return name;
        }
    }
    panic!("No feature branch found");
}
