import { useState, useCallback, useEffect, useRef, useMemo } from "react";
import type {
  AnalysisOutput,
  FlowGroup,
  FileDiffContent,
  Pass1Response,
  Pass1GroupAnnotation,
  Pass2Response,
  RepoInfo,
  BranchInfo,
  LlmSettings,
  LlmProvider,
  RefinementResult,
  RefinementResponse,
  ReviewComment,
  CommentInput,
} from "./types";
import { LLM_PROVIDERS, MODELS_BY_PROVIDER } from "./types";
import DiffViewer, { type DiffViewerHandle } from "./components/DiffViewer";
import FlowGraph from "./components/FlowGraph";
// RiskHeatmap hidden (Phase 9.4) — component kept for future re-enablement
// import RiskHeatmap from "./components/RiskHeatmap";
import ErrorBoundary from "./components/ErrorBoundary";
import { MOCK_ANALYSIS, MOCK_DIFFS, MOCK_PASS1, MOCK_PASS2, MOCK_REPO_INFO, MOCK_LLM_SETTINGS, MOCK_REFINEMENT } from "./mock";

/** Detect if running inside Tauri (vs plain browser for demo/testing). */
const IS_TAURI = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

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
  // Test-only: when set, the named panel's ErrorBoundary will catch a deliberate crash
  const [crashPanel, setCrashPanel] = useState<string | null>(null);

  // LLM annotation state
  const [overview, setOverview] = useState<Pass1Response | null>(null);
  const [deepAnalyses, setDeepAnalyses] = useState<Record<string, Pass2Response>>({});
  const [annotating, setAnnotating] = useState(false);
  const [deepAnalyzing, setDeepAnalyzing] = useState(false);
  // Counter to track concurrent deep analysis requests — prevents premature loading state clear
  const deepAnalyzingCount = useRef(0);

  // Repo and git state
  const [repoPath, setRepoPath] = useState(IS_TAURI ? "" : "/demo/repo");
  const [baseRef, setBaseRef] = useState("main");
  const [repoInfo, setRepoInfo] = useState<RepoInfo | null>(null);
  const [branchDropdownOpen, setBranchDropdownOpen] = useState(false);

  // LLM API key availability
  const [hasApiKey, setHasApiKey] = useState(!IS_TAURI); // Demo mode always has "key"

  // LLM settings
  const [llmSettings, setLlmSettings] = useState<LlmSettings | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [apiKeyInput, setApiKeyInput] = useState("");

  // Ignore paths state
  const [ignorePaths, setIgnorePaths] = useState<string[]>([]);
  const [ignorePathInput, setIgnorePathInput] = useState("");

  // Flow review tick-off state (session-only)
  const [reviewedGroupIds, setReviewedGroupIds] = useState<Set<string>>(new Set());

  // Flow replay state
  const [replayActive, setReplayActive] = useState(false);
  const [replayStep, setReplayStep] = useState(0);
  const [replayVisited, setReplayVisited] = useState<Set<string>>(new Set());

  // Refinement state
  const [originalGroups, setOriginalGroups] = useState<FlowGroup[] | null>(null);
  const [refinedGroups, setRefinedGroups] = useState<FlowGroup[] | null>(null);
  const [refinementResponse, setRefinementResponse] = useState<RefinementResponse | null>(null);
  const [refinementProvider, setRefinementProvider] = useState<string | null>(null);
  const [refinementModel, setRefinementModel] = useState<string | null>(null);
  const [showRefined, setShowRefined] = useState(false);
  const [refining, setRefining] = useState(false);

  // Context menu state (right-click on file items)
  const [contextMenu, setContextMenu] = useState<{ x: number; y: number; filePath: string } | null>(null);

  // Review comments state
  const [comments, setComments] = useState<ReviewComment[]>([]);
  const [commentInput, setCommentInput] = useState<CommentInput | null>(null);
  const [commentText, setCommentText] = useState("");
  const commentInputRef = useRef<HTMLTextAreaElement>(null);
  const diffViewerRef = useRef<DiffViewerHandle>(null);
  const repoInputRef = useRef<HTMLInputElement>(null);

  // Flow graph collapse state (collapses when node is clicked in graph)
  const [graphCollapsed, setGraphCollapsed] = useState(false);
  const [edgesCollapsed, setEdgesCollapsed] = useState(true);
  const [commentsCollapsed, setCommentsCollapsed] = useState(false);
  const [activeCommentId, setActiveCommentId] = useState<string | null>(null);

  // Infrastructure group collapse state
  const [infraExpanded, setInfraExpanded] = useState(false);
  const [infraShowAll, setInfraShowAll] = useState(false);

  // Toast notification state (auto-dismiss)
  const [toast, setToast] = useState<string | null>(null);
  const toastTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Demo mode: auto-load mock data on mount when not in Tauri
  const demoLoaded = useRef(false);

  // Refs for keyboard nav to access latest state without re-registering listener
  const selectedGroupRef = useRef(selectedGroup);
  const selectedFileRef = useRef(selectedFile);
  const sortedGroupsRef = useRef<FlowGroup[]>([]);
  const replayActiveRef = useRef(replayActive);
  const replayStepRef = useRef(replayStep);
  selectedGroupRef.current = selectedGroup;
  selectedFileRef.current = selectedFile;
  replayActiveRef.current = replayActive;
  replayStepRef.current = replayStep;

  // Debounce refs for keyboard file navigation
  const pendingFileNav = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Staleness guard: incremented on each file selection, stale responses are discarded
  const fileDiffGeneration = useRef(0);

  /** Load LLM settings from backend. */
  const loadLlmSettings = useCallback(async (path: string | null) => {
    try {
      let settings: LlmSettings;
      if (IS_TAURI) {
        settings = await tauriInvoke<LlmSettings>("get_llm_settings", {
          repoPath: path,
        });
      } else {
        settings = MOCK_LLM_SETTINGS;
      }
      setLlmSettings(settings);
      setHasApiKey(settings.has_api_key);
    } catch {
      // Non-fatal
    }
  }, []);

  /** Save LLM settings to backend. */
  const saveLlmSettings = useCallback(async (settings: LlmSettings) => {
    setLlmSettings(settings);
    setHasApiKey(settings.has_api_key);
    if (IS_TAURI && repoPath) {
      try {
        await tauriInvoke("save_llm_settings", {
          repoPath,
          settings,
        });
        // Re-check API key availability after save
        const updated = await tauriInvoke<LlmSettings>("get_llm_settings", {
          repoPath,
        });
        setLlmSettings(updated);
        setHasApiKey(updated.has_api_key);
      } catch {
        // Non-fatal: settings are still applied in-memory
      }
    }
  }, [repoPath]);

  /** Load ignore paths from .flowdiff.toml. */
  const loadIgnorePaths = useCallback(async (path: string | null) => {
    if (!IS_TAURI || !path) return;
    try {
      const paths = await tauriInvoke<string[]>("get_ignore_paths", { repoPath: path });
      setIgnorePaths(paths);
    } catch {
      // Non-fatal: ignore paths just won't be shown
    }
  }, []);

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
    // Load LLM settings (includes API key check)
    loadLlmSettings(path);
    // Load ignore paths
    loadIgnorePaths(path);
  }, [loadLlmSettings, loadIgnorePaths]);

  // Load repo info when repo path changes
  useEffect(() => {
    if (repoPath) {
      loadRepoInfo(repoPath);
    } else {
      setRepoInfo(null);
    }
  }, [repoPath, loadRepoInfo]);

  const handleSelectFile = useCallback(
    async (path: string) => {
      setSelectedFile(path);
      // Increment generation to mark any in-flight request as stale
      const generation = ++fileDiffGeneration.current;
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
          // Only apply if this is still the latest request
          if (generation === fileDiffGeneration.current) {
            setFileDiff(diff);
          }
        } catch (e) {
          if (generation === fileDiffGeneration.current) {
            setFileDiff(null);
            setError(`Failed to load diff for ${path}: ${String(e)}`);
          }
        }
      } else {
        if (generation === fileDiffGeneration.current) {
          setFileDiff(MOCK_DIFFS[path] || null);
        }
      }
    },
    [repoPath, baseRef],
  );

  /** Debounced file selection for keyboard navigation — updates highlight immediately,
   *  delays the expensive diff fetch by 150ms so rapid j/k presses only fetch the final file. */
  const handleSelectFileDebounced = useCallback(
    (path: string) => {
      // Immediately update highlight for visual feedback
      setSelectedFile(path);
      // Debounce the expensive diff fetch
      if (pendingFileNav.current) clearTimeout(pendingFileNav.current);
      pendingFileNav.current = setTimeout(() => {
        handleSelectFile(path);
      }, 150);
    },
    [handleSelectFile],
  );

  /** Called when a node in the React Flow graph is clicked — opens the file without collapsing the graph. */
  const handleGraphNodeClick = useCallback(
    (path: string) => {
      handleSelectFile(path);
    },
    [handleSelectFile],
  );

  const handleSelectGroup = useCallback(
    async (group: FlowGroup) => {
      // Cancel any pending debounced file nav from the previous group
      if (pendingFileNav.current) clearTimeout(pendingFileNav.current);
      setSelectedGroup(group);
      // Exit replay mode when switching groups
      setReplayActive(false);
      setReplayStep(0);
      setReplayVisited(new Set());
      // Reset graph collapse state when switching groups
      setGraphCollapsed(false);
      // Auto-select first file in group
      if (group.files.length > 0) {
        handleSelectFile(group.files[0].path);
      } else {
        setSelectedFile(null);
        setFileDiff(null);
      }
    },
    [handleSelectFile],
  );

  const runAnalysis = useCallback(async () => {
    if (!repoPath) return;
    setLoading(true);
    setError(null);
    // Reset LLM state on new analysis
    setOverview(null);
    setDeepAnalyses({});
    // Reset refinement state
    setOriginalGroups(null);
    setRefinedGroups(null);
    setRefinementResponse(null);
    setRefinementProvider(null);
    setRefinementModel(null);
    setShowRefined(false);
    // Reset review tick-off state
    setReviewedGroupIds(new Set());
    // Reset infrastructure group state
    setInfraExpanded(false);
    setInfraShowAll(false);
    // Reset comments
    setComments([]);
    setCommentInput(null);
    setCommentText("");
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
      // Re-focus the repo input so user can fix the path
      repoInputRef.current?.focus();
      repoInputRef.current?.select();
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
        result = await tauriInvoke<Pass1Response>("annotate_overview", {
          repoPath: repoPath || null,
          llmProvider: llmSettings?.provider ?? null,
          llmModel: llmSettings?.model ?? null,
        });
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
  }, [repoPath, llmSettings]);

  /** Run LLM Pass 2: deep analysis for the selected group. */
  const runDeepAnalysis = useCallback(async () => {
    if (!selectedGroup) return;
    deepAnalyzingCount.current += 1;
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
          llmProvider: llmSettings?.provider ?? null,
          llmModel: llmSettings?.model ?? null,
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
      deepAnalyzingCount.current -= 1;
      // Only clear loading state when all concurrent deep analyses have completed
      if (deepAnalyzingCount.current <= 0) {
        deepAnalyzingCount.current = 0;
        setDeepAnalyzing(false);
      }
    }
  }, [selectedGroup, repoPath, baseRef, llmSettings]);

  /** Run LLM refinement pass on the current analysis groups. */
  const runRefinement = useCallback(async () => {
    if (!analysis) return;
    setRefining(true);
    setError(null);
    try {
      let result: RefinementResult;
      if (IS_TAURI) {
        result = await tauriInvoke<RefinementResult>("refine_groups", {
          repoPath: repoPath || null,
          llmProvider: llmSettings?.refinement_provider ?? llmSettings?.provider ?? null,
          llmModel: llmSettings?.refinement_model ?? llmSettings?.model ?? null,
        });
      } else {
        await new Promise((r) => setTimeout(r, 1200));
        result = MOCK_REFINEMENT;
      }

      // Store original groups before switching
      if (!originalGroups) {
        setOriginalGroups(analysis.groups);
      }

      setRefinedGroups(result.refined_groups);
      setRefinementResponse(result.refinement_response);
      setRefinementProvider(result.provider);
      setRefinementModel(result.model);

      if (result.had_changes) {
        // Switch to refined view and update the analysis
        setShowRefined(true);
        setAnalysis((prev) =>
          prev
            ? {
                ...prev,
                groups: result.refined_groups,
                infrastructure_group: result.infrastructure_group ?? prev.infrastructure_group,
              }
            : prev,
        );
        // Reset reviewed state — groups have been reorganized
        setReviewedGroupIds(new Set());
        showToast("Groups updated by refinement \u2014 review state reset");
        // Re-select first group in refined view
        const sorted = [...result.refined_groups].sort(
          (a, b) => a.review_order - b.review_order,
        );
        if (sorted.length > 0) {
          handleSelectGroup(sorted[0]);
        }
      }
    } catch (e) {
      setError(`Refinement failed: ${String(e)}`);
    } finally {
      setRefining(false);
    }
  }, [analysis, repoPath, llmSettings, originalGroups, handleSelectGroup]);

  /** Toggle between original and refined groups. */
  const toggleRefinedView = useCallback(
    (useRefined: boolean) => {
      if (!analysis) return;
      setShowRefined(useRefined);
      const groups = useRefined ? refinedGroups : originalGroups;
      if (groups) {
        setAnalysis((prev) =>
          prev ? { ...prev, groups } : prev,
        );
        const sorted = [...groups].sort(
          (a, b) => a.review_order - b.review_order,
        );
        if (sorted.length > 0) {
          handleSelectGroup(sorted[0]);
        }
      }
    },
    [analysis, refinedGroups, originalGroups, handleSelectGroup],
  );

  // Auto-load demo data when not in Tauri
  useEffect(() => {
    if (!IS_TAURI && !demoLoaded.current) {
      demoLoaded.current = true;
      runAnalysis();
    }
  }, [runAnalysis]);

  // Test API for Playwright — only available in demo/browser mode
  useEffect(() => {
    if (IS_TAURI) return;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (window as any).__TEST_API__ = {
      setRepoInfo: (data: RepoInfo | null) => setRepoInfo(data),
      setLlmSettings: (data: LlmSettings) => { setLlmSettings(data); setHasApiKey(data.has_api_key); },
      setAnalysis: (data: AnalysisOutput | null) => { setAnalysis(data); if (data && data.groups.length > 0) { const sorted = [...data.groups].sort((a, b) => a.review_order - b.review_order); handleSelectGroup(sorted[0]); } },
      setError: (msg: string | null) => setError(msg),
      clearAnalysis: () => { setAnalysis(null); setSelectedGroup(null); setSelectedFile(null); setFileDiff(null); setOverview(null); setDeepAnalyses({}); setOriginalGroups(null); setRefinedGroups(null); setRefinementResponse(null); setShowRefined(false); setReviewedGroupIds(new Set()); setComments([]); setCommentInput(null); setCommentText(""); },
      enterReplay: () => enterReplay(),
      exitReplay: () => exitReplay(),
      getReplayState: () => ({ active: replayActive, step: replayStep, visited: Array.from(replayVisited) }),
      toggleGroupReviewed: (id: string) => toggleGroupReviewed(id),
      getReviewedGroupIds: () => Array.from(reviewedGroupIds),
      crashPanel: (name: string | null) => setCrashPanel(name),
      copyFilePath: (path: string) => copyFilePath(path),
      getToast: () => toast,
      getComments: () => comments,
      openCommentInput: (input?: CommentInput) => openCommentInput(input),
      submitComment: () => submitComment(),
      cancelComment: () => cancelComment(),
      setCommentText: (text: string) => setCommentText(text),
      deleteComment: (id: string) => deleteComment(id),
      exportComments: () => exportComments(),
      getCommentInput: () => commentInput,
      getSelectedFile: () => selectedFile,
      getSelectedGroup: () => selectedGroup,
    };
    return () => { delete (window as any).__TEST_API__; };
  });

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

  // Close settings panel on Escape
  useEffect(() => {
    if (!settingsOpen) return;
    function handleKey(e: KeyboardEvent) {
      if (e.key === "Escape") setSettingsOpen(false);
    }
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, [settingsOpen]);

  // Close context menu on click or Escape
  useEffect(() => {
    if (!contextMenu) return;
    function handleClick() {
      setContextMenu(null);
    }
    function handleKey(e: KeyboardEvent) {
      if (e.key === "Escape") setContextMenu(null);
    }
    window.addEventListener("click", handleClick);
    window.addEventListener("keydown", handleKey);
    return () => {
      window.removeEventListener("click", handleClick);
      window.removeEventListener("keydown", handleKey);
    };
  }, [contextMenu]);

  /** Update a single LLM setting field and persist. */
  const updateSetting = useCallback(
    (field: keyof LlmSettings, value: string | boolean | number) => {
      if (!llmSettings) return;
      const updated = { ...llmSettings, [field]: value };
      // When provider changes, reset model to default for that provider
      if (field === "provider") {
        const provider = value as LlmProvider;
        const models = MODELS_BY_PROVIDER[provider] ?? [];
        updated.model = models[0] ?? "";
      }
      if (field === "refinement_provider") {
        const provider = value as LlmProvider;
        const models = MODELS_BY_PROVIDER[provider] ?? [];
        updated.refinement_model = models[0] ?? "";
      }
      saveLlmSettings(updated);
    },
    [llmSettings, saveLlmSettings],
  );

  /** Save an API key to .flowdiff.toml and refresh settings. */
  const handleSaveApiKey = useCallback(async () => {
    const key = apiKeyInput.trim();
    if (!key || !repoPath) return;
    try {
      if (IS_TAURI) {
        await tauriInvoke("save_api_key", { repoPath, apiKey: key });
      }
      setApiKeyInput("");
      // Refresh settings to pick up the new key
      await loadLlmSettings(repoPath);
    } catch {
      setError("Failed to save API key");
    }
  }, [apiKeyInput, repoPath, loadLlmSettings]);

  /** Clear the stored API key from .flowdiff.toml and refresh settings. */
  const handleClearApiKey = useCallback(async () => {
    if (!repoPath) return;
    try {
      if (IS_TAURI) {
        await tauriInvoke("clear_api_key", { repoPath });
      }
      setApiKeyInput("");
      // Refresh settings to reflect removal
      await loadLlmSettings(repoPath);
    } catch {
      setError("Failed to clear API key");
    }
  }, [repoPath, loadLlmSettings]);

  /** Add an ignore path pattern and persist to .flowdiff.toml. */
  const handleAddIgnorePath = useCallback(async () => {
    const pattern = ignorePathInput.trim();
    if (!pattern || !repoPath) return;
    if (ignorePaths.includes(pattern)) {
      setIgnorePathInput("");
      return;
    }
    const updated = [...ignorePaths, pattern];
    setIgnorePaths(updated);
    setIgnorePathInput("");
    if (IS_TAURI) {
      try {
        await tauriInvoke("save_ignore_paths", { repoPath, paths: updated });
      } catch {
        setError("Failed to save ignore paths");
      }
    }
  }, [ignorePathInput, repoPath, ignorePaths]);

  /** Remove an ignore path pattern and persist to .flowdiff.toml. */
  const handleRemoveIgnorePath = useCallback(async (pattern: string) => {
    if (!repoPath) return;
    const updated = ignorePaths.filter((p) => p !== pattern);
    setIgnorePaths(updated);
    if (IS_TAURI) {
      try {
        await tauriInvoke("save_ignore_paths", { repoPath, paths: updated });
      } catch {
        setError("Failed to save ignore paths");
      }
    }
  }, [repoPath, ignorePaths]);

  // Sort groups by review_order (memoized to avoid re-sorting on every render)
  const sortedGroups = useMemo(
    () => analysis
      ? [...analysis.groups].sort((a, b) => a.review_order - b.review_order)
      : [],
    [analysis],
  );
  sortedGroupsRef.current = sortedGroups;

  // Get the Pass 1 annotation for the currently selected group
  const groupAnnotation: Pass1GroupAnnotation | undefined = overview?.groups.find(
    (g) => g.id === selectedGroup?.id,
  );

  // Get the Pass 2 deep analysis for the currently selected group
  const groupDeepAnalysis: Pass2Response | undefined = selectedGroup
    ? deepAnalyses[selectedGroup.id]
    : undefined;

  /** Toggle reviewed state for a flow group. */
  const toggleGroupReviewed = useCallback((groupId: string) => {
    setReviewedGroupIds((prev) => {
      const next = new Set(prev);
      if (next.has(groupId)) {
        next.delete(groupId);
      } else {
        next.add(groupId);
      }
      return next;
    });
  }, []);

  /** Show a toast notification that auto-dismisses. */
  const showToast = useCallback((message: string) => {
    if (toastTimer.current) clearTimeout(toastTimer.current);
    setToast(message);
    toastTimer.current = setTimeout(() => setToast(null), 3500);
  }, []);

  /** Build the absolute file path from repo path + relative path. */
  const buildAbsolutePath = useCallback(
    (relativePath: string): string => {
      if (!repoPath) return relativePath;
      const base = repoPath.endsWith("/") ? repoPath.slice(0, -1) : repoPath;
      return `${base}/${relativePath}`;
    },
    [repoPath],
  );

  /** Copy a file's absolute path to clipboard. */
  const copyFilePath = useCallback(
    async (relativePath: string) => {
      const absPath = buildAbsolutePath(relativePath);
      try {
        await navigator.clipboard.writeText(absPath);
        showToast("Path copied to clipboard");
      } catch {
        showToast("Failed to copy path");
      }
    },
    [buildAbsolutePath, showToast],
  );

  /** Copy all file paths in a flow group to clipboard (absolute, one per line, flow order). */
  const copyFlowPaths = useCallback(
    async (group: FlowGroup) => {
      const paths = group.files
        .map((f) => buildAbsolutePath(f.path))
        .join("\n");
      try {
        await navigator.clipboard.writeText(paths);
        showToast(`${group.files.length} file paths copied to clipboard`);
      } catch {
        showToast("Failed to copy paths");
      }
    },
    [buildAbsolutePath, showToast],
  );

  /** Compute a simple hash of the analysis for comment scoping. */
  const analysisHash = analysis
    ? `${analysis.diff_source.base_sha ?? ""}:${analysis.diff_source.head_sha ?? ""}:${analysis.summary.total_files_changed}`
    : "";

  /** Load comments from backend for the current analysis. */
  const loadComments = useCallback(async () => {
    if (!repoPath || !analysisHash) return;
    try {
      if (IS_TAURI) {
        const result = await tauriInvoke<ReviewComment[]>("load_comments", {
          repoPath,
          analysisHash,
        });
        setComments(result);
      }
    } catch {
      // Non-fatal: comments will just be empty
    }
  }, [repoPath, analysisHash]);

  // Load comments when analysis changes
  useEffect(() => {
    if (analysisHash) {
      loadComments();
    }
  }, [analysisHash, loadComments]);

  /** Save a comment (persist to .flowdiff/comments.json). */
  const saveComment = useCallback(
    async (comment: ReviewComment) => {
      setComments((prev) => [...prev, comment]);
      if (IS_TAURI && repoPath && analysisHash) {
        try {
          await tauriInvoke("save_comment", {
            repoPath,
            analysisHash,
            comment,
          });
        } catch {
          // Already saved in state, persistence failure is non-fatal
        }
      }
    },
    [repoPath, analysisHash],
  );

  /** Delete a comment by ID. */
  const deleteComment = useCallback(
    async (commentId: string) => {
      setComments((prev) => prev.filter((c) => c.id !== commentId));
      if (IS_TAURI && repoPath && analysisHash) {
        try {
          await tauriInvoke("delete_comment", {
            repoPath,
            analysisHash,
            commentId,
          });
        } catch {
          // Non-fatal
        }
      }
    },
    [repoPath, analysisHash],
  );

  /** Open the comment input — context-sensitive based on current selection. */
  const openCommentInput = useCallback(
    (overrideInput?: CommentInput) => {
      const group = selectedGroupRef.current;
      const file = selectedFileRef.current;
      if (!group) return;

      const input: CommentInput = overrideInput ?? (file
        ? { type: "file", group_id: group.id, file_path: file }
        : { type: "group", group_id: group.id });

      setCommentInput(input);
      setCommentText("");
      // Focus the textarea after render
      setTimeout(() => commentInputRef.current?.focus(), 50);
    },
    [],
  );

  /** Submit the current comment. */
  const submitComment = useCallback(() => {
    if (!commentInput || !commentText.trim()) return;

    const comment: ReviewComment = {
      id: `comment_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`,
      type: commentInput.type,
      group_id: commentInput.group_id,
      file_path: commentInput.file_path ?? null,
      start_line: commentInput.start_line ?? null,
      end_line: commentInput.end_line ?? null,
      selected_code: commentInput.selected_code ?? null,
      text: commentText.trim(),
      created_at: new Date().toISOString(),
    };

    saveComment(comment);
    setCommentInput(null);
    setCommentText("");
    showToast("Comment saved");
  }, [commentInput, commentText, saveComment, showToast]);

  /** Cancel the current comment input. */
  const cancelComment = useCallback(() => {
    setCommentInput(null);
    setCommentText("");
  }, []);

  /** Export all comments as formatted text, copy to clipboard. */
  const exportComments = useCallback(async () => {
    if (comments.length === 0) {
      showToast("No comments to copy");
      return;
    }

    // Build formatted output locally (works in both Tauri and demo mode)
    const base = repoPath.endsWith("/") ? repoPath.slice(0, -1) : repoPath;
    let output = "";

    // Group comments by group_id, then sort by type for clean output
    for (const comment of comments) {
      switch (comment.type) {
        case "code": {
          if (comment.file_path) {
            const absPath = `${base}/${comment.file_path}`;
            if (comment.start_line != null && comment.end_line != null) {
              output += `${absPath}:${comment.start_line}-${comment.end_line}\n`;
            } else {
              output += `${absPath}\n`;
            }
            if (comment.selected_code) {
              output += "```\n";
              output += comment.selected_code;
              if (!comment.selected_code.endsWith("\n")) output += "\n";
              output += "```\n";
            }
            output += `> ${comment.text}\n\n`;
          }
          break;
        }
        case "file": {
          if (comment.file_path) {
            const absPath = `${base}/${comment.file_path}`;
            output += `${absPath}\n`;
            output += `> ${comment.text}\n\n`;
          }
          break;
        }
        case "group": {
          // Find the group name for better export
          const group = analysis?.groups.find((g) => g.id === comment.group_id);
          const label = group ? group.name : comment.group_id;
          output += `Flow: "${label}"\n`;
          output += `> ${comment.text}\n\n`;
          break;
        }
      }
    }

    try {
      await navigator.clipboard.writeText(output);
      showToast(`${comments.length} comment${comments.length === 1 ? "" : "s"} copied to clipboard`);
    } catch {
      showToast("Failed to copy comments");
    }
  }, [comments, repoPath, analysis, showToast]);

  /** Pre-indexed comment counts by group for O(1) lookup. */
  const commentsByGroupMap = useMemo(() => {
    const map = new Map<string, number>();
    for (const c of comments) {
      map.set(c.group_id, (map.get(c.group_id) ?? 0) + 1);
    }
    return map;
  }, [comments]);

  /** Pre-indexed comments by file path for O(1) lookup. */
  const commentsByFileMap = useMemo(() => {
    const map = new Map<string, ReviewComment[]>();
    for (const c of comments) {
      if (c.file_path) {
        const arr = map.get(c.file_path);
        if (arr) arr.push(c);
        else map.set(c.file_path, [c]);
      }
    }
    return map;
  }, [comments]);

  /** Get the count of comments for a specific group. */
  const commentCountForGroup = useCallback(
    (groupId: string): number => commentsByGroupMap.get(groupId) ?? 0,
    [commentsByGroupMap],
  );

  /** Get comments for the currently selected file. */
  const commentsForFile = useCallback(
    (filePath: string): ReviewComment[] => commentsByFileMap.get(filePath) ?? [],
    [commentsByFileMap],
  );

  /** Code-level comments for the selected file, passed to DiffViewer. */
  const codeCommentsForSelectedFile = useMemo(
    () => selectedFile
      ? comments.filter((c) => c.type === "code" && c.file_path === selectedFile)
      : [],
    [comments, selectedFile],
  );

  /** Open the current file in an external editor. */
  type EditorId = "vscode" | "cursor" | "zed" | "vim" | "terminal";
  const [openWithDropdown, setOpenWithDropdown] = useState(false);
  const [lastEditor, setLastEditor] = useState<EditorId>("vscode");
  const [availableEditors, setAvailableEditors] = useState<Set<EditorId> | null>(null);
  const openWithRef = useRef<HTMLDivElement>(null);

  const editorIcons: Record<EditorId, string> = {
    vscode: `<svg width="16" height="16" viewBox="0 0 256 256" xmlns="http://www.w3.org/2000/svg"><path d="M180.3 4.5l-56 43.2L59 4.2a8.3 8.3 0 0 0-10.2 1.5L5.1 47.4a8 8 0 0 0 0 11.2L44 96l-39 37.4a8 8 0 0 0 0 11.2l43.7 41.7a8.3 8.3 0 0 0 10.2 1.5l65.3-43.5 56 43.2a12.2 12.2 0 0 0 7 2.5c2 0 4-.5 5.8-1.6l47.5-23a12 12 0 0 0 6.5-10.6V33.2c0-4.4-2.5-8.5-6.5-10.6L193.1 0c-4-2-8.8-1.3-12.8 2.5v2zM192 52.8v150.4L123 128z" fill="#007ACC"/></svg>`,
    cursor: `<svg width="16" height="16" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M1 1l6.5 14L10 9l6-2.5z" stroke="#cdd6f4" stroke-width="1.5" stroke-linejoin="round" fill="none"/><path d="M10 9l4.5 4.5" stroke="#cdd6f4" stroke-width="1.5" stroke-linecap="round"/></svg>`,
    zed: `<svg width="16" height="16" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M2 3h12L2 13h12" stroke="#cdd6f4" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>`,
    vim: `<svg width="16" height="16" viewBox="0 0 544 544" xmlns="http://www.w3.org/2000/svg"><polygon points="272,16 16,272 144,272 272,144 272,272 400,272 528,272 272,16" fill="#019833"/><polygon points="272,528 528,272 400,272 272,400 272,272 144,272 16,272 272,528" fill="#33cc33"/></svg>`,
    terminal: `<svg width="16" height="16" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg"><rect x="1" y="2" width="14" height="12" rx="2" stroke="#cdd6f4" stroke-width="1.2"/><path d="M4 6l2.5 2L4 10" stroke="#a6e3a1" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/><path d="M8.5 10H12" stroke="#6c7086" stroke-width="1.2" stroke-linecap="round"/></svg>`,
  };

  const allEditorOptions: { id: EditorId; label: string }[] = [
    { id: "vscode", label: "VS Code" },
    { id: "cursor", label: "Cursor" },
    { id: "zed", label: "Zed" },
    { id: "vim", label: "Vim" },
    { id: "terminal", label: "Terminal" },
  ];

  const editorOptions = availableEditors
    ? allEditorOptions.filter((opt) => availableEditors.has(opt.id))
    : allEditorOptions;

  // Detect which editors are installed
  useEffect(() => {
    if (!IS_TAURI) return;
    tauriInvoke<Record<string, boolean>>("check_editors_available").then(
      (result) => {
        const available = new Set<EditorId>();
        for (const [id, isAvailable] of Object.entries(result)) {
          if (isAvailable) available.add(id as EditorId);
        }
        setAvailableEditors(available);
        // If current lastEditor is not available, switch to first available
        if (!available.has(lastEditor)) {
          const first = allEditorOptions.find((opt) => available.has(opt.id));
          if (first) setLastEditor(first.id);
        }
      },
      () => {
        // On error, show all editors (graceful fallback)
      },
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const openInEditor = useCallback(
    async (editor: EditorId) => {
      const file = selectedFile;
      if (!file) {
        showToast("No file selected");
        return;
      }
      const absPath = buildAbsolutePath(file);
      setLastEditor(editor);
      setOpenWithDropdown(false);
      if (!IS_TAURI) {
        showToast(`Would open ${absPath} in ${editor}`);
        return;
      }
      try {
        await tauriInvoke("open_in_editor", { editor, filePath: absPath });
      } catch (e: any) {
        const msg = typeof e === "string" ? e : e?.message || String(e);
        showToast(msg);
      }
    },
    [selectedFile, buildAbsolutePath, showToast],
  );

  // Close "Open With" dropdown when clicking outside
  useEffect(() => {
    if (!openWithDropdown) return;
    function handleClick(e: MouseEvent) {
      if (openWithRef.current && !openWithRef.current.contains(e.target as Node)) {
        setOpenWithDropdown(false);
      }
    }
    document.addEventListener("mousedown", handleClick);
    return () => document.removeEventListener("mousedown", handleClick);
  }, [openWithDropdown]);

  /** Handle right-click context menu on a file item. */
  const handleFileContextMenu = useCallback(
    (e: React.MouseEvent, filePath: string) => {
      e.preventDefault();
      e.stopPropagation();
      setContextMenu({ x: e.clientX, y: e.clientY, filePath });
    },
    [],
  );

  /** Enter flow replay mode for the currently selected group. */
  const enterReplay = useCallback(() => {
    const group = selectedGroupRef.current;
    if (!group || group.files.length === 0) return;
    setReplayActive(true);
    setReplayStep(0);
    setReplayVisited(new Set([group.files[0].path]));
    handleSelectFile(group.files[0].path);
  }, [handleSelectFile]);

  /** Exit flow replay mode. */
  const exitReplay = useCallback(() => {
    setReplayActive(false);
    setReplayStep(0);
    setReplayVisited(new Set());
  }, []);

  /** Move to a specific replay step. */
  const goToReplayStep = useCallback(
    (step: number) => {
      const group = selectedGroupRef.current;
      if (!group) return;
      const clamped = Math.max(0, Math.min(step, group.files.length - 1));
      setReplayStep(clamped);
      const filePath = group.files[clamped].path;
      setReplayVisited((prev) => {
        const next = new Set(prev);
        next.add(filePath);
        return next;
      });
      handleSelectFile(filePath);
    },
    [handleSelectFile],
  );

  // Keyboard navigation: j/k = next/prev file, J/K = next/prev group, r = replay
  // Registered on capture phase so shortcuts work even when Monaco editor has focus.
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      // Skip if user is typing in an input field — but not Monaco's internal textarea
      const target = e.target as HTMLElement;
      const isInMonaco = !!target.closest(".monaco-editor");
      if (
        !isInMonaco &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.tagName === "SELECT")
      ) {
        return;
      }

      // When Monaco has focus, only intercept known app shortcut keys.
      // Let other keys (arrows, Page Up/Down, etc.) pass through to Monaco for scrolling.
      if (isInMonaco) {
        const appKeys = new Set(["j", "k", "J", "K", "r", "x", "y", "Y", "c", "C"]);
        if (!appKeys.has(e.key)) {
          return;
        }
      }

      // Helper: prevent default AND stop propagation (needed to prevent Monaco
      // from showing "Cannot edit in read-only editor" for intercepted keys).
      const consume = () => {
        e.preventDefault();
        e.stopPropagation();
      };

      const groups = sortedGroupsRef.current;
      const group = selectedGroupRef.current;
      const file = selectedFileRef.current;
      const isReplaying = replayActiveRef.current;
      const step = replayStepRef.current;

      // Replay mode keys
      if (isReplaying && group) {
        if (e.key === "Escape" || e.key === "r") {
          consume();
          exitReplay();
          return;
        }
        if (e.key === "n" || e.key === "ArrowRight" || e.key === " ") {
          consume();
          if (step < group.files.length - 1) {
            goToReplayStep(step + 1);
          }
          return;
        }
        if (e.key === "p" || e.key === "ArrowLeft") {
          consume();
          if (step > 0) {
            goToReplayStep(step - 1);
          }
          return;
        }
        // Block other navigation while replaying
        if (["j", "k", "J", "K"].includes(e.key)) {
          consume();
          return;
        }
      }

      // Normal mode: r enters replay
      if (e.key === "r" && group && group.files.length > 0) {
        consume();
        enterReplay();
        return;
      }

      // x toggles reviewed state on the currently selected group
      if (e.key === "x" && group) {
        consume();
        toggleGroupReviewed(group.id);
        return;
      }

      // y copies the absolute path of the currently selected file
      if (e.key === "y" && !e.shiftKey && file) {
        consume();
        copyFilePath(file);
        return;
      }

      // Y (shift+y) copies all file paths in the current group
      if (e.key === "Y" && group) {
        consume();
        copyFlowPaths(group);
        return;
      }

      // c opens context-sensitive comment input — if text is selected in Monaco, include it
      if (e.key === "c" && !e.shiftKey && group) {
        consume();
        // Try to grab the current selection from Monaco's modified (right-side) editor
        const monacoEditors = (window as any).monaco?.editor?.getEditors?.();
        if (monacoEditors && file) {
          for (const ed of monacoEditors) {
            const sel = ed.getSelection?.();
            if (sel && sel.startLineNumber !== sel.endLineNumber) {
              const model = ed.getModel?.();
              if (model) {
                const startLine = Math.min(sel.startLineNumber, sel.endLineNumber);
                const endLine = Math.max(sel.startLineNumber, sel.endLineNumber);
                const lines: string[] = [];
                for (let i = startLine; i <= endLine; i++) {
                  lines.push(model.getLineContent(i));
                }
                openCommentInput({
                  type: "code",
                  group_id: group.id,
                  file_path: file,
                  start_line: startLine,
                  end_line: endLine,
                  selected_code: lines.join("\n"),
                });
                return;
              }
            }
          }
        }
        openCommentInput();
        return;
      }

      // C (shift+c) copies all comments
      if (e.key === "C" && group) {
        consume();
        exportComments();
        return;
      }

      if (groups.length === 0 || !group) return;

      const groupIdx = groups.findIndex((g) => g.id === group.id);
      if (groupIdx === -1) return;

      if (e.key === "j") {
        // Next file in current group (debounced to avoid IPC storms)
        consume();
        const fileIdx = group.files.findIndex((f) => f.path === file);
        if (fileIdx < group.files.length - 1) {
          handleSelectFileDebounced(group.files[fileIdx + 1].path);
        }
      } else if (e.key === "k") {
        // Previous file in current group (debounced)
        consume();
        const fileIdx = group.files.findIndex((f) => f.path === file);
        if (fileIdx > 0) {
          handleSelectFileDebounced(group.files[fileIdx - 1].path);
        }
      } else if (e.key === "J") {
        // Next group
        consume();
        if (groupIdx < groups.length - 1) {
          handleSelectGroup(groups[groupIdx + 1]);
        }
      } else if (e.key === "K") {
        // Previous group
        consume();
        if (groupIdx > 0) {
          handleSelectGroup(groups[groupIdx - 1]);
        }
      }
    }

    window.addEventListener("keydown", handleKeyDown, true);
    return () => window.removeEventListener("keydown", handleKeyDown, true);
  }, [handleSelectFile, handleSelectFileDebounced, handleSelectGroup, enterReplay, exitReplay, goToReplayStep, toggleGroupReviewed, copyFilePath, copyFlowPaths, openCommentInput, exportComments]);

  const handleSelectBase = useCallback((branch: string) => {
    setBaseRef(branch);
    setBranchDropdownOpen(false);
  }, []);

  // Derived: non-current branches for the base branch dropdown
  const baseBranches: BranchInfo[] = repoInfo?.branches ?? [];

  // Status display
  const statusText = formatBranchStatus(repoInfo);

  /** Test-only component that throws during render to exercise ErrorBoundary. */
  function CrashTest({ panel }: { panel: string }) {
    if (crashPanel === panel) {
      throw new Error(`Test crash in ${panel}`);
    }
    return null;
  }

  return (
    <div className="app">
      {/* Top bar */}
      <header className="top-bar">
        <div className="top-bar-left">
          <span className="logo">flowdiff</span>
        </div>
        <div className="top-bar-center">
          <input
            ref={repoInputRef}
            className="input repo-input"
            type="text"
            placeholder="Repository path..."
            value={repoPath}
            onChange={(e) => setRepoPath(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && repoPath && !loading) {
                (e.target as HTMLInputElement).blur();
                runAnalysis();
              }
            }}
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
          {/* Settings gear icon */}
          <button
            className="btn btn-settings"
            onClick={() => setSettingsOpen(!settingsOpen)}
            title="Settings"
          >
            &#9881;
          </button>
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
              {reviewedGroupIds.size > 0 && (
                <span className="reviewed-counter">
                  {" "}&middot; {reviewedGroupIds.size}/{sortedGroups.length} reviewed
                </span>
              )}
            </span>
          )}
        </div>
      </header>

      {/* Settings Panel Overlay */}
      {settingsOpen && llmSettings && (
        <div className="settings-overlay" onClick={() => setSettingsOpen(false)}>
          <div className="settings-panel" onClick={(e) => e.stopPropagation()}>
            <div className="settings-header">
              <h2>Settings</h2>
              <button className="btn-close" onClick={() => setSettingsOpen(false)}>
                &times;
              </button>
            </div>
            <div className="settings-body">
              {/* API Key Status */}
              <div className="settings-section">
                <h3>API Key</h3>
                <div className={`api-key-status ${llmSettings.has_api_key ? "configured" : "missing"}`}>
                  <span className="api-key-dot" />
                  <span>
                    {llmSettings.has_api_key
                      ? `Configured via ${llmSettings.api_key_source}`
                      : "Not configured"}
                  </span>
                </div>
                {/* API Key Input */}
                <div className="api-key-input-row">
                  <input
                    type="password"
                    className="settings-input api-key-input"
                    placeholder="Paste your API key"
                    value={apiKeyInput}
                    onChange={(e) => setApiKeyInput(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && apiKeyInput.trim()) {
                        handleSaveApiKey();
                      }
                    }}
                  />
                  <button
                    className="btn btn-save-key"
                    disabled={!apiKeyInput.trim()}
                    onClick={handleSaveApiKey}
                    title="Save API key to .flowdiff.toml"
                  >
                    Save
                  </button>
                  {llmSettings.api_key_source === "config file" && (
                    <button
                      className="btn btn-clear-key"
                      onClick={handleClearApiKey}
                      title="Remove stored API key"
                    >
                      Clear
                    </button>
                  )}
                </div>
                {!llmSettings.has_api_key && (
                  <p className="settings-hint">
                    Paste your key above, or set <code>FLOWDIFF_API_KEY</code>, a provider-specific env var
                    (<code>ANTHROPIC_API_KEY</code>, <code>OPENAI_API_KEY</code>, <code>GEMINI_API_KEY</code>),
                    or configure <code>key_cmd</code> in <code>.flowdiff.toml</code>.
                  </p>
                )}
              </div>

              {/* Annotations Toggle */}
              <div className="settings-section">
                <h3>Annotations</h3>
                <label className="settings-toggle">
                  <input
                    type="checkbox"
                    checked={llmSettings.annotations_enabled}
                    onChange={(e) => updateSetting("annotations_enabled", e.target.checked)}
                  />
                  <span>Enable LLM annotations</span>
                </label>
                <p className="settings-hint">
                  When enabled, "Summarize PR" and "Analyze This Flow" buttons are active.
                </p>
              </div>

              {/* Provider Selector */}
              <div className="settings-section">
                <h3>Provider</h3>
                <select
                  className="settings-select"
                  value={llmSettings.provider}
                  onChange={(e) => updateSetting("provider", e.target.value)}
                >
                  {LLM_PROVIDERS.map((p) => (
                    <option key={p} value={p}>
                      {p.charAt(0).toUpperCase() + p.slice(1)}
                    </option>
                  ))}
                </select>
              </div>

              {/* Model Selector */}
              <div className="settings-section">
                <h3>Model</h3>
                <select
                  className="settings-select"
                  value={llmSettings.model}
                  onChange={(e) => updateSetting("model", e.target.value)}
                >
                  {(MODELS_BY_PROVIDER[llmSettings.provider as LlmProvider] ?? []).map(
                    (m) => (
                      <option key={m} value={m}>
                        {m}
                      </option>
                    ),
                  )}
                </select>
              </div>

              {/* Refinement Section */}
              <div className="settings-section settings-refinement">
                <h3>Refinement</h3>
                <label className="settings-toggle">
                  <input
                    type="checkbox"
                    checked={llmSettings.refinement_enabled}
                    onChange={(e) => updateSetting("refinement_enabled", e.target.checked)}
                  />
                  <span>Enable LLM refinement</span>
                </label>
                <p className="settings-hint">
                  Refines deterministic groupings using an LLM pass. Can use a different provider/model.
                </p>
                {llmSettings.refinement_enabled && (
                  <>
                    <div className="settings-row">
                      <label>Provider</label>
                      <select
                        className="settings-select"
                        value={llmSettings.refinement_provider}
                        onChange={(e) => updateSetting("refinement_provider", e.target.value)}
                      >
                        {LLM_PROVIDERS.map((p) => (
                          <option key={p} value={p}>
                            {p.charAt(0).toUpperCase() + p.slice(1)}
                          </option>
                        ))}
                      </select>
                    </div>
                    <div className="settings-row">
                      <label>Model</label>
                      <select
                        className="settings-select"
                        value={llmSettings.refinement_model}
                        onChange={(e) => updateSetting("refinement_model", e.target.value)}
                      >
                        {(MODELS_BY_PROVIDER[llmSettings.refinement_provider as LlmProvider] ?? []).map(
                          (m) => (
                            <option key={m} value={m}>
                              {m}
                            </option>
                          ),
                        )}
                      </select>
                    </div>
                    <div className="settings-row">
                      <label>Max iterations</label>
                      <input
                        type="number"
                        className="settings-number"
                        min={1}
                        max={10}
                        value={llmSettings.refinement_max_iterations}
                        onChange={(e) =>
                          updateSetting("refinement_max_iterations", Math.max(1, parseInt(e.target.value) || 1))
                        }
                      />
                    </div>
                  </>
                )}
              </div>

              {/* Exclude Paths Section */}
              <div className="settings-section">
                <h3>Exclude Paths</h3>
                <p className="settings-hint" style={{ marginTop: 0, marginBottom: 8 }}>
                  Glob patterns for files/folders to exclude from analysis. Matched against repo-relative paths.
                </p>
                {ignorePaths.length > 0 && (
                  <div className="ignore-paths-list">
                    {ignorePaths.map((pattern) => (
                      <span key={pattern} className="ignore-path-tag">
                        <code>{pattern}</code>
                        <button
                          className="ignore-path-remove"
                          onClick={() => handleRemoveIgnorePath(pattern)}
                          title={`Remove ${pattern}`}
                        >
                          &times;
                        </button>
                      </span>
                    ))}
                  </div>
                )}
                <div className="ignore-path-input-row">
                  <input
                    type="text"
                    className="settings-input ignore-path-input"
                    placeholder="e.g. dist/**, **/*.generated.ts"
                    value={ignorePathInput}
                    onChange={(e) => setIgnorePathInput(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && ignorePathInput.trim()) {
                        handleAddIgnorePath();
                      }
                    }}
                  />
                  <button
                    className="btn btn-save-key"
                    disabled={!ignorePathInput.trim()}
                    onClick={handleAddIgnorePath}
                  >
                    Add
                  </button>
                </div>
              </div>
            </div>
          </div>
        </div>
      )}

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
          <div className="panel-header">
            <span>Flow Groups</span>
            {comments.length > 0 && (
              <button
                className="btn btn-copy-comments"
                onClick={exportComments}
                title="Copy all comments to clipboard (Shift+C)"
              >
                Copy Comments ({comments.length})
              </button>
            )}
            {showRefined && refinementProvider && (
              <span className="refined-badge" title={`Refined by ${refinementProvider}/${refinementModel}`}>
                Refined by {refinementModel}
              </span>
            )}
          </div>
          <div className="panel-body">
            {/* Refinement banner — shown after analysis when API key is available */}
            {analysis && !refinedGroups && !refining && hasApiKey && llmSettings?.refinement_enabled && (
              <div className="refinement-banner">
                <span>AI can improve these groupings</span>
                <button
                  className="btn btn-refine"
                  onClick={runRefinement}
                  title={`Refine groupings using ${llmSettings?.refinement_provider ?? "anthropic"} (${llmSettings?.refinement_model ?? "default"})`}
                >
                  Refine
                </button>
              </div>
            )}

            {/* Refinement loading state */}
            {refining && (
              <div className="refinement-loading">
                <span className="refine-spinner" />
                <span>
                  Refining with {llmSettings?.refinement_provider ?? "anthropic"}/{llmSettings?.refinement_model ?? "..."}
                </span>
              </div>
            )}

            {/* Original/Refined toggle — shown when refined groups exist */}
            {refinedGroups && originalGroups && (
              <div className="refinement-toggle">
                <button
                  className={`toggle-btn ${!showRefined ? "active" : ""}`}
                  onClick={() => toggleRefinedView(false)}
                >
                  Original
                </button>
                <button
                  className={`toggle-btn ${showRefined ? "active" : ""}`}
                  onClick={() => toggleRefinedView(true)}
                >
                  Refined
                </button>
              </div>
            )}

            {sortedGroups.map((group) => {
              const changeIndicator = showRefined
                ? getGroupChangeIndicator(group, refinementResponse)
                : null;

              return (
                <div
                  key={group.id}
                  className={`group-item ${selectedGroup?.id === group.id ? "selected" : ""} ${changeIndicator ? "refined-change" : ""} ${reviewedGroupIds.has(group.id) ? "group-reviewed" : ""}`}
                  onClick={() => handleSelectGroup(group)}
                >
                  <div className="group-header">
                    <span
                      className={`group-review-check ${reviewedGroupIds.has(group.id) ? "checked" : ""}`}
                      title={reviewedGroupIds.has(group.id) ? "Mark as unreviewed" : "Mark as reviewed"}
                      onClick={(e) => {
                        e.stopPropagation();
                        toggleGroupReviewed(group.id);
                      }}
                    >
                      {reviewedGroupIds.has(group.id) ? "\u2713" : ""}
                    </span>
                    <span className="group-name">{group.name}</span>
                    <button
                      className="copy-flow-btn"
                      title="Copy all file paths in this flow"
                      onClick={(e) => {
                        e.stopPropagation();
                        copyFlowPaths(group);
                      }}
                    >
                      &#128203;
                    </button>
                    {commentCountForGroup(group.id) > 0 && (
                      <span className="comment-count-badge" title={`${commentCountForGroup(group.id)} comment${commentCountForGroup(group.id) === 1 ? "" : "s"}`}>
                        {commentCountForGroup(group.id)}
                      </span>
                    )}
                    <span className="risk-badge" data-risk={riskLevel(group.risk_score)}>
                      {group.risk_score.toFixed(2)}
                    </span>
                  </div>
                  {changeIndicator && (
                    <div className="change-indicator" title={changeIndicator.reason}>
                      <span className={`change-tag change-${changeIndicator.type}`}>
                        {changeIndicator.label}
                      </span>
                    </div>
                  )}
                  {selectedGroup?.id === group.id && (
                    <ul className="file-list">
                      {group.files.map((file) => {
                        const fileMoved = showRefined
                          ? getFileMovedIndicator(file.path, refinementResponse)
                          : null;
                        const fileCommentCount = commentsForFile(file.path).length;

                        return (
                          <li
                            key={file.path}
                            className={`file-item ${selectedFile === file.path ? "selected" : ""} ${fileMoved ? "file-moved" : ""}`}
                            onClick={(e) => {
                              e.stopPropagation();
                              handleSelectFile(file.path);
                            }}
                            onContextMenu={(e) => handleFileContextMenu(e, file.path)}
                          >
                            {replayActive && replayVisited.has(file.path) && (
                              <span className="replay-visited-check" title="Visited">&#10003;</span>
                            )}
                            <span className="file-role">{file.role}</span>
                            <span className="file-path">{shortPath(file.path)}</span>
                            <span className="file-changes">
                              +{file.changes.additions} -{file.changes.deletions}
                            </span>
                            {fileCommentCount > 0 && (
                              <button
                                className="file-comment-btn"
                                title={`${fileCommentCount} comment${fileCommentCount === 1 ? "" : "s"} — click to view`}
                                onClick={(e) => {
                                  e.stopPropagation();
                                  handleSelectFile(file.path);
                                  setCommentsCollapsed(false);
                                }}
                              >
                                <span className="file-comment-icon">&#128172;</span>
                                <span className="file-comment-count">{fileCommentCount}</span>
                              </button>
                            )}
                            {fileMoved && (
                              <span className="file-moved-tag" title={fileMoved.reason}>
                                moved from {fileMoved.from}
                              </span>
                            )}
                          </li>
                        );
                      })}
                    </ul>
                  )}
                </div>
              );
            })}
            {/* Infrastructure group — collapsed by default, shows count */}
            {analysis?.infrastructure_group && analysis.infrastructure_group.files.length > 0 && (
              <div className="group-item infra-group">
                <div
                  className="group-header"
                  style={{ cursor: "pointer" }}
                  onClick={() => setInfraExpanded((prev) => !prev)}
                >
                  <span className="group-name">
                    Infrastructure
                  </span>
                  <span className="risk-badge" data-risk="low">
                    {analysis.infrastructure_group.files.length} files
                  </span>
                  <span style={{ marginLeft: 4, fontSize: 10, opacity: 0.6 }}>
                    {infraExpanded ? "\u25B2" : "\u25BC"}
                  </span>
                </div>
                {infraExpanded && (
                  <ul className="file-list">
                    {(infraShowAll
                      ? analysis.infrastructure_group.files
                      : analysis.infrastructure_group.files.slice(0, 50)
                    ).map((f) => (
                      <li key={f} className="file-item">
                        <span className="file-path">{shortPath(f)}</span>
                      </li>
                    ))}
                    {!infraShowAll && analysis.infrastructure_group.files.length > 50 && (
                      <li
                        className="file-item"
                        style={{ opacity: 0.7, cursor: "pointer", textAlign: "center" }}
                        onClick={(e) => { e.stopPropagation(); setInfraShowAll(true); }}
                      >
                        Show all {analysis.infrastructure_group.files.length} files...
                      </li>
                    )}
                  </ul>
                )}
              </div>
            )}
            {loading && (
              <div className="empty-state loading-state">
                <span className="spinner" />
                Analyzing repository...
              </div>
            )}
            {!analysis && !loading && (
              <div className="empty-state">
                Enter a repository path and click Analyze to start.
              </div>
            )}
          </div>
          {/* Sticky footer bar — always visible when comments exist */}
          {comments.length > 0 && (
            <div className="panel-footer">
              <span className="panel-footer-count">{comments.length} comment{comments.length === 1 ? "" : "s"}</span>
              <button
                className="btn btn-copy-comments-footer"
                onClick={exportComments}
                title="Copy all comments to clipboard (Shift+C)"
              >
                Copy All Comments
              </button>
              <span className="panel-footer-hint">Shift+C</span>
            </div>
          )}
        </aside>

        {/* Center panel: Monaco Diff Viewer */}
        <main className="panel panel-center">
          <div className="panel-header">
            <span className="panel-header-title">{fileDiff ? fileDiff.path : "Diff Viewer"}</span>
            {fileDiff && (
              <div className="editor-toolbar" ref={openWithRef}>
                <button
                  className="editor-btn open-with-btn"
                  onClick={() => openInEditor(lastEditor)}
                  title={`Open in ${editorOptions.find((e) => e.id === lastEditor)?.label ?? lastEditor}`}
                >
                  Open With
                </button>
                <button
                  className="editor-btn open-with-arrow"
                  onClick={() => setOpenWithDropdown(!openWithDropdown)}
                  aria-label="Choose editor"
                >
                  {openWithDropdown ? "\u25B2" : "\u25BC"}
                </button>
                {openWithDropdown && (
                  <div className="open-with-dropdown">
                    {editorOptions.map((opt) => (
                      <button
                        key={opt.id}
                        className={`open-with-option ${opt.id === lastEditor ? "active" : ""}`}
                        onClick={() => openInEditor(opt.id)}
                      >
                        <span
                          className="editor-icon"
                          dangerouslySetInnerHTML={{ __html: editorIcons[opt.id] }}
                        />
                        {opt.label}
                      </button>
                    ))}
                  </div>
                )}
              </div>
            )}
          </div>
          {/* Replay bar — shown when replay mode is active */}
          {replayActive && selectedGroup && (
            <div className="replay-bar">
              <div className="replay-bar-left">
                <span className="replay-badge">REPLAY</span>
                <span className="replay-step-label">
                  Step {replayStep + 1} of {selectedGroup.files.length}
                </span>
                {selectedGroup.files[replayStep] && (
                  <span className="replay-file-role">
                    {selectedGroup.files[replayStep].role}
                  </span>
                )}
              </div>
              <div className="replay-bar-center">
                <div className="replay-progress">
                  {selectedGroup.files.map((f, i) => (
                    <button
                      key={f.path}
                      className={`replay-dot ${i === replayStep ? "active" : ""} ${replayVisited.has(f.path) ? "visited" : ""}`}
                      onClick={() => goToReplayStep(i)}
                      title={shortPath(f.path)}
                    />
                  ))}
                </div>
              </div>
              <div className="replay-bar-right">
                <button
                  className="btn replay-btn"
                  onClick={() => goToReplayStep(replayStep - 1)}
                  disabled={replayStep === 0}
                  title="Previous (p / Left Arrow)"
                >
                  &#9664;
                </button>
                <button
                  className="btn replay-btn"
                  onClick={() => goToReplayStep(replayStep + 1)}
                  disabled={replayStep >= selectedGroup.files.length - 1}
                  title="Next (n / Right Arrow / Space)"
                >
                  &#9654;
                </button>
                <button
                  className="btn replay-btn replay-exit"
                  onClick={exitReplay}
                  title="Exit replay (Esc)"
                >
                  &#10005;
                </button>
              </div>
            </div>
          )}
          <div className="panel-body diff-viewer">
            <ErrorBoundary panelName="Diff Viewer">
              <CrashTest panel="Diff Viewer" />
              <DiffViewer
                ref={diffViewerRef}
                fileDiff={fileDiff}
                onCommentRequest={(startLine: number, endLine: number, selectedCode: string) => {
                  const group = selectedGroupRef.current;
                  const file = selectedFileRef.current;
                  if (group && file) {
                    openCommentInput({
                      type: "code",
                      group_id: group.id,
                      file_path: file,
                      start_line: startLine,
                      end_line: endLine,
                      selected_code: selectedCode,
                    });
                  }
                }}
                codeComments={codeCommentsForSelectedFile}
                onGlyphClick={(commentId) => {
                  setActiveCommentId(commentId);
                  setCommentsCollapsed(false);
                  // Scroll the comment card into view in the strip
                  setTimeout(() => {
                    const el = document.querySelector(`.comment-strip-item[data-comment-id="${commentId}"]`);
                    el?.scrollIntoView({ behavior: "smooth", block: "nearest" });
                  }, 100);
                }}
              />
            </ErrorBoundary>
          </div>
          {/* Comment strip below the diff — header + left nav + detail view */}
          {selectedFile && (() => {
            const fileComments = comments.filter(
              (c) => c.file_path === selectedFile && selectedGroup && c.group_id === selectedGroup.id,
            );
            if (fileComments.length === 0) return null;
            return (
              <div className={`comment-strip ${commentsCollapsed ? "comment-strip-collapsed" : ""}`}>
                {/* Header bar with "Comments" label and collapse toggle */}
                <div className="comment-strip-header">
                  <button
                    className="comment-strip-toggle"
                    onClick={() => setCommentsCollapsed(!commentsCollapsed)}
                    aria-expanded={!commentsCollapsed}
                  >
                    <span className="section-toggle-icon">{commentsCollapsed ? "\u25B6" : "\u25BC"}</span>
                    <span>Comments</span>
                    <span className="comment-strip-count">{fileComments.length}</span>
                  </button>
                </div>
                {/* Collapsible body */}
                <div className="comment-strip-body">
                  {/* Left nav — compact pill list for quick toggling */}
                  <div className="comment-strip-nav">
                    {fileComments.map((comment, i) => (
                      <button
                        key={comment.id}
                        className={`comment-strip-nav-item comment-strip-nav-${comment.type} ${activeCommentId === comment.id ? "comment-strip-nav-active" : ""}`}
                        onClick={() => {
                          setActiveCommentId(comment.id);
                          if (comment.start_line != null) {
                            diffViewerRef.current?.scrollToLine(comment.start_line, comment.end_line ?? undefined);
                          }
                          // Scroll corresponding card into view
                          setTimeout(() => {
                            const el = document.querySelector(`.comment-strip-item[data-comment-id="${comment.id}"]`);
                            el?.scrollIntoView({ behavior: "smooth", block: "nearest" });
                          }, 50);
                        }}
                        title={comment.text.slice(0, 60) + (comment.text.length > 60 ? "..." : "")}
                      >
                        <span className="comment-strip-nav-num">{i + 1}</span>
                        {comment.start_line != null && (
                          <span className="comment-strip-nav-line">L{comment.start_line}</span>
                        )}
                      </button>
                    ))}
                  </div>
                  {/* Right detail — scrollable comment cards */}
                  <div className="comment-strip-detail">
                    {fileComments.map((comment) => (
                      <div
                        key={comment.id}
                        data-comment-id={comment.id}
                        className={`comment-strip-item ${activeCommentId === comment.id ? "comment-strip-item-active" : ""}`}
                        onClick={() => {
                          setActiveCommentId(comment.id);
                          if (comment.start_line != null) {
                            diffViewerRef.current?.scrollToLine(comment.start_line, comment.end_line ?? undefined);
                          }
                        }}
                        role={comment.start_line != null ? "button" : undefined}
                        tabIndex={comment.start_line != null ? 0 : undefined}
                      >
                        <div className="comment-strip-meta">
                          <span className={`comment-strip-badge comment-strip-badge-${comment.type}`}>{comment.type}</span>
                          {comment.file_path && (
                            <span className="comment-strip-filepath">{shortPath(comment.file_path)}</span>
                          )}
                          {comment.start_line != null && comment.end_line != null && (
                            <span className="comment-strip-lines">:{comment.start_line}-{comment.end_line}</span>
                          )}
                          <button
                            className="comment-strip-delete"
                            onClick={(e) => { e.stopPropagation(); deleteComment(comment.id); }}
                            title="Delete comment"
                          >
                            &times;
                          </button>
                        </div>
                        {comment.selected_code && (
                          <pre className="comment-strip-code">{comment.selected_code}</pre>
                        )}
                        <p className="comment-strip-text">{comment.text}</p>
                      </div>
                    ))}
                  </div>
                </div>
              </div>
            );
          })()}
        </main>

        {/* Right panel: Annotations & Graph */}
        <aside className="panel panel-right">
          <div className="panel-header">Annotations</div>
          <div className="panel-body">
            {/* Risk heatmap — hidden (Phase 9.4). Component kept for future re-enablement. */}
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
                  {selectedGroup.files.length > 1 && !replayActive && (
                    <button
                      className="btn btn-replay"
                      onClick={enterReplay}
                      title="Step through files in data flow order (r)"
                    >
                      &#9654; Replay Flow
                    </button>
                  )}
                  {replayActive && (
                    <button
                      className="btn btn-replay-exit"
                      onClick={exitReplay}
                      title="Exit replay mode (Esc)"
                    >
                      &#10005; Exit Replay
                    </button>
                  )}
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

                {/* Flow graph (React Flow) — smooth collapse */}
                {selectedGroup.edges.length > 0 && (
                  <div className={`annotation-section flow-graph-section ${graphCollapsed ? "flow-graph-collapsed" : ""}`}>
                    <button
                      className="section-toggle"
                      onClick={() => setGraphCollapsed(!graphCollapsed)}
                      aria-expanded={!graphCollapsed}
                    >
                      <span className="section-toggle-icon">{graphCollapsed ? "\u25B6" : "\u25BC"}</span>
                      <span className="section-toggle-label">Flow Graph</span>
                    </button>
                    <div className="collapsible-body collapsible-graph">
                      <ErrorBoundary panelName="Flow Graph">
                        <CrashTest panel="Flow Graph" />
                        <FlowGraph
                          edges={selectedGroup.edges}
                          files={selectedGroup.files}
                          onNodeClick={handleGraphNodeClick}
                          replayNodeId={replayActive && selectedGroup.files[replayStep] ? selectedGroup.files[replayStep].path : null}
                        />
                      </ErrorBoundary>
                    </div>
                  </div>
                )}

                {/* Edge list — smooth collapse, collapsed by default */}
                {selectedGroup.edges.length > 0 && (
                  <div className={`annotation-section edges-section ${edgesCollapsed ? "edges-collapsed" : ""}`}>
                    <button
                      className="section-toggle"
                      onClick={() => setEdgesCollapsed(!edgesCollapsed)}
                      aria-expanded={!edgesCollapsed}
                    >
                      <span className="section-toggle-icon">{edgesCollapsed ? "\u25B6" : "\u25BC"}</span>
                      <span className="section-toggle-label">Edges</span>
                      <span className="section-toggle-count">{selectedGroup.edges.length}</span>
                    </button>
                    <div className="collapsible-body collapsible-edges">
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
                  </div>
                )}

                {/* Comments now shown in the comment strip below the diff viewer */}

                {/* LLM loading indicators */}
                {annotating && (
                  <div className="annotation-section llm-loading">
                    <span className="spinner" /> Generating overview...
                  </div>
                )}
                {deepAnalyzing && (
                  <div className="annotation-section llm-loading">
                    <span className="spinner" /> Analyzing flow group...
                  </div>
                )}

                {/* LLM action buttons */}
                <div className="annotation-section annotation-actions">
                  {!overview && !annotating && (
                    <button
                      className={`btn btn-summarize ${!hasApiKey ? "no-api-key" : ""}`}
                      onClick={runAnnotateOverview}
                      disabled={annotating || !hasApiKey || !(llmSettings?.annotations_enabled ?? false)}
                      title={
                        hasApiKey
                          ? `Run LLM Pass 1 via ${llmSettings?.provider ?? "anthropic"} (${llmSettings?.model ?? "default"}): generate an overview summary of all flow groups.`
                          : "Requires API key — click the gear icon to configure LLM settings"
                      }
                    >
                      {hasApiKey ? "Summarize PR" : "Summarize PR (API key required)"}
                    </button>
                  )}
                  {!groupDeepAnalysis && !deepAnalyzing && (
                    <button
                      className={`btn btn-analyze-flow ${!hasApiKey ? "no-api-key" : ""}`}
                      onClick={runDeepAnalysis}
                      disabled={deepAnalyzing || !hasApiKey || !(llmSettings?.annotations_enabled ?? false)}
                      title={
                        hasApiKey
                          ? `Run LLM Pass 2 via ${llmSettings?.provider ?? "anthropic"} (${llmSettings?.model ?? "default"}): deep analysis of this flow group.`
                          : "Requires API key — click the gear icon to configure LLM settings"
                      }
                    >
                      {hasApiKey ? "Analyze This Flow" : "Analyze Flow (API key required)"}
                    </button>
                  )}
                  {/* Provider/model indicator */}
                  {hasApiKey && llmSettings && (
                    <span className="llm-provider-badge">
                      {llmSettings.provider}/{llmSettings.model}
                    </span>
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
          {replayActive ? (
            <>
              <span><kbd>n</kbd> / <kbd>&#8594;</kbd> / <kbd>Space</kbd> next step</span>
              <span><kbd>p</kbd> / <kbd>&#8592;</kbd> prev step</span>
              <span><kbd>Esc</kbd> exit replay</span>
            </>
          ) : (
            <>
              <span><kbd>j</kbd> next file</span>
              <span><kbd>k</kbd> prev file</span>
              <span><kbd>J</kbd> next group</span>
              <span><kbd>K</kbd> prev group</span>
              <span><kbd>x</kbd> mark reviewed</span>
              <span><kbd>y</kbd> copy path</span>
              <span><kbd>Y</kbd> copy flow</span>
              <span><kbd>c</kbd> comment</span>
              <span><kbd>C</kbd> copy comments</span>
              <span><kbd>r</kbd> replay flow</span>
            </>
          )}
        </footer>
      )}

      {/* Comment input overlay */}
      {commentInput && (
        <div className="comment-overlay" onClick={cancelComment}>
          <div className="comment-input-panel" onClick={(e) => e.stopPropagation()}>
            <div className="comment-input-header">
              <span className="comment-input-scope">
                {commentInput.type === "code" && commentInput.file_path
                  ? `${shortPath(commentInput.file_path)}:${commentInput.start_line}-${commentInput.end_line}`
                  : commentInput.type === "file" && commentInput.file_path
                    ? shortPath(commentInput.file_path)
                    : "Group comment"}
              </span>
              <button className="btn-close" onClick={cancelComment}>&times;</button>
            </div>
            {commentInput.selected_code && (
              <pre className="comment-input-code">{commentInput.selected_code}</pre>
            )}
            <textarea
              ref={commentInputRef}
              className="comment-textarea"
              placeholder="Add a review comment..."
              value={commentText}
              onChange={(e) => setCommentText(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  submitComment();
                }
                if (e.key === "Escape") {
                  e.preventDefault();
                  cancelComment();
                }
              }}
              rows={3}
            />
            <div className="comment-input-footer">
              <span className="comment-input-hint">Enter to save, Escape to cancel, Shift+Enter for newline</span>
              <button
                className="btn btn-comment-save"
                onClick={submitComment}
                disabled={!commentText.trim()}
              >
                Save
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Context menu (right-click on file) */}
      {contextMenu && (
        <div
          className="context-menu"
          style={{ top: contextMenu.y, left: contextMenu.x }}
          onClick={(e) => e.stopPropagation()}
        >
          <button
            className="context-menu-item"
            onClick={() => {
              copyFilePath(contextMenu.filePath);
              setContextMenu(null);
            }}
          >
            Copy File Path
          </button>
          <button
            className="context-menu-item"
            onClick={() => {
              const group = selectedGroupRef.current;
              if (group) {
                openCommentInput({ type: "file", group_id: group.id, file_path: contextMenu.filePath });
              }
              setContextMenu(null);
            }}
          >
            Add Comment
          </button>
        </div>
      )}

      {/* Toast notification */}
      {toast && (
        <div className="toast">
          <span>{toast}</span>
          <button className="toast-close" onClick={() => setToast(null)} aria-label="Dismiss">&times;</button>
        </div>
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

/** Get a change indicator for a group based on the refinement response. */
function getGroupChangeIndicator(
  group: FlowGroup,
  response: RefinementResponse | null,
): { type: string; label: string; reason: string } | null {
  if (!response) return null;

  // Check if this group was created by a split
  for (const split of response.splits) {
    for (const newGroup of split.new_groups) {
      if (group.name === newGroup.name || group.id.startsWith("group_refined_")) {
        return {
          type: "split",
          label: `split from ${split.source_group_id}`,
          reason: split.reason,
        };
      }
    }
  }

  // Check if this group was created by a merge
  for (const merge of response.merges) {
    if (group.name === merge.merged_name) {
      return {
        type: "merge",
        label: `merged from ${merge.group_ids.join(" + ")}`,
        reason: merge.reason,
      };
    }
  }

  // Check if this group was re-ranked
  for (const reRank of response.re_ranks) {
    if (group.id === reRank.group_id) {
      return {
        type: "rerank",
        label: `re-ranked to #${reRank.new_position}`,
        reason: reRank.reason,
      };
    }
  }

  return null;
}

/** Check if a file was moved by a reclassification. */
function getFileMovedIndicator(
  filePath: string,
  response: RefinementResponse | null,
): { from: string; reason: string } | null {
  if (!response) return null;

  for (const reclass of response.reclassifications) {
    if (reclass.file === filePath) {
      return {
        from: reclass.from_group_id,
        reason: reclass.reason,
      };
    }
  }
  return null;
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
