import * as vscode from "vscode";
import type { FlowGroup, AnalysisOutput, Pass1Response, Pass2Response } from "./types";

export class AnnotationsPanel {
  private panel: vscode.WebviewPanel | null = null;
  private currentGroup: FlowGroup | null = null;
  private pass1: Pass1Response | null = null;
  private pass2Map: Map<string, Pass2Response> = new Map();

  constructor(_extensionUri: vscode.Uri) {}

  setPass1(response: Pass1Response): void {
    this.pass1 = response;
    this.updateContent();
  }

  setPass2(groupId: string, response: Pass2Response): void {
    this.pass2Map.set(groupId, response);
    if (this.currentGroup?.id === groupId) {
      this.updateContent();
    }
  }

  showGroup(group: FlowGroup, _analysis: AnalysisOutput): void {
    this.currentGroup = group;

    if (!this.panel) {
      this.panel = vscode.window.createWebviewPanel(
        "flowdiff.annotations",
        "flowdiff: Annotations",
        vscode.ViewColumn.Beside,
        {
          enableScripts: false,
          retainContextWhenHidden: true,
        }
      );
      this.panel.onDidDispose(() => {
        this.panel = null;
      });
    }

    this.updateContent();
    this.panel.reveal(vscode.ViewColumn.Beside, true);
  }

  dispose(): void {
    this.panel?.dispose();
    this.panel = null;
  }

  private updateContent(): void {
    if (!this.panel || !this.currentGroup) {
      return;
    }
    this.panel.webview.html = this.buildHtml(this.currentGroup);
  }

  private buildHtml(group: FlowGroup): string {
    const pass1Group = this.pass1?.groups.find((g) => g.id === group.id);
    const pass2 = this.pass2Map.get(group.id);

    return /* html */ `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline';">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <style>
    body {
      font-family: var(--vscode-font-family);
      color: var(--vscode-foreground);
      background: var(--vscode-editor-background);
      padding: 16px;
      line-height: 1.5;
    }
    h1 { font-size: 1.3em; margin: 0 0 8px; }
    h2 { font-size: 1.1em; margin: 16px 0 8px; border-bottom: 1px solid var(--vscode-panel-border); padding-bottom: 4px; }
    .meta { color: var(--vscode-descriptionForeground); font-size: 0.9em; margin-bottom: 12px; }
    .risk-badge {
      display: inline-block;
      padding: 2px 8px;
      border-radius: 4px;
      font-size: 0.85em;
      font-weight: bold;
    }
    .risk-high { background: var(--vscode-inputValidation-errorBackground); color: var(--vscode-inputValidation-errorForeground); }
    .risk-medium { background: var(--vscode-inputValidation-warningBackground); color: var(--vscode-inputValidation-warningForeground); }
    .risk-low { background: var(--vscode-inputValidation-infoBackground); color: var(--vscode-inputValidation-infoForeground); }
    .flag { display: inline-block; margin: 2px 4px 2px 0; padding: 1px 6px; border-radius: 3px; font-size: 0.8em; background: var(--vscode-badge-background); color: var(--vscode-badge-foreground); }
    .edge { font-family: var(--vscode-editor-font-family); font-size: 0.85em; color: var(--vscode-descriptionForeground); }
    .file-annotation { margin: 8px 0; padding: 8px; border-left: 3px solid var(--vscode-textLink-foreground); background: var(--vscode-editor-inactiveSelectionBackground); }
    .file-annotation h3 { margin: 0 0 4px; font-size: 0.95em; }
    ul { margin: 4px 0; padding-left: 20px; }
    li { margin: 2px 0; }
    .narrative { font-style: italic; margin: 8px 0; padding: 8px; background: var(--vscode-textBlockQuote-background); border-left: 3px solid var(--vscode-textBlockQuote-border); }
    pre.mermaid { white-space: pre-wrap; font-size: 0.85em; background: var(--vscode-textCodeBlock-background); padding: 12px; border-radius: 4px; overflow-x: auto; }
  </style>
</head>
<body>
  <h1>${escapeHtml(group.name)}</h1>
  <div class="meta">
    ${riskBadge(group.risk_score)}
    Review order: #${group.review_order}
    &middot; ${group.files.length} file${group.files.length !== 1 ? "s" : ""}
    ${group.entrypoint ? `&middot; Entry: <code>${escapeHtml(group.entrypoint.symbol)}</code> (${group.entrypoint.entrypoint_type})` : ""}
  </div>

  ${pass1Group ? renderPass1(pass1Group) : ""}

  ${pass2 ? renderPass2(pass2) : ""}

  <h2>Edges</h2>
  ${
    group.edges.length > 0
      ? group.edges
          .map(
            (e) =>
              `<div class="edge">${escapeHtml(e.from)} <strong>${e.edge_type}</strong> ${escapeHtml(e.to)}</div>`
          )
          .join("\n  ")
      : "<p>No edges in this group.</p>"
  }

  <h2>Mermaid Flow Graph</h2>
  <pre class="mermaid">${escapeHtml(buildMermaid(group))}</pre>
</body>
</html>`;
  }
}

// ── HTML helpers ────────────────────────────────────────────────────

function escapeHtml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function riskBadge(score: number): string {
  if (score >= 0.7) {
    return `<span class="risk-badge risk-high">HIGH ${score.toFixed(2)}</span>`;
  }
  if (score >= 0.4) {
    return `<span class="risk-badge risk-medium">MEDIUM ${score.toFixed(2)}</span>`;
  }
  return `<span class="risk-badge risk-low">LOW ${score.toFixed(2)}</span>`;
}

function renderPass1(annotation: { summary: string; risk_flags: string[]; review_order_rationale: string }): string {
  return `
  <h2>LLM Summary</h2>
  <p>${escapeHtml(annotation.summary)}</p>
  ${annotation.risk_flags.length > 0 ? `<div>${annotation.risk_flags.map((f) => `<span class="flag">${escapeHtml(f)}</span>`).join("")}</div>` : ""}
  <p><em>${escapeHtml(annotation.review_order_rationale)}</em></p>`;
}

function renderPass2(pass2: Pass2Response): string {
  let html = `
  <h2>Deep Analysis</h2>
  <div class="narrative">${escapeHtml(pass2.flow_narrative)}</div>`;

  for (const fa of pass2.file_annotations) {
    html += `
  <div class="file-annotation">
    <h3>${escapeHtml(fa.file)}</h3>
    <p><strong>Role:</strong> ${escapeHtml(fa.role_in_flow)}</p>
    <p>${escapeHtml(fa.changes_summary)}</p>
    ${fa.risks.length > 0 ? `<p><strong>Risks:</strong></p><ul>${fa.risks.map((r) => `<li>${escapeHtml(r)}</li>`).join("")}</ul>` : ""}
    ${fa.suggestions.length > 0 ? `<p><strong>Suggestions:</strong></p><ul>${fa.suggestions.map((s) => `<li>${escapeHtml(s)}</li>`).join("")}</ul>` : ""}
  </div>`;
  }

  if (pass2.cross_cutting_concerns.length > 0) {
    html += `
  <h2>Cross-cutting Concerns</h2>
  <ul>${pass2.cross_cutting_concerns.map((c) => `<li>${escapeHtml(c)}</li>`).join("")}</ul>`;
  }

  return html;
}

/** Escape a string for use inside a Mermaid node label (within double quotes). */
function escapeMermaidLabel(s: string): string {
  return s.replace(/"/g, "#quot;").replace(/</g, "#lt;").replace(/>/g, "#gt;");
}

function buildMermaid(group: FlowGroup): string {
  if (group.edges.length === 0) {
    return "graph TD\n  A[No edges]";
  }

  const lines = ["graph TD"];
  const nodeIds = new Map<string, string>();
  let counter = 0;

  function getNodeId(label: string): string {
    if (!nodeIds.has(label)) {
      nodeIds.set(label, `N${counter++}`);
    }
    return nodeIds.get(label)!;
  }

  for (const edge of group.edges) {
    const fromId = getNodeId(edge.from);
    const toId = getNodeId(edge.to);
    const fromLabel = escapeMermaidLabel(edge.from.split("::").pop() ?? edge.from);
    const toLabel = escapeMermaidLabel(edge.to.split("::").pop() ?? edge.to);
    lines.push(`  ${fromId}["${fromLabel}"] -->|${edge.edge_type}| ${toId}["${toLabel}"]`);
  }

  return lines.join("\n");
}
