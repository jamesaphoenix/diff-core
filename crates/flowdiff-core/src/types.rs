use serde::{Deserialize, Serialize};

/// A symbol in the codebase (function, class, type, module).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub file: String,
    pub kind: SymbolKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SymbolKind {
    Function,
    Class,
    Struct,
    Interface,
    TypeAlias,
    Constant,
    Module,
}

/// Edge types in the symbol graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EdgeType {
    Imports,
    Calls,
    Extends,
    Instantiates,
    Reads,
    Writes,
    Emits,
    Handles,
}

/// An edge in the flow graph between two symbols.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FlowEdge {
    pub from: String,
    pub to: String,
    pub edge_type: EdgeType,
}

/// Change statistics for a file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChangeStats {
    pub additions: u32,
    pub deletions: u32,
}

/// A changed file within a flow group.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileChange {
    pub path: String,
    pub flow_position: u32,
    pub role: FileRole,
    pub changes: ChangeStats,
    pub symbols_changed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FileRole {
    Entrypoint,
    Handler,
    Service,
    Repository,
    Model,
    Utility,
    Config,
    Test,
    Infrastructure,
}

/// Entrypoint type detection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entrypoint {
    pub file: String,
    pub symbol: String,
    pub entrypoint_type: EntrypointType,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EntrypointType {
    HttpRoute,
    CliCommand,
    QueueConsumer,
    CronJob,
    ReactPage,
    TestFile,
    EventHandler,
}

/// A semantic flow group — a set of files participating in the same data flow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FlowGroup {
    pub id: String,
    pub name: String,
    pub entrypoint: Option<Entrypoint>,
    pub files: Vec<FileChange>,
    pub edges: Vec<FlowEdge>,
    pub risk_score: f64,
    pub review_order: u32,
}

/// Risk indicators detected in changed files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RiskIndicators {
    /// Schema/migration changes detected
    pub has_schema_change: bool,
    /// Public API surface changes
    pub has_api_change: bool,
    /// Auth/security-related changes
    pub has_auth_change: bool,
    /// Database migration files changed
    pub has_db_migration: bool,
}

/// Input data for ranking a single flow group.
#[derive(Debug, Clone)]
pub struct GroupRankInput {
    pub group_id: String,
    /// Risk score component [0.0, 1.0]
    pub risk: f64,
    /// Centrality score component [0.0, 1.0] (PageRank or betweenness)
    pub centrality: f64,
    /// Surface area score component [0.0, 1.0] (normalized change volume)
    pub surface_area: f64,
    /// Uncertainty score component [0.0, 1.0] (inverse test coverage, heuristic edges)
    pub uncertainty: f64,
}

/// Weights for the composite ranking score.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RankWeights {
    pub risk: f64,
    pub centrality: f64,
    pub surface_area: f64,
    pub uncertainty: f64,
}

impl Default for RankWeights {
    fn default() -> Self {
        Self {
            risk: 0.35,
            centrality: 0.25,
            surface_area: 0.20,
            uncertainty: 0.20,
        }
    }
}

/// Output of the ranking process for a single group.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RankedGroup {
    pub group_id: String,
    pub composite_score: f64,
    pub review_order: u32,
}

/// Diff source information.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiffSource {
    pub diff_type: DiffType,
    pub base: Option<String>,
    pub head: Option<String>,
    pub base_sha: Option<String>,
    pub head_sha: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DiffType {
    BranchComparison,
    CommitRange,
    Staged,
    Unstaged,
}

/// Summary statistics for the analysis output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnalysisSummary {
    pub total_files_changed: u32,
    pub total_groups: u32,
    pub languages_detected: Vec<String>,
    pub frameworks_detected: Vec<String>,
}

/// Infrastructure group for files not reachable from any entrypoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InfrastructureGroup {
    pub files: Vec<String>,
    pub reason: String,
}

/// Complete analysis output matching the CLI JSON schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnalysisOutput {
    pub version: String,
    pub diff_source: DiffSource,
    pub summary: AnalysisSummary,
    pub groups: Vec<FlowGroup>,
    pub infrastructure_group: Option<InfrastructureGroup>,
    pub annotations: Option<serde_json::Value>,
}
