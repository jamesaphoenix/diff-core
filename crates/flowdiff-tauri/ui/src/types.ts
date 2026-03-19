/** TypeScript types matching the Rust AnalysisOutput schema. */

export type DiffType = "BranchComparison" | "CommitRange" | "Staged" | "Unstaged";

export type EdgeType = "Imports" | "Calls" | "Extends" | "Instantiates" | "Reads" | "Writes" | "Emits" | "Handles";

export type FileRole = "Entrypoint" | "Handler" | "Service" | "Repository" | "Model" | "Utility" | "Config" | "Test" | "Infrastructure";

export type EntrypointType = "HttpRoute" | "CliCommand" | "QueueConsumer" | "CronJob" | "ReactPage" | "TestFile" | "EventHandler" | "EffectService";

export interface DiffSource {
  diff_type: DiffType;
  base: string | null;
  head: string | null;
  base_sha: string | null;
  head_sha: string | null;
}

export interface ChangeStats {
  additions: number;
  deletions: number;
}

export interface FlowEdge {
  from: string;
  to: string;
  edge_type: EdgeType;
}

export interface Entrypoint {
  file: string;
  symbol: string;
  entrypoint_type: EntrypointType;
}

export interface FileChange {
  path: string;
  flow_position: number;
  role: FileRole;
  changes: ChangeStats;
  symbols_changed: string[];
}

export interface FlowGroup {
  id: string;
  name: string;
  entrypoint: Entrypoint | null;
  files: FileChange[];
  edges: FlowEdge[];
  risk_score: number;
  review_order: number;
}

export interface InfrastructureGroup {
  files: string[];
  reason: string;
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
  annotations: unknown | null;
}

export interface FileDiffContent {
  path: string;
  old_content: string;
  new_content: string;
  language: string;
}

/** Parameters for the analyze command. */
export interface AnalyzeParams {
  repo_path: string;
  base?: string;
  head?: string;
  range?: string;
  staged: boolean;
  unstaged: boolean;
}
