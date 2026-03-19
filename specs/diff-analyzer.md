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

All input is git-based. Three modes:

| Mode | CLI Flag | Description |
|------|----------|-------------|
| Branch comparison | `--base main --head feature` | Compare two refs |
| Commit range | `--range HEAD~5..HEAD` | Review a range of commits |
| Working tree | `--staged` / `--unstaged` | Review uncommitted changes |

Implementation: `git2` crate for all git operations. No shelling out to `git`.

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
provider = "anthropic"  # or "openai"
model = "claude-3-7-sonnet-20250219"
# API key via FLOWDIFF_API_KEY env var or:
# key_cmd = "op read op://vault/flowdiff/api-key"

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
- [x] Tree-sitter AST parsing (TS/JS + Python grammars first)
- [x] Symbol graph construction (imports, exports, calls)
- [x] Basic entrypoint detection
- [x] Semantic clustering (forward reachability from entrypoints)
- [x] Review ranking (composite score)
- [x] JSON output (output.rs — AnalysisOutput builder, JSON serialization, Mermaid diagram generation, 42 tests)
- [x] CLI with clap (`flowdiff analyze --base main`)
- [ ] Test against a real multi-file PR
- [x] Core data types (types.rs — FlowGroup, FileChange, Symbol, Edge, etc.)
- [x] Property-based tests for ranking (proptest — 11 properties)
- [x] Unit tests for ranking (26 tests — scoring, risk, surface area, path detection)
- [x] Unit tests for AST parsing (25 tests — TS/JS imports, exports, definitions, calls; Python imports, functions, class hierarchy; changed symbol detection; performance)
- [x] Unit tests for graph construction (25 tests — import edges, call edges, namespace/default/aliased imports, cyclic imports, re-export chains, index file resolution, cross-directory imports, Python imports/calls, serialization roundtrip, node lookup, determinism)
- [x] Property-based tests for graph construction (6 tests — every definition has node, node count ≥ file count, no self-edges, serialization roundtrip, determinism, empty input)
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
- [ ] Full data flow tracing (parameters, return values, variable assignments across call chains)
- [x] Config file support (config.rs — `.flowdiff.toml` parsing, validation, defaults merging, entrypoint glob resolution, ignore patterns, layer names, LLM config; 17 unit tests + 6 property-based tests)

### Phase 3: Tauri App (Week 3-4)
- [ ] Tauri project setup with React frontend
- [ ] Three-panel layout shell
- [ ] Left panel: flow group tree view
- [ ] Center panel: Monaco diff viewer integration
- [ ] Right panel: annotations and graph display
- [ ] Mermaid rendering
- [ ] Keyboard navigation (j/k/J/K)
- [ ] File navigation within flow groups

### Phase 4: LLM Integration (Week 4-5)
- [x] Anthropic API client (Messages API, extended thinking support via content block parsing)
- [x] OpenAI API client (Chat Completions, o1/o3 reasoning model detection — no system messages, max_completion_tokens)
- [x] Structured output schemas (Pass1Request/Response, Pass2Request/Response, Annotations types, JSON schema descriptions)
- [x] Pass 1: overview annotation (system prompt + user prompt builders, structured JSON output parsing with markdown fence stripping)
- [x] Pass 2: on-demand deep group analysis (per-group context with file diffs, graph context, role annotations)
- [ ] LLM results rendering in Tauri app
- [x] API key configuration (FLOWDIFF_API_KEY env var, provider-specific env vars ANTHROPIC_API_KEY/OPENAI_API_KEY/GEMINI_API_KEY, key_cmd for 1Password via `op read`, precedence: key_cmd > FLOWDIFF_API_KEY > provider env var)
- [x] Context window management (token estimation heuristic, truncation to budget with line-boundary preservation, per-model context window sizes)
- [x] Provider trait (`LlmProvider`) with `create_provider()` factory, default provider is Anthropic, supports Anthropic/OpenAI/Gemini
- [x] Unit tests for LLM module (97 tests — schema roundtrips, request format, response parsing, markdown stripping, context windows, API key resolution, prompt building, error display, provider creation, reasoning model detection, Gemini generateContent format, Gemini response parsing, Gemini safety filter handling, Gemini endpoint URL construction)
- [x] Live integration tests — Anthropic (real API call, Pass 1 overview returns valid structured output, Pass 2 deep analysis returns file-level annotations)
- [x] Google Gemini API client (generateContent API, system instructions, JSON response mode, safety filter handling, context window management)
- [x] Live integration tests — Google Gemini (real API call, Pass 1 overview, Pass 2 deep analysis, structured output compliance, context window handling, error handling for invalid keys)
- [x] Live integration tests — OpenAI (real API call, Pass 1 overview with gpt-4o, structured output compliance)
- [x] Live integration tests — end-to-end (full pipeline: Pass 1 + Pass 2, combined Annotations serialization roundtrip, gated behind `FLOWDIFF_RUN_LIVE_LLM_TESTS=1`)
- [x] Live integration tests — error handling (invalid API key detection for both Anthropic and OpenAI)

### Phase 5: VS Code Extension (Week 5-6)
- [ ] Extension scaffold
- [ ] CLI binary invocation and JSON parsing
- [ ] Activity bar + sidebar tree view
- [ ] Webview panel for annotations/graph
- [ ] Commands: analyze, annotate, nextFile, nextGroup
- [ ] Open native VS Code diff on file click

### Phase 6: Polish & Integration (Week 6-7)
- [ ] Beyond Compare launcher integration
- [ ] Risk heatmap visualization
- [ ] Flow replay mode
- [ ] Performance optimization (rayon parallelism, caching)
- [ ] Error handling and edge cases
- [ ] README and usage documentation

## 12. Testing Plan

### 12.1 Test Infrastructure

**Framework:** `cargo test` for Rust, Vitest for Tauri React frontend, Jest for VS Code extension

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

### 12.9 VS Code Extension Tests

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
