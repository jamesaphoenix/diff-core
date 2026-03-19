# flowdiff — Specification

## Context

When AI agents modify 50–100 files in a single PR, existing diff tools (VS Code, Beyond Compare, GitHub) present changes as a flat file list. This forces reviewers to mentally reconstruct data flow, architectural impact, and causal ordering — the most cognitively expensive part of code review.

**flowdiff** solves this by transforming flat file diffs into ranked, semantically grouped review flows. It answers: "what changed, in what order should I review it, and how does data flow through the changes?"

Two modes:
- **Deterministic**: static analysis only — graph construction, flow grouping, ranking
- **LLM-annotated**: reasoning models narrate over the deterministic graph (BYOK)

---

## 1. Product Overview

| Field | Value |
|-------|-------|
| Name | `flowdiff` |
| Language | Rust |
| UIs | Tauri desktop app + VS Code extension |
| Target user | Solo developer (James), architect for eventual open-source |
| License | TBD |

### Problem Statement

AI coding agents produce large, semantically entangled diffs. Current diff tools show **ΔF = {changed files}** — an unordered set. Humans need **ranked paths through a dependency graph G** — ordered sequences that follow data flow, not filesystem structure.

### Core Insight

Diff review is a **graph problem**, not a **set problem**. The right primitive is not "file A changed" but "request enters here → transformed here → validated here → persisted here → emitted here → rendered here."

## 2. Architecture

```
┌─────────────────────────────────────────────────┐
│                   flowdiff CLI                   │
│                  (Rust binary)                   │
├─────────────────────────────────────────────────┤
│  Git Layer     │ Diff extraction (git2)          │
│  AST Layer     │ Tree-sitter (all languages)     │
│  Graph Layer   │ Symbol graph (petgraph)         │
│  Flow Layer    │ Data flow tracing + heuristics  │
│  Cluster Layer │ Semantic grouping               │
│  Rank Layer    │ Review ordering + scoring       │
│  LLM Layer     │ OpenAI + Anthropic (optional)   │
│  Export Layer  │ JSON output                     │
├─────────────────────────────────────────────────┤
│  IPC: JSON over stdin/stdout or local socket     │
├──────────────────────┬──────────────────────────┤
│   Tauri App          │   VS Code Extension      │
│   (Three-panel UI)   │   (Thin shell over CLI)  │
│   Monaco diff viewer │   Webview + tree views   │
└──────────────────────┴──────────────────────────┘
```

### Component Responsibilities

**Rust Core (CLI + library)**
- All analysis logic lives here — single source of truth
- Exposes both CLI interface and library API (for Tauri)
- Stateless per invocation (no daemon required for v1)

**Tauri App**
- Three-panel layout: flow groups | Monaco diff viewer | annotations/graph
- Calls Rust core directly via Tauri commands (no CLI subprocess)
- Renders Mermaid diagrams for flow visualization

**VS Code Extension**
- Thin TypeScript shell
- Spawns `flowdiff` CLI binary, parses JSON output
- Renders results in webviews and tree views
- Opens VS Code's native diff editor for file-level review

## 3. Diff Input Sources

All input is git-based. Four modes:

| Mode | CLI Flag | Description |
|------|----------|-------------|
| PR preview (default) | `--pr` or no flags | Merge-base diff: `main...HEAD` — shows everything on this branch that's ahead of main. The "what would my PR look like?" mode. Auto-detects current branch and default branch |
| Branch comparison | `--base main --head feature` | Compare two refs |
| Commit range | `--range HEAD~5..HEAD` | Review a range of commits |
| Working tree | `--staged` / `--unstaged` | Review uncommitted changes |

Implementation: `git2` crate for all git operations. No shelling out to `git`. PR preview mode uses `git2::Repository::merge_base()` to find the common ancestor, then diffs from there to HEAD.

## 4. Analysis Pipeline

### 4.1 Diff Extraction

```
git diff → list of (file_path, old_content, new_content, hunks)
```

For each changed file, extract:
- File path (old and new, handling renames)
- Full old/new content for AST parsing
- Hunk-level changes for precise change localization

### 4.2 AST Parsing (Tree-sitter)

Parse both old and new versions of each changed file using tree-sitter.

Tree-sitter supports 100+ languages via community grammars. No language-specific code needed for basic AST extraction.

Extract from each file:
- **Definitions**: functions, classes, structs, interfaces, type aliases, constants
- **Imports/exports**: what each file depends on and provides
- **Call expressions**: which functions are called and where
- **Changed symbols**: which definitions were added/modified/removed

### 4.3 Symbol Graph Construction (petgraph)

Build a directed graph `G = (V, E)` where:

**Vertices (V)**: symbols (functions, classes, types, modules)

**Edges (E)** with types:
- `imports(A, B)` — A imports from B
- `calls(A, B)` — function A calls function B
- `extends(A, B)` — class/type A extends B
- `instantiates(A, B)` — A creates an instance of B
- `reads(A, D)` — function A reads from data source D
- `writes(A, D)` — function A writes to data source D
- `emits(A, E)` — function A emits event E
- `handles(A, E)` — function A handles event E

### 4.4 Full Data Flow Tracing

Go beyond import graphs — trace how data moves through the system:

**Static tracing:**
- Follow function parameters and return types across call chains
- Track variable assignments that flow into function calls
- Resolve type signatures to connect producers and consumers

**Heuristic inference:**
- If function A calls function B and B contains a DB write pattern (e.g., `.save()`, `.insert()`, `INSERT INTO`), mark edge as `persistence`
- If a function matches HTTP handler patterns (decorators, router registrations), mark as `entrypoint`
- If a function publishes to a queue/event bus, mark as `emission`
- If a function reads from env/config, mark as `configuration`

**Framework pattern detection (optional, auto-detected):**
- Express/Fastify/Next.js route handlers
- FastAPI/Flask/Django view functions
- React component trees and prop drilling
- Prisma/SQLAlchemy/TypeORM model usage
- Message queue producers/consumers
- Effect.ts `HttpApi`/`HttpApiEndpoint`/`HttpApiGroup` routes, `Layer`/`Effect.Service`/`Context.Tag` services, `@effect/sql` Drizzle integration, `@effect/cli` commands

### 4.5 Entrypoint Detection

Automatically detect entry points into the application:

| Type | Detection heuristic |
|------|-------------------|
| HTTP routes | Decorator patterns, router registrations, file-based routing |
| HTTP routes (Effect.ts) | `HttpApi`, `HttpApiEndpoint`, `HttpApiGroup`, `HttpRouter`, `HttpServer` patterns |
| CLI commands | `main()`, argument parser setup, `bin` entries |
| CLI commands (Effect.ts) | `@effect/cli` `Command`, `Args`, `Options` patterns |
| Queue consumers | Message handler registrations, subscriber patterns |
| Queue consumers (Effect.ts) | Effect `Queue`, `PubSub` consumer patterns |
| Cron jobs | Scheduler registrations, cron expressions |
| Cron jobs (Effect.ts) | Effect `Schedule`, `@effect/cron` patterns |
| React pages | Default exports from page/route directories |
| Test files | Test function/describe block patterns |
| Test files (Effect.ts) | `@effect/vitest` `it.effect`, `it.scoped`, `describe` patterns |
| Event handlers | Event listener registrations |
| Event handlers (Effect.ts) | Effect `Stream`, `PubSub`, `Hub` listener patterns |
| Effect.ts Services | `Effect.Service`, `Context.Tag`, `Layer` definitions — primary service/DI entrypoints |

### 4.6 Semantic Clustering

Group changed files into "flow groups" — sets of files that participate in the same logical data flow.

**Algorithm:**

1. For each detected entrypoint in the changed set, compute its **forward reachability** in graph G (BFS/DFS following call/import/data edges)
2. Intersect each reachability set with the changed file set ΔF
3. Files reachable from the same entrypoint and in ΔF belong to the same flow group
4. Files in ΔF not reachable from any entrypoint form an "infrastructure/shared" group
5. Files reachable from multiple entrypoints get assigned to the group where they have the shortest path distance

**Output:** `FlowGroup[]` where each group has:
- `id: string`
- `name: string` (auto-generated, e.g., "POST /api/users handler chain")
- `entrypoint: Symbol | null`
- `files: FileChange[]` (ordered by flow position)
- `edges: Edge[]` (internal edges within the group)
- `risk_score: f64`

### 4.7 Review Ranking

Rank flow groups for review order using a composite score:

```
score(group) = w₁·risk + w₂·centrality + w₃·surface_area + w₄·uncertainty
```

Where:
- **risk** = schema changes, public API changes, auth/security-related, DB migrations → higher risk
- **centrality** = PageRank or betweenness centrality of changed nodes in G → more central = review first
- **surface_area** = number of changed lines / files in the group
- **uncertainty** = inverse of test coverage overlap, number of heuristic (vs static) edges

Within each group, files are ordered by **flow position** — entrypoint first, then downstream in data flow order.

Default weights: `w₁=0.35, w₂=0.25, w₃=0.20, w₄=0.20`

## 5. LLM-Annotated Mode

### 5.1 Provider Support

| Provider | API | Models |
|----------|-----|--------|
| Anthropic | Messages API | Claude reasoning models (claude-3-7-sonnet with extended thinking, future reasoning models) |
| Google | Gemini API | Gemini 2.5 Pro, Gemini 2.5 Flash |
| OpenAI | Chat Completions API | o1, o3-mini, o3, GPT-4o |

BYOK (Bring Your Own Key): user provides API key via `.flowdiff.toml` or environment variable.

**Structured outputs** used for all LLM responses — typed JSON schemas ensure parseable, consistent annotations.

### 5.2 Two-Pass Architecture

**Pass 1: Overview (automatic on request)**
- Input: full diff summary + deterministic flow groups + graph structure
- Output (structured):
  ```json
  {
    "groups": [
      {
        "id": "group_1",
        "name": "User authentication token refresh",
        "summary": "Changes the token refresh flow to use rotating refresh tokens...",
        "review_order_rationale": "Review first — changes auth contract that downstream groups depend on",
        "risk_flags": ["auth_change", "breaking_api"]
      }
    ],
    "overall_summary": "...",
    "suggested_review_order": ["group_1", "group_3", "group_2"]
  }
  ```

**Pass 2: Deep analysis (on-demand, per-group)**
- Input: full file contents + diffs for one group + graph context
- Output (structured):
  ```json
  {
    "group_id": "group_1",
    "flow_narrative": "Data enters at POST /auth/refresh, validated by...",
    "file_annotations": [
      {
        "file": "src/handlers/auth.rs",
        "role_in_flow": "Entrypoint — receives refresh token request",
        "changes_summary": "Added rotation logic...",
        "risks": ["Token invalidation race condition if..."],
        "suggestions": ["Consider adding a mutex on..."]
      }
    ],
    "cross_cutting_concerns": ["Error handling path doesn't cover..."]
  }
  ```

### 5.3 Reasoning Model Usage

Use reasoning/thinking models (Claude extended thinking, o1/o3) for:
- Pass 1: overview requires reasoning about architectural impact
- Pass 2: deep analysis benefits from chain-of-thought on data flow

Standard models as fallback if reasoning models are unavailable or user prefers lower cost.

## 6. Configuration

### 6.1 Auto-Detection (Default)

flowdiff infers:
- Language from file extensions + tree-sitter grammar availability
- Framework from import patterns and file structure conventions
- Entrypoints from code patterns
- Architectural layers from directory structure heuristics

### 6.2 Optional Config File: `.flowdiff.toml`

```toml
[project]
name = "my-app"

[entrypoints]
# Declare known entrypoints if auto-detection misses them
http = ["src/routes/**/*.ts"]
workers = ["src/jobs/**/*.ts"]
cli = ["src/cli/main.rs"]

[layers]
# Name architectural layers for better grouping
api = "src/handlers/**"
domain = "src/services/**"
persistence = "src/repositories/**"
ui = "src/components/**"

[ignore]
# Files to exclude from analysis
paths = ["**/*.test.ts", "**/*.spec.ts", "migrations/**"]

[llm]
provider = "anthropic"  # "anthropic", "openai", or "gemini"
model = "claude-sonnet-4-6"
# API key via FLOWDIFF_API_KEY env var or:
# key_cmd = "op read op://vault/flowdiff/api-key"

[llm.refinement]
# Optional LLM refinement pass — improves grouping/ranking using semantic understanding.
# Deterministic analysis runs first (free, fast), then LLM refines the output.
# Only applied if enabled and API key is available. Falls back to deterministic if LLM fails.
enabled = false
provider = "anthropic"       # can differ from annotation provider
model = "claude-sonnet-4-6"  # user selects the model for refinement
# key_cmd = "op read op://vault/flowdiff/refinement-key"
# What the refinement pass can do:
# - split groups that contain logically unrelated changes
# - merge groups that are part of the same logical change
# - re-rank groups based on semantic review ordering ("read schema before handler")
# - reclassify file roles (e.g. "shared utility" → "critical change")
# - re-assign files between groups when static reachability gets it wrong
max_iterations = 1  # evaluator-optimizer loop iterations (1 = single refinement, 2+ = iterative)

[ranking]
# Override default weights
risk = 0.35
centrality = 0.25
surface_area = 0.20
uncertainty = 0.20
```

## 7. CLI Interface

```bash
# Analyze a branch diff
flowdiff analyze --base main --head feature-branch

# Analyze a commit range
flowdiff analyze --range HEAD~5..HEAD

# Analyze staged changes
flowdiff analyze --staged

# Output to file
flowdiff analyze --base main -o review.json

# With LLM annotations (pass 1)
flowdiff analyze --base main --annotate

# Deep analysis on a specific group
flowdiff annotate --group group_1 --input review.json

# Launch Tauri app with analysis
flowdiff ui --base main

# Open specific files in Beyond Compare (integration)
flowdiff launch --tool bcompare --group group_1 --input review.json
```

### CLI JSON Output Schema

```json
{
  "version": "1.0.0",
  "diff_source": {
    "type": "branch_comparison",
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
  "groups": [
    {
      "id": "group_1",
      "name": "POST /api/users creation flow",
      "entrypoint": {
        "file": "src/app/api/users/route.ts",
        "symbol": "POST",
        "type": "http_route"
      },
      "risk_score": 0.82,
      "review_order": 1,
      "files": [
        {
          "path": "src/app/api/users/route.ts",
          "flow_position": 0,
          "role": "entrypoint",
          "changes": { "additions": 25, "deletions": 10 },
          "symbols_changed": ["POST", "validateUserInput"]
        }
      ],
      "edges": [
        {
          "from": "src/app/api/users/route.ts::POST",
          "to": "src/services/user-service.ts::createUser",
          "type": "calls"
        }
      ],
      "flow_graph_mermaid": "graph TD\n  A[route.ts::POST] --> B[user-service.ts::createUser]\n  B --> C[user-repo.ts::insert]"
    }
  ],
  "infrastructure_group": {
    "files": ["tsconfig.json", "package.json"],
    "reason": "Not reachable from any detected entrypoint"
  },
  "annotations": null
}
```

## 8. Tauri App UI

### Three-Panel Layout

```
┌──────────────────┬────────────────────────────┬──────────────────┐
│  FLOW GROUPS     │      DIFF VIEWER           │   ANNOTATIONS    │
│                  │      (Monaco Editor)       │                  │
│  ▼ Group 1 (0.82)│                            │  Flow: POST →    │
│    ├ route.ts    │  - old line                │  validate →      │
│    ├ service.ts  │  + new line                │  persist →       │
│    └ repo.ts     │  - old line                │  emit            │
│                  │  + new line                │                  │
│  ▶ Group 2 (0.65)│                            │  Risk: 0.82      │
│  ▶ Group 3 (0.41)│                            │  Schema change   │
│                  │                            │  Auth affected   │
│  ─────────────── │                            │  ──────────────  │
│  Infrastructure  │                            │  [Annotate ▶]    │
│    ├ tsconfig    │                            │  [Mermaid ▶]     │
│    └ package.json│                            │                  │
│                  │                            │  LLM Summary:    │
│  ──────────────  │                            │  "This group..." │
│  [Deterministic] │                            │                  │
│  [LLM Annotate]  │                            │                  │
└──────────────────┴────────────────────────────┴──────────────────┘
```

**Left panel — Flow Groups:**
- Tree view of semantic groups, ranked by score
- Each group expandable to show files in flow order
- Risk score badge per group
- Click file → opens in center Monaco diff viewer
- "Next file in flow" / "Next group" navigation
- Toggle between deterministic and LLM-annotated mode

**Center panel — Monaco Diff Viewer:**
- Side-by-side or inline diff view
- Full syntax highlighting via Monaco
- Hunk-level navigation
- Inline annotations from LLM (if enabled)

**Right panel — Annotations & Graph:**
- Flow diagram (Mermaid rendered)
- Risk flags and scores
- Per-file role explanation ("this file is the persistence layer for this flow")
- LLM summary and commentary (when annotated)
- "Annotate this group" button for on-demand Pass 2

### Key Interactions
- **Keyboard-driven**: `j/k` for next/prev file, `J/K` for next/prev group, `a` to annotate
- **Flow replay**: step through a group's files in data flow order
- **Risk heatmap**: visual indicator of which groups need most attention

## 9. VS Code Extension

### Architecture
- TypeScript extension, minimal logic
- Spawns `flowdiff` CLI binary
- Parses JSON output
- Renders in VS Code native UI primitives

### UI Elements
- **Activity Bar icon**: flowdiff logo
- **Sidebar tree view**: flow groups → files (same structure as Tauri left panel)
- **Webview panel**: annotations, Mermaid graph, risk scores
- **Commands**:
  - `flowdiff.analyze` — run analysis on current branch
  - `flowdiff.analyzeRange` — analyze commit range
  - `flowdiff.annotate` — trigger LLM annotation
  - `flowdiff.nextFile` — next file in current flow
  - `flowdiff.nextGroup` — next group
- **Click file** → opens VS Code's native diff editor (not Monaco webview — use the built-in)

## 10. Rust Crate Structure

```
flowdiff/
├── Cargo.toml                  # Workspace root
├── crates/
│   ├── flowdiff-core/          # Library: all analysis logic
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── git.rs          # Git diff extraction (git2)
│   │   │   ├── ast.rs          # Tree-sitter parsing
│   │   │   ├── graph.rs        # Symbol graph (petgraph)
│   │   │   ├── flow.rs         # Data flow tracing
│   │   │   ├── cluster.rs      # Semantic grouping
│   │   │   ├── rank.rs         # Review ordering
│   │   │   ├── llm/
│   │   │   │   ├── mod.rs
│   │   │   │   ├── anthropic.rs
│   │   │   │   ├── openai.rs
│   │   │   │   └── schema.rs   # Structured output schemas
│   │   │   ├── config.rs       # .flowdiff.toml parsing
│   │   │   └── output.rs       # JSON serialization
│   │   └── Cargo.toml
│   ├── flowdiff-cli/           # Binary: CLI interface
│   │   ├── src/main.rs
│   │   └── Cargo.toml
│   └── flowdiff-tauri/         # Tauri app
│       ├── src/
│       │   ├── main.rs         # Tauri setup
│       │   └── commands.rs     # Tauri IPC commands
│       ├── ui/                 # Frontend (TypeScript + React)
│       │   ├── src/
│       │   │   ├── App.tsx
│       │   │   ├── panels/
│       │   │   │   ├── FlowGroups.tsx
│       │   │   │   ├── DiffViewer.tsx    # Monaco wrapper
│       │   │   │   └── Annotations.tsx
│       │   │   ├── components/
│       │   │   │   ├── MermaidGraph.tsx
│       │   │   │   ├── RiskBadge.tsx
│       │   │   │   └── FlowNavigation.tsx
│       │   │   └── hooks/
│       │   │       └── useFlowdiff.ts    # Tauri IPC hooks
│       │   ├── package.json
│       │   └── tsconfig.json
│       ├── tauri.conf.json
│       └── Cargo.toml
├── extensions/
│   └── vscode/                 # VS Code extension
│       ├── src/
│       │   ├── extension.ts
│       │   ├── flowdiffRunner.ts   # CLI invocation
│       │   ├── treeView.ts         # Sidebar tree
│       │   └── webviewPanel.ts     # Annotations panel
│       ├── package.json
│       └── tsconfig.json
└── specs/
    └── spec.md
```

### Key Rust Dependencies

| Crate | Purpose |
|-------|---------|
| `git2` | Git operations (diff, blame, log) |
| `tree-sitter` + grammars | AST parsing for all languages |
| `petgraph` | Directed graph construction and traversal |
| `serde` / `serde_json` | JSON serialization |
| `clap` | CLI argument parsing |
| `rayon` | Parallel file processing |
| `reqwest` | HTTP client for LLM APIs |
| `tokio` | Async runtime for LLM calls |
| `toml` | Config file parsing |
| `tauri` | Desktop app framework |

## 11. Build Phases

### Phase 1: Core Engine (Week 1-2)
- [x] Cargo workspace setup
- [x] Git diff extraction via git2
- [x] Unit tests for git layer (54 tests — 21 unit tests: branch comparison with SHA verification, commit range single/multi-commit, range validation/invalid format/triple-dot/empty parts, staged changes including new files, unstaged changes, staged-vs-unstaged isolation, file rename with old→new path tracking, rename with content change, binary file skipping via blob-level detection, empty repo graceful error, deleted files with old content, new files with new content, hunk extraction, line count accuracy, old/new content retrieval, multiple files with mixed statuses, deeply nested paths, unicode content preservation, empty file add/modify, additions-only and deletions-only, large diff with 30 files, deterministic output, is_binary always false for returned files, copy detection, ref-not-found error, no-changes empty result, subdirectories, FileDiff::path() helper including fallback to "\<unknown\>", error display formatting, serde roundtrips for DiffHunk/FileStatus/FileDiff including null fields; 11 property-based tests: FileStatus serde roundtrip, DiffHunk serde roundtrip, FileDiff serde roundtrip, path() never empty, hunk old_start positive, is_binary always false, additions+deletions bounded, FileDiff clone equality, DiffHunk clone equality, JSON has status field, JSON always parseable)
- [x] Tree-sitter AST parsing (TS/JS + Python grammars first)
- [x] Symbol graph construction (imports, exports, calls)
- [x] Basic entrypoint detection
- [x] Semantic clustering (forward reachability from entrypoints)
- [x] Review ranking (composite score)
- [x] JSON output (output.rs — AnalysisOutput builder, JSON serialization, Mermaid diagram generation, 42 tests)
- [x] CLI with clap (`flowdiff analyze --base main`)
- [x] Test against a real multi-file PR (15 e2e integration tests — programmatic git repos: simple Express app, Python FastAPI, branch comparison, empty diff, JSON schema compliance, cross-cutting refactor, multiple entrypoints, mixed language, determinism, new-files-only, risk scoring, 20-file diff performance, Mermaid generation, commit range, entrypoint detection)
- [x] Core data types (types.rs — FlowGroup, FileChange, Symbol, Edge, etc.)
- [x] Unit tests for core data types (46 tests — serde roundtrip for all 13 structs/enums including Symbol, FlowEdge, ChangeStats, FileChange, FlowGroup, RiskIndicators, RankWeights, RankedGroup, DiffSource, AnalysisSummary, InfrastructureGroup, AnalysisOutput; all enum variant coverage for SymbolKind/EdgeType/FileRole/EntrypointType/DiffType; Default impl for RankWeights with sum-to-one invariant; Clone/PartialEq semantics; JSON field naming verification; deserialization from raw JSON including spec schema compliance; empty collection edge cases; unicode/special character paths; Hash trait distinctness for SymbolKind and EdgeType; 11 property-based tests for serde roundtrips of arbitrary values, non-negative weight fields, change stats bounds)
- [x] Property-based tests for ranking (proptest — 11 properties)
- [x] Unit tests for ranking (26 tests — scoring, risk, surface area, path detection)
- [x] Unit tests for AST parsing (25 tests — TS/JS imports, exports, definitions, calls; Python imports, functions, class hierarchy; changed symbol detection; performance)
- [x] Unit tests for graph construction (123 tests — 25 original: import edges, call edges, namespace/default/aliased imports, cyclic imports, re-export chains, index file resolution, cross-directory imports, Python imports/calls, serialization roundtrip, node lookup, determinism; 11 IR parity tests; 20 helper function tests: normalize_path dot/dotdot/empty/trailing, normalize_python_import single/double/triple dot and dotted remainder, parent_dir nested/no-slash, file_stem with/without extension, resolve_import_path exact/extension/index/parent/python/priority; 8 IR extends edge tests: local class, imported class, multiple bases, self-edge prevention, missing base graceful, empty bases, cross-file inheritance chain; 13 IR node type tests: class/struct/interface/type-alias/enum/constant/function kind mapping, mixed definitions, duplicate names across files, duplicate within file skipped, IR call edges with containing_function, module-level calls; 15 edge case tests: unicode file paths and symbol names, deeply nested paths, import-only files, 50-file scale, empty graph edges/node_ids, get_symbol missing, add_edge directly, from_serializable with invalid/both-invalid endpoints, all 8 edge types preserved in serialization, same name different dirs, multiple importers of same symbol; 10 property-based tests: all edges reference valid nodes, module node id equals file path, roundtrip preserves edge types and symbol kinds, node count equals unique defs + modules, GraphError display, normalize_path/normalize_python_import/file_stem never panic, non-relative imports never resolve; 6 IR property-based: parity node/edge counts, no self-edges, determinism, empty input, every definition has node)
- [x] Property-based tests for graph construction (16 tests — 6 original ParsedFile path: every definition has node, node count ≥ file count, no self-edges, serialization roundtrip, determinism, empty input; 10 extended: all edges reference valid nodes, module node id = file path, serializable roundtrip preserves edge types and symbol kinds, node count = unique defs + modules, GraphError display, normalize_path/normalize_python_import/file_stem never panic, non-relative imports never resolve)
- [x] Unit tests for entrypoint detection (75 tests — HTTP routes for Express/FastAPI/Flask/Next.js, CLI commands with click/commander/argparse, test file detection, queue consumers, cron jobs, React pages, event handlers, deduplication, edge cases, plus 34 Effect.ts tests)
- [x] Unit tests for semantic clustering (16 tests — single/multiple entrypoint groups, shared file assignment by shortest path, infrastructure group, empty diff, disconnected components, file ordering by flow position, determinism, entrypoint not in graph, internal edges, file role inference, group name generation)
- [x] Property-based tests for semantic clustering (6 tests — every file in exactly one group, empty diff → empty result, single file → single group, no entrypoints → all infrastructure, determinism, no edges → only entrypoint files grouped)
- [x] Effect.ts entrypoint detection (`HttpApi`/`HttpApiEndpoint`/`HttpApiGroup`/`HttpRouter`, `@effect/cli` Command, `Queue`/`PubSub` consumers, `Schedule`/`@effect/cron`, `@effect/vitest` test patterns, `Stream`/`Hub` handlers, `Effect.Service`/`Context.Tag`/`Layer` definitions)
- [x] Unit tests for Effect.ts entrypoint detection (34 tests — HTTP routes via HttpApiEndpoint/HttpApi/HttpApiGroup/HttpRouter, CLI commands via @effect/cli Command, queue consumers via Queue/PubSub, cron jobs via Schedule/@effect/cron, test files via @effect/vitest, event handlers via Stream/Hub, services via Effect.Service/Context.Tag/Layer, edge cases for import validation and deduplication)

### Phase 2: Data Flow Depth (Week 2-3)
- [x] Heuristic inference (flow.rs — DB writes/reads, event emission/handling, config reads, HTTP calls, logging detection with confidence scoring, false positive guards for collection methods/stdlib; graph enrichment adds Writes/Reads/Emits/Handles edges)
- [x] Framework pattern detection (flow.rs — auto-detect Express, Next.js, React, FastAPI, Flask, Django, Prisma, Effect.ts, and 30+ frameworks from import patterns + file structure conventions)
- [x] Call chain tracing (flow.rs — BFS traversal of call edges with configurable depth limit, cycle-safe)
- [x] Unit tests for data flow (64 tests — persistence detection for save/insert/create/update/delete, DB reads for find/query/findMany, event emission for emit/publish/dispatch/send, event handling for on/subscribe/listen, config reads for process.env/os.environ/os.getenv, HTTP calls for fetch/axios/requests, logging for console/logger/logging, false positive guards for arrays/maps/sets/localStorage/JSON/Promise, graph enrichment, call chain tracing with depth limits and cycles)
- [x] Property-based tests for data flow (6 tests — classify never panics, confidence in range, edge fields valid, frameworks sorted + deduplicated, empty input → empty output, deterministic analysis)
- [x] Framework detection tests (12 tests — Express, Next.js imports + file structure, React, FastAPI, Flask, Django, Prisma, Effect.ts, multiple frameworks, no frameworks, sorted output, deduplication)
- [x] Mermaid graph generation in JSON output
- [x] Commit range and staged/unstaged support
- [x] Full data flow tracing (flow.rs — variable assignment tracking from call return values, call argument extraction, within-function data flow edge building connecting producers to consumers via shared variables, cross-file trace_data_flow API; ast.rs — extract_data_flow_info with VarCallAssignment/CallWithArgs/DataFlowInfo types, TypeScript + Python support including await unwrapping)
- [x] Unit tests for data flow tracing (30 tests — 16 AST extraction tests for TS variable assignments, method calls, await, chained assignments, call arguments, arrow functions, module-level, nested calls, Python assignments/chains/methods, edge cases; 14 flow edge tests for simple chains, pipelines, multiple consumers, scope isolation, self-edge prevention, cross-file, determinism; 6 property-based tests for never-panics, valid fields, no self-edges, via-matches-variable, determinism, empty input)
- [x] Shared IR (intermediate representation) — ir.rs defines language-agnostic AST types: IrFile, IrFunctionDef, IrTypeDef, IrConstant, IrImport/IrExport with IrImportSpecifier, IrCallExpression, IrAssignment with IrPattern (Identifier, ObjectDestructure, ArrayDestructure, TupleDestructure) and IrExpression (Call, Identifier, Await, Yield, MemberAccess, Other). Bidirectional conversion: ParsedFile → IrFile (from_parsed_file), IrFile → ParsedFile (to_parsed_file) for backward compat. DataFlowInfo enrichment (enrich_with_data_flow) and extraction (to_data_flow_info). Covers: simple assignments (`const x = foo()`), destructuring (`const { a, b } = foo()`, `const [first, ...rest] = bar()`, Python tuple unpacking), Effect.ts `yield*` destructuring (`const { svc } = yield* _(Tag)`), spread/rest patterns, nested destructuring, default values. 72 unit tests + 12 property-based tests
- [x] Declarative tree-sitter query layer — query_engine.rs provides a generic engine that loads `.scm` query files per language (e.g. `queries/typescript/imports.scm`, `queries/python/imports.scm`) and maps `@capture` names to ParsedFile/DataFlowInfo types via pattern_index + capture naming convention. Uses tree-sitter's StreamingIterator API with collected matches pattern. Engine compiles all queries at construction time (include_str! embedding). Adding a new language = writing `.scm` files, zero Rust code.
- [x] Per-language `.scm` query files (TypeScript/JS: imports.scm, exports.scm, definitions.scm, calls.scm, assignments.scm; Python: imports.scm, definitions.scm, calls.scm, assignments.scm). Covers: default/named/namespace/aliased/side-effect imports, exported declarations/re-exports/wildcards, function/class/interface/type/constant/method definitions, call expressions with containing function resolution, variable assignment from calls including await unwrapping
- [x] Generic query engine (query_engine.rs — QueryEngine struct with QueryWithCaptures, collect_matches helper for StreamingIterator, typed extraction methods for imports/exports/definitions/calls/assignments, language-agnostic containing-function resolution via parent traversal, argument text extraction with Python keyword arg support)
- [x] Refactor existing ast.rs, entrypoint.rs, flow.rs to consume query engine / IR types instead of imperative tree-sitter code
  - graph.rs: `SymbolGraph::build_from_ir(&[IrFile])` — native IR graph construction with extends edges from `IrTypeDef.bases`, import edge resolution via `IrImportSpecifier`, call edge resolution via `IrCallExpression`
  - entrypoint.rs: `detect_entrypoints_ir(&[IrFile])` — IR-based entrypoint detection
  - flow.rs: `analyze_data_flow_ir(&[IrFile])`, `detect_frameworks_ir(&[IrFile])`, `build_data_flow_edges_from_ir(&IrFile)` — IR-based heuristic analysis and data flow tracing without source re-parsing, `trace_data_flow_ir(&[IrFile])` — multi-file data flow from IR
  - pipeline.rs: `parse_to_ir(engine, path, source)`, `parse_all_to_ir(engine, files)` — unified QueryEngine → IrFile pipeline entry points
  - 68 tests: 22 parity tests (IR vs ParsedFile produce identical results), 12 property-based tests (invariants on IR-based APIs), 11 IR data flow unit tests, 23 pipeline tests (6 original + 17 new: empty/whitespace/comments-only source, empty file list, order preservation, mixed language parsing, syntax error tolerance for TS and Python, deeply nested/dynamic route/dotfile paths, data flow enrichment for TS and Python assignments, PipelineError display formatting, determinism for single and batch parsing, 5 property-based tests for never-panics on arbitrary input, path preservation, file count invariant, determinism, empty source produces no definitions)
- [x] Unit tests for IR types (72 unit tests — Span construction/line_count, IrPattern bound_names for all variants including nested/holes/rest, IrExpression callee extraction through Await/Yield wrappers, IrImportSpecifier local/remote name resolution, IrFile construction/accessors, ParsedFile→IrFile conversion for functions/type_defs/constants/imports/exports/call_expressions, namespace/aliased/side-effect imports, enrichment with DataFlowInfo, ParsedFile roundtrip preservation, DataFlowInfo roundtrip with destructure exclusion, serde roundtrip for all pattern/expression variants, edge cases for empty/unknown/module/struct/type_alias, assignment patterns for all destructuring styles, function params with destructuring)
- [x] Property-based tests for IR types (12 tests — bound_names never panics, identifier patterns have exactly one bound name, is_identifier correctness, span line_count >= 1, IrFile serde roundtrip, ParsedFile→IrFile definition count preservation, IrPattern serde roundtrip, all_definition_names count, empty file invariants, empty DataFlowInfo enrichment is no-op)
- [x] Unit tests for query engine (53 tests — 37 unit tests for TS imports/exports/definitions/calls/data-flow, Python imports/definitions/calls/data-flow, unknown language handling, parity with ast.rs for full TS and Python files, determinism, empty source, syntax error tolerance; 8 property-based tests for never-panics, determinism, always-valid-output, empty-source-empty-result)
- [x] Refactor query engine to use capture-name-presence matching instead of fragile `pattern_index` — switched from `match m.pattern_index { 0 => ..., 1 => ... }` to `CollectedMatch::has_capture()` / `get_capture()` checking which `@capture` names are present. `.scm` pattern ordering is now irrelevant. Updated all extraction methods (imports, exports, export_definitions, definitions for both TS and Python). Renamed `.scm` captures to be distinct per definition kind (e.g. `@fn_name`/`@fn_node`, `@class_name`/`@class_node`). Removed `pattern_index` field from `CollectedMatch`. Added `Copy` derive to `SymbolKind`. All 791 tests pass.
- [x] Config file support (config.rs — `.flowdiff.toml` parsing, validation, defaults merging, entrypoint glob resolution, ignore patterns, layer names, LLM config; 17 unit tests + 6 property-based tests)
- [x] Restructure tests to Rust convention — integration tests in `crates/flowdiff-core/tests/` (5 files: `e2e_pipeline.rs`, `eval_suite.rs`, `llm_live.rs`, `vcr_integration.rs`, `llm_judge.rs`). Unit tests co-located in source files. Created `tests/helpers/` with `mod.rs`, `repo_builder.rs` (shared `RepoBuilder` + `run_pipeline` + `find_feature_branch`), `graph_assertions.rs` (7 assertion helpers: `assert_all_files_accounted`, `assert_valid_scores`, `assert_language_detected`, `assert_file_in_some_group`, `assert_json_roundtrip`, `assert_valid_json_schema`, `assert_valid_mermaid`), `llm_helpers.rs` (shared `should_run_live`, `load_env`, `sample_pass1_request`, `sample_pass2_request`). All 5 integration test files refactored to use shared helpers, eliminating `RepoBuilder`/`run_pipeline`/`load_env` duplication. Live tests gated behind `FLOWDIFF_RUN_LIVE_LLM_TESTS=1`. All 791 tests pass

### Phase 3: Tauri App (Week 3-4)
- [x] Tauri project setup with React frontend (Tauri v2 + React 19 + Vite 6 + TypeScript, `flowdiff-tauri` crate with Cargo workspace integration, `tauri.conf.json` with 1440x900 window, RGBA placeholder icons, `capabilities/default.json`)
- [x] Three-panel layout shell (CSS-based three-panel layout: 280px left / flex center / 300px right, top bar with repo path input + base ref + analyze button, dark Catppuccin-inspired theme)
- [x] Left panel: flow group tree view (groups sorted by review_order, expandable file lists with role/path/change stats, risk score badges with high/medium/low color coding, infrastructure group section, selected state highlighting)
- [x] Center panel: Monaco diff viewer integration (`@monaco-editor/react` DiffEditor with custom `flowdiff-dark` theme matching Catppuccin palette, side-by-side view, read-only, syntax highlighting for all supported languages, `automaticLayout` for responsive sizing)
- [x] Right panel: annotations and graph display (flow group details: name, entrypoint info, risk score, file count, review order; Mermaid diagram rendering; edge list with type/from/to)
- [x] Mermaid rendering (`mermaid` library with dark theme, async SVG rendering via `mermaid.render()`, error handling for invalid diagrams, Catppuccin-themed color variables)
- [x] Keyboard navigation (j/k/J/K) — `j`=next file, `k`=prev file, `J`=next group, `K`=prev group. Uses `useRef` for latest state access without re-registering listeners. Disabled when Monaco editor or input fields are focused. Keyboard hints footer bar shown when analysis is loaded
- [x] File navigation within flow groups (click file in tree → loads diff via `get_file_diff` IPC → Monaco DiffEditor updates. Auto-selects first file when group is selected. Files shown in flow_position order with role badge, short path, +/- change stats)
- [x] Generate app icon using AI image generation (gpt-image-1.5 generated directed-graph icon with teal/lavender nodes on dark indigo background, `cargo tauri icon` generated all platform sizes: PNG 32/64/128/256, .icns, .ico, iOS, Android, Windows AppX)
- [x] Visual polish pass — Playwright E2E screenshot tests for all UI states (14 tests: loaded analysis, flow groups panel, Monaco diff viewer, annotations/Mermaid panel, second/third group selection, file navigation, j/k keyboard nav, J/K group nav, top bar, Mermaid close-up, error state, infrastructure group, keyboard hints). Demo mode added for browser-based testing (mock data auto-loads when not in Tauri). Visual fixes: edge text ellipsis truncation, entrypoint monospace wrapping, selected group blue accent border, annotation section typography improvements, file role min-width. 14 baseline screenshots saved to `docs/screenshots/`
- [x] Replace Mermaid with React Flow (`@xyflow/react`) — replaced Mermaid with `@xyflow/react` + `@dagrejs/dagre` for production-grade flow graph visualization. Custom `FlowNode` component styled to match Catppuccin theme (dark surface background, role-colored headers). Auto-layout via dagre (top-to-bottom). Smooth animated smoothstep edges with edge labels (calls, imports, writes, etc.). Interactive: click node → selects file in diff viewer, pan/zoom/drag. Minimap for graphs with 6+ nodes. Edge type color coding (Calls=blue, Imports=lavender, Writes=peach, Reads=green, Extends=yellow, Instantiates=sky, Emits=red, Handles=teal). Node badges showing file role and change stats (+/-). Edges deduplicated to file-level with self-edge filtering. Falls back to simple list view if graph has 100+ nodes. Playwright screenshot baselines updated (14 tests pass). Removed mermaid dependency entirely
- [x] Rename LLM action buttons — "Annotate All Groups" → "Summarize PR" (runs Pass 1 overview), "Deep Analyze Group" → "Analyze This Flow" (runs Pass 2 per-group). Tooltips explain what each does (Pass 1 overview / Pass 2 deep analysis) and that they require an API key. Greyed out with "Requires API key" message and 50% opacity when no key configured. `check_api_key` IPC command added to Tauri backend (checks key_cmd, FLOWDIFF_API_KEY, provider-specific env vars via `resolve_api_key`). CSS classes renamed: `.btn-annotate` → `.btn-summarize`, `.btn-deep` → `.btn-analyze-flow`, `.no-api-key` style added. 2 new Tauri unit tests (check_api_key with no repo, invalid path). All 1,083 tests pass
- [x] React Flow polish — 9 improvements to the flow graph visualization: (1) Animated bezier edges with flowing dot animation showing data direction via `getBezierPath` + SVG `<animateMotion>`, custom `AnimatedBezierEdge` edge type replacing smoothstep. (2) Edge labels enhanced on hover — label rect highlights with edge color, hover tooltip shows full from→to file paths below the label. (3) Node hover effect — increased glow/border highlight (12px spread), selected node state with 16px glow, CSS tooltip below node showing full file path + change summary. (4) Fullscreen mode — maximize button (top-right corner `⤢` icon), `position: fixed; inset: 0; z-index: 1000` fills entire viewport, Escape key exits fullscreen via `useEffect` keyboard listener. (5) Better node spacing — dagre `ranksep: 60→80`, `nodesep: 40→60`, `marginx/y: 20→30` for less cramped layout. (6) Edge dimming — when a node is clicked, non-connected edges get `dimmed: true` (opacity 0.15, strokeWidth 1, no animation dot), connected edges stay full brightness. Click pane to clear selection via `onPaneClick`. (7) Smooth zoom transitions — `fitViewOptions.duration: 800` for smooth initial fit, React Flow Controls component added for zoom in/out/fit buttons. (8) Legend overlay — collapsible panel (bottom-left) showing all 8 edge types with color-coded lines (calls=blue, imports=lavender, writes=peach, reads=green, extends=yellow, new=sky, emits=red, handles=teal). (9) Export graph as PNG/SVG — `html-to-image` dependency added, two buttons (top-right) export the graph container at 2x pixel ratio for PNG. All themed to Catppuccin dark palette. Playwright E2E test fix: `.base-input` assertion updated to `.branch-dropdown-trigger .branch-name` for branch dropdown compatibility. All 14 Playwright tests pass, all Rust tests pass
- [x] React Flow graph visibility fix — root cause: custom `FlowNode` component was missing `<Handle>` components, so React Flow had no attachment points for edges. Fix: (1) Added `<Handle type="target" position={Position.Top}>` and `<Handle type="source" position={Position.Bottom}>` to FlowNode with invisible styling (`.flow-node-handle`), (2) Increased graph container height 300px→400px, (3) Edge strokeWidth 2→3 with larger arrowheads (16px→24px), (4) Edge labels now always-visible pill badges with rounded corners (`rx=10`), edge-color border stroke, brighter text (#cdd6f4), larger font (11px), (5) Node min-width 160px→220px with more padding (8px 10px→10px 14px), (6) CSS `drop-shadow(0 0 3px currentColor)` on `.react-flow__edge-path` for edge glow, (7) Adaptive dagre spacing: ranksep 120/nodesep 80 for 2-6 node graphs, 80/60 for larger, (8) fitView padding 0.2→0.3, (9) Flowing dot radius 3→4 with 0.9 opacity, dagre nodeWidth 200→220/nodeHeight 56→60. All 50 Playwright tests pass, all 1,092 Rust tests pass. Updated screenshots visually verified — edges clearly visible with labels, arrows, and animated dots connecting all nodes
- [x] Git auto-discovery in Tauri — on app launch or when repo path is entered: auto-detect current branch, list all local branches in a dropdown (replace text input), list git worktrees if any exist, show push status ("3 ahead of origin/main, 0 behind"). Default diff mode is PR preview (`main...HEAD`). User can change base branch via dropdown. IPC commands: `list_branches`, `list_worktrees`, `get_branch_status`, `get_repo_info`. Uses git2 for all operations. Rust: `git.rs` adds `BranchInfo`/`WorktreeInfo`/`BranchStatus` types, `list_branches()`/`list_worktrees()`/`get_branch_status()`/`detect_default_branch()`/`diff_merge_base()`/`current_branch()` functions. `commands.rs` adds 4 IPC commands + `RepoInfo` aggregate type + `open_repo()` helper. Frontend: branch dropdown replaces base ref text input, repo status bar shows current branch + ahead/behind + worktree count, `RepoInfo`/`BranchInfo`/`WorktreeInfo`/`BranchStatus` TypeScript types. 20 new git unit tests (branch listing, sorting, worktree listing, branch status, default branch detection, merge-base diff basic/diverged/same-commit/invalid, current branch, serde roundtrips for all 3 new types), 2 new command tests (RepoInfo serde). All 1081 tests pass
- [x] PR preview diff mode — implement merge-base diff in git layer (`git2::Repository::merge_base()` → diff from ancestor to HEAD). Default when no flags given. `diff_merge_base()` in `git.rs` finds merge base between two refs, diffs from ancestor to head. `detect_default_branch()` auto-detects main/master/develop. Tauri `analyze` command gains `pr_preview` parameter (default true in UI). `extract_diff()` routes to merge-base when `pr_preview=true`. Frontend sends `prPreview: true` by default. 4 unit tests for merge-base diff (basic, diverged branches, same commit, invalid ref)
- [x] LLM controls in Tauri UI — settings panel (gear icon in top bar → slide-out panel) with: (1) API key status indicator (green "Configured via [source]" / red "Not configured" with setup instructions), (2) Enable/disable LLM annotations toggle (controls Summarize PR / Analyze This Flow button state), (3) Enable/disable LLM refinement toggle (with collapsible refinement sub-settings), (4) Provider selector dropdown (Anthropic/OpenAI/Gemini), (5) Model selector dropdown (populated per provider: claude-sonnet-4/opus-4/haiku-4.5, gpt-4o/o1/o3-mini/o3, gemini-2.5-pro/flash), (6) Refinement provider/model/max-iterations controls when refinement enabled. Settings persisted to `.flowdiff.toml` via `save_llm_settings` IPC command (preserves non-LLM config sections). `get_llm_settings` IPC command reads config + resolves API key source. `annotate_overview` and `annotate_group` commands accept `llm_provider`/`llm_model` overrides from frontend, load config from repo dir (fixed bug where `FlowdiffConfig::default()` was used instead of repo config). `FlowdiffConfig::save_to_dir()` added for TOML serialization. Catppuccin-styled slide-out panel with Escape-to-close, click-outside-to-close, dark theme selects/toggles. Provider badge shown below annotation buttons. 7 new Tauri tests (LlmSettings serde, default models, get_llm_settings with no repo/invalid path, load_config_from_path). All 1,090 tests pass
- [x] Refinement UX — progressive disclosure flow for LLM refinement of groupings. `refine_groups` Tauri IPC command calls `LlmProvider::refine_groups()` → `apply_refinement()` from flowdiff-core refinement module. `RefinementResult` struct returned with refined groups, raw response (splits/merges/re-ranks/reclassifications), provider/model info, and `had_changes` flag. Frontend: `originalGroups`/`refinedGroups` state with `showRefined` toggle. Cached analysis updated in-place when switching views. Change indicators computed from `RefinementResponse`: split (blue tag "split from X"), merge (purple "merged from X + Y"), re-rank (yellow "re-ranked to #N"), file moved (purple "moved from Group A"). UI components:
  - Banner at top of flow groups panel: "AI can improve these groupings" + "Refine" button (only shown if API key configured + refinement enabled + no existing refinement)
  - Loading state with spinner showing provider/model being used, on the flow groups panel only (never blocks full UI)
  - "Refined by [model name]" pill badge in panel header when viewing refined groups
  - Original/Refined toggle (two-button pill toggle) lets user flip between views — clicking "Original" reverts to deterministic groupings, no data lost
  - Change indicators: per-group tags (split/merge/rerank), per-file "moved from" tags, groups with changes get accent left border
  - Deterministic results always available first — refinement is purely additive
  - Summarize PR / Analyze This Flow work on whichever grouping is currently active
  - Mock refinement data for demo/Playwright mode (splits group_1 to separate data model definitions, re-ranks auth flow)
  - 2 new Rust tests (RefinementResult serde roundtrip, RefinementResult with changes). All 1,092 tests pass. Vite + tsc build clean
- [x] Tauri app hardening — comprehensive Playwright screenshot + visual inspection pass across all new features and edge cases. 36 new hardening tests in `hardening.spec.ts` (50 total Playwright tests). Test API hook added to App.tsx for data injection in demo mode. Mock data generators for edge cases: 50+ branches, multiple worktrees, diverged/up-to-date status, large datasets (25 groups, 100+ files), no API key, single-node analysis. States captured and verified:
  - Branch dropdown: open with 6 branches, selected branch highlighted, 55 branches with long names — all render correctly, scrollable
  - Worktree indicator: multiple worktrees badge ("3 worktrees") visible and properly styled
  - Push status display: ahead (3 ahead), diverged (5 ahead, 12 behind), up-to-date — all render correctly
  - PR preview mode: default view on launch with main as base, switching base branch via dropdown to develop
  - React Flow graph: 3-5 node graph (default mock), node click with edge dimming and selection highlight, legend overlay with all 8 edge type colors, fullscreen mode with Escape exit, export PNG/SVG buttons, single-node analysis (no graph shown when 0 edges)
  - LLM controls: settings panel with all sections (API Key, Annotations, Provider, Model, Refinement), API key configured (green indicator), API key missing (red warning with setup instructions), refinement sub-settings expanded, close via Escape
  - Annotate button states: idle (Summarize PR + Analyze This Flow), complete with LLM overview (risk flags, rationale, overall summary), complete with deep analysis (flow narrative, file annotations with risks/suggestions, cross-cutting concerns), no API key (greyed out "Requires API key")
  - Refine button states: refinement banner ("AI can improve these groupings"), complete with original/refined toggle, change indicators (split from group_1, re-ranked to #3), toggle back to original view preserving data
  - Empty states: no analysis (placeholder messages in all 3 panels), right panel empty when no group selected
  - Error states: real error bar with dismiss button, error message visible and dismissible
  - Large datasets: 25 groups with 100+ files, left panel scrolls without overflow, infrastructure group with 5 files at bottom
  - Responsive layout: narrow (1024x768), wide (1920x1080), minimum (800x600) — three-panel layout holds at all sizes, text truncation works, no overflow
  - All 50 screenshots saved to `docs/screenshots/` (15-50 numbered). Visual inspection passed: no layout bugs, text overflow, misaligned elements, or broken rendering found. All 29 Tauri Rust tests pass, tsc build clean

### Phase 4: LLM Integration (Week 4-5)
- [x] Anthropic API client (Messages API, extended thinking support via content block parsing)
- [x] OpenAI API client (Chat Completions, o1/o3 reasoning model detection — no system messages, max_completion_tokens)
- [x] Structured output schemas (Pass1Request/Response, Pass2Request/Response, Annotations types, JSON schema descriptions)
- [x] Migrate to provider-native structured outputs APIs — OpenAI `response_format: { type: "json_schema" }`, Anthropic tool_use with input_schema and forced tool_choice, Gemini `responseSchema` in generationConfig. `schemars` crate derives JSON Schema from Rust response types (Pass1Response, Pass2Response, JudgeResponse, RefinementResponse). Schema generation functions in schema.rs. Markdown stripping retained as defensive fallback. System prompts simplified (removed "Respond ONLY with valid JSON" — schema now enforced by provider APIs). 7 new schema tests, updated request format tests for all 3 providers. All 967+ tests pass
- [x] Pass 1: overview annotation (system prompt + user prompt builders, structured JSON output parsing with markdown fence stripping)
- [x] Pass 2: on-demand deep group analysis (per-group context with file diffs, graph context, role annotations)
- [x] LLM results rendering in Tauri app (Pass 1 overview + Pass 2 deep analysis rendering in right panel, "Annotate All Groups" and "Deep Analyze Group" buttons, `annotate_overview`/`annotate_group` IPC commands, typed TypeScript interfaces for Pass1Response/Pass2Response, mock data for demo mode, risk flag badges, file-level annotations with risks/suggestions, cross-cutting concerns, flow narrative, Catppuccin-styled LLM sections with accent border; 16 Tauri tests pass, 967 core tests pass)
- [x] API key configuration (FLOWDIFF_API_KEY env var, provider-specific env vars ANTHROPIC_API_KEY/OPENAI_API_KEY/GEMINI_API_KEY, key_cmd for 1Password via `op read`, precedence: key_cmd > FLOWDIFF_API_KEY > provider env var)
- [x] Context window management (token estimation heuristic, truncation to budget with line-boundary preservation, per-model context window sizes)
- [x] Provider trait (`LlmProvider`) with `create_provider()` factory, default provider is Anthropic, supports Anthropic/OpenAI/Gemini
- [x] Unit tests for LLM module (212 tests — 97 existing: schema roundtrips, request format, response parsing, markdown stripping, context windows, API key resolution, prompt building, error display, provider creation, reasoning model detection, Gemini generateContent format, Gemini response parsing, Gemini safety filter handling, Gemini endpoint URL construction; 36 VCR tests: record/replay Pass 1/Pass 2/judge, auto-mode caching, replay-miss errors, different-request isolation, cache key determinism and variance, SHA-256 validation, provider delegation, cache management, entry serialization roundtrip, record overwrites, template hash invalidation, nested dir creation, judge template hash tests, 6 property-based tests; 22 judge tests: build/validate/normalize, source file collection, criteria constants, 4 property-based tests; 9 new schema tests for judge types)
- [x] Live integration tests — Anthropic (real API call, Pass 1 overview returns valid structured output, Pass 2 deep analysis returns file-level annotations)
- [x] Google Gemini API client (generateContent API, system instructions, JSON response mode, safety filter handling, context window management)
- [x] Live integration tests — Google Gemini (real API call, Pass 1 overview, Pass 2 deep analysis, structured output compliance, context window handling, error handling for invalid keys)
- [x] Live integration tests — OpenAI (real API call, Pass 1 overview with gpt-4o, structured output compliance)
- [x] Live integration tests — end-to-end (full pipeline: Pass 1 + Pass 2, combined Annotations serialization roundtrip, gated behind `FLOWDIFF_RUN_LIVE_LLM_TESTS=1`)
- [x] Live integration tests — error handling (invalid API key detection for both Anthropic and OpenAI)
- [x] LLM refinement pass — optional post-analysis step that takes deterministic groups v1 and refines them. `refine_groups` method on `LlmProvider` trait. Structured output schema: `RefinementResponse { splits: [], merges: [], re_ranks: [], reclassifications: [], reasoning: string }`. `RefinementRequest { analysis_json, diff_summary, groups: [RefinementGroupInput] }`. Operations: `RefinementSplit` (split one group into sub-groups), `RefinementMerge` (combine groups), `RefinementReRank` (change review order), `RefinementReclassify` (move files between groups). `apply_refinement()` applies operations in order: reclassify → split → merge → re-rank. `validate_refinement()` checks all operations reference valid groups/files. `build_refinement_request()` builds request from `FlowGroup[]`. `has_refinements()` checks if response contains any operations. Controlled by `[llm.refinement]` config: enabled (default false), provider/model (user-selectable, can differ from annotation provider), key_cmd, max_iterations (default 1). VCR cacheable via `VcrProvider` with `refinement_template_hash()`. Falls back to deterministic v1 if LLM fails or score doesn't improve
- [x] LLM refinement tests — 48 tests total: 9 schema tests (roundtrips for all refinement types, schema description validation), 7 config tests (defaults, parsing, validation for enabled/provider/model/max_iterations/key_cmd, invalid provider rejection, zero iterations rejection), 3 prompt tests (system prompt content, user prompt with groups and analysis JSON), 12 validation tests (unknown group/source/target detection, file-not-in-group, missing files in split, file not found in reclassify, infrastructure source/target), 9 apply_refinement unit tests (empty refinement, split, merge, re-rank, reclassify to/from/between infrastructure/groups, combined operations, entrypoint preservation, empty group removal), 6 property-based tests (empty refinement preserves group count and file count, validation never fails for empty refinement, has_refinements false for empty, build_request group count, review order sequential), 2 has_refinements tests. All mock providers in VCR and judge tests updated with `refine_groups` stubs
- [x] CLI flag `--refine` to enable refinement pass (overrides config), `--refine-model <model>` to select model
- [x] Full CLI implementation (`flowdiff analyze`) with `--base`/`--head`/`--range`/`--staged`/`--unstaged`, `--output`, `--annotate`, `--refine`/`--refine-model`, `--repo` flags. Complete analysis pipeline: git diff → AST parse → graph build → entrypoint detect → flow analyze → cluster → rank → JSON output. LLM refinement via `--refine` (falls back to deterministic on failure). LLM annotation via `--annotate`. Config override from CLI flags. 17 unit tests (arg parsing, config overrides, refinement LlmConfig construction/fallback, output serialization)

### Phase 5: VS Code Extension (Week 5-6)
- [x] Extension scaffold — TypeScript + Vitest project in `extensions/vscode/`, `package.json` with commands/views/keybindings/configuration contributions, `tsconfig.json`, `.vscodeignore`, vscode mock for unit testing
- [x] CLI binary invocation and JSON parsing — `flowdiffRunner.ts` with `runFlowdiff()` async executor, `buildArgs()` CLI argument builder (branch/range/staged/unstaged/annotate/refine), `parseOutput()` JSON parser with schema validation, configurable binary path via `flowdiff.binaryPath` setting, 50MB buffer + 2min timeout
- [x] Activity bar + sidebar tree view — `treeView.ts` with `FlowGroupsProvider` (TreeDataProvider for flow groups sorted by review_order, files sorted by flow_position), `InfrastructureProvider` (infrastructure files), `GroupItem` with risk score badges (high/medium/low color-coded ThemeIcons), `FileItem` with role icons (Entrypoint/Handler/Service/Repository/Model/Test/Config), short path display, +/- change stats, click-to-diff command
- [x] Webview panel for annotations/graph — `webviewPanel.ts` with `AnnotationsPanel` class, Pass 1 overview rendering (LLM summary, risk flags, review order rationale), Pass 2 deep analysis rendering (flow narrative, per-file annotations with risks/suggestions, cross-cutting concerns), Mermaid flow graph generation with node ID reuse, VS Code theme-aware CSS styling, HTML escaping for XSS prevention
- [x] Commands: analyze, annotate, nextFile, nextGroup — 9 commands registered: `flowdiff.analyze` (branch comparison with configurable default base), `flowdiff.analyzeRange` (commit range via input box), `flowdiff.annotate` (LLM annotation with progress notification), `flowdiff.nextFile`/`prevFile` (j/k navigation within group), `flowdiff.nextGroup`/`prevGroup` (J/K navigation between groups), `flowdiff.openDiff` (native diff viewer), `flowdiff.openAnnotations` (show annotations panel). Keybindings for j/k/J/K with `flowdiff.active` context guard
- [x] Open native VS Code diff on file click — `cmdOpenDiff` uses `vscode.diff` command with git scheme URI for base ref and file URI for head, falls back to `vscode.open` if git scheme fails. File click in tree view triggers diff via TreeItem command binding
- [x] Unit tests — 68 tests across 3 test files: `flowdiffRunner.test.ts` (25 tests: buildArgs for branch/range/staged/unstaged/annotate/refine/precedence/spaces, parseOutput for valid/whitespace/empty/invalid/missing-fields/annotations/null-infra/empty-groups/field-preservation), `treeView.test.ts` (23 tests: group sorting, file sorting, risk classification with boundaries, short path generation, infrastructure group, entrypoint display, change stats, navigation state), `webviewPanel.test.ts` (20 tests: HTML escaping, risk badge levels, Mermaid graph generation including empty/single/multi-edge/node-reuse/symbol-extraction, Pass1/Pass2 annotation structure validation). vscode module mocked via vitest alias

### Phase 6: Polish & Integration (Week 6-7)
- [ ] Beyond Compare launcher integration
- [ ] Risk heatmap visualization
- [ ] Flow replay mode
- [ ] Performance optimization (rayon parallelism, caching)
- [ ] Error handling and edge cases
- [ ] README and usage documentation
- [ ] Clippy strict deny wall — add to `crates/flowdiff-core/src/lib.rs` (and all crate roots) to ban lazy escape hatches. No style obsession, just crash/debug/placeholder constraints. Fix all violations across the codebase. Add `#![deny(clippy::unwrap_used)]`, `#![deny(clippy::expect_used)]`, `#![deny(clippy::panic)]`, `#![deny(clippy::todo)]`, `#![deny(clippy::unimplemented)]`, `#![deny(clippy::dbg_macro)]`, `#![deny(clippy::print_stdout)]`, `#![deny(clippy::print_stderr)]`. Replace all `unwrap()`/`expect()` with proper `?` propagation or `map_err`, replace `todo!()`/`unimplemented!()` with real implementations or explicit error types, replace `println!`/`eprintln!` with `tracing` or `log` macros. Allow `#[allow(...)]` only in tests via `#[cfg(test)]`

### Phase 7: Synthetic Eval Suite (Future)
- [x] Synthetic fixture codebases — 5 realistic projects with pre-made code changes (branches with known diffs): (1) TypeScript Express HTTP API with services + DB layer + events, (2) Python FastAPI with SQLAlchemy + Celery queue workers, (3) Next.js fullstack with React pages + API routes + Prisma, (4) Rust CLI with modules + lib, (5) multi-language monorepo (TS frontend + Python backend)
- [x] Expected output baselines — for each fixture codebase, defined expected: flow groups (which files grouped together via `must_contain`/`must_not_contain`), entrypoints detected (file + type), languages, review ordering constraints (risk ordering), file count bounds, group count bounds. Baselines are the "ground truth" for the eval
- [x] Deterministic scoring functions — 6 per-criterion scorers all producing [0.0, 1.0]: group_coherence, entrypoint_accuracy, review_ordering, risk_reasonableness, language_detection, file_accounting. Overall weighted composite score. Eval report with per-fixture and aggregate tables
- [x] Eval tests — 16 tests: 5 per-fixture evals (minimum score thresholds), 5 cross-fixture invariant tests (determinism, JSON roundtrip, file accounting, risk bounds, Mermaid generation), 5 property-based tests for scoring function invariants (bounds, min/max containment, empty input safety, determinism, perfect match high score), 1 aggregate report. Current avg score: 0.89
- [x] VCR caching layer for LLM calls — `vcr.rs` implements `LlmProvider` as a decorator wrapping any real provider. Three modes: Record (always call provider, save to disk), Replay (read from cache only, error on miss), Auto (cache-through: use cache if available, call provider on miss and cache result). Cache keyed by SHA-256 hash of (provider, model, serialized request JSON, prompt template hash). Automatic cache invalidation when system prompt templates change. Disk-based JSON cache with `CacheEntry<T>` wrapper (provider, model, request_hash, prompt_template_hash, recorded_at, response). Cache management: `list_entries()`, `clear_cache()`. 29 unit tests (23 behavioral + 6 property-based via proptest: SHA-256 output invariants, cache key determinism, collision resistance, serde roundtrip, never-panics). 6 integration tests (3 non-live: pre-recorded fixture replay for Pass 1/Pass 2, auto-mode record-replay cycle; 3 live: real Anthropic API record→replay for Pass 1, Pass 2, and full pipeline — gated behind `FLOWDIFF_RUN_LIVE_LLM_TESTS=1`)
- [x] LLM-as-judge evaluator — `judge.rs` module under `llm/` with orchestration functions. Structured output types (JudgeRequest/JudgeResponse/JudgeCriterionScore in schema.rs). 5 evaluation criteria scored 1-5: group_coherence, review_ordering, entrypoint_identification, risk_reasonableness, mermaid_accuracy. `evaluate_quality` method on LlmProvider trait implemented in all 3 providers (Anthropic, OpenAI, Gemini). VCR caching support for deterministic CI replay. Judge prompt builders (system + user). Validation (completeness, bounds, average consistency, failure explanation coverage). Score normalization (1-5 → 0.0-1.0). Source file collection from fixture repos. Formatted report printer. 22 unit tests (build_judge_request, validate_judge_response for valid/missing/bounds/mismatch/explanations, normalize_judge_scores for all-5s/all-1s/mixed, collect_source_files with hidden dirs/binary skip/sorting/empty, JUDGE_CRITERIA count+uniqueness, 4 property-based tests: normalize bounds, validate never panics, build_request never panics, normalization determinism). 10 integration tests (mock judge valid response/validation/normalization, VCR record-replay/auto-mode, fixture source file collection, request construction, analysis field completeness, 2 live Anthropic tests gated behind FLOWDIFF_RUN_LIVE_LLM_TESTS=1). 7 VCR judge tests (record-replay, auto-mode caching, replay-miss errors, template hash determinism, template hash differs from pass1/pass2)
- [ ] Eval harness — CLI command (`flowdiff eval`) that runs all fixture codebases, compares against baselines, runs LLM judge, produces a score report. Tracks scores over time to detect regressions. CI integration: fail if overall score drops below threshold
- [ ] Eval dashboard — simple HTML report showing per-fixture scores, per-criterion breakdown, historical trend, diff against last run

### Phase 8: Hardening (Future)

Automated bug-finding phase. Run parallel sub-agents that independently audit each layer of the application, find bugs, and add new tasks to the spec for anything they discover.

- [~] Rust core audit — graph.rs audited: 75 new tests added (48→123 total), covering helper functions (normalize_path, normalize_python_import, file_stem, parent_dir, resolve_import_path), IR extends edges (local/imported/multiple/chain inheritance, self-edge prevention, missing base), IR node types (all TypeDefKind→SymbolKind mappings, constants, duplicate handling), edge cases (unicode paths/symbols, 50-file scale, invalid serialization, multiple importers), property-based fuzzing (edge validity, roundtrip preservation, never-panic on arbitrary input). Remaining: ast.rs, cluster.rs, query_engine.rs audits, clippy, unwrap/expect scan
- [ ] Query engine + .scm audit — sub-agent reviews all `.scm` query files against tree-sitter grammar docs for each language, looks for: missing AST node types (e.g. `satisfies` expressions in TS, walrus operator in Python), incorrect capture names, patterns that silently fail on syntax errors, patterns that don't match newer language features, overlapping patterns that cause duplicate results
- [ ] LLM provider audit — sub-agent tests all three providers (Anthropic, OpenAI, Gemini) with adversarial inputs: huge diffs that exceed context windows, malformed JSON responses, rate limit handling, timeout handling, API key rotation during a session, structured output schema violations, unicode/emoji in code, concurrent requests
- [ ] Tauri app audit — sub-agent reviews React components for: missing error boundaries, memory leaks from Monaco instances, unhandled IPC errors, XSS via unsanitized diff content in Mermaid/Monaco, keyboard event conflicts with Monaco, state desync between panels, large dataset rendering perf (100+ groups, 1000+ files)
- [ ] VS Code extension audit — sub-agent reviews for: extension activation failures, CLI binary not found, JSON parsing failures on malformed CLI output, webview CSP issues, tree view memory leaks on large results, command palette conflicts
- [ ] Cross-layer integration audit — sub-agent runs full pipeline (CLI → Rust core → JSON → Tauri IPC → React render) on adversarial repos: empty repos, repos with only binary files, repos with 10K+ files, repos with deeply nested circular imports, repos with non-UTF8 filenames, repos with symlinks, monorepos with 50+ packages
- [ ] Security audit — sub-agent checks for: command injection via repo paths or filenames, path traversal in file reads, SSRF via LLM API URLs from config, secret leakage in JSON output or logs, unsafe deserialization of LLM responses, CSP violations in Tauri/VS Code webviews
- [ ] Aggregate findings — collect all bugs found by sub-agents, deduplicate, prioritize by severity, add as new tasks to the appropriate phase in this spec

## 12. Testing Plan

### 12.1 Test Convention

**Rust convention — structural separation, not file suffixes:**

- **Unit tests** — co-located in the source file via `#[cfg(test)] mod tests { }` at the bottom. Tests internal/private functions. Fast, no I/O.
- **Integration tests** — separate `tests/` directory at the crate root. Each file compiles as its own binary and can only access the crate's public API. Tests cross-module behavior.
- **Slow/live tests** — gated with `#[ignore]`, run via `cargo test -- --ignored`. Includes live LLM calls, large fixture repos, performance benchmarks.

```
crates/flowdiff-core/
├── src/
│   ├── lib.rs              # pub API surface
│   ├── ast.rs              # #[cfg(test)] mod tests { } at bottom (unit)
│   ├── graph.rs            # same
│   ├── ir.rs               # same
│   ├── query_engine.rs     # same
│   └── ...
└── tests/                  # integration tests (public API only)
    ├── e2e_pipeline.rs         # full pipeline: git → AST → IR → graph → cluster → rank → output
    ├── e2e_real_repos.rs       # test against synthetic fixture repos with real git commits
    ├── e2e_llm_live.rs         # live LLM provider tests (#[ignore], gated behind FLOWDIFF_RUN_LIVE_LLM_TESTS=1)
    ├── e2e_eval.rs             # eval suite scoring against fixture baselines
    ├── ir_parity.rs            # IR path vs ParsedFile path produce identical results
    ├── vcr_replay.rs           # VCR cached LLM response replay tests
    └── helpers/
        ├── mod.rs              # shared test utilities
        ├── repo_builder.rs     # programmatically create test git repos
        └── graph_assertions.rs # custom assertions for graph structures
```

**Frontend convention:**

- **Unit tests** — co-located as `Component.test.tsx` next to `Component.tsx` (Vitest + React Testing Library). Tests component logic, state, props.
- **Integration tests** — `tests/integration/` at the Tauri UI root. Tests IPC bridge, store ↔ component wiring.
- **E2E tests** — `tests/e2e/` using Playwright. Tests real rendered output in a browser context. **Prefer integration/E2E tests over unit tests when code touches renderers** (Monaco, Mermaid, Tauri webview) — mocked renderers give false confidence.

```
crates/flowdiff-tauri/ui/
├── src/
│   ├── components/
│   │   ├── FlowGroups.tsx
│   │   ├── FlowGroups.test.tsx     # unit test (co-located)
│   │   ├── DiffViewer.tsx
│   │   ├── DiffViewer.test.tsx
│   │   └── ...
│   ├── hooks/
│   │   ├── useFlowdiff.ts
│   │   └── useFlowdiff.test.ts
│   └── store/
│       ├── store.ts
│       └── store.test.ts
├── tests/
│   ├── integration/
│   │   ├── ipc-contract.test.ts        # IPC response matches Rust AnalysisOutput schema
│   │   ├── store-component.test.ts     # store updates → component re-renders
│   │   └── monaco-lifecycle.test.ts    # Monaco instances created/destroyed correctly
│   └── e2e/
│       ├── analyze-flow.spec.ts        # full user journey (Playwright)
│       ├── keyboard-navigation.spec.ts
│       ├── monaco-diff.spec.ts
│       ├── mermaid-graph.spec.ts
│       ├── layout.spec.ts
│       ├── error-states.spec.ts
│       └── visual.spec.ts             # screenshot regression tests
└── playwright.config.ts
```

**Running tests:**

```bash
# Rust unit tests (fast, co-located)
cargo test --workspace

# Rust integration tests (slower, real git repos)
cargo test --workspace -- --ignored

# Live LLM tests (requires API keys)
FLOWDIFF_RUN_LIVE_LLM_TESTS=1 cargo test --workspace -- --ignored

# Frontend unit + integration tests
cd crates/flowdiff-tauri/ui && npm test

# Frontend E2E (Playwright)
cd crates/flowdiff-tauri/ui && npx playwright test

# VS Code extension tests
cd extensions/vscode && npm test
```

### 12.2 Test Infrastructure

**Framework:** `cargo test` for Rust, Vitest + React Testing Library for Tauri UI unit/integration, Playwright for Tauri E2E, Jest for VS Code extension

**Test fixtures directory:**
```
tests/
├── fixtures/
│   ├── repos/                    # Synthetic git repos (created by test setup)
│   │   ├── simple-ts-app/        # 5-file Express app with clear data flow
│   │   ├── nextjs-fullstack/     # Next.js + Prisma, 20+ files
│   │   ├── python-fastapi/       # FastAPI + SQLAlchemy, 15+ files
│   │   ├── multi-entrypoint/     # App with HTTP + queue + cron entrypoints
│   │   ├── monorepo/             # Workspace with shared packages
│   │   └── rename-heavy/         # PR with lots of file renames
│   ├── diffs/                    # Pre-computed diff snapshots
│   │   ├── 50-file-agent-pr.patch
│   │   └── cross-cutting-refactor.patch
│   ├── graphs/                   # Expected graph structures (JSON)
│   │   ├── simple-ts-app.expected.json
│   │   └── nextjs-fullstack.expected.json
│   └── llm-responses/            # Fixture LLM responses for mock testing
│       ├── pass1-overview.json
│       └── pass2-group-detail.json
├── helpers/
│   ├── repo_builder.rs           # Programmatically create test git repos
│   └── graph_assertions.rs       # Custom assertions for graph structures
```

**Fixture repo builder:** A test helper that programmatically creates git repos with known structure, commits changes, and produces diffs with predictable flow groupings. This is critical — it makes tests deterministic and self-contained.

### 12.2 Unit Tests — Core Engine

#### Git Layer (`git.rs`)
| Test | What it verifies |
|------|-----------------|
| `test_diff_branch_comparison` | Extracts correct file list and hunks from branch comparison |
| `test_diff_commit_range` | Handles `HEAD~N..HEAD` ranges correctly |
| `test_diff_staged_changes` | Reads staged (index) changes from working tree |
| `test_diff_unstaged_changes` | Reads unstaged (working directory) changes |
| `test_diff_file_rename` | Detects renames and tracks old→new paths |
| `test_diff_binary_files_skipped` | Binary files excluded from analysis |
| `test_diff_empty_repo` | Graceful handling of empty/no-commit repos |
| `test_diff_deleted_files` | Correctly handles fully deleted files |
| `test_diff_new_files` | Handles newly added files (no old version) |

#### AST Layer (`ast.rs`)
| Test | What it verifies |
|------|-----------------|
| `test_parse_ts_imports` | Extracts named, default, and namespace imports from TypeScript |
| `test_parse_ts_exports` | Extracts named, default, and re-exports |
| `test_parse_ts_functions` | Extracts function declarations, arrow functions, methods |
| `test_parse_ts_call_sites` | Identifies function call expressions with resolved targets |
| `test_parse_python_imports` | Handles `import x`, `from x import y`, relative imports |
| `test_parse_python_functions` | Extracts functions, methods, decorators |
| `test_parse_python_class_hierarchy` | Detects class inheritance |
| `test_parse_rust_modules` | Handles `mod`, `use`, `pub` visibility |
| `test_parse_unknown_language` | Falls back gracefully for unsupported file types |
| `test_changed_symbols_detection` | Correctly identifies which symbols were added/modified/removed between old and new AST |
| `test_large_file_performance` | Parses a 10K+ line file within acceptable time (<500ms) |

#### Graph Layer (`graph.rs`)
| Test | What it verifies |
|------|-----------------|
| `test_build_import_edges` | Creates correct `imports` edges between files |
| `test_build_call_edges` | Creates `calls` edges from call site analysis |
| `test_build_extends_edges` | Creates `extends` edges from class inheritance |
| `test_cyclic_imports` | Handles circular dependencies without infinite loop |
| `test_cross_package_edges` | Resolves imports across monorepo package boundaries |
| `test_dynamic_imports` | Handles `import()` / `require()` dynamic imports |
| `test_reexport_chains` | Traces through barrel files (`index.ts` re-exports) |
| `test_graph_node_count` | Correct vertex count for known fixture |
| `test_graph_edge_count` | Correct edge count for known fixture |
| `test_graph_serialization_roundtrip` | Graph → JSON → Graph is lossless |

#### Flow Layer (`flow.rs`)
| Test | What it verifies |
|------|-----------------|
| `test_trace_param_flow` | Traces a parameter from function A through call to function B |
| `test_trace_return_value` | Tracks return value from callee back to caller |
| `test_trace_variable_assignment` | Follows `const x = foo(); bar(x)` chains |
| `test_heuristic_db_write` | Detects `.save()`, `.insert()`, `INSERT INTO` as persistence |
| `test_heuristic_http_handler` | Detects Express `app.get()`, FastAPI `@app.route` as entrypoints |
| `test_heuristic_event_emission` | Detects `.emit()`, `.publish()`, `.send()` as emission edges |
| `test_heuristic_config_read` | Detects `process.env`, `os.environ` as config reads |
| `test_no_false_positive_heuristics` | Common patterns that look like but aren't DB writes/handlers |
| `test_flow_depth_limit` | Tracing stops at configurable depth to prevent runaway |

#### Cluster Layer (`cluster.rs`)
| Test | What it verifies |
|------|-----------------|
| `test_single_entrypoint_group` | All files reachable from one entrypoint form one group |
| `test_multiple_entrypoints` | Distinct entrypoints produce distinct groups |
| `test_shared_file_assignment` | File reachable from 2 entrypoints assigned to nearest |
| `test_infrastructure_group` | Files not reachable from any entrypoint go to infrastructure |
| `test_empty_diff` | No files changed → no groups |
| `test_all_infrastructure` | No entrypoints detected → everything is infrastructure |
| `test_disconnected_components` | Handles files with no edges to anything |
| `test_group_file_ordering` | Files within a group are ordered by flow position (entrypoint first, downstream next) |
| `test_deterministic_output` | Same input always produces same grouping (no random ordering) |

#### Rank Layer (`rank.rs`)
| Test | What it verifies |
|------|-----------------|
| `test_risk_scoring_schema_change` | DB migration or schema file change → high risk |
| `test_risk_scoring_auth` | Auth/security file changes → high risk |
| `test_risk_scoring_test_only` | Test-only changes → low risk |
| `test_centrality_hub_node` | File imported by many others → high centrality |
| `test_centrality_leaf_node` | Leaf file with no dependents → low centrality |
| `test_surface_area` | More changed lines → higher surface area score |
| `test_composite_score` | Weighted combination produces expected ranking |
| `test_custom_weights` | Config-provided weights override defaults |
| `test_ranking_stability` | Same input → same ranking (deterministic) |
| `test_single_group_ranking` | One group still gets a valid score |

#### Config Layer (`config.rs`)
| Test | What it verifies |
|------|-----------------|
| `test_parse_valid_config` | Parses well-formed `.flowdiff.toml` |
| `test_missing_config` | Works fine without config file (auto-detect) |
| `test_partial_config` | Handles config with only some sections |
| `test_invalid_config` | Clear error message on malformed TOML |
| `test_entrypoint_globs` | Glob patterns in config resolve to correct files |
| `test_ignore_patterns` | Ignored files excluded from analysis |
| `test_custom_layer_names` | Layer names from config used in group naming |

#### Output Layer (`output.rs`)
| Test | What it verifies |
|------|-----------------|
| `test_json_schema_compliance` | Output matches documented JSON schema exactly |
| `test_mermaid_generation` | Valid Mermaid syntax generated for flow graphs |
| `test_empty_annotations_field` | `annotations` is `null` when LLM not used |
| `test_output_file_write` | `-o` flag writes to file correctly |
| `test_stdout_output` | Default outputs to stdout |

### 12.3 Unit Tests — LLM Layer

| Test | What it verifies |
|------|-----------------|
| `test_anthropic_request_format` | Builds correct Messages API request with extended thinking |
| `test_openai_request_format` | Builds correct Chat Completions request for o1/o3 |
| `test_structured_output_schema` | JSON schema sent to API matches expected structure |
| `test_parse_pass1_response` | Correctly deserializes Pass 1 overview response |
| `test_parse_pass2_response` | Correctly deserializes Pass 2 deep analysis response |
| `test_api_error_handling` | Rate limits, timeouts, auth errors handled gracefully |
| `test_api_key_from_env` | Reads `FLOWDIFF_API_KEY` from environment |
| `test_api_key_from_config` | Reads key from `.flowdiff.toml` |
| `test_api_key_from_op` | Reads key via `op read` command |
| `test_context_window_truncation` | Large diffs truncated to fit provider context window |
| `test_mock_anthropic_pass1` | Full Pass 1 flow with mock HTTP responses |
| `test_mock_openai_pass2` | Full Pass 2 flow with mock HTTP responses |

### 12.4 Integration Tests — End-to-End Pipeline

These tests create real git repos, make real commits, and run the full pipeline.

| Test | Setup | Verification |
|------|-------|-------------|
| `test_e2e_simple_express_app` | Create 5-file Express app, add a new route with handler→service→repo | Produces 1 flow group with files in correct order: route→service→repo |
| `test_e2e_nextjs_page_change` | Create Next.js app, modify a page + API route + Prisma model | Produces 2 groups: API flow and UI flow, correctly separated |
| `test_e2e_python_fastapi` | Create FastAPI app, add endpoint + schema + repository | Correct entrypoint detection, DB write heuristic fires |
| `test_e2e_cross_cutting_refactor` | Rename a shared utility used by 30 files | All 30 files in infrastructure/shared group, low risk |
| `test_e2e_multi_entrypoint` | Change code touched by both HTTP handler and queue worker | 2 flow groups, shared file assigned to nearest entrypoint |
| `test_e2e_50_file_diff` | Apply a 50-file patch to a synthetic app | Completes within 5 seconds, produces reasonable groupings |
| `test_e2e_100_file_diff` | Apply a 100-file patch | Completes within 15 seconds, no OOM |
| `test_e2e_branch_comparison` | `--base main --head feature` | Correct diff extraction and grouping |
| `test_e2e_commit_range` | `--range HEAD~3..HEAD` | All 3 commits' changes included |
| `test_e2e_staged_changes` | Stage some files, leave others unstaged | `--staged` only includes staged files |
| `test_e2e_json_output_valid` | Any analysis run | Output parses as valid JSON matching schema |
| `test_e2e_no_changes` | Run on repo with no diff | Graceful empty result, not an error |
| `test_e2e_config_overrides` | Provide `.flowdiff.toml` with custom entrypoints | Config entrypoints detected even if heuristics miss them |

### 12.5 Snapshot Tests

Use `insta` crate for snapshot testing of JSON output.

For each fixture repo:
1. Run `flowdiff analyze`
2. Compare JSON output against stored snapshot
3. If graph construction or ranking algorithm changes, review and approve new snapshots

This catches unintended regressions in grouping or ranking logic.

```rust
#[test]
fn test_snapshot_simple_app() {
    let result = analyze_fixture("simple-ts-app");
    insta::assert_json_snapshot!(result);
}
```

### 12.6 Property-Based Tests

Use `proptest` crate for fuzzing graph construction and ranking.

| Property | Description |
|----------|-------------|
| Every changed file appears in exactly one group | No file lost, no file duplicated |
| Group file order is topologically valid | No file appears before its dependency within the same group |
| Ranking scores are in [0.0, 1.0] | No score exceeds bounds |
| Ranking is total order | No two groups have identical rank (tie-break is deterministic) |
| Empty diff → empty groups | No phantom groups from empty input |
| Single file diff → single group | Minimal case always works |
| Graph with no edges → all infrastructure | Disconnected files go to infrastructure group |
| Determinism | `analyze(X) == analyze(X)` for any input (run 10 times) |

### 12.7 Performance Benchmarks

Use `criterion` crate for benchmarking critical paths.

| Benchmark | Target |
|-----------|--------|
| `bench_parse_100_ts_files` | < 2s |
| `bench_graph_construction_100_files` | < 500ms |
| `bench_clustering_100_nodes` | < 100ms |
| `bench_ranking_20_groups` | < 10ms |
| `bench_full_pipeline_50_files` | < 5s total |
| `bench_full_pipeline_100_files` | < 15s total |
| `bench_json_serialization` | < 50ms for 100-file output |

Benchmarks run in CI to catch performance regressions.

### 12.8 Tauri App Tests

**React component tests (Vitest + React Testing Library):**

| Test | What it verifies |
|------|-----------------|
| `FlowGroups.test.tsx` | Renders group tree from JSON, expand/collapse works |
| `FlowGroups.test.tsx` | Click file dispatches correct event |
| `FlowGroups.test.tsx` | Groups sorted by risk score descending |
| `DiffViewer.test.tsx` | Monaco initializes with correct diff content |
| `DiffViewer.test.tsx` | Syntax highlighting matches file extension |
| `Annotations.test.tsx` | Renders risk badges, flow description, Mermaid graph |
| `Annotations.test.tsx` | "Annotate" button triggers LLM call |
| `FlowNavigation.test.tsx` | j/k navigates files, J/K navigates groups |
| `MermaidGraph.test.tsx` | Renders valid Mermaid diagram from graph data |
| `App.test.tsx` | Three panels render in correct layout |

**Tauri IPC tests:**

| Test | What it verifies |
|------|-----------------|
| `test_analyze_command` | Tauri `analyze` command returns valid JSON |
| `test_annotate_command` | Tauri `annotate` command triggers LLM and returns annotations |
| `test_ipc_error_handling` | Invalid repo path returns user-friendly error |
| `test_ipc_slow_analysis` | Shows loading state during long analysis |
| `test_ipc_cancellation` | Cancelling mid-analysis cleans up gracefully |
| `test_ipc_schema_match` | IPC response matches Rust `AnalysisOutput` JSON schema exactly |

**State management tests:**

| Test | What it verifies |
|------|-----------------|
| `store.test.ts` | Store initializes correctly from analysis JSON |
| `store.test.ts` | Selecting a file updates active file + diff content |
| `store.test.ts` | Selecting a group expands it and selects first file |
| `store.test.ts` | Store handles empty groups array gracefully |
| `store.test.ts` | Store handles missing/null annotations |

**Keyboard navigation edge case tests:**

| Test | What it verifies |
|------|-----------------|
| `FlowNavigation.test.tsx` | `j` at last file in group wraps or stops (not crash) |
| `FlowNavigation.test.tsx` | `K` at first group wraps or stops (not crash) |
| `FlowNavigation.test.tsx` | Keyboard nav disabled when Monaco editor is focused |
| `FlowNavigation.test.tsx` | Rapid key presses don't cause double navigation |
| `FlowNavigation.test.tsx` | Navigation works with single-file groups |

**Monaco integration tests:**

| Test | What it verifies |
|------|-----------------|
| `DiffViewer.test.tsx` | Diff renders correctly for TS, Python, Rust, JSON |
| `DiffViewer.test.tsx` | Large file (10K+ lines) renders without freezing |
| `DiffViewer.test.tsx` | Scroll position preserved when switching between files in same group |
| `DiffViewer.test.tsx` | Inline LLM annotations render at correct line numbers |

**Mermaid edge case tests:**

| Test | What it verifies |
|------|-----------------|
| `MermaidGraph.test.tsx` | Handles cyclic graphs without infinite rendering |
| `MermaidGraph.test.tsx` | Handles 50+ node graphs without overflow |
| `MermaidGraph.test.tsx` | Special characters in node labels are escaped |
| `MermaidGraph.test.tsx` | Empty graph renders placeholder message |

**Layout and responsive tests:**

| Test | What it verifies |
|------|-----------------|
| `App.test.tsx` | Panels resize correctly via drag handle |
| `App.test.tsx` | Panels enforce minimum widths |
| `App.test.tsx` | Panel collapse/expand toggles work |
| `App.test.tsx` | Focus management moves correctly between panels (Tab/Shift+Tab) |

**Accessibility tests:**

| Test | What it verifies |
|------|-----------------|
| `App.test.tsx` | Full keyboard-only navigation works end-to-end (no mouse required) |
| `FlowGroups.test.tsx` | Tree items have ARIA labels and roles |
| `DiffViewer.test.tsx` | Monaco instance has accessible label |
| `Annotations.test.tsx` | Risk badges have screen-reader text |

### 12.9 Tauri App — Playwright E2E Tests

**Testing philosophy:** Prefer integration tests over unit tests when code touches renderers (Monaco, Mermaid, Tauri webview). Unit tests with mocked renderers give false confidence — Playwright tests hit the real rendered output in a real browser context.

**Setup:** Playwright tests launch the Tauri app via `tauri-driver` (WebDriver protocol) or directly against the dev server with mocked IPC. Test fixtures use pre-computed analysis JSON from the synthetic eval codebases (Phase 7).

**Full workflow E2E tests:**

| Test | What it verifies |
|------|-----------------|
| `e2e/analyze-flow.spec.ts` | Open app → load analysis → flow groups appear in left panel → click group → files expand → click file → diff renders in Monaco → annotations show in right panel |
| `e2e/keyboard-navigation.spec.ts` | Load analysis → press `j` → next file selected + diff updates → press `J` → next group selected → press `k` → previous file → press `K` → previous group → verify focus + scroll position at each step |
| `e2e/annotate-flow.spec.ts` | Load analysis → click "Annotate" on a group → loading spinner appears → LLM annotations render in right panel → risk badges update → Mermaid graph updates |
| `e2e/multi-group-review.spec.ts` | Load 50-file analysis → verify all groups render → navigate through every group sequentially → verify no stale state between groups → verify Monaco doesn't leak instances |

**Monaco renderer integration tests:**

| Test | What it verifies |
|------|-----------------|
| `e2e/monaco-diff.spec.ts` | Diff renders with correct old/new content (screenshot comparison) |
| `e2e/monaco-diff.spec.ts` | Syntax highlighting is correct for TypeScript, Python, Rust, JSON (visual regression) |
| `e2e/monaco-diff.spec.ts` | Inline annotations render at correct line positions (check DOM line decorations) |
| `e2e/monaco-diff.spec.ts` | Switching files updates Monaco without creating duplicate editor instances (check DOM node count) |
| `e2e/monaco-diff.spec.ts` | Large file (10K lines) renders within 2s and scrolling is smooth (performance assertion) |

**Mermaid renderer integration tests:**

| Test | What it verifies |
|------|-----------------|
| `e2e/mermaid-graph.spec.ts` | Flow graph SVG renders in right panel (check SVG element exists) |
| `e2e/mermaid-graph.spec.ts` | Graph nodes match files in the selected group |
| `e2e/mermaid-graph.spec.ts` | Clicking a node in the Mermaid graph selects the corresponding file |
| `e2e/mermaid-graph.spec.ts` | Graph updates when switching between groups |
| `e2e/mermaid-graph.spec.ts` | Cyclic graph renders without hanging (timeout assertion) |

**Panel layout integration tests:**

| Test | What it verifies |
|------|-----------------|
| `e2e/layout.spec.ts` | Three panels visible on load with correct proportions (measure widths) |
| `e2e/layout.spec.ts` | Drag resize handle → panels resize → Monaco reflows (no overflow) |
| `e2e/layout.spec.ts` | Collapse left panel → center + right expand → expand again → original widths restored |
| `e2e/layout.spec.ts` | Window resize → panels reflow proportionally → no horizontal scroll |

**Error state E2E tests:**

| Test | What it verifies |
|------|-----------------|
| `e2e/error-states.spec.ts` | Invalid repo path → user-friendly error message in UI (not blank screen) |
| `e2e/error-states.spec.ts` | Empty diff → "No changes found" message with helpful guidance |
| `e2e/error-states.spec.ts` | LLM annotation failure → error toast, app still functional |
| `e2e/error-states.spec.ts` | Corrupted analysis JSON → error boundary catches, recovery option shown |

**Visual regression tests (Playwright screenshots):**

| Test | What it verifies |
|------|-----------------|
| `e2e/visual.spec.ts` | Full app screenshot matches baseline after loading analysis |
| `e2e/visual.spec.ts` | Dark mode / light mode rendering (if supported) |
| `e2e/visual.spec.ts` | Risk heatmap colors render correctly |
| `e2e/visual.spec.ts` | Mermaid graph layout is stable (no random repositioning between runs) |

### 12.10 VS Code Extension Tests

**Unit tests (Jest):**

| Test | What it verifies |
|------|-----------------|
| `flowdiffRunner.test.ts` | Spawns CLI binary with correct args |
| `flowdiffRunner.test.ts` | Parses CLI JSON output into typed objects |
| `flowdiffRunner.test.ts` | Handles CLI errors (non-zero exit, invalid JSON) |
| `treeView.test.ts` | Builds correct tree from analysis result |
| `treeView.test.ts` | Tree items have correct icons and descriptions |
| `webviewPanel.test.ts` | Generates correct HTML for annotations |

**Integration tests (VS Code Extension Test API):**

| Test | What it verifies |
|------|-----------------|
| `test_activate` | Extension activates without errors |
| `test_analyze_command` | `flowdiff.analyze` command runs and populates tree view |
| `test_open_diff` | Clicking tree item opens VS Code diff editor |
| `test_next_file_command` | `flowdiff.nextFile` navigates to correct file |

### 12.10 LLM Integration Tests (Live, Optional)

These tests hit real APIs and are gated behind `FLOWDIFF_RUN_LIVE_LLM_TESTS=1`.

| Test | What it verifies |
|------|-----------------|
| `test_live_anthropic_pass1` | Real Anthropic API call returns valid structured output |
| `test_live_openai_pass1` | Real OpenAI API call returns valid structured output |
| `test_live_anthropic_pass2` | Deep analysis returns file-level annotations |
| `test_live_reasoning_model` | Extended thinking / o3 models produce richer output |
| `test_live_structured_output_compliance` | Response conforms to schema (no hallucinated fields) |

### 12.11 Regression Test Suite

Maintained list of real-world diffs that previously caused issues:

```
tests/regressions/
├── 001-barrel-file-explosion/     # index.ts re-exporting 50 modules
├── 002-circular-dependency/       # A→B→C→A import cycle
├── 003-dynamic-import/            # import() not detected
├── 004-monorepo-cross-package/    # imports across workspace packages
├── 005-file-rename-chain/         # A renamed to B, B renamed to C
├── 006-generated-code/            # Large generated files dominating analysis
└── 007-mixed-language-project/    # TS + Python + Rust in same repo
```

Each regression test:
1. Has a `setup.sh` that creates the problematic repo state
2. Has an `expected.json` with correct output
3. Runs in CI to prevent regression

### 12.12 CI Pipeline

```yaml
# Runs on every PR
test-core:
  - cargo test --workspace
  - cargo test --workspace -- --ignored  # slow integration tests

test-snapshots:
  - cargo insta test

test-benchmarks:
  - cargo bench -- --output-format=criterion  # compare against baseline

test-tauri-ui:
  - cd crates/flowdiff-tauri/ui && npm test

test-vscode:
  - cd extensions/vscode && npm test

# Runs nightly or on-demand
test-live-llm:
  - FLOWDIFF_RUN_LIVE_LLM_TESTS=1 cargo test llm_live

# Runs on release
test-binary-artifacts:
  - Build CLI for linux/mac/windows
  - Run e2e tests against built binaries
  - Test Tauri app launches on each platform
```

### 12.13 Manual Acceptance Testing Checklist

Before each release, run through manually:

- [ ] Clone a real project with 50+ file PR, run `flowdiff analyze --base main`
- [ ] Verify flow groups intuitively match the PR's logical changes
- [ ] Verify files within each group are in reasonable data flow order
- [ ] Open Tauri app, navigate all three panels
- [ ] Keyboard nav (j/k/J/K) works smoothly
- [ ] Monaco diff viewer shows correct old/new with syntax highlighting
- [ ] Mermaid graph renders and matches the flow group
- [ ] Click "Annotate" → LLM returns structured annotations
- [ ] Annotations display in right panel
- [ ] VS Code extension: run `flowdiff.analyze`, verify tree view populates
- [ ] VS Code: click file in tree → native diff editor opens
- [ ] VS Code: `flowdiff.nextFile` advances through flow
- [ ] Run on a Python project — verify tree-sitter + heuristics work
- [ ] Run on a monorepo — verify cross-package edges resolve
- [ ] Run with no config file — auto-detection works
- [ ] Run with `.flowdiff.toml` — overrides apply correctly
- [ ] Run on empty diff — graceful "no changes" message
- [ ] Performance: 100-file diff completes in under 15 seconds
