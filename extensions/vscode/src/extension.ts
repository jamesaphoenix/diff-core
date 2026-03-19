import * as vscode from "vscode";
import * as path from "path";
import { runFlowdiff } from "./flowdiffRunner";
import { FlowGroupsProvider, InfrastructureProvider, GroupItem, FileItem } from "./treeView";
import { AnnotationsPanel } from "./webviewPanel";
import type { AnalysisOutput, FlowGroup } from "./types";

// ── State ───────────────────────────────────────────────────────────

let analysis: AnalysisOutput | null = null;
let repoPath = "";
let selectedGroupIndex = 0;
let selectedFileIndex = 0;

let groupsProvider: FlowGroupsProvider;
let infraProvider: InfrastructureProvider;
let annotationsPanel: AnnotationsPanel;
let groupsView: vscode.TreeView<GroupItem | FileItem>;

// ── Activation ──────────────────────────────────────────────────────

export function activate(context: vscode.ExtensionContext): void {
  // Tree data providers
  groupsProvider = new FlowGroupsProvider();
  infraProvider = new InfrastructureProvider();

  groupsView = vscode.window.createTreeView("flowdiff.groups", {
    treeDataProvider: groupsProvider,
    showCollapseAll: true,
  });

  vscode.window.createTreeView("flowdiff.infrastructure", {
    treeDataProvider: infraProvider,
  });

  // Annotations webview panel
  annotationsPanel = new AnnotationsPanel(context.extensionUri);

  // Register commands
  context.subscriptions.push(
    vscode.commands.registerCommand("flowdiff.analyze", cmdAnalyze),
    vscode.commands.registerCommand("flowdiff.analyzeRange", cmdAnalyzeRange),
    vscode.commands.registerCommand("flowdiff.annotate", cmdAnnotate),
    vscode.commands.registerCommand("flowdiff.nextFile", cmdNextFile),
    vscode.commands.registerCommand("flowdiff.prevFile", cmdPrevFile),
    vscode.commands.registerCommand("flowdiff.nextGroup", cmdNextGroup),
    vscode.commands.registerCommand("flowdiff.prevGroup", cmdPrevGroup),
    vscode.commands.registerCommand("flowdiff.openDiff", cmdOpenDiff),
    vscode.commands.registerCommand("flowdiff.openAnnotations", cmdOpenAnnotations),
    groupsView,
    { dispose: () => annotationsPanel.dispose() }
  );

  // Set context for keybinding activation
  vscode.commands.executeCommand("setContext", "flowdiff.active", false);
}

export function deactivate(): void {
  annotationsPanel?.dispose();
}

// ── Commands ────────────────────────────────────────────────────────

async function cmdAnalyze(): Promise<void> {
  const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
  if (!workspaceFolder) {
    vscode.window.showErrorMessage("No workspace folder open.");
    return;
  }

  repoPath = workspaceFolder.uri.fsPath;
  const config = vscode.workspace.getConfiguration("flowdiff");
  const base = config.get<string>("defaultBase", "main");

  await runAnalysis({ repoPath, base });
}

async function cmdAnalyzeRange(): Promise<void> {
  const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
  if (!workspaceFolder) {
    vscode.window.showErrorMessage("No workspace folder open.");
    return;
  }

  const range = await vscode.window.showInputBox({
    prompt: "Enter commit range (e.g., HEAD~5..HEAD)",
    placeHolder: "HEAD~5..HEAD",
  });

  if (!range) {
    return;
  }

  repoPath = workspaceFolder.uri.fsPath;
  await runAnalysis({ repoPath, range });
}

async function cmdAnnotate(): Promise<void> {
  if (!analysis) {
    vscode.window.showWarningMessage("Run flowdiff.analyze first.");
    return;
  }

  await vscode.window.withProgress(
    {
      location: vscode.ProgressLocation.Notification,
      title: "flowdiff: Running LLM annotation...",
      cancellable: false,
    },
    async () => {
      try {
        const result = await runFlowdiff({
          repoPath,
          base: analysis!.diff_source.base ?? "main",
          head: analysis!.diff_source.head ?? undefined,
          annotate: true,
        });
        analysis = result.output;
        groupsProvider.setAnalysis(analysis, repoPath);

        if (analysis.annotations) {
          annotationsPanel.setPass1(analysis.annotations as any);
        }

        vscode.window.showInformationMessage("flowdiff: LLM annotation complete.");
      } catch (err: unknown) {
        const msg = err instanceof Error ? err.message : String(err);
        vscode.window.showErrorMessage(`flowdiff annotation failed: ${msg}`);
      }
    }
  );
}

function cmdNextFile(): void {
  if (!analysis || analysis.groups.length === 0) {
    return;
  }
  const group = sortedGroups()[selectedGroupIndex];
  if (selectedFileIndex < group.files.length - 1) {
    selectedFileIndex++;
    openCurrentFile();
  }
}

function cmdPrevFile(): void {
  if (!analysis || analysis.groups.length === 0) {
    return;
  }
  if (selectedFileIndex > 0) {
    selectedFileIndex--;
    openCurrentFile();
  }
}

function cmdNextGroup(): void {
  if (!analysis || analysis.groups.length === 0) {
    return;
  }
  if (selectedGroupIndex < analysis.groups.length - 1) {
    selectedGroupIndex++;
    selectedFileIndex = 0;
    openCurrentFile();
    showCurrentGroupAnnotations();
  }
}

function cmdPrevGroup(): void {
  if (!analysis || analysis.groups.length === 0) {
    return;
  }
  if (selectedGroupIndex > 0) {
    selectedGroupIndex--;
    selectedFileIndex = 0;
    openCurrentFile();
    showCurrentGroupAnnotations();
  }
}

async function cmdOpenDiff(
  diffRepoPath: string,
  filePath: string,
  _groupId: string
): Promise<void> {
  const baseRef = analysis?.diff_source.base ?? "main";
  const headRef = analysis?.diff_source.head ?? "HEAD";

  try {
    const baseUri = vscode.Uri.parse(
      `git-show:${baseRef}:${filePath}`
    ).with({ scheme: "git", query: JSON.stringify({ ref: baseRef, path: filePath }) });

    const headUri = vscode.Uri.file(path.join(diffRepoPath, filePath));

    await vscode.commands.executeCommand(
      "vscode.diff",
      baseUri,
      headUri,
      `${filePath} (${baseRef} ↔ ${headRef})`
    );
  } catch {
    // Fallback: open file directly if git scheme fails
    const fileUri = vscode.Uri.file(path.join(diffRepoPath, filePath));
    await vscode.commands.executeCommand("vscode.open", fileUri);
  }
}

function cmdOpenAnnotations(): void {
  if (!analysis || analysis.groups.length === 0) {
    return;
  }
  showCurrentGroupAnnotations();
}

// ── Helpers ─────────────────────────────────────────────────────────

function sortedGroups(): FlowGroup[] {
  if (!analysis) {
    return [];
  }
  return analysis.groups.slice().sort((a, b) => a.review_order - b.review_order);
}

async function runAnalysis(options: {
  repoPath: string;
  base?: string;
  head?: string;
  range?: string;
}): Promise<void> {
  await vscode.window.withProgress(
    {
      location: vscode.ProgressLocation.Notification,
      title: "flowdiff: Analyzing...",
      cancellable: false,
    },
    async () => {
      try {
        const result = await runFlowdiff(options);
        analysis = result.output;
        selectedGroupIndex = 0;
        selectedFileIndex = 0;

        groupsProvider.setAnalysis(analysis, options.repoPath);
        infraProvider.setInfrastructure(analysis.infrastructure_group);

        vscode.commands.executeCommand("setContext", "flowdiff.active", true);

        const { summary } = analysis;
        vscode.window.showInformationMessage(
          `flowdiff: ${summary.total_files_changed} files in ${summary.total_groups} groups ` +
            `(${summary.languages_detected.join(", ")})`
        );

        // Auto-open first file
        if (analysis.groups.length > 0) {
          openCurrentFile();
        }
      } catch (err: unknown) {
        const msg = err instanceof Error ? err.message : String(err);
        vscode.window.showErrorMessage(`flowdiff: ${msg}`);
      }
    }
  );
}

function openCurrentFile(): void {
  const groups = sortedGroups();
  if (groups.length === 0) {
    return;
  }
  const group = groups[selectedGroupIndex];
  const sortedFiles = group.files.slice().sort((a, b) => a.flow_position - b.flow_position);
  if (sortedFiles.length === 0) {
    return;
  }
  const file = sortedFiles[selectedFileIndex];
  vscode.commands.executeCommand("flowdiff.openDiff", repoPath, file.path, group.id);
}

function showCurrentGroupAnnotations(): void {
  const groups = sortedGroups();
  if (groups.length === 0 || !analysis) {
    return;
  }
  annotationsPanel.showGroup(groups[selectedGroupIndex], analysis);
}
