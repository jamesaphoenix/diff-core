import * as vscode from "vscode";
import type { AnalysisOutput, FlowGroup, FileChange, InfrastructureGroup } from "./types";

// ── Tree item types ─────────────────────────────────────────────────

export class GroupItem extends vscode.TreeItem {
  constructor(public readonly group: FlowGroup) {
    super(group.name, vscode.TreeItemCollapsibleState.Collapsed);
    this.contextValue = "flowGroup";
    this.description = `risk: ${group.risk_score.toFixed(2)}`;
    this.tooltip = new vscode.MarkdownString(
      `**${group.name}**\n\n` +
        `- Risk: ${group.risk_score.toFixed(2)}\n` +
        `- Files: ${group.files.length}\n` +
        `- Review order: ${group.review_order}\n` +
        (group.entrypoint
          ? `- Entrypoint: \`${group.entrypoint.symbol}\` (${group.entrypoint.entrypoint_type})`
          : "- No entrypoint detected")
    );
    this.iconPath = riskIcon(group.risk_score);
  }
}

export class FileItem extends vscode.TreeItem {
  constructor(
    public readonly file: FileChange,
    public readonly groupId: string,
    private readonly repoPath: string
  ) {
    const shortPath = file.path.split("/").slice(-2).join("/");
    super(shortPath, vscode.TreeItemCollapsibleState.None);
    this.contextValue = "flowFile";
    this.description = `+${file.changes.additions} -${file.changes.deletions}`;
    this.tooltip = new vscode.MarkdownString(
      `**${file.path}**\n\n` +
        `- Role: ${file.role}\n` +
        `- Flow position: ${file.flow_position}\n` +
        `- Symbols changed: ${file.symbols_changed.join(", ") || "none"}`
    );
    this.iconPath = roleIcon(file.role);
    this.command = {
      command: "flowdiff.openDiff",
      title: "Open Diff",
      arguments: [this.repoPath, file.path, groupId],
    };
  }
}

export class InfraFileItem extends vscode.TreeItem {
  constructor(public readonly filePath: string) {
    const shortPath = filePath.split("/").slice(-2).join("/");
    super(shortPath, vscode.TreeItemCollapsibleState.None);
    this.contextValue = "infraFile";
    this.iconPath = new vscode.ThemeIcon("file");
  }
}

// ── Icon helpers ────────────────────────────────────────────────────

function riskIcon(score: number): vscode.ThemeIcon {
  if (score >= 0.7) {
    return new vscode.ThemeIcon("warning", new vscode.ThemeColor("charts.red"));
  }
  if (score >= 0.4) {
    return new vscode.ThemeIcon("info", new vscode.ThemeColor("charts.yellow"));
  }
  return new vscode.ThemeIcon("pass", new vscode.ThemeColor("charts.green"));
}

function roleIcon(role: string): vscode.ThemeIcon {
  switch (role) {
    case "Entrypoint":
      return new vscode.ThemeIcon("symbol-event");
    case "Handler":
      return new vscode.ThemeIcon("symbol-method");
    case "Service":
      return new vscode.ThemeIcon("symbol-class");
    case "Repository":
      return new vscode.ThemeIcon("database");
    case "Model":
      return new vscode.ThemeIcon("symbol-struct");
    case "Test":
      return new vscode.ThemeIcon("beaker");
    case "Config":
      return new vscode.ThemeIcon("settings-gear");
    default:
      return new vscode.ThemeIcon("file-code");
  }
}

// ── Flow Groups tree data provider ──────────────────────────────────

type FlowTreeItem = GroupItem | FileItem;

export class FlowGroupsProvider implements vscode.TreeDataProvider<FlowTreeItem> {
  private _onDidChangeTreeData = new vscode.EventEmitter<FlowTreeItem | undefined>();
  readonly onDidChangeTreeData = this._onDidChangeTreeData.event;

  private analysis: AnalysisOutput | null = null;
  private repoPath = "";

  setAnalysis(analysis: AnalysisOutput, repoPath: string): void {
    this.analysis = analysis;
    this.repoPath = repoPath;
    this._onDidChangeTreeData.fire(undefined);
  }

  clear(): void {
    this.analysis = null;
    this._onDidChangeTreeData.fire(undefined);
  }

  getAnalysis(): AnalysisOutput | null {
    return this.analysis;
  }

  getRepoPath(): string {
    return this.repoPath;
  }

  getTreeItem(element: FlowTreeItem): vscode.TreeItem {
    return element;
  }

  getChildren(element?: FlowTreeItem): FlowTreeItem[] {
    if (!this.analysis) {
      return [];
    }

    if (!element) {
      // Root level: return groups sorted by review_order
      return this.analysis.groups
        .slice()
        .sort((a, b) => a.review_order - b.review_order)
        .map((g) => new GroupItem(g));
    }

    if (element instanceof GroupItem) {
      // Group children: return files sorted by flow_position
      return element.group.files
        .slice()
        .sort((a, b) => a.flow_position - b.flow_position)
        .map((f) => new FileItem(f, element.group.id, this.repoPath));
    }

    return [];
  }
}

// ── Infrastructure tree data provider ───────────────────────────────

export class InfrastructureProvider implements vscode.TreeDataProvider<InfraFileItem> {
  private _onDidChangeTreeData = new vscode.EventEmitter<InfraFileItem | undefined>();
  readonly onDidChangeTreeData = this._onDidChangeTreeData.event;

  private infra: InfrastructureGroup | null = null;

  setInfrastructure(infra: InfrastructureGroup | null): void {
    this.infra = infra;
    this._onDidChangeTreeData.fire(undefined);
  }

  clear(): void {
    this.infra = null;
    this._onDidChangeTreeData.fire(undefined);
  }

  getTreeItem(element: InfraFileItem): vscode.TreeItem {
    return element;
  }

  getChildren(): InfraFileItem[] {
    if (!this.infra) {
      return [];
    }
    return this.infra.files.map((f) => new InfraFileItem(f));
  }
}
