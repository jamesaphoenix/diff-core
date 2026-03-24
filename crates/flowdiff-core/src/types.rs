use serde::{Deserialize, Serialize};

/// A symbol in the codebase (function, class, type, module).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Symbol {
    pub name: String,
    pub file: String,
    pub kind: SymbolKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
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
    EffectService,
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

/// Category for infrastructure sub-groups.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum InfraCategory {
    /// True infrastructure: Docker, CI/CD, env configs, build configs, package manager
    Infrastructure,
    /// Schema/type/DTO files
    Schema,
    /// Shell scripts and dev tooling scripts
    Script,
    /// Database migrations and seed files
    Migration,
    /// Deployment scripts and configs
    Deployment,
    /// Documentation files
    Documentation,
    /// Linter/formatter configs
    Lint,
    /// Test utilities, fixtures, helpers
    TestUtil,
    /// Generated code
    Generated,
    /// Files grouped by shared directory prefix
    DirectoryGroup,
    /// Files with no category match
    Unclassified,
}

/// A sub-group within the ungrouped files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InfraSubGroup {
    /// Human-readable name: "Schemas", "scripts/", "Configuration", etc.
    pub name: String,
    /// Classification category
    pub category: InfraCategory,
    /// Files in this sub-group
    pub files: Vec<String>,
}

/// Infrastructure group for files not reachable from any entrypoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InfrastructureGroup {
    /// Flat file list for backward compatibility with JSON consumers
    pub files: Vec<String>,
    /// Semantically organized sub-groups
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sub_groups: Vec<InfraSubGroup>,
    /// Reason these files weren't assigned to flow groups
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod tests {
    use super::*;

    // ── Helper constructors ──────────────────────────────────────────

    fn sample_symbol() -> Symbol {
        Symbol {
            name: "handleRequest".into(),
            file: "src/handler.ts".into(),
            kind: SymbolKind::Function,
        }
    }

    fn sample_flow_edge() -> FlowEdge {
        FlowEdge {
            from: "src/handler.ts::handleRequest".into(),
            to: "src/service.ts::createUser".into(),
            edge_type: EdgeType::Calls,
        }
    }

    fn sample_change_stats() -> ChangeStats {
        ChangeStats {
            additions: 25,
            deletions: 10,
        }
    }

    fn sample_file_change() -> FileChange {
        FileChange {
            path: "src/handler.ts".into(),
            flow_position: 0,
            role: FileRole::Entrypoint,
            changes: sample_change_stats(),
            symbols_changed: vec!["handleRequest".into(), "validateInput".into()],
        }
    }

    fn sample_entrypoint() -> Entrypoint {
        Entrypoint {
            file: "src/handler.ts".into(),
            symbol: "POST".into(),
            entrypoint_type: EntrypointType::HttpRoute,
        }
    }

    fn sample_flow_group() -> FlowGroup {
        FlowGroup {
            id: "group_1".into(),
            name: "POST /api/users creation flow".into(),
            entrypoint: Some(sample_entrypoint()),
            files: vec![sample_file_change()],
            edges: vec![sample_flow_edge()],
            risk_score: 0.82,
            review_order: 1,
        }
    }

    fn sample_diff_source() -> DiffSource {
        DiffSource {
            diff_type: DiffType::BranchComparison,
            base: Some("main".into()),
            head: Some("feature-branch".into()),
            base_sha: Some("abc123".into()),
            head_sha: Some("def456".into()),
        }
    }

    fn sample_analysis_output() -> AnalysisOutput {
        AnalysisOutput {
            version: "1.0.0".into(),
            diff_source: sample_diff_source(),
            summary: AnalysisSummary {
                total_files_changed: 5,
                total_groups: 2,
                languages_detected: vec!["typescript".into()],
                frameworks_detected: vec!["express".into()],
            },
            groups: vec![sample_flow_group()],
            infrastructure_group: Some(InfrastructureGroup {
                files: vec!["tsconfig.json".into(), "package.json".into()],
                sub_groups: vec![],
                reason: "Not reachable from any detected entrypoint".into(),
            }),
            annotations: None,
        }
    }

    // ── Serde roundtrip tests ────────────────────────────────────────

    #[test]
    fn serde_roundtrip_symbol() {
        let s = sample_symbol();
        let json = serde_json::to_string(&s).unwrap();
        let back: Symbol = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn serde_roundtrip_all_symbol_kinds() {
        let kinds = [
            SymbolKind::Function,
            SymbolKind::Class,
            SymbolKind::Struct,
            SymbolKind::Interface,
            SymbolKind::TypeAlias,
            SymbolKind::Constant,
            SymbolKind::Module,
        ];
        for kind in &kinds {
            let s = Symbol {
                name: "x".into(),
                file: "f".into(),
                kind: *kind,
            };
            let json = serde_json::to_string(&s).unwrap();
            let back: Symbol = serde_json::from_str(&json).unwrap();
            assert_eq!(s, back, "roundtrip failed for {:?}", kind);
        }
    }

    #[test]
    fn serde_roundtrip_all_edge_types() {
        let types = [
            EdgeType::Imports,
            EdgeType::Calls,
            EdgeType::Extends,
            EdgeType::Instantiates,
            EdgeType::Reads,
            EdgeType::Writes,
            EdgeType::Emits,
            EdgeType::Handles,
        ];
        for et in &types {
            let edge = FlowEdge {
                from: "a".into(),
                to: "b".into(),
                edge_type: et.clone(),
            };
            let json = serde_json::to_string(&edge).unwrap();
            let back: FlowEdge = serde_json::from_str(&json).unwrap();
            assert_eq!(edge, back, "roundtrip failed for {:?}", et);
        }
    }

    #[test]
    fn serde_roundtrip_all_file_roles() {
        let roles = [
            FileRole::Entrypoint,
            FileRole::Handler,
            FileRole::Service,
            FileRole::Repository,
            FileRole::Model,
            FileRole::Utility,
            FileRole::Config,
            FileRole::Test,
            FileRole::Infrastructure,
        ];
        for role in &roles {
            let fc = FileChange {
                path: "f.ts".into(),
                flow_position: 0,
                role: role.clone(),
                changes: ChangeStats {
                    additions: 1,
                    deletions: 0,
                },
                symbols_changed: vec![],
            };
            let json = serde_json::to_string(&fc).unwrap();
            let back: FileChange = serde_json::from_str(&json).unwrap();
            assert_eq!(fc, back, "roundtrip failed for {:?}", role);
        }
    }

    #[test]
    fn serde_roundtrip_all_entrypoint_types() {
        let types = [
            EntrypointType::HttpRoute,
            EntrypointType::CliCommand,
            EntrypointType::QueueConsumer,
            EntrypointType::CronJob,
            EntrypointType::ReactPage,
            EntrypointType::TestFile,
            EntrypointType::EventHandler,
            EntrypointType::EffectService,
        ];
        for et in &types {
            let ep = Entrypoint {
                file: "f".into(),
                symbol: "s".into(),
                entrypoint_type: et.clone(),
            };
            let json = serde_json::to_string(&ep).unwrap();
            let back: Entrypoint = serde_json::from_str(&json).unwrap();
            assert_eq!(ep, back, "roundtrip failed for {:?}", et);
        }
    }

    #[test]
    fn serde_roundtrip_all_diff_types() {
        let types = [
            DiffType::BranchComparison,
            DiffType::CommitRange,
            DiffType::Staged,
            DiffType::Unstaged,
        ];
        for dt in &types {
            let ds = DiffSource {
                diff_type: dt.clone(),
                base: None,
                head: None,
                base_sha: None,
                head_sha: None,
            };
            let json = serde_json::to_string(&ds).unwrap();
            let back: DiffSource = serde_json::from_str(&json).unwrap();
            assert_eq!(ds, back, "roundtrip failed for {:?}", dt);
        }
    }

    #[test]
    fn serde_roundtrip_flow_group() {
        let g = sample_flow_group();
        let json = serde_json::to_string(&g).unwrap();
        let back: FlowGroup = serde_json::from_str(&json).unwrap();
        assert_eq!(g, back);
    }

    #[test]
    fn serde_roundtrip_flow_group_no_entrypoint() {
        let g = FlowGroup {
            id: "infra".into(),
            name: "Infrastructure".into(),
            entrypoint: None,
            files: vec![],
            edges: vec![],
            risk_score: 0.1,
            review_order: 5,
        };
        let json = serde_json::to_string(&g).unwrap();
        let back: FlowGroup = serde_json::from_str(&json).unwrap();
        assert_eq!(g, back);
    }

    #[test]
    fn serde_roundtrip_risk_indicators() {
        let ri = RiskIndicators {
            has_schema_change: true,
            has_api_change: false,
            has_auth_change: true,
            has_db_migration: false,
        };
        let json = serde_json::to_string(&ri).unwrap();
        let back: RiskIndicators = serde_json::from_str(&json).unwrap();
        assert_eq!(ri, back);
    }

    #[test]
    fn serde_roundtrip_rank_weights() {
        let w = RankWeights {
            risk: 0.4,
            centrality: 0.3,
            surface_area: 0.2,
            uncertainty: 0.1,
        };
        let json = serde_json::to_string(&w).unwrap();
        let back: RankWeights = serde_json::from_str(&json).unwrap();
        assert_eq!(w, back);
    }

    #[test]
    fn serde_roundtrip_ranked_group() {
        let rg = RankedGroup {
            group_id: "g1".into(),
            composite_score: 0.75,
            review_order: 1,
        };
        let json = serde_json::to_string(&rg).unwrap();
        let back: RankedGroup = serde_json::from_str(&json).unwrap();
        assert_eq!(rg, back);
    }

    #[test]
    fn serde_roundtrip_analysis_output() {
        let out = sample_analysis_output();
        let json = serde_json::to_string(&out).unwrap();
        let back: AnalysisOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(out, back);
    }

    #[test]
    fn serde_roundtrip_analysis_output_with_annotations() {
        let mut out = sample_analysis_output();
        out.annotations = Some(serde_json::json!({
            "overall_summary": "This PR adds user creation",
            "groups": [{"id": "group_1", "summary": "User creation flow"}]
        }));
        let json = serde_json::to_string(&out).unwrap();
        let back: AnalysisOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(out, back);
    }

    #[test]
    fn serde_roundtrip_analysis_output_no_infrastructure() {
        let mut out = sample_analysis_output();
        out.infrastructure_group = None;
        let json = serde_json::to_string(&out).unwrap();
        let back: AnalysisOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(out, back);
    }

    // ── Default impl tests ───────────────────────────────────────────

    #[test]
    fn rank_weights_default_values() {
        let w = RankWeights::default();
        assert!((w.risk - 0.35).abs() < f64::EPSILON);
        assert!((w.centrality - 0.25).abs() < f64::EPSILON);
        assert!((w.surface_area - 0.20).abs() < f64::EPSILON);
        assert!((w.uncertainty - 0.20).abs() < f64::EPSILON);
    }

    #[test]
    fn rank_weights_default_sum_to_one() {
        let w = RankWeights::default();
        let sum = w.risk + w.centrality + w.surface_area + w.uncertainty;
        assert!(
            (sum - 1.0).abs() < f64::EPSILON,
            "default weights should sum to 1.0, got {}",
            sum
        );
    }

    // ── Clone/PartialEq tests ────────────────────────────────────────

    #[test]
    fn symbol_clone_eq() {
        let s = sample_symbol();
        let cloned = s.clone();
        assert_eq!(s, cloned);
    }

    #[test]
    fn symbol_ne_different_name() {
        let s1 = sample_symbol();
        let mut s2 = sample_symbol();
        s2.name = "other".into();
        assert_ne!(s1, s2);
    }

    #[test]
    fn symbol_ne_different_kind() {
        let s1 = sample_symbol();
        let mut s2 = sample_symbol();
        s2.kind = SymbolKind::Class;
        assert_ne!(s1, s2);
    }

    #[test]
    fn flow_group_clone_eq() {
        let g = sample_flow_group();
        assert_eq!(g, g.clone());
    }

    #[test]
    fn analysis_output_clone_eq() {
        let out = sample_analysis_output();
        assert_eq!(out, out.clone());
    }

    // ── JSON field naming tests ──────────────────────────────────────

    #[test]
    fn json_field_names_snake_case() {
        let out = sample_analysis_output();
        let json = serde_json::to_value(&out).unwrap();
        let obj = json.as_object().unwrap();
        assert!(obj.contains_key("diff_source"));
        assert!(obj.contains_key("infrastructure_group"));

        let ds = obj.get("diff_source").unwrap().as_object().unwrap();
        assert!(ds.contains_key("diff_type"));
        assert!(ds.contains_key("base_sha"));
        assert!(ds.contains_key("head_sha"));

        let summary = obj.get("summary").unwrap().as_object().unwrap();
        assert!(summary.contains_key("total_files_changed"));
        assert!(summary.contains_key("total_groups"));
        assert!(summary.contains_key("languages_detected"));
        assert!(summary.contains_key("frameworks_detected"));
    }

    #[test]
    fn json_enum_variant_names() {
        let json = serde_json::to_value(SymbolKind::TypeAlias).unwrap();
        assert_eq!(json.as_str().unwrap(), "TypeAlias");

        let json = serde_json::to_value(EdgeType::Instantiates).unwrap();
        assert_eq!(json.as_str().unwrap(), "Instantiates");

        let json = serde_json::to_value(FileRole::Infrastructure).unwrap();
        assert_eq!(json.as_str().unwrap(), "Infrastructure");

        let json = serde_json::to_value(EntrypointType::EffectService).unwrap();
        assert_eq!(json.as_str().unwrap(), "EffectService");

        let json = serde_json::to_value(DiffType::BranchComparison).unwrap();
        assert_eq!(json.as_str().unwrap(), "BranchComparison");
    }

    // ── Deserialization from raw JSON strings ────────────────────────

    #[test]
    fn deserialize_symbol_from_json_literal() {
        let json = r#"{"name":"foo","file":"bar.ts","kind":"Function"}"#;
        let s: Symbol = serde_json::from_str(json).unwrap();
        assert_eq!(s.name, "foo");
        assert_eq!(s.file, "bar.ts");
        assert_eq!(s.kind, SymbolKind::Function);
    }

    #[test]
    fn deserialize_rejects_unknown_symbol_kind() {
        let json = r#"{"name":"x","file":"f","kind":"Banana"}"#;
        let result: Result<Symbol, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn deserialize_rejects_missing_required_field() {
        let json = r#"{"name":"x","file":"f"}"#;
        let result: Result<Symbol, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn deserialize_analysis_output_from_spec_json() {
        let json = r#"{
            "version": "1.0.0",
            "diff_source": {
                "diff_type": "BranchComparison",
                "base": "main",
                "head": "feature-branch",
                "base_sha": "abc123",
                "head_sha": "def456"
            },
            "summary": {
                "total_files_changed": 47,
                "total_groups": 5,
                "languages_detected": ["typescript", "python"],
                "frameworks_detected": ["nextjs", "fastapi"]
            },
            "groups": [],
            "infrastructure_group": {
                "files": ["tsconfig.json"],
                "reason": "Not reachable from any detected entrypoint"
            },
            "annotations": null
        }"#;
        let out: AnalysisOutput = serde_json::from_str(json).unwrap();
        assert_eq!(out.version, "1.0.0");
        assert_eq!(out.summary.total_files_changed, 47);
        assert_eq!(out.summary.languages_detected.len(), 2);
        assert!(out.infrastructure_group.is_some());
        assert!(out.annotations.is_none());
    }

    // ── Edge case: empty collections ─────────────────────────────────

    #[test]
    fn flow_group_empty_files_and_edges() {
        let g = FlowGroup {
            id: "empty".into(),
            name: "Empty group".into(),
            entrypoint: None,
            files: vec![],
            edges: vec![],
            risk_score: 0.0,
            review_order: 0,
        };
        let json = serde_json::to_string(&g).unwrap();
        let back: FlowGroup = serde_json::from_str(&json).unwrap();
        assert_eq!(g, back);
        assert!(back.files.is_empty());
        assert!(back.edges.is_empty());
    }

    #[test]
    fn analysis_output_empty_groups() {
        let out = AnalysisOutput {
            version: "1.0.0".into(),
            diff_source: DiffSource {
                diff_type: DiffType::Staged,
                base: None,
                head: None,
                base_sha: None,
                head_sha: None,
            },
            summary: AnalysisSummary {
                total_files_changed: 0,
                total_groups: 0,
                languages_detected: vec![],
                frameworks_detected: vec![],
            },
            groups: vec![],
            infrastructure_group: None,
            annotations: None,
        };
        let json = serde_json::to_string(&out).unwrap();
        let back: AnalysisOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(out, back);
    }

    // ── Edge case: special characters ────────────────────────────────

    #[test]
    fn symbol_with_unicode_name() {
        let s = Symbol {
            name: "处理请求".into(),
            file: "src/处理.ts".into(),
            kind: SymbolKind::Function,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: Symbol = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn file_change_with_special_path_chars() {
        let fc = FileChange {
            path: "src/components/[slug]/page.tsx".into(),
            flow_position: 0,
            role: FileRole::Entrypoint,
            changes: ChangeStats {
                additions: 1,
                deletions: 0,
            },
            symbols_changed: vec!["default".into()],
        };
        let json = serde_json::to_string(&fc).unwrap();
        let back: FileChange = serde_json::from_str(&json).unwrap();
        assert_eq!(fc, back);
    }

    // ── SymbolKind Copy trait ────────────────────────────────────────

    #[test]
    fn symbol_kind_is_copy() {
        let k = SymbolKind::Function;
        let k2 = k; // Copy, not move
        assert_eq!(k, k2);
    }

    // ── Hash trait for SymbolKind and EdgeType ───────────────────────

    #[test]
    fn symbol_kind_hash_distinct_variants() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(SymbolKind::Function);
        set.insert(SymbolKind::Class);
        set.insert(SymbolKind::Struct);
        set.insert(SymbolKind::Interface);
        set.insert(SymbolKind::TypeAlias);
        set.insert(SymbolKind::Constant);
        set.insert(SymbolKind::Module);
        assert_eq!(set.len(), 7, "all 7 SymbolKind variants should be distinct");
    }

    #[test]
    fn edge_type_hash_distinct_variants() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(EdgeType::Imports);
        set.insert(EdgeType::Calls);
        set.insert(EdgeType::Extends);
        set.insert(EdgeType::Instantiates);
        set.insert(EdgeType::Reads);
        set.insert(EdgeType::Writes);
        set.insert(EdgeType::Emits);
        set.insert(EdgeType::Handles);
        assert_eq!(set.len(), 8, "all 8 EdgeType variants should be distinct");
    }

    // ── Property-based tests ─────────────────────────────────────────

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        fn arb_symbol_kind() -> impl Strategy<Value = SymbolKind> {
            prop_oneof![
                Just(SymbolKind::Function),
                Just(SymbolKind::Class),
                Just(SymbolKind::Struct),
                Just(SymbolKind::Interface),
                Just(SymbolKind::TypeAlias),
                Just(SymbolKind::Constant),
                Just(SymbolKind::Module),
            ]
        }

        fn arb_edge_type() -> impl Strategy<Value = EdgeType> {
            prop_oneof![
                Just(EdgeType::Imports),
                Just(EdgeType::Calls),
                Just(EdgeType::Extends),
                Just(EdgeType::Instantiates),
                Just(EdgeType::Reads),
                Just(EdgeType::Writes),
                Just(EdgeType::Emits),
                Just(EdgeType::Handles),
            ]
        }

        fn arb_file_role() -> impl Strategy<Value = FileRole> {
            prop_oneof![
                Just(FileRole::Entrypoint),
                Just(FileRole::Handler),
                Just(FileRole::Service),
                Just(FileRole::Repository),
                Just(FileRole::Model),
                Just(FileRole::Utility),
                Just(FileRole::Config),
                Just(FileRole::Test),
                Just(FileRole::Infrastructure),
            ]
        }

        fn arb_entrypoint_type() -> impl Strategy<Value = EntrypointType> {
            prop_oneof![
                Just(EntrypointType::HttpRoute),
                Just(EntrypointType::CliCommand),
                Just(EntrypointType::QueueConsumer),
                Just(EntrypointType::CronJob),
                Just(EntrypointType::ReactPage),
                Just(EntrypointType::TestFile),
                Just(EntrypointType::EventHandler),
                Just(EntrypointType::EffectService),
            ]
        }

        fn arb_diff_type() -> impl Strategy<Value = DiffType> {
            prop_oneof![
                Just(DiffType::BranchComparison),
                Just(DiffType::CommitRange),
                Just(DiffType::Staged),
                Just(DiffType::Unstaged),
            ]
        }

        fn arb_symbol() -> impl Strategy<Value = Symbol> {
            ("[a-zA-Z_][a-zA-Z0-9_]{0,30}", "[a-z/]{1,50}\\.ts", arb_symbol_kind()).prop_map(
                |(name, file, kind)| Symbol { name, file, kind },
            )
        }

        fn arb_flow_edge() -> impl Strategy<Value = FlowEdge> {
            (".{1,50}", ".{1,50}", arb_edge_type()).prop_map(|(from, to, edge_type)| FlowEdge {
                from,
                to,
                edge_type,
            })
        }

        fn arb_change_stats() -> impl Strategy<Value = ChangeStats> {
            (0u32..10000, 0u32..10000)
                .prop_map(|(additions, deletions)| ChangeStats {
                    additions,
                    deletions,
                })
        }

        fn arb_rank_weights() -> impl Strategy<Value = RankWeights> {
            (0.0f64..=1.0, 0.0f64..=1.0, 0.0f64..=1.0, 0.0f64..=1.0).prop_map(
                |(risk, centrality, surface_area, uncertainty)| RankWeights {
                    risk,
                    centrality,
                    surface_area,
                    uncertainty,
                },
            )
        }

        proptest! {
            #[test]
            fn prop_symbol_serde_roundtrip(s in arb_symbol()) {
                let json = serde_json::to_string(&s).unwrap();
                let back: Symbol = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(&s, &back);
            }

            #[test]
            fn prop_flow_edge_serde_roundtrip(e in arb_flow_edge()) {
                let json = serde_json::to_string(&e).unwrap();
                let back: FlowEdge = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(&e, &back);
            }

            #[test]
            fn prop_change_stats_serde_roundtrip(cs in arb_change_stats()) {
                let json = serde_json::to_string(&cs).unwrap();
                let back: ChangeStats = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(&cs, &back);
            }

            #[test]
            fn prop_rank_weights_serde_roundtrip(w in arb_rank_weights()) {
                let json = serde_json::to_string(&w).unwrap();
                let back: RankWeights = serde_json::from_str(&json).unwrap();
                // Use approximate comparison — JSON f64 roundtrip can lose ULP precision
                prop_assert!((w.risk - back.risk).abs() < 1e-14);
                prop_assert!((w.centrality - back.centrality).abs() < 1e-14);
                prop_assert!((w.surface_area - back.surface_area).abs() < 1e-14);
                prop_assert!((w.uncertainty - back.uncertainty).abs() < 1e-14);
            }

            #[test]
            fn prop_symbol_kind_serde_roundtrip(k in arb_symbol_kind()) {
                let json = serde_json::to_string(&k).unwrap();
                let back: SymbolKind = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(k, back);
            }

            #[test]
            fn prop_edge_type_serde_roundtrip(et in arb_edge_type()) {
                let json = serde_json::to_string(&et).unwrap();
                let back: EdgeType = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(&et, &back);
            }

            #[test]
            fn prop_file_role_serde_roundtrip(r in arb_file_role()) {
                let json = serde_json::to_string(&r).unwrap();
                let back: FileRole = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(&r, &back);
            }

            #[test]
            fn prop_entrypoint_type_serde_roundtrip(et in arb_entrypoint_type()) {
                let json = serde_json::to_string(&et).unwrap();
                let back: EntrypointType = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(&et, &back);
            }

            #[test]
            fn prop_diff_type_serde_roundtrip(dt in arb_diff_type()) {
                let json = serde_json::to_string(&dt).unwrap();
                let back: DiffType = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(&dt, &back);
            }

            #[test]
            fn prop_rank_weights_all_fields_nonnegative(w in arb_rank_weights()) {
                prop_assert!(w.risk >= 0.0);
                prop_assert!(w.centrality >= 0.0);
                prop_assert!(w.surface_area >= 0.0);
                prop_assert!(w.uncertainty >= 0.0);
            }

            #[test]
            fn prop_change_stats_total_nonnegative(cs in arb_change_stats()) {
                // additions + deletions should not overflow for values < 10000
                let total = cs.additions as u64 + cs.deletions as u64;
                prop_assert!(total < 20000);
            }

            #[test]
            fn prop_infra_category_serde_roundtrip(cat in arb_infra_category()) {
                let json = serde_json::to_string(&cat).unwrap();
                let back: InfraCategory = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(&cat, &back);
            }

            #[test]
            fn prop_infra_sub_group_serde_roundtrip(
                name in "[a-zA-Z ]{1,20}",
                cat in arb_infra_category(),
                files in prop::collection::vec("[a-z/]{1,30}\\.[a-z]{1,4}", 0..5)
            ) {
                let sg = InfraSubGroup { name, category: cat, files };
                let json = serde_json::to_string(&sg).unwrap();
                let back: InfraSubGroup = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(&sg, &back);
            }
        }

        fn arb_infra_category() -> impl Strategy<Value = InfraCategory> {
            prop_oneof![
                Just(InfraCategory::Infrastructure),
                Just(InfraCategory::Schema),
                Just(InfraCategory::Script),
                Just(InfraCategory::Migration),
                Just(InfraCategory::Deployment),
                Just(InfraCategory::Documentation),
                Just(InfraCategory::Lint),
                Just(InfraCategory::TestUtil),
                Just(InfraCategory::Generated),
                Just(InfraCategory::DirectoryGroup),
                Just(InfraCategory::Unclassified),
            ]
        }
    }

    // ── InfraCategory / InfraSubGroup tests ─────────────────────────

    #[test]
    fn serde_roundtrip_all_infra_categories() {
        let categories = [
            InfraCategory::Infrastructure,
            InfraCategory::Schema,
            InfraCategory::Script,
            InfraCategory::Migration,
            InfraCategory::Deployment,
            InfraCategory::Documentation,
            InfraCategory::Lint,
            InfraCategory::TestUtil,
            InfraCategory::Generated,
            InfraCategory::DirectoryGroup,
            InfraCategory::Unclassified,
        ];
        for cat in &categories {
            let json = serde_json::to_string(&cat).unwrap();
            let back: InfraCategory = serde_json::from_str(&json).unwrap();
            assert_eq!(cat, &back, "roundtrip failed for {:?}", cat);
        }
    }

    #[test]
    fn infra_category_hash_distinct_variants() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(InfraCategory::Infrastructure);
        set.insert(InfraCategory::Schema);
        set.insert(InfraCategory::Script);
        set.insert(InfraCategory::Migration);
        set.insert(InfraCategory::Deployment);
        set.insert(InfraCategory::Documentation);
        set.insert(InfraCategory::Lint);
        set.insert(InfraCategory::TestUtil);
        set.insert(InfraCategory::Generated);
        set.insert(InfraCategory::DirectoryGroup);
        set.insert(InfraCategory::Unclassified);
        assert_eq!(set.len(), 11, "all 11 InfraCategory variants should be distinct");
    }

    #[test]
    fn serde_roundtrip_infra_sub_group() {
        let sg = InfraSubGroup {
            name: "Schemas".into(),
            category: InfraCategory::Schema,
            files: vec!["schemas/user.ts".into(), "schemas/billing.ts".into()],
        };
        let json = serde_json::to_string(&sg).unwrap();
        let back: InfraSubGroup = serde_json::from_str(&json).unwrap();
        assert_eq!(sg, back);
    }

    #[test]
    fn serde_roundtrip_infrastructure_group_with_sub_groups() {
        let ig = InfrastructureGroup {
            files: vec!["Dockerfile".into(), "schemas/user.ts".into()],
            sub_groups: vec![
                InfraSubGroup {
                    name: "Infrastructure".into(),
                    category: InfraCategory::Infrastructure,
                    files: vec!["Dockerfile".into()],
                },
                InfraSubGroup {
                    name: "Schemas".into(),
                    category: InfraCategory::Schema,
                    files: vec!["schemas/user.ts".into()],
                },
            ],
            reason: "test".into(),
        };
        let json = serde_json::to_string(&ig).unwrap();
        let back: InfrastructureGroup = serde_json::from_str(&json).unwrap();
        assert_eq!(ig, back);
    }

    #[test]
    fn serde_infrastructure_group_backward_compat_no_sub_groups() {
        // Old JSON without sub_groups should still deserialize (default = [])
        let json = r#"{"files":["tsconfig.json"],"reason":"test"}"#;
        let ig: InfrastructureGroup = serde_json::from_str(json).unwrap();
        assert_eq!(ig.files, vec!["tsconfig.json"]);
        assert!(ig.sub_groups.is_empty());
    }

    #[test]
    fn serde_infrastructure_group_empty_sub_groups_not_serialized() {
        let ig = InfrastructureGroup {
            files: vec!["tsconfig.json".into()],
            sub_groups: vec![],
            reason: "test".into(),
        };
        let json = serde_json::to_string(&ig).unwrap();
        // sub_groups should be skipped when empty
        assert!(!json.contains("sub_groups"), "empty sub_groups should not be serialized");
    }
}
