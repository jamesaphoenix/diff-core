import { useState, useCallback, useEffect, useRef } from "react";
import type { AnalysisOutput, FlowGroup, FileDiffContent } from "./types";
import DiffViewer from "./components/DiffViewer";
import MermaidGraph from "./components/MermaidGraph";
import { MOCK_ANALYSIS, MOCK_MERMAID, MOCK_DIFFS } from "./mock";

/** Detect if running inside Tauri (vs plain browser for demo/testing). */
const IS_TAURI = typeof window !== "undefined" && "__TAURI__" in window;

/** Lazy-import Tauri invoke only when in Tauri context. */
async function tauriInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

/** Three-panel layout: flow groups | diff viewer | annotations */
export default function App() {
  const [analysis, setAnalysis] = useState<AnalysisOutput | null>(null);
  const [selectedGroup, setSelectedGroup] = useState<FlowGroup | null>(null);
  const [selectedFile, setSelectedFile] = useState<string | null>(null);
  const [fileDiff, setFileDiff] = useState<FileDiffContent | null>(null);
  const [mermaid, setMermaid] = useState<string>("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Repo path — in a real app this would come from a dialog or CLI arg
  const [repoPath, setRepoPath] = useState(IS_TAURI ? "" : "/demo/repo");
  const [baseRef, setBaseRef] = useState("main");

  // Demo mode: auto-load mock data on mount when not in Tauri
  const demoLoaded = useRef(false);

  // Refs for keyboard nav to access latest state without re-registering listener
  const selectedGroupRef = useRef(selectedGroup);
  const selectedFileRef = useRef(selectedFile);
  const sortedGroupsRef = useRef<FlowGroup[]>([]);
  selectedGroupRef.current = selectedGroup;
  selectedFileRef.current = selectedFile;

  const handleSelectGroup = useCallback(
    async (group: FlowGroup) => {
      setSelectedGroup(group);
      // Load mermaid
      if (IS_TAURI) {
        try {
          const diagram = await tauriInvoke<string>("get_mermaid", {
            groupId: group.id,
          });
          setMermaid(diagram);
        } catch {
          setMermaid("");
        }
      } else {
        setMermaid(MOCK_MERMAID[group.id] || "");
      }
      // Auto-select first file in group
      if (group.files.length > 0) {
        handleSelectFile(group.files[0].path);
      } else {
        setSelectedFile(null);
        setFileDiff(null);
      }
    },
    [repoPath, baseRef],
  );

  const handleSelectFile = useCallback(
    async (path: string) => {
      setSelectedFile(path);
      if (IS_TAURI) {
        if (!repoPath) return;
        try {
          const diff = await tauriInvoke<FileDiffContent>("get_file_diff", {
            repoPath,
            filePath: path,
            base: baseRef || "main",
            head: null,
            range: null,
            staged: false,
            unstaged: false,
          });
          setFileDiff(diff);
        } catch {
          setFileDiff(null);
        }
      } else {
        setFileDiff(MOCK_DIFFS[path] || null);
      }
    },
    [repoPath, baseRef],
  );

  const runAnalysis = useCallback(async () => {
    if (!repoPath) return;
    setLoading(true);
    setError(null);
    try {
      let result: AnalysisOutput;
      if (IS_TAURI) {
        result = await tauriInvoke<AnalysisOutput>("analyze", {
          repoPath,
          base: baseRef || "main",
          head: null,
          range: null,
          staged: false,
          unstaged: false,
        });
      } else {
        // Demo mode: simulate short delay then return mock data
        await new Promise((r) => setTimeout(r, 400));
        result = MOCK_ANALYSIS;
      }
      setAnalysis(result);
      // Auto-select first group
      if (result.groups.length > 0) {
        const sorted = [...result.groups].sort(
          (a, b) => a.review_order - b.review_order,
        );
        handleSelectGroup(sorted[0]);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [repoPath, baseRef, handleSelectGroup]);

  // Auto-load demo data when not in Tauri
  useEffect(() => {
    if (!IS_TAURI && !demoLoaded.current) {
      demoLoaded.current = true;
      runAnalysis();
    }
  }, [runAnalysis]);

  // Sort groups by review_order
  const sortedGroups = analysis
    ? [...analysis.groups].sort((a, b) => a.review_order - b.review_order)
    : [];
  sortedGroupsRef.current = sortedGroups;

  // Keyboard navigation: j/k = next/prev file, J/K = next/prev group
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      // Skip if user is typing in an input or the Monaco editor is focused
      const target = e.target as HTMLElement;
      if (
        target.tagName === "INPUT" ||
        target.tagName === "TEXTAREA" ||
        target.closest(".monaco-editor")
      ) {
        return;
      }

      const groups = sortedGroupsRef.current;
      const group = selectedGroupRef.current;
      const file = selectedFileRef.current;

      if (groups.length === 0 || !group) return;

      const groupIdx = groups.findIndex((g) => g.id === group.id);
      if (groupIdx === -1) return;

      if (e.key === "j") {
        // Next file in current group
        e.preventDefault();
        const fileIdx = group.files.findIndex((f) => f.path === file);
        if (fileIdx < group.files.length - 1) {
          handleSelectFile(group.files[fileIdx + 1].path);
        }
      } else if (e.key === "k") {
        // Previous file in current group
        e.preventDefault();
        const fileIdx = group.files.findIndex((f) => f.path === file);
        if (fileIdx > 0) {
          handleSelectFile(group.files[fileIdx - 1].path);
        }
      } else if (e.key === "J") {
        // Next group
        e.preventDefault();
        if (groupIdx < groups.length - 1) {
          handleSelectGroup(groups[groupIdx + 1]);
        }
      } else if (e.key === "K") {
        // Previous group
        e.preventDefault();
        if (groupIdx > 0) {
          handleSelectGroup(groups[groupIdx - 1]);
        }
      }
    }

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [handleSelectFile, handleSelectGroup]);

  return (
    <div className="app">
      {/* Top bar */}
      <header className="top-bar">
        <div className="top-bar-left">
          <span className="logo">flowdiff</span>
        </div>
        <div className="top-bar-center">
          <input
            className="input repo-input"
            type="text"
            placeholder="Repository path..."
            value={repoPath}
            onChange={(e) => setRepoPath(e.target.value)}
          />
          <input
            className="input base-input"
            type="text"
            placeholder="Base ref (main)"
            value={baseRef}
            onChange={(e) => setBaseRef(e.target.value)}
          />
          <button
            className="btn btn-primary"
            onClick={runAnalysis}
            disabled={loading || !repoPath}
          >
            {loading ? "Analyzing..." : "Analyze"}
          </button>
        </div>
        <div className="top-bar-right">
          {analysis && (
            <span className="summary">
              {analysis.summary.total_files_changed} files,{" "}
              {analysis.summary.total_groups} groups
            </span>
          )}
        </div>
      </header>

      {/* Error display */}
      {error && (
        <div className="error-bar">
          <span>{error}</span>
          <button className="btn-close" onClick={() => setError(null)}>
            &times;
          </button>
        </div>
      )}

      {/* Three-panel layout */}
      <div className="panels">
        {/* Left panel: Flow Groups */}
        <aside className="panel panel-left">
          <div className="panel-header">Flow Groups</div>
          <div className="panel-body">
            {sortedGroups.map((group) => (
              <div
                key={group.id}
                className={`group-item ${selectedGroup?.id === group.id ? "selected" : ""}`}
                onClick={() => handleSelectGroup(group)}
              >
                <div className="group-header">
                  <span className="group-name">{group.name}</span>
                  <span className="risk-badge" data-risk={riskLevel(group.risk_score)}>
                    {group.risk_score.toFixed(2)}
                  </span>
                </div>
                {selectedGroup?.id === group.id && (
                  <ul className="file-list">
                    {group.files.map((file) => (
                      <li
                        key={file.path}
                        className={`file-item ${selectedFile === file.path ? "selected" : ""}`}
                        onClick={(e) => {
                          e.stopPropagation();
                          handleSelectFile(file.path);
                        }}
                      >
                        <span className="file-role">{file.role}</span>
                        <span className="file-path">{shortPath(file.path)}</span>
                        <span className="file-changes">
                          +{file.changes.additions} -{file.changes.deletions}
                        </span>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            ))}
            {/* Infrastructure group */}
            {analysis?.infrastructure_group && (
              <div className="group-item infra-group">
                <div className="group-header">
                  <span className="group-name">Infrastructure</span>
                </div>
                <ul className="file-list">
                  {analysis.infrastructure_group.files.map((f) => (
                    <li key={f} className="file-item">
                      <span className="file-path">{shortPath(f)}</span>
                    </li>
                  ))}
                </ul>
              </div>
            )}
            {!analysis && !loading && (
              <div className="empty-state">
                Enter a repository path and click Analyze to start.
              </div>
            )}
          </div>
        </aside>

        {/* Center panel: Monaco Diff Viewer */}
        <main className="panel panel-center">
          <div className="panel-header">
            {fileDiff ? fileDiff.path : "Diff Viewer"}
          </div>
          <div className="panel-body diff-viewer">
            <DiffViewer fileDiff={fileDiff} />
          </div>
        </main>

        {/* Right panel: Annotations & Graph */}
        <aside className="panel panel-right">
          <div className="panel-header">Annotations</div>
          <div className="panel-body">
            {selectedGroup && (
              <>
                <div className="annotation-section">
                  <h3>Flow Group</h3>
                  <p className="group-detail-name">{selectedGroup.name}</p>
                  {selectedGroup.entrypoint && (
                    <p className="entrypoint-info">
                      Entrypoint: {selectedGroup.entrypoint.symbol} (
                      {selectedGroup.entrypoint.entrypoint_type})
                    </p>
                  )}
                  <p>
                    Risk: <strong>{selectedGroup.risk_score.toFixed(2)}</strong>{" "}
                    | Files: <strong>{selectedGroup.files.length}</strong> |
                    Review order: <strong>#{selectedGroup.review_order}</strong>
                  </p>
                </div>
                {mermaid && (
                  <div className="annotation-section">
                    <h3>Flow Graph</h3>
                    <MermaidGraph code={mermaid} />
                  </div>
                )}
                {selectedGroup.edges.length > 0 && (
                  <div className="annotation-section">
                    <h3>Edges</h3>
                    <ul className="edge-list">
                      {selectedGroup.edges.map((edge, i) => (
                        <li key={i} className="edge-item">
                          <span className="edge-type">{edge.edge_type}</span>
                          <span className="edge-from">{shortSymbol(edge.from)}</span>
                          <span className="edge-arrow">&rarr;</span>
                          <span className="edge-to">{shortSymbol(edge.to)}</span>
                        </li>
                      ))}
                    </ul>
                  </div>
                )}
              </>
            )}
            {!selectedGroup && (
              <div className="empty-state">
                Select a group to see annotations.
              </div>
            )}
          </div>
        </aside>
      </div>

      {/* Keyboard shortcuts bar */}
      {analysis && (
        <footer className="keyboard-hints">
          <span><kbd>j</kbd> next file</span>
          <span><kbd>k</kbd> prev file</span>
          <span><kbd>J</kbd> next group</span>
          <span><kbd>K</kbd> prev group</span>
        </footer>
      )}
    </div>
  );
}

function riskLevel(score: number): string {
  if (score >= 0.7) return "high";
  if (score >= 0.4) return "medium";
  return "low";
}

function shortPath(path: string): string {
  const parts = path.split("/");
  if (parts.length <= 2) return path;
  return parts.slice(-2).join("/");
}

function shortSymbol(symbol: string): string {
  const parts = symbol.split("::");
  if (parts.length <= 1) return symbol;
  return parts[parts.length - 1];
}
