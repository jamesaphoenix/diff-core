/** TypeScript types matching the diffcore-core Rust output schema (types.rs + schema.rs). */

export type SymbolKind =
  | "Function"
  | "Class"
  | "Struct"
  | "Interface"
  | "TypeAlias"
  | "Constant"
  | "Module";

export type EdgeType =
  | "Imports"
  | "Calls"
  | "Extends"
  | "Instantiates"
  | "Reads"
  | "Writes"
  | "Emits"
  | "Handles";

export type FileRole =
  | "Entrypoint"
  | "Handler"
  | "Service"
  | "Repository"
  | "Model"
  | "Utility"
  | "Config"
  | "Test"
  | "Infrastructure";

export type EntrypointType =
  | "HttpRoute"
  | "CliCommand"
  | "QueueConsumer"
  | "CronJob"
  | "ReactPage"
  | "TestFile"
  | "EventHandler"
  | "EffectService";

export type DiffType =
  | "BranchComparison"
  | "CommitRange"
  | "Staged"
  | "Unstaged"
  | "Worktree"
  | "BranchWithWorktree";

export interface Symbol {
  name: string;
  file: string;
  kind: SymbolKind;
}

export interface FlowEdge {
  from: string;
  to: string;
  edge_type: EdgeType;
}

export interface ChangeStats {
  additions: number;
  deletions: number;
}

export interface FileChange {
  path: string;
  flow_position: number;
  role: FileRole;
  changes: ChangeStats;
  symbols_changed: string[];
}

export interface Entrypoint {
  file: string;
  symbol: string;
  entrypoint_type: EntrypointType;
}

export type GroupType =
  | "Feat" | "Fix" | "Perf" | "Refactor" | "Test" | "Docs" | "Build" | "Ci" | "Chore";
export type Risk = "Low" | "Medium" | "High" | "Critical";
export type ImpactScope = "Local" | "Module" | "CrossCutting" | "System";
export type ReviewComplexity = "Trivial" | "Simple" | "Moderate" | "Complex";
export type ReviewFocus =
  | "Correctness" | "Security" | "Concurrency" | "Performance"
  | "DataIntegrity" | "Compatibility" | "ErrorHandling" | "ApiContract";

export interface FlowGroup {
  id: string;
  name: string;
  entrypoint: Entrypoint | null;
  files: FileChange[];
  edges: FlowEdge[];
  risk_score: number;
  review_order: number;
  group_type?: GroupType | null;
  description?: string | null;
  risk?: Risk | null;
  impact?: ImpactScope | null;
  complexity?: ReviewComplexity | null;
  review_focus?: ReviewFocus[];
  summary?: string[];
  invariant?: string | null;
}

export interface InfrastructureGroup {
  files: string[];
  reason: string;
}

export interface DiffSource {
  diff_type: DiffType;
  base: string | null;
  head: string | null;
  base_sha: string | null;
  head_sha: string | null;
}

export interface AnalysisSummary {
  total_files_changed: number;
  total_groups: number;
  languages_detected: string[];
  frameworks_detected: string[];
}

export interface AnalysisOutput {
  version: string;
  diff_source: DiffSource;
  summary: AnalysisSummary;
  groups: FlowGroup[];
  infrastructure_group: InfrastructureGroup | null;
  annotations: Pass1Response | null;
}

// LLM annotation types (from schema.rs)

export interface Pass1Response {
  overall_summary: string;
  suggested_review_order: string[];
}

export interface Pass2FileAnnotation {
  file: string;
  role_in_flow: string;
  changes_summary: string;
  risks: string[];
  suggestions: string[];
}

export interface Pass2Response {
  group_id: string;
  flow_narrative: string;
  file_annotations: Pass2FileAnnotation[];
  cross_cutting_concerns: string[];
}
