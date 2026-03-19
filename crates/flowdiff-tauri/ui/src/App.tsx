import { useState, useCallback, useEffect, useRef } from "react";
import type {
  AnalysisOutput,
  FlowGroup,
  FileDiffContent,
  Pass1Response,
  Pass1GroupAnnotation,
  Pass2Response,
  RepoInfo,
  BranchInfo,
} from "./types";
import DiffViewer from "./components/DiffViewer";
import FlowGraph from "./components/FlowGraph";
import { MOCK_ANALYSIS, MOCK_DIFFS, MOCK_PASS1, MOCK_PASS2, MOCK_REPO_INFO } from "./mock";

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
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // LLM annotation state
  const [overview, setOverview] = useState<Pass1Response | null>(null);
  const [deepAnalyses, setDeepAnalyses] = useState<Record<string, Pass2Response>>({});
  const [annotating, setAnnotating] = useState(false);
  const [deepAnalyzing, setDeepAnalyzing] = useState(false);

  // Repo and git state
  const [repoPath, setRepoPath] = useState(IS_TAURI ? "" : "/demo/repo");
  const [baseRef, setBaseRef] = useState("main");
  const [repoInfo, setRepoInfo] = useState<RepoInfo | null>(null);
  const [branchDropdownOpen, setBranchDropdownOpen] = useState(false);

  // Demo mode: auto-load mock data on mount when not in Tauri
  const demoLoaded = useRef(false);

  // Refs for keyboard nav to access latest state without re-registering listener
  const selectedGroupRef = useRef(selectedGroup);
  const selectedFileRef = useRef(selectedFile);
  const sortedGroupsRef = useRef<FlowGroup[]>([]);
  selectedGroupRef.current = selectedGroup;
  selectedFileRef.current = selectedFile;

  /** Fetch repository info (branches, worktrees, status). */
  const loadRepoInfo = useCallback(async (path: string) => {
    if (!path) return;
    try {
      let info: RepoInfo;
      if (IS_TAURI) {
        info = await tauriInvoke<RepoInfo>("get_repo_info", { repoPath: path });
      } else {
        await new Promise((r) => setTimeout(r, 100));
        info = MOCK_REPO_INFO;
      }
      setRepoInfo(info);
      // Auto-set base ref to the detected default branch
      setBaseRef(info.default_branch);
    } catch {
      // Non-fatal: we can still analyze without repo info
      setRepoInfo(null);
    }
  }, []);

  // Load repo info when repo path changes
  useEffect(() => {
    if (repoPath) {
      loadRepoInfo(repoPath);
    } else {
      setRepoInfo(null);
    }
  }, [repoPath, loadRepoInfo]);

  const handleSelectGroup = useCallback(
    async (group: FlowGroup) => {
      setSelectedGroup(group);
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
    // Reset LLM state on new analysis
    setOverview(null);
    setDeepAnalyses({});
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
          prPreview: true, // Default to PR preview mode (merge-base diff)
        });
      } else {
        // Demo mode: simulate short delay then return mock data
        await new Promise((r) => setTimeout(r, 400));
        result = MOCK_ANALYSIS;
      }
      setAnalysis(result);
      // If analysis came with annotations already (e.g., from --annotate), load them
      if (result.annotations) {
        setOverview(result.annotations);
      }
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

  /** Run LLM Pass 1: overview annotation for all groups. */
  const runAnnotateOverview = useCallback(async () => {
    setAnnotating(true);
    setError(null);
    try {
      let result: Pass1Response;
      if (IS_TAURI) {
        result = await tauriInvoke<Pass1Response>("annotate_overview");
      } else {
        await new Promise((r) => setTimeout(r, 800));
        result = MOCK_PASS1;
      }
      setOverview(result);
    } catch (e) {
      setError(`Annotation failed: ${String(e)}`);
    } finally {
      setAnnotating(false);
    }
  }, []);

  /** Run LLM Pass 2: deep analysis for the selected group. */
  const runDeepAnalysis = useCallback(async () => {
    if (!selectedGroup) return;
    setDeepAnalyzing(true);
    setError(null);
    try {
      let result: Pass2Response;
      if (IS_TAURI) {
        result = await tauriInvoke<Pass2Response>("annotate_group", {
          groupId: selectedGroup.id,
          repoPath,
          base: baseRef || "main",
          head: null,
          range: null,
          staged: false,
          unstaged: false,
        });
      } else {
        await new Promise((r) => setTimeout(r, 600));
        result = MOCK_PASS2[selectedGroup.id] || {
          group_id: selectedGroup.id,
          flow_narrative: "No deep analysis available for this group in demo mode.",
          file_annotations: [],
          cross_cutting_concerns: [],
        };
      }
      setDeepAnalyses((prev) => ({ ...prev, [selectedGroup.id]: result }));
    } catch (e) {
      setError(`Deep analysis failed: ${String(e)}`);
    } finally {
      setDeepAnalyzing(false);
    }
  }, [selectedGroup, repoPath, baseRef]);

  // Auto-load demo data when not in Tauri
  useEffect(() => {
    if (!IS_TAURI && !demoLoaded.current) {
      demoLoaded.current = true;
      runAnalysis();
    }
  }, [runAnalysis]);

  // Close branch dropdown when clicking outside
  useEffect(() => {
    if (!branchDropdownOpen) return;
    function handleClick(e: MouseEvent) {
      const target = e.target as HTMLElement;
      if (!target.closest(".branch-dropdown-wrapper")) {
        setBranchDropdownOpen(false);
      }
    }
    window.addEventListener("click", handleClick);
    return () => window.removeEventListener("click", handleClick);
  }, [branchDropdownOpen]);

  // Sort groups by review_order
  const sortedGroups = analysis
    ? [...analysis.groups].sort((a, b) => a.review_order - b.review_order)
    : [];
  sortedGroupsRef.current = sortedGroups;

  // Get the Pass 1 annotation for the currently selected group
  const groupAnnotation: Pass1GroupAnnotation | undefined = overview?.groups.find(
    (g) => g.id === selectedGroup?.id,
  );

  // Get the Pass 2 deep analysis for the currently selected group
  const groupDeepAnalysis: Pass2Response | undefined = selectedGroup
    ? deepAnalyses[selectedGroup.id]
    : undefined;

  // Keyboard navigation: j/k = next/prev file, J/K = next/prev group, a = annotate
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      // Skip if user is typing in an input or the Monaco editor is focused
      const target = e.target as HTMLElement;
      if (
        target.tagName === "INPUT" ||
        target.tagName === "TEXTAREA" ||
        target.tagName === "SELECT" ||
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

  const handleSelectBase = useCallback((branch: string) => {
    setBaseRef(branch);
    setBranchDropdownOpen(false);
  }, []);

  // Derived: non-current branches for the base branch dropdown
  const baseBranches: BranchInfo[] = repoInfo?.branches ?? [];

  // Status display
  const statusText = formatBranchStatus(repoInfo);

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

          {/* Base branch dropdown (replaces text input) */}
          <div className="branch-dropdown-wrapper">
            <button
              className="btn branch-dropdown-trigger"
              onClick={() => setBranchDropdownOpen(!branchDropdownOpen)}
              title="Select base branch for comparison"
            >
              <span className="branch-icon">&#9741;</span>
              <span className="branch-name">{baseRef}</span>
              <span className="dropdown-arrow">&#9662;</span>
            </button>
            {branchDropdownOpen && (
              <ul className="branch-dropdown">
                {baseBranches.map((b) => (
                  <li
                    key={b.name}
                    className={`branch-option ${b.name === baseRef ? "selected" : ""} ${b.is_current ? "current" : ""}`}
                    onClick={() => handleSelectBase(b.name)}
                  >
                    <span className="branch-option-name">{b.name}</span>
                    {b.is_current && <span className="branch-current-badge">current</span>}
                    {b.has_upstream && <span className="branch-upstream-badge">tracked</span>}
                  </li>
                ))}
                {baseBranches.length === 0 && (
                  <li className="branch-option disabled">No branches found</li>
                )}
              </ul>
            )}
          </div>

          <button
            className="btn btn-primary"
            onClick={runAnalysis}
            disabled={loading || !repoPath}
          >
            {loading ? "Analyzing..." : "Analyze"}
          </button>
        </div>
        <div className="top-bar-right">
          {/* Branch status indicator */}
          {repoInfo && (
            <div className="repo-status">
              {repoInfo.current_branch && (
                <span className="current-branch" title="Current branch">
                  <span className="branch-icon">&#9741;</span>
                  {repoInfo.current_branch}
                </span>
              )}
              {statusText && (
                <span className="push-status" title="Tracking status">
                  {statusText}
                </span>
              )}
              {/* Worktree indicator */}
              {repoInfo.worktrees.length > 1 && (
                <span className="worktree-badge" title={`${repoInfo.worktrees.length} worktrees`}>
                  {repoInfo.worktrees.length} worktrees
                </span>
              )}
            </div>
          )}
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
                {/* Deterministic group info */}
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

                {/* LLM Overview (Pass 1) — Overall summary shown once */}
                {overview && !groupAnnotation && (
                  <div className="annotation-section llm-section">
                    <h3>LLM Overview</h3>
                    <p className="llm-summary">{overview.overall_summary}</p>
                  </div>
                )}

                {/* LLM Group Annotation (Pass 1) */}
                {groupAnnotation && (
                  <div className="annotation-section llm-section">
                    <h3>LLM Summary</h3>
                    <p className="llm-summary">{groupAnnotation.summary}</p>
                    <p className="llm-rationale">
                      <strong>Review rationale:</strong> {groupAnnotation.review_order_rationale}
                    </p>
                    {groupAnnotation.risk_flags.length > 0 && (
                      <div className="risk-flags">
                        {groupAnnotation.risk_flags.map((flag, i) => (
                          <span key={i} className="risk-flag">{flag}</span>
                        ))}
                      </div>
                    )}
                  </div>
                )}

                {/* Overall summary (shown when overview exists and group annotation exists) */}
                {overview && groupAnnotation && (
                  <div className="annotation-section llm-section">
                    <h3>Overall Summary</h3>
                    <p className="llm-summary">{overview.overall_summary}</p>
                  </div>
                )}

                {/* LLM Deep Analysis (Pass 2) */}
                {groupDeepAnalysis && (
                  <>
                    <div className="annotation-section llm-section">
                      <h3>Flow Narrative</h3>
                      <p className="llm-narrative">{groupDeepAnalysis.flow_narrative}</p>
                    </div>

                    {groupDeepAnalysis.file_annotations.length > 0 && (
                      <div className="annotation-section llm-section">
                        <h3>File Annotations</h3>
                        {groupDeepAnalysis.file_annotations.map((fa, i) => (
                          <div key={i} className="file-annotation">
                            <div className="file-annotation-header">
                              <span className="file-annotation-path">{shortPath(fa.file)}</span>
                              <span className="file-annotation-role">{fa.role_in_flow}</span>
                            </div>
                            <p className="file-annotation-changes">{fa.changes_summary}</p>
                            {fa.risks.length > 0 && (
                              <div className="file-annotation-list">
                                <span className="annotation-label risk-label">Risks:</span>
                                <ul>
                                  {fa.risks.map((r, j) => (
                                    <li key={j}>{r}</li>
                                  ))}
                                </ul>
                              </div>
                            )}
                            {fa.suggestions.length > 0 && (
                              <div className="file-annotation-list">
                                <span className="annotation-label suggestion-label">Suggestions:</span>
                                <ul>
                                  {fa.suggestions.map((s, j) => (
                                    <li key={j}>{s}</li>
                                  ))}
                                </ul>
                              </div>
                            )}
                          </div>
                        ))}
                      </div>
                    )}

                    {groupDeepAnalysis.cross_cutting_concerns.length > 0 && (
                      <div className="annotation-section llm-section">
                        <h3>Cross-Cutting Concerns</h3>
                        <ul className="concerns-list">
                          {groupDeepAnalysis.cross_cutting_concerns.map((c, i) => (
                            <li key={i}>{c}</li>
                          ))}
                        </ul>
                      </div>
                    )}
                  </>
                )}

                {/* Flow graph (React Flow) */}
                {selectedGroup.edges.length > 0 && (
                  <div className="annotation-section">
                    <h3>Flow Graph</h3>
                    <FlowGraph
                      edges={selectedGroup.edges}
                      files={selectedGroup.files}
                      onNodeClick={handleSelectFile}
                    />
                  </div>
                )}

                {/* Edge list */}
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

                {/* LLM action buttons */}
                <div className="annotation-section annotation-actions">
                  {!overview && (
                    <button
                      className="btn btn-annotate"
                      onClick={runAnnotateOverview}
                      disabled={annotating}
                    >
                      {annotating ? "Annotating..." : "Annotate All Groups"}
                    </button>
                  )}
                  {!groupDeepAnalysis && (
                    <button
                      className="btn btn-deep"
                      onClick={runDeepAnalysis}
                      disabled={deepAnalyzing}
                    >
                      {deepAnalyzing ? "Analyzing..." : "Deep Analyze Group"}
                    </button>
                  )}
                </div>
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

/** Format the branch tracking status into a readable string. */
function formatBranchStatus(repoInfo: RepoInfo | null): string | null {
  if (!repoInfo?.status) return null;
  const { ahead, behind, upstream } = repoInfo.status;
  if (!upstream) return null;
  if (ahead === 0 && behind === 0) return "up to date";
  const parts: string[] = [];
  if (ahead > 0) parts.push(`${ahead} ahead`);
  if (behind > 0) parts.push(`${behind} behind`);
  return parts.join(", ");
}
