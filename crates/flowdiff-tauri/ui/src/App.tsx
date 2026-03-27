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
  AsyncLlmJobStart,
  LlmActivityEntry,
  LlmActivityJob,
  RefinementResult,
  RefinementResponse,
  ReviewComment,
  CommentInput,
  InfraSubGroup,
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

const PROVIDER_LABELS: Record<LlmProvider, string> = {
  codex: "Codex CLI",
  claude: "Claude Code",
  anthropic: "Anthropic API",
  openai: "OpenAI API",
  gemini: "Gemini API",
};

type OnboardingStep = "recommended" | "api";
type SubscriptionProvider = "codex" | "claude";
type RightPanelTab = "activity" | "annotations";
type ActivityKind =
  | "system"
  | "search"
  | "read"
  | "command"
  | "reasoning"
  | "result"
  | "warning"
  | "error";

const API_PROVIDER_OPTIONS: LlmProvider[] = ["openai", "anthropic", "gemini"];

const SUBSCRIPTION_BACKENDS: Array<{
  provider: SubscriptionProvider;
  title: string;
  description: string;
  installCommand: string;
  loginCommand: string;
}> = [
  {
    provider: "codex",
    title: "Codex CLI",
    description: "Best path if you already use Codex. flowdiff can reuse that login and let Codex inspect the repo directly.",
    installCommand: "npm install -g @openai/codex",
    loginCommand: "codex login",
  },
  {
    provider: "claude",
    title: "Claude Code",
    description: "Use your Claude Code subscription instead of pasting a separate Anthropic key into every repo.",
    installCommand: "brew install claude-code",
    loginCommand: "claude auth login",
  },
];

function isApiProvider(provider: string): boolean {
  return provider === "anthropic" || provider === "openai" || provider === "gemini";
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
  const [activityJob, setActivityJob] = useState<LlmActivityJob | null>(null);
  const [activityEntries, setActivityEntries] = useState<LlmActivityEntry[]>([]);
  const [activityError, setActivityError] = useState<string | null>(null);
  const activitySourceRef = useRef<EventSource | null>(null);
  const [rightPanelTab, setRightPanelTab] = useState<RightPanelTab>("annotations");

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
  const [aiSetupOpen, setAiSetupOpen] = useState(false);
  const [aiSetupStep, setAiSetupStep] = useState<OnboardingStep>("recommended");
  const [apiProviderDraft, setApiProviderDraft] = useState<LlmProvider>("openai");
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
  const [refinementHadChanges, setRefinementHadChanges] = useState<boolean | null>(null);
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
  const [infraSubGroupsExpanded, setInfraSubGroupsExpanded] = useState<Set<string>>(new Set());

  // Toast notification state (auto-dismiss)
  const [toast, setToast] = useState<string | null>(null);
  const toastTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Demo mode: auto-load mock data on mount when not in Tauri
  const demoLoaded = useRef(false);
  const demoLlmSettingsRef = useRef<LlmSettings>(MOCK_LLM_SETTINGS);
  const aiSetupDismissed = useRef(false);

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
        settings = demoLlmSettingsRef.current;
      }
      setLlmSettings(settings);
      setHasApiKey(settings.has_api_key);
      if (isApiProvider(settings.provider)) {
        setApiProviderDraft(settings.provider as LlmProvider);
      }
    } catch {
      // Non-fatal
    }
  }, []);

  /** Save LLM settings to backend. */
  const saveLlmSettings = useCallback(async (settings: LlmSettings) => {
    setLlmSettings(settings);
    setHasApiKey(settings.has_api_key);
    if (isApiProvider(settings.provider)) {
      setApiProviderDraft(settings.provider as LlmProvider);
    }
    if (!IS_TAURI) {
      demoLlmSettingsRef.current = settings;
      return;
    }
    try {
      await tauriInvoke("save_llm_settings", {
        repoPath: repoPath || "",
        settings,
      });
      // Re-check API key availability after save
      const updated = await tauriInvoke<LlmSettings>("get_llm_settings", {
        repoPath: repoPath || null,
      });
      setLlmSettings(updated);
      setHasApiKey(updated.has_api_key);
      if (isApiProvider(updated.provider)) {
        setApiProviderDraft(updated.provider as LlmProvider);
      }
    } catch {
      // Non-fatal: settings are still applied in-memory
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

  useEffect(() => {
    if (!repoPath) {
      loadLlmSettings(null);
    }
  }, [repoPath, loadLlmSettings]);

  useEffect(() => {
    if (!llmSettings) return;
    if (
      llmSettings.has_api_key
      || llmSettings.codex_authenticated
      || llmSettings.claude_authenticated
    ) {
      setAiSetupOpen(false);
      return;
    }
    if (aiSetupDismissed.current) return;
    setAiSetupOpen(true);
    setAiSetupStep("recommended");
  }, [llmSettings]);

  const closeActivityStream = useCallback(() => {
    if (activitySourceRef.current) {
      activitySourceRef.current.close();
      activitySourceRef.current = null;
    }
  }, []);

  useEffect(() => () => closeActivityStream(), [closeActivityStream]);

  useEffect(() => {
    if (activityJob || activityEntries.length > 0 || activityError) {
      setRightPanelTab("activity");
    }
  }, [activityEntries.length, activityError, activityJob]);

  const appendActivityEntry = useCallback((entry: LlmActivityEntry) => {
    setActivityEntries((prev) => {
      const next = [...prev, entry];
      return next.slice(-80);
    });
  }, []);

  const runMockActivityJob = useCallback(
    async <T,>(
      job: LlmActivityJob,
      entries: Array<Omit<LlmActivityEntry, "timestamp_ms">>,
      result: T,
      onComplete: (value: T) => void,
    ) => {
      closeActivityStream();
      setActivityJob(job);
      setActivityEntries([]);
      setActivityError(null);
      for (const [index, entry] of entries.entries()) {
        await new Promise((resolve) => setTimeout(resolve, index === 0 ? 120 : 220));
        appendActivityEntry({ ...entry, timestamp_ms: Date.now() });
      }
      onComplete(result);
      setActivityJob(null);
    },
    [appendActivityEntry, closeActivityStream],
  );

  const runStreamingJob = useCallback(
    async <T,>(
      command: string,
      args: Record<string, unknown>,
      onComplete: (value: T) => void,
    ) => {
      closeActivityStream();
      setActivityEntries([]);
      setActivityError(null);

      const start = await tauriInvoke<AsyncLlmJobStart>(command, args);
      setActivityJob({
        job_id: start.job_id,
        operation: start.operation,
        provider: start.provider,
        model: start.model,
        title: start.title,
      });

      await new Promise<void>((resolve, reject) => {
        const source = new EventSource(start.stream_url);
        activitySourceRef.current = source;

        source.addEventListener("job_started", (event) => {
          try {
            const payload = JSON.parse((event as MessageEvent).data) as { title: string; provider: string; model: string; job_id: string; operation: string };
            setActivityJob({
              job_id: payload.job_id,
              operation: payload.operation,
              provider: payload.provider,
              model: payload.model,
              title: payload.title,
            });
          } catch {
            // Ignore malformed status events
          }
        });

        source.addEventListener("activity", (event) => {
          try {
            const payload = JSON.parse((event as MessageEvent).data) as { entry: LlmActivityEntry };
            appendActivityEntry(payload.entry);
          } catch {
            // Ignore malformed activity events
          }
        });

        source.addEventListener("completed", (event) => {
          closeActivityStream();
          try {
            const payload = JSON.parse((event as MessageEvent).data) as { result: T };
            onComplete(payload.result);
            setActivityJob(null);
            resolve();
          } catch (error) {
            const message = error instanceof Error ? error.message : String(error);
            setActivityError(message);
            reject(new Error(message));
          }
        });

        source.addEventListener("failed", (event) => {
          closeActivityStream();
          try {
            const payload = JSON.parse((event as MessageEvent).data) as { error: string };
            setActivityError(payload.error);
            appendActivityEntry({
              source: "flowdiff",
              level: "error",
              message: payload.error,
              event_type: "job.failed",
              timestamp_ms: Date.now(),
            });
            setActivityJob(null);
            reject(new Error(payload.error));
          } catch {
            setActivityError("Activity stream failed");
            setActivityJob(null);
            reject(new Error("Activity stream failed"));
          }
        });

        source.onerror = () => {
          closeActivityStream();
          setActivityError("Activity stream disconnected");
          setActivityJob(null);
          reject(new Error("Activity stream disconnected"));
        };
      });
    },
    [appendActivityEntry, closeActivityStream],
  );

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
    closeActivityStream();
    setActivityJob(null);
    setActivityEntries([]);
    setActivityError(null);
    setRightPanelTab("annotations");
    // Reset refinement state
    setOriginalGroups(null);
    setRefinedGroups(null);
    setRefinementResponse(null);
    setRefinementProvider(null);
    setRefinementModel(null);
    setRefinementHadChanges(null);
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
  }, [repoPath, baseRef, handleSelectGroup, closeActivityStream]);

  const recommendedSubscriptionProvider: SubscriptionProvider | null = llmSettings?.codex_authenticated
    ? "codex"
    : llmSettings?.claude_authenticated
      ? "claude"
      : null;
  const resolvedPrimaryProvider = resolveInteractiveProvider(
    llmSettings?.provider ?? null,
    recommendedSubscriptionProvider,
  );
  const resolvedPrimaryModel = resolveInteractiveModel(
    llmSettings?.model ?? null,
    llmSettings?.provider ?? null,
    resolvedPrimaryProvider,
  );
  const resolvedRefinementProvider = resolveInteractiveProvider(
    llmSettings?.refinement_provider ?? llmSettings?.provider ?? null,
    recommendedSubscriptionProvider,
  );
  const resolvedRefinementModel = resolveInteractiveModel(
    llmSettings?.refinement_model ?? llmSettings?.model ?? null,
    llmSettings?.refinement_provider ?? llmSettings?.provider ?? null,
    resolvedRefinementProvider,
  );
  const aiAccessReady = hasApiKey || !!recommendedSubscriptionProvider;
  const annotationsEnabled = (llmSettings?.annotations_enabled ?? false) || !!recommendedSubscriptionProvider;

  /** Run LLM Pass 1: overview annotation for all groups. */
  const runAnnotateOverview = useCallback(async () => {
    setAnnotating(true);
    setError(null);
    try {
      if (IS_TAURI) {
        await runStreamingJob<Pass1Response>("start_annotate_overview", {
          repoPath: repoPath || null,
          llmProvider: resolvedPrimaryProvider,
          llmModel: resolvedPrimaryModel,
        }, (result) => {
          setOverview(result);
        });
      } else {
        await runMockActivityJob<Pass1Response>(
          {
            job_id: "mock-overview",
            operation: "overview",
            provider: resolvedPrimaryProvider ?? "codex",
            model: resolvedPrimaryModel ?? "default",
            title: "Summarizing PR",
          },
          buildMockActivityEntries("overview", resolvedPrimaryProvider ?? "codex"),
          MOCK_PASS1,
          (result) => setOverview(result),
        );
      }
    } catch (e) {
      setError(`Annotation failed: ${String(e)}`);
    } finally {
      setAnnotating(false);
    }
  }, [repoPath, resolvedPrimaryModel, resolvedPrimaryProvider, runMockActivityJob, runStreamingJob]);

  /** Run LLM Pass 2: deep analysis for the selected group. */
  const runDeepAnalysis = useCallback(async () => {
    if (!selectedGroup) return;
    deepAnalyzingCount.current += 1;
    setDeepAnalyzing(true);
    setError(null);
    try {
      if (IS_TAURI) {
        await runStreamingJob<Pass2Response>("start_annotate_group", {
          groupId: selectedGroup.id,
          repoPath,
          base: baseRef || "main",
          head: null,
          range: null,
          staged: false,
          unstaged: false,
          llmProvider: resolvedPrimaryProvider,
          llmModel: resolvedPrimaryModel,
        }, (result) => {
          setDeepAnalyses((prev) => ({ ...prev, [selectedGroup.id]: result }));
        });
      } else {
        await runMockActivityJob<Pass2Response>(
          {
            job_id: `mock-group-${selectedGroup.id}`,
            operation: "group",
            provider: resolvedPrimaryProvider ?? "codex",
            model: resolvedPrimaryModel ?? "default",
            title: `Analyzing ${selectedGroup.id}`,
          },
          buildMockActivityEntries("group", resolvedPrimaryProvider ?? "codex"),
          MOCK_PASS2[selectedGroup.id] || {
            group_id: selectedGroup.id,
            flow_narrative: "No deep analysis available for this group in demo mode.",
            file_annotations: [],
            cross_cutting_concerns: [],
          },
          (result) => setDeepAnalyses((prev) => ({ ...prev, [selectedGroup.id]: result })),
        );
      }
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
  }, [selectedGroup, repoPath, baseRef, resolvedPrimaryModel, resolvedPrimaryProvider, runMockActivityJob, runStreamingJob]);

  /** Show a toast notification that auto-dismisses. */
  const showToast = useCallback((message: string) => {
    if (toastTimer.current) clearTimeout(toastTimer.current);
    setToast(message);
    toastTimer.current = setTimeout(() => setToast(null), 3500);
  }, []);

  const applyRefinementResult = useCallback((result: RefinementResult) => {
    if (!analysis) return;

    if (!originalGroups) {
      setOriginalGroups(analysis.groups);
    }

    setRefinedGroups(result.refined_groups);
    setRefinementResponse(result.refinement_response);
    setRefinementProvider(result.provider);
    setRefinementModel(result.model);
    setRefinementHadChanges(result.had_changes);

    if (result.had_changes) {
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
      setReviewedGroupIds(new Set());
      showToast("Groups updated by refinement \u2014 review state reset");
      const sorted = [...result.refined_groups].sort(
        (a, b) => a.review_order - b.review_order,
      );
      if (sorted.length > 0) {
        handleSelectGroup(sorted[0]);
      }
    } else {
      setShowRefined(false);
      showToast("Refinement kept the existing grouping");
    }
  }, [analysis, originalGroups, handleSelectGroup, showToast]);

  /** Run LLM refinement pass on the current analysis groups. */
  const runRefinement = useCallback(async () => {
    if (!analysis) return;
    setRefining(true);
    setError(null);
    try {
      if (IS_TAURI) {
        await runStreamingJob<RefinementResult>("start_refine_groups", {
          repoPath: repoPath || null,
          llmProvider: resolvedRefinementProvider,
          llmModel: resolvedRefinementModel,
        }, (result) => {
          applyRefinementResult(result);
        });
      } else {
        await runMockActivityJob<RefinementResult>(
          {
            job_id: "mock-refinement",
            operation: "refinement",
            provider: resolvedRefinementProvider ?? "claude",
            model: resolvedRefinementModel ?? "default",
            title: "Refining groups",
          },
          buildMockActivityEntries("refinement", resolvedRefinementProvider ?? "claude"),
          MOCK_REFINEMENT,
          (result) => applyRefinementResult(result),
        );
      }
    } catch (e) {
      setError(`Refinement failed: ${String(e)}`);
    } finally {
      setRefining(false);
    }
  }, [analysis, repoPath, resolvedRefinementModel, resolvedRefinementProvider, runMockActivityJob, runStreamingJob, applyRefinementResult]);

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
      setLlmSettings: (data: LlmSettings) => {
        demoLlmSettingsRef.current = data;
        setLlmSettings(data);
        setHasApiKey(data.has_api_key);
        if (isApiProvider(data.provider)) {
          setApiProviderDraft(data.provider as LlmProvider);
        }
      },
      setAnalysis: (data: AnalysisOutput | null) => { setAnalysis(data); if (data && data.groups.length > 0) { const sorted = [...data.groups].sort((a, b) => a.review_order - b.review_order); handleSelectGroup(sorted[0]); } },
      setError: (msg: string | null) => setError(msg),
      clearAnalysis: () => { setAnalysis(null); setSelectedGroup(null); setSelectedFile(null); setFileDiff(null); setOverview(null); setDeepAnalyses({}); setOriginalGroups(null); setRefinedGroups(null); setRefinementResponse(null); setRefinementProvider(null); setRefinementModel(null); setRefinementHadChanges(null); setShowRefined(false); setReviewedGroupIds(new Set()); setComments([]); setCommentInput(null); setCommentText(""); setRightPanelTab("annotations"); setActivityJob(null); setActivityEntries([]); setActivityError(null); },
      openAiSetup: (step: OnboardingStep = "recommended") => openAiSetup(step),
      dismissAiSetup: () => dismissAiSetup(),
      getAiSetupState: () => ({ open: aiSetupOpen, step: aiSetupStep }),
      enterReplay: () => enterReplay(),
      exitReplay: () => exitReplay(),
      getReplayState: () => ({ active: replayActive, step: replayStep, visited: Array.from(replayVisited) }),
      toggleGroupReviewed: (id: string) => toggleGroupReviewed(id),
      getReviewedGroupIds: () => Array.from(reviewedGroupIds),
      getActivityEntries: () => activityEntries,
      getActivityJob: () => activityJob,
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
        if (isApiProvider(provider)) {
          setApiProviderDraft(provider);
        }
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

  const selectedApiModel = useMemo(
    () => MODELS_BY_PROVIDER[apiProviderDraft]?.[0] ?? "default",
    [apiProviderDraft],
  );

  /** Save an API key to the shared flowdiff config and refresh settings. */
  const handleSaveApiKey = useCallback(async () => {
    const key = apiKeyInput.trim();
    if (!key || !llmSettings) return;
    const updated: LlmSettings = {
      ...llmSettings,
      annotations_enabled: true,
      provider: apiProviderDraft,
      model: selectedApiModel,
      api_key_source: "~/.flowdiff/config.toml",
      has_api_key: true,
      refinement_provider: apiProviderDraft,
      refinement_model: selectedApiModel,
    };
    try {
      await saveLlmSettings(updated);
      if (IS_TAURI) {
        await tauriInvoke("save_api_key", { repoPath: repoPath || "", apiKey: key });
      } else {
        demoLlmSettingsRef.current = updated;
      }
      setApiKeyInput("");
      setAiSetupOpen(false);
      // Refresh settings to pick up the new key
      await loadLlmSettings(repoPath || null);
    } catch {
      setError("Failed to save API key");
    }
  }, [apiKeyInput, apiProviderDraft, llmSettings, repoPath, loadLlmSettings, saveLlmSettings, selectedApiModel]);

  /** Clear the stored API key from the shared flowdiff config and refresh settings. */
  const handleClearApiKey = useCallback(async () => {
    try {
      if (IS_TAURI) {
        await tauriInvoke("clear_api_key", { repoPath: repoPath || "" });
      } else if (llmSettings) {
        const updated: LlmSettings = {
          ...llmSettings,
          api_key_source: "none",
          has_api_key: false,
          annotations_enabled: false,
        };
        demoLlmSettingsRef.current = updated;
        setLlmSettings(updated);
        setHasApiKey(false);
      }
      setApiKeyInput("");
      // Refresh settings to reflect removal
      await loadLlmSettings(repoPath || null);
    } catch {
      setError("Failed to clear API key");
    }
  }, [llmSettings, repoPath, loadLlmSettings]);

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

  const activityProvider = activityJob?.provider
    ?? refinementProvider
    ?? resolvedRefinementProvider
    ?? resolvedPrimaryProvider
    ?? null;
  const activityTimeline = useMemo(
    () => activityEntries.map((entry, index) => ({
      id: `${entry.timestamp_ms}-${index}`,
      entry,
      presentation: describeActivityEntry(entry),
    })),
    [activityEntries],
  );
  const activityStats = useMemo(() => summarizeActivityTimeline(activityTimeline), [activityTimeline]);
  const activitySupportsToolStreaming = providerSupportsToolActivity(activityProvider);
  const refinementVerdict = useMemo(() => {
    if (!refinementResponse || !refinementProvider) return null;
    return {
      provider: refinementProvider,
      model: refinementModel ?? "default",
      hadChanges: refinementHadChanges === true,
      title: refinementHadChanges
        ? "Applied structural changes"
        : "Kept the current grouping",
      reasoning: refinementResponse.reasoning?.trim() || null,
    };
  }, [refinementHadChanges, refinementModel, refinementProvider, refinementResponse]);

  const openAiSetup = useCallback((step: OnboardingStep = "recommended") => {
    aiSetupDismissed.current = false;
    setAiSetupStep(step);
    setAiSetupOpen(true);
    setSettingsOpen(false);
  }, []);

  const dismissAiSetup = useCallback(() => {
    aiSetupDismissed.current = true;
    setAiSetupOpen(false);
  }, []);

  const refreshAiAccess = useCallback(async () => {
    await loadLlmSettings(repoPath || null);
  }, [loadLlmSettings, repoPath]);

  const activateSubscriptionProvider = useCallback(async (provider: SubscriptionProvider) => {
    if (!llmSettings) return;
    const model = MODELS_BY_PROVIDER[provider]?.[0] ?? "default";
    const updated: LlmSettings = {
      ...llmSettings,
      annotations_enabled: true,
      provider,
      model,
      api_key_source: provider === "codex" ? "Codex CLI login" : "Claude Code subscription",
      has_api_key: true,
      refinement_provider: provider,
      refinement_model: model,
    };
    await saveLlmSettings(updated);
    setAiSetupOpen(false);
  }, [llmSettings, saveLlmSettings]);

  const activatePreferredActivityProvider = useCallback(async () => {
    if (!recommendedSubscriptionProvider || !llmSettings) return;
    const model = MODELS_BY_PROVIDER[recommendedSubscriptionProvider]?.[0] ?? "default";
    const updated: LlmSettings = {
      ...llmSettings,
      annotations_enabled: true,
      provider: recommendedSubscriptionProvider,
      model,
      api_key_source: recommendedSubscriptionProvider === "codex" ? "Codex CLI login" : "Claude Code subscription",
      has_api_key: true,
      refinement_provider: recommendedSubscriptionProvider,
      refinement_model: model,
    };
    await saveLlmSettings(updated);
    showToast(`Using ${PROVIDER_LABELS[recommendedSubscriptionProvider]} for live AI jobs`);
  }, [llmSettings, recommendedSubscriptionProvider, saveLlmSettings, showToast]);

  const openApiKeyFallback = useCallback(() => {
    if (llmSettings && isApiProvider(llmSettings.provider)) {
      setApiProviderDraft(llmSettings.provider as LlmProvider);
    } else {
      setApiProviderDraft("openai");
    }
    openAiSetup("api");
  }, [llmSettings, openAiSetup]);

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

  /** Export the overview as a PR-description-style brief. */
  const copyPrDescription = useCallback(async () => {
    if (!overview) {
      showToast("Generate a summary first");
      return;
    }

    const orderedGroups = overview.suggested_review_order
      .map((id) => overview.groups.find((group) => group.id === id))
      .filter((group): group is Pass1GroupAnnotation => Boolean(group));
    const fallbackGroups = overview.groups.filter(
      (group) => !orderedGroups.some((ordered) => ordered.id === group.id),
    );

    const lines = [
      "# Summary",
      "",
      overview.overall_summary,
      "",
      "# Review Flow",
      "",
      ...[...orderedGroups, ...fallbackGroups].flatMap((group) => [
        `- ${group.name}: ${group.summary}`,
      ]),
    ];

    try {
      await navigator.clipboard.writeText(lines.join("\n"));
      showToast("PR description copied to clipboard");
    } catch {
      showToast("Failed to copy PR description");
    }
  }, [overview, showToast]);

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

  const annotationsTabContent = selectedGroup ? (
    <>
      <div className="annotation-section" data-testid="annotations-panel">
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

      {refinementVerdict && (
        <div className="annotation-section refinement-verdict-section" data-testid="refinement-verdict">
          <h3>Refinement Verdict</h3>
          <p className="refinement-verdict-title">{refinementVerdict.title}</p>
          <p className="refinement-verdict-meta">
            {PROVIDER_LABELS[refinementVerdict.provider as LlmProvider] ?? refinementVerdict.provider}/{refinementVerdict.model}
          </p>
          {refinementVerdict.reasoning && (
            <p className="refinement-verdict-reasoning">{refinementVerdict.reasoning}</p>
          )}
        </div>
      )}

      {overview && !groupAnnotation && (
        <div className="annotation-section llm-section">
          <h3>LLM Overview</h3>
          <p className="llm-summary">{overview.overall_summary}</p>
        </div>
      )}

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

      {overview && groupAnnotation && (
        <div className="annotation-section llm-section">
          <h3>Overall Summary</h3>
          <p className="llm-summary">{overview.overall_summary}</p>
        </div>
      )}

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
    </>
  ) : (
    <div className="empty-state">
      Select a group to see annotations.
    </div>
  );

  const activityTabContent = (
    <>
      <div className="annotation-section activity-hero-section" data-testid="activity-panel">
        <div className="activity-hero">
          <div>
            <p className="activity-hero-eyebrow">AI Activity</p>
            <p className="activity-hero-title">
              {activityJob
                ? activityJob.title
                : activityTimeline.length > 0
                  ? "Latest AI run"
                  : "No AI activity yet"}
            </p>
            <p className="activity-hero-subtitle">
              {activityJob
                ? `${PROVIDER_LABELS[activityJob.provider as LlmProvider] ?? activityJob.provider}/${activityJob.model}`
                : activityProvider
                  ? `${PROVIDER_LABELS[activityProvider as LlmProvider] ?? activityProvider}${activitySupportsToolStreaming ? " can stream repo activity live." : " is running in direct API mode."}`
                  : "Run Summarize PR, Analyze This Flow, or Refine to inspect AI work."}
            </p>
          </div>
          {activityJob ? (
            <span className="activity-live-badge">Live</span>
          ) : activityTimeline.length > 0 ? (
            <span className="activity-live-badge activity-live-idle">Saved</span>
          ) : null}
        </div>

        {(activityJob || activityTimeline.length > 0) && (
          <div className="activity-stats" data-testid="activity-stats">
            <div className="activity-stat">
              <span className="activity-stat-value">{activityStats.total}</span>
              <span className="activity-stat-label">events</span>
            </div>
            <div className="activity-stat">
              <span className="activity-stat-value">{activityStats.search}</span>
              <span className="activity-stat-label">search</span>
            </div>
            <div className="activity-stat">
              <span className="activity-stat-value">{activityStats.read}</span>
              <span className="activity-stat-label">reads</span>
            </div>
            <div className="activity-stat">
              <span className="activity-stat-value">{activityStats.command}</span>
              <span className="activity-stat-label">commands</span>
            </div>
          </div>
        )}

        {!activitySupportsToolStreaming && (
          <div className="activity-callout" data-testid="activity-direct-api-note">
            <p className="activity-callout-title">Direct API mode</p>
            <p className="activity-callout-body">
              OpenAI, Anthropic, and Gemini only emit high-level progress. File reads, grep searches,
              and shell steps appear when Flowdiff routes the job through Codex CLI or Claude Code.
            </p>
            {recommendedSubscriptionProvider && (
              <button className="btn" onClick={activatePreferredActivityProvider}>
                Use {PROVIDER_LABELS[recommendedSubscriptionProvider]}
              </button>
            )}
          </div>
        )}

        {activitySupportsToolStreaming && activityProvider && (
          <div className="activity-callout activity-callout-positive">
            <p className="activity-callout-title">Live repo activity enabled</p>
            <p className="activity-callout-body">
              {PROVIDER_LABELS[activityProvider as LlmProvider] ?? activityProvider} can inspect the repo directly,
              so file reads, searches, and shell commands stream here while you wait.
            </p>
          </div>
        )}

        {refinementVerdict && (
          <div className={`activity-callout ${refinementVerdict.hadChanges ? "activity-callout-positive" : ""}`} data-testid="activity-refinement-verdict">
            <p className="activity-callout-title">{refinementVerdict.title}</p>
            <p className="activity-callout-body">
              {PROVIDER_LABELS[refinementVerdict.provider as LlmProvider] ?? refinementVerdict.provider}/{refinementVerdict.model}
            </p>
            {refinementVerdict.reasoning && (
              <p className="activity-callout-body">{refinementVerdict.reasoning}</p>
            )}
          </div>
        )}
      </div>

      <div className="annotation-section activity-section" data-testid="activity-log-panel">
        <div className="activity-log-header">
          <h3>Timeline</h3>
          {activityError && <span className="activity-error-inline">{activityError}</span>}
        </div>
        <div className="activity-log activity-log-rich" data-testid="activity-log">
          {activityTimeline.length === 0 && (
            <div className="activity-empty">
              Activity will appear here once an AI job starts.
            </div>
          )}
          {activityTimeline.map(({ id, entry, presentation }) => (
            <div
              key={id}
              className={`activity-card activity-card-${presentation.kind} activity-${entry.level}`}
              data-testid="activity-entry"
            >
              <div className="activity-card-meta">
                <span className={`activity-kind-badge activity-kind-${presentation.kind}`}>{presentation.badge}</span>
                <span className="activity-source-pill">{presentation.sourceLabel}</span>
                <span className="activity-time">{formatActivityTimestamp(entry.timestamp_ms)}</span>
              </div>
              <p className="activity-card-title">{presentation.title}</p>
              {presentation.detail && (
                <p className="activity-card-detail">{presentation.detail}</p>
              )}
            </div>
          ))}
        </div>
      </div>
    </>
  );

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
          <span className="logo">Flowdiff</span>
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
          {!aiAccessReady && llmSettings && (
            <button
              className="btn btn-ai-setup"
              onClick={() => openAiSetup("recommended")}
              title="Choose Codex CLI, Claude Code, or a direct API key"
            >
              Setup AI
            </button>
          )}
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

      {aiSetupOpen && llmSettings && (
        <div className="ai-setup-overlay" onClick={dismissAiSetup}>
          <div
            className="ai-setup-modal"
            data-testid="ai-onboarding"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="ai-setup-header">
              <div>
                <h2>Set up AI access</h2>
                <p>
                  Start with a repo-aware CLI if you already have one. Only fall back to an API key if you want direct API calls.
                </p>
              </div>
              <button className="btn-close" onClick={dismissAiSetup} title="Close onboarding">
                &times;
              </button>
            </div>
            <div className="ai-setup-body">
              <div className={`ai-setup-status ${aiAccessReady ? "ready" : "missing"}`}>
                <span className="api-key-dot" />
                <span>
                  {aiAccessReady
                    ? `Ready via ${llmSettings.api_key_source}`
                    : "No active AI backend selected yet"}
                </span>
              </div>
              <p className="ai-setup-path">
                Shared config lives in <code>{llmSettings.global_config_path}</code>, so new repos reuse the same setup.
              </p>

              <section className="ai-setup-section">
                <div className="ai-setup-section-header">
                  <h3>Use an existing subscription</h3>
                  <p>If you already use Codex CLI or Claude Code, flowdiff can reuse that login. No separate API key is required.</p>
                </div>
                <div className="ai-backend-grid">
                  {SUBSCRIPTION_BACKENDS.map((backend) => {
                    const available = backend.provider === "codex"
                      ? llmSettings.codex_available
                      : llmSettings.claude_available;
                    const authenticated = backend.provider === "codex"
                      ? llmSettings.codex_authenticated
                      : llmSettings.claude_authenticated;
                    const statusLabel = authenticated
                      ? "Ready"
                      : available
                        ? "Needs login"
                        : "Not found";
                    const statusDetail = authenticated
                      ? `Use ${backend.title} as the primary backend for summaries and flow analysis.`
                      : available
                        ? `Finish setup with \`${backend.loginCommand}\`, then recheck.`
                        : `Install it with \`${backend.installCommand}\`, then recheck.`;

                    return (
                      <article
                        key={backend.provider}
                        className={`ai-backend-card ${authenticated ? "ready" : available ? "login" : "missing"} ${llmSettings.provider === backend.provider ? "selected" : ""}`}
                        data-testid={`ai-card-${backend.provider}`}
                      >
                        <div className="ai-backend-header">
                          <div>
                            <h4>{backend.title}</h4>
                            <p>{backend.description}</p>
                          </div>
                          <span className={`ai-backend-badge ${authenticated ? "ready" : available ? "login" : "missing"}`}>
                            {statusLabel}
                          </span>
                        </div>
                        <p className="ai-backend-detail">{statusDetail}</p>
                        {!authenticated && (
                          <code className="ai-backend-command">
                            {available ? backend.loginCommand : backend.installCommand}
                          </code>
                        )}
                        <div className="ai-backend-actions">
                          {authenticated ? (
                            <button
                              className={`btn ${recommendedSubscriptionProvider === backend.provider ? "btn-primary" : ""}`}
                              onClick={() => activateSubscriptionProvider(backend.provider)}
                            >
                              Use {backend.title}
                            </button>
                          ) : (
                            <button className="btn" onClick={refreshAiAccess}>
                              Recheck
                            </button>
                          )}
                        </div>
                      </article>
                    );
                  })}
                </div>
              </section>

              <section className={`ai-setup-section ai-api-section ${aiSetupStep === "api" ? "expanded" : ""}`}>
                <div className="ai-setup-section-header">
                  <h3>Direct API fallback</h3>
                  <p>Use this only when you want flowdiff to talk to OpenAI, Anthropic, or Gemini directly.</p>
                </div>
                {aiSetupStep !== "api" ? (
                  <button className="btn" onClick={openApiKeyFallback}>
                    Use API key instead
                  </button>
                ) : (
                  <>
                    <div className="settings-row" style={{ marginTop: 0 }}>
                      <label>Provider</label>
                    </div>
                    <select
                      className="settings-select"
                      value={apiProviderDraft}
                      onChange={(e) => setApiProviderDraft(e.target.value as LlmProvider)}
                      data-testid="api-provider-select"
                    >
                      {API_PROVIDER_OPTIONS.map((provider) => (
                        <option key={provider} value={provider}>
                          {PROVIDER_LABELS[provider]}
                        </option>
                      ))}
                    </select>
                    <div className="api-key-input-row">
                      <input
                        type="password"
                        className="settings-input api-key-input"
                        placeholder={`Paste your ${PROVIDER_LABELS[apiProviderDraft]} key`}
                        value={apiKeyInput}
                        onChange={(e) => setApiKeyInput(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter" && apiKeyInput.trim()) {
                            handleSaveApiKey();
                          }
                        }}
                        data-testid="api-key-input"
                      />
                      <button
                        className="btn btn-save-key"
                        disabled={!apiKeyInput.trim()}
                        onClick={handleSaveApiKey}
                        data-testid="api-key-save"
                      >
                        Save and continue
                      </button>
                    </div>
                    <p className="settings-hint">
                      flowdiff will save the provider choice and key globally, then reuse it for future repos.
                    </p>
                  </>
                )}
              </section>
            </div>
            <div className="ai-setup-footer">
              <button className="btn" onClick={dismissAiSetup}>
                Continue without AI
              </button>
              <button className="btn" onClick={refreshAiAccess}>
                Recheck setup
              </button>
            </div>
          </div>
        </div>
      )}

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
              {/* LLM Access / Onboarding */}
              <div className="settings-section">
                <div className="settings-section-title-row">
                  <h3>AI Access</h3>
                  <button className="btn btn-inline-setup" onClick={() => openAiSetup("recommended")}>
                    Open setup flow
                  </button>
                </div>
                <div className={`api-key-status ${aiAccessReady ? "configured" : "missing"}`}>
                  <span className="api-key-dot" />
                  <span>
                    {aiAccessReady
                      ? `Ready via ${recommendedSubscriptionProvider
                        ? PROVIDER_LABELS[recommendedSubscriptionProvider]
                        : llmSettings.api_key_source}`
                      : "Not configured yet"}
                  </span>
                </div>
                <p className="settings-hint">
                  Saved globally in <code>{llmSettings.global_config_path}</code>, so new projects reuse the same setup.
                </p>
                <p className="settings-hint">
                  Prefer Codex CLI or Claude Code if you already use them. Direct API keys are the fallback path.
                </p>
                {resolvedPrimaryProvider && (
                  <p className="settings-hint">
                    Effective summary backend on this machine: <strong>{PROVIDER_LABELS[resolvedPrimaryProvider as LlmProvider]}/{resolvedPrimaryModel ?? "default"}</strong>
                  </p>
                )}
                {llmSettings.refinement_enabled && resolvedRefinementProvider && (
                  <p className="settings-hint">
                    Effective refinement backend on this machine: <strong>{PROVIDER_LABELS[resolvedRefinementProvider as LlmProvider]}/{resolvedRefinementModel ?? "default"}</strong>
                  </p>
                )}
                {recommendedSubscriptionProvider && isApiProvider(llmSettings.provider) && (
                  <p className="settings-hint">
                    Flowdiff will prefer {PROVIDER_LABELS[recommendedSubscriptionProvider]} for live jobs on this machine,
                    so file reads, greps, and shell commands can stream into the Activity tab while API keys stay available
                    as fallback.
                  </p>
                )}
                <p className="settings-hint">
                  Codex CLI: {llmSettings.codex_authenticated ? "ready" : llmSettings.codex_available ? "installed, needs login" : "not found"}
                  {" "}· Claude Code: {llmSettings.claude_authenticated ? "ready" : llmSettings.claude_available ? "installed, needs login" : "not found"}
                </p>
                <div className="settings-row">
                  <label>Primary backend</label>
                </div>
                <select
                  className="settings-select"
                  value={llmSettings.provider}
                  onChange={(e) => updateSetting("provider", e.target.value)}
                >
                  {LLM_PROVIDERS.map((p) => (
                    <option key={p} value={p}>
                      {PROVIDER_LABELS[p]}
                    </option>
                  ))}
                </select>
                <div className="settings-row" style={{ marginTop: 12 }}>
                  <label>Model</label>
                </div>
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
                {!isApiProvider(resolvedPrimaryProvider ?? llmSettings.provider) && (
                  <p className="settings-hint">
                    No API key needed here. flowdiff will call {PROVIDER_LABELS[(resolvedPrimaryProvider ?? llmSettings.provider) as LlmProvider]}
                    {" "}inside the repo so it can inspect the filesystem before producing structured output.
                  </p>
                )}
                {isApiProvider(llmSettings.provider) && (
                  <>
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
                        title="Save API key to ~/.flowdiff/config.toml"
                      >
                        Save
                      </button>
                      {llmSettings.api_key_source === "~/.flowdiff/config.toml" && (
                        <button
                          className="btn btn-clear-key"
                          onClick={handleClearApiKey}
                          title="Remove stored API key"
                        >
                          Clear
                        </button>
                      )}
                    </div>
                    <p className="settings-hint">
                      New users can usually skip keys by using Codex CLI or Claude Code if they are already signed in.
                      If you prefer direct API calls, paste a key above, set <code>FLOWDIFF_API_KEY</code>, a provider-specific
                      env var (<code>ANTHROPIC_API_KEY</code>, <code>OPENAI_API_KEY</code>, <code>GEMINI_API_KEY</code>), or
                      configure <code>key_cmd</code> in <code>~/.flowdiff/config.toml</code>.
                    </p>
                    {recommendedSubscriptionProvider && (
                      <button className="btn" onClick={activatePreferredActivityProvider}>
                        Use {PROVIDER_LABELS[recommendedSubscriptionProvider]} instead
                      </button>
                    )}
                  </>
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
                  When enabled, flowdiff can generate a PR-ready summary plus deeper flow analysis.
                </p>
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
                  Refines deterministic groupings using an LLM pass. This is still useful with Codex CLI or Claude Code:
                  the provider changes, but the refinement step is what decides whether the deterministic groups should be
                  split, merged, re-ranked, or kept as-is.
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
                            {PROVIDER_LABELS[p]}
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
            {/* Refinement banner — shown after analysis when LLM access is available */}
            {analysis && !refinedGroups && !refining && aiAccessReady && llmSettings?.refinement_enabled && (
              <div className="refinement-banner">
                <span>AI can improve these groupings</span>
                <button
                  className="btn btn-refine"
                  onClick={runRefinement}
                  title={`Refine groupings using ${resolvedRefinementProvider ?? "anthropic"} (${resolvedRefinementModel ?? "default"})`}
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
                  Refining with {resolvedRefinementProvider ?? "anthropic"}/{resolvedRefinementModel ?? "..."}
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
            {/* Infrastructure group — collapsed by default, shows count, with sub-groups */}
            {analysis?.infrastructure_group && analysis.infrastructure_group.files.length > 0 && (
              <div className="group-item infra-group">
                <div
                  className="group-header"
                  style={{ cursor: "pointer" }}
                  onClick={() => setInfraExpanded((prev) => !prev)}
                >
                  <span className="group-name">
                    Ungrouped
                  </span>
                  <span className="risk-badge" data-risk="low">
                    {analysis.infrastructure_group.files.length} files
                  </span>
                  <span style={{ marginLeft: 4, fontSize: 10, opacity: 0.6 }}>
                    {infraExpanded ? "\u25B2" : "\u25BC"}
                  </span>
                </div>
                {infraExpanded && (
                  <>
                    {analysis.infrastructure_group.sub_groups && analysis.infrastructure_group.sub_groups.length > 0 ? (
                      analysis.infrastructure_group.sub_groups.map((sg: InfraSubGroup) => {
                        const isSubExpanded = infraSubGroupsExpanded.has(sg.name);
                        return (
                          <div key={sg.name} className="infra-sub-group">
                            <div
                              className="infra-sub-group-header"
                              onClick={(e) => {
                                e.stopPropagation();
                                setInfraSubGroupsExpanded((prev) => {
                                  const next = new Set(prev);
                                  if (next.has(sg.name)) {
                                    next.delete(sg.name);
                                  } else {
                                    next.add(sg.name);
                                  }
                                  return next;
                                });
                              }}
                            >
                              <span style={{ fontSize: 10, opacity: 0.6, marginRight: 4 }}>
                                {isSubExpanded ? "\u25BC" : "\u25B6"}
                              </span>
                              <span className="infra-sub-group-name">{sg.name}</span>
                              <span className="infra-sub-group-count">
                                {sg.files.length} file{sg.files.length === 1 ? "" : "s"}
                              </span>
                            </div>
                            {isSubExpanded && (
                              <ul className="file-list">
                                {sg.files.map((f) => (
                                  <li
                                    key={f}
                                    className={`file-item ${selectedFile === f ? "selected" : ""}`}
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      handleSelectFile(f);
                                    }}
                                  >
                                    <span className="file-path">{shortPath(f)}</span>
                                  </li>
                                ))}
                              </ul>
                            )}
                          </div>
                        );
                      })
                    ) : (
                      <ul className="file-list">
                        {(infraShowAll
                          ? analysis.infrastructure_group.files
                          : analysis.infrastructure_group.files.slice(0, 50)
                        ).map((f) => (
                          <li
                            key={f}
                            className={`file-item ${selectedFile === f ? "selected" : ""}`}
                            onClick={(e) => {
                              e.stopPropagation();
                              handleSelectFile(f);
                            }}
                          >
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
                  </>
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

        {/* Right panel: Activity + Annotations */}
        <aside className="panel panel-right">
          <div className="panel-header panel-header-tabs" data-testid="right-panel-tabs">
            <button
              className={`panel-tab ${rightPanelTab === "activity" ? "active" : ""}`}
              onClick={() => setRightPanelTab("activity")}
              data-testid="activity-tab"
            >
              Activity
              {(activityJob || activityTimeline.length > 0) && (
                <span className="panel-tab-count">{activityTimeline.length || 1}</span>
              )}
            </button>
            <button
              className={`panel-tab ${rightPanelTab === "annotations" ? "active" : ""}`}
              onClick={() => setRightPanelTab("annotations")}
              data-testid="annotations-tab"
            >
              Annotations
            </button>
          </div>
          <div className="panel-body panel-body-right">
            {rightPanelTab === "activity" ? activityTabContent : annotationsTabContent}

            {(annotating || deepAnalyzing || refining) && rightPanelTab === "activity" && (
              <div className="annotation-section llm-loading">
                <span className="spinner" />
                {annotating
                  ? "Generating overview..."
                  : deepAnalyzing
                    ? "Analyzing flow group..."
                    : "Refining groups..."}
              </div>
            )}

            {!aiAccessReady && llmSettings && (
              <div className="annotation-section llm-loading llm-setup-cta">
                <span>Connect {PROVIDER_LABELS.codex}, {PROVIDER_LABELS.claude}, or a direct API key to unlock summaries and refinement.</span>
                <button className="btn" onClick={() => openAiSetup("recommended")}>
                  Setup AI
                </button>
              </div>
            )}

            {selectedGroup && (
              <div className="annotation-section annotation-actions">
                {overview && !annotating && (
                  <button
                    className="btn btn-copy-comments-footer"
                    onClick={copyPrDescription}
                    title="Copy the generated summary as a PR description"
                  >
                    Copy PR Description
                  </button>
                )}
                {!overview && !annotating && (
                  <button
                    className={`btn btn-summarize ${!aiAccessReady ? "no-api-key" : ""}`}
                    onClick={runAnnotateOverview}
                    disabled={annotating || !aiAccessReady || !annotationsEnabled}
                    title={
                      aiAccessReady
                        ? `Run LLM Pass 1 via ${resolvedPrimaryProvider ?? "codex"} (${resolvedPrimaryModel ?? "default"}): generate an overview summary of all flow groups.`
                        : "AI setup required — choose Codex CLI, Claude Code, or a direct API key"
                    }
                  >
                    {aiAccessReady ? "Summarize PR" : "Summarize PR (Setup required)"}
                  </button>
                )}
                {!groupDeepAnalysis && !deepAnalyzing && (
                  <button
                    className={`btn btn-analyze-flow ${!aiAccessReady ? "no-api-key" : ""}`}
                    onClick={runDeepAnalysis}
                    disabled={deepAnalyzing || !aiAccessReady || !annotationsEnabled}
                    title={
                      aiAccessReady
                        ? `Run LLM Pass 2 via ${resolvedPrimaryProvider ?? "codex"} (${resolvedPrimaryModel ?? "default"}): deep analysis of this flow group.`
                        : "AI setup required — choose Codex CLI, Claude Code, or a direct API key"
                    }
                  >
                    {aiAccessReady ? "Analyze This Flow" : "Analyze Flow (Setup required)"}
                  </button>
                )}
                {aiAccessReady && resolvedPrimaryProvider && (
                  <span className="llm-provider-badge">
                    {PROVIDER_LABELS[resolvedPrimaryProvider as LlmProvider]}/{resolvedPrimaryModel ?? "default"}
                  </span>
                )}
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

interface ActivityPresentation {
  kind: ActivityKind;
  badge: string;
  title: string;
  detail?: string;
  sourceLabel: string;
}

function providerSupportsToolActivity(provider: string | null | undefined): boolean {
  return provider === "codex" || provider === "claude";
}

function resolveInteractiveProvider(
  configuredProvider: string | null,
  preferredProvider: SubscriptionProvider | null,
): LlmProvider | null {
  if (preferredProvider && configuredProvider && isApiProvider(configuredProvider)) {
    return preferredProvider;
  }
  if (configuredProvider && LLM_PROVIDERS.includes(configuredProvider as LlmProvider)) {
    return configuredProvider as LlmProvider;
  }
  return preferredProvider;
}

function resolveInteractiveModel(
  configuredModel: string | null,
  configuredProvider: string | null,
  resolvedProvider: LlmProvider | null,
): string | null {
  if (!resolvedProvider) return configuredModel;
  if (configuredProvider === resolvedProvider && configuredModel) {
    return configuredModel;
  }
  return MODELS_BY_PROVIDER[resolvedProvider]?.[0] ?? configuredModel ?? "default";
}

function buildMockActivityEntries(
  operation: "overview" | "group" | "refinement",
  provider: string,
): Array<Omit<LlmActivityEntry, "timestamp_ms">> {
  const toolBacked = providerSupportsToolActivity(provider);
  const source = toolBacked ? provider : "flowdiff";
  const providerName = provider === "claude" ? "Claude" : "Codex";

  const sharedStart: Array<Omit<LlmActivityEntry, "timestamp_ms">> = [
    {
      source: "flowdiff",
      level: "info",
      message: `Preparing ${operation === "group" ? "deep analysis" : operation} request`,
      event_type: "flowdiff.prepare",
    },
  ];

  if (!toolBacked) {
    return [
      ...sharedStart,
      {
        source: "flowdiff",
        level: "info",
        message: "Direct API mode only streams high-level progress. Switch to Codex CLI or Claude Code for file reads, grep, and shell activity.",
        event_type: "flowdiff.direct_api",
      },
      {
        source: provider,
        level: "info",
        message: operation === "refinement" ? "Reviewing current groups and producing a structured verdict" : "Submitting structured request to the API provider",
        event_type: "provider.request",
      },
      {
        source: provider,
        level: "info",
        message: operation === "refinement" ? "Refinement rationale: keep the current grouping because the changed files already form coherent review flows." : "Structured response ready",
        event_type: "provider.result",
      },
    ];
  }

  if (operation === "overview") {
    return [
      ...sharedStart,
      {
        source,
        level: "info",
        message: `${providerName} is running rg --files crates/flowdiff-tauri/ui/src`,
        event_type: "stdout.command_execution",
      },
      {
        source,
        level: "info",
        message: `${providerName} is running sed -n '1,240p' crates/flowdiff-tauri/ui/src/App.tsx`,
        event_type: "stdout.command_execution",
      },
      {
        source,
        level: "info",
        message: "Writing PR-ready summary",
        event_type: "provider.summary",
      },
    ];
  }

  if (operation === "group") {
    return [
      ...sharedStart,
      {
        source,
        level: "info",
        message: `${providerName} is running rg \"UserService|CreateUserInput\" -n crates/flowdiff-tauri/ui/src`,
        event_type: "stdout.command_execution",
      },
      {
        source,
        level: "info",
        message: `${providerName} is running sed -n '1,220p' crates/flowdiff-tauri/ui/src/mock.ts`,
        event_type: "stdout.command_execution",
      },
      {
        source,
        level: "info",
        message: "Assembling cross-cutting concerns",
        event_type: "provider.cross_cutting",
      },
    ];
  }

  return [
    ...sharedStart,
    {
      source,
      level: "info",
      message: `${providerName} is running rg --files /demo/repo | head -n 40`,
      event_type: "stdout.command_execution",
    },
    {
      source,
      level: "info",
      message: `${providerName} is running sed -n '1,220p' /demo/repo/src/routes/users.ts`,
      event_type: "stdout.command_execution",
    },
    {
      source,
      level: "info",
      message: "Refinement rationale: keep the current grouping because the changed files already form coherent review flows.",
      event_type: "refinement.reasoning",
    },
  ];
}

function describeActivityEntry(entry: LlmActivityEntry): ActivityPresentation {
  const sourceLabel = activitySourceLabel(entry.source);
  const message = entry.message.trim();
  const lowerMessage = message.toLowerCase();
  const eventType = entry.event_type?.toLowerCase() ?? "";
  const command = extractCommandFromActivity(message);

  if (entry.level === "error") {
    return { kind: "error", badge: "ERROR", title: message, sourceLabel };
  }

  if (entry.level === "warning") {
    return { kind: "warning", badge: "WARN", title: message, sourceLabel };
  }

  if (command) {
    if (isSearchCommand(command) || eventType.includes("search") || eventType.includes("grep")) {
      return {
        kind: "search",
        badge: "SEARCH",
        title: lowerMessage.includes("finished") ? "Repo search finished" : "Searching the repo",
        detail: command,
        sourceLabel,
      };
    }
    if (isReadCommand(command) || eventType.includes("read") || eventType.includes("open") || eventType.includes("view")) {
      return {
        kind: "read",
        badge: "READ",
        title: lowerMessage.includes("finished") ? "Finished reading files" : "Reading files",
        detail: command,
        sourceLabel,
      };
    }
    return {
      kind: "command",
      badge: "CMD",
      title: lowerMessage.includes("finished") ? "Shell command finished" : "Running shell command",
      detail: command,
      sourceLabel,
    };
  }

  if (eventType.includes("tool_use")) {
    const toolName = eventType.split(".").pop() ?? "tool";
    if (isSearchCommand(toolName)) {
      return { kind: "search", badge: "SEARCH", title: `${sourceLabel} searched the repo`, detail: toolName, sourceLabel };
    }
    if (isReadCommand(toolName)) {
      return { kind: "read", badge: "READ", title: `${sourceLabel} inspected files`, detail: toolName, sourceLabel };
    }
    return { kind: "command", badge: "TOOL", title: `${sourceLabel} used ${toolName}`, sourceLabel };
  }

  if (lowerMessage.includes("reasoning") || eventType.includes("thinking")) {
    return {
      kind: "reasoning",
      badge: "THINK",
      title: `${sourceLabel} is reasoning`,
      detail: message === "Claude is reasoning" ? undefined : message,
      sourceLabel,
    };
  }

  if (
    lowerMessage.includes("structured response ready")
    || lowerMessage.includes("prepared structured output")
    || lowerMessage.includes("writing pr-ready summary")
    || lowerMessage.includes("completed the response")
    || lowerMessage.includes("finished its turn")
  ) {
    return {
      kind: "result",
      badge: "RESULT",
      title: message,
      sourceLabel,
    };
  }

  if (lowerMessage.includes("preparing")) {
    return { kind: "system", badge: "PLAN", title: message, sourceLabel };
  }

  if (lowerMessage.includes("using ")) {
    return { kind: "system", badge: "MODEL", title: message, sourceLabel };
  }

  return { kind: "system", badge: "STEP", title: message, sourceLabel };
}

function summarizeActivityTimeline(
  timeline: Array<{ presentation: ActivityPresentation }>,
): { total: number; search: number; read: number; command: number } {
  return timeline.reduce(
    (summary, item) => {
      summary.total += 1;
      if (item.presentation.kind === "search") summary.search += 1;
      if (item.presentation.kind === "read") summary.read += 1;
      if (item.presentation.kind === "command") summary.command += 1;
      return summary;
    },
    { total: 0, search: 0, read: 0, command: 0 },
  );
}

function formatActivityTimestamp(timestampMs: number): string {
  return new Date(timestampMs).toLocaleTimeString([], {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

function activitySourceLabel(source: string): string {
  if (source in PROVIDER_LABELS) {
    return PROVIDER_LABELS[source as LlmProvider];
  }
  if (source.toLowerCase() === "flowdiff") {
    return "Flowdiff";
  }
  return source.toUpperCase();
}

function extractCommandFromActivity(message: string): string | null {
  const match = message.match(/(?:is running|finished)\s+(.+)$/i);
  return match?.[1]?.trim() ?? null;
}

function isSearchCommand(command: string): boolean {
  const lower = command.toLowerCase();
  return lower.includes("rg")
    || lower.includes("grep")
    || lower.includes("fd")
    || lower.includes("find")
    || lower.includes("glob")
    || lower.includes("search");
}

function isReadCommand(command: string): boolean {
  const lower = command.toLowerCase();
  return lower.includes("cat")
    || lower.includes("sed")
    || lower.includes("head")
    || lower.includes("tail")
    || lower.includes("read")
    || lower.includes("open")
    || lower.includes("view");
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
