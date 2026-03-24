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

Automatically detect entry points into the application via three mechanisms:

**Call-site detection** — pattern-match explicit framework API calls in the AST:

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

**Path-based detection (Tier 1)** — if the file path matches route/handler/controller/command directory or filename patterns AND the file imports from a known web/CLI framework, all exported/public functions become entrypoints. Works across all 14 supported languages with per-language framework import tables. Directory patterns: `/routes/`, `/handlers/`, `/controllers/`, `/endpoints/`, `/commands/`, `/cmd/`, `/cli/`. File patterns: `*.routes.*`, `*.handler.*`, `*.controller.*`, `*.endpoint.*`, `*.command.*`, `*.cli.*`.

**Path-based detection (Tier 2)** — very strong path signals detect entrypoints even without a framework import check. This catches files where the import came from an unchanged transitive dependency. Strong signals: files containing `entrypoint`/`entrypoints` in name, `server.*`/`app.*` at project root, files in `/commands/` or `/cmd/` directories.

See [improved-clustering.md §1](./improved-clustering.md) for the full path pattern and framework import tables.

### 4.6 Semantic Clustering

Group changed files into "flow groups" — sets of files that participate in the same logical data flow.

**Algorithm:**

1. For each detected entrypoint in the changed set, compute **bidirectional reachability** in graph G:
   - **Forward BFS** (`Direction::Outgoing`, cost-per-hop=1) — follows calls/imports downstream from the entrypoint
   - **Reverse BFS** (`Direction::Incoming`, cost-per-hop=2) — follows edges upstream to find files that depend on the entrypoint's group
   - Merge: keep the minimum distance for each file across both passes
2. Intersect each reachability set with the changed file set ΔF
3. Files reachable from the same entrypoint and in ΔF belong to the same flow group
4. Files reachable from multiple entrypoints get assigned to the group where they have the shortest path distance (forward edges always win due to lower cost)
5. Ungrouped files (not reachable from any entrypoint) are classified into infrastructure sub-groups:
   - **Convention-based**: true infrastructure (Docker, CI/CD, env, configs), schemas, scripts, migrations, deployment, documentation, lint configs, test utilities, generated code
   - **Import-edge clustering**: remaining files connected by graph edges form component groups
   - **Directory proximity**: files sharing a directory prefix (≥2 files) form directory groups
   - **Fallback**: anything left goes to "Unclassified"

The reverse BFS cost=2 ensures forward-reachable files are always preferred for group assignment while preventing reverse-reachable files from being dumped into infrastructure. See [improved-clustering.md §2-3](./improved-clustering.md) for details.

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
model = "claude-sonnet-4-6"     # claude-opus-4-6, claude-sonnet-4-6, claude-haiku-4-5
# API key via FLOWDIFF_API_KEY env var or:
# key_cmd = "op read op://vault/flowdiff/api-key"

[llm.refinement]
# Optional LLM refinement pass — improves grouping/ranking using semantic understanding.
# Deterministic analysis runs first (free, fast), then LLM refines the output.
# Only applied if enabled and API key is available. Falls back to deterministic if LLM fails.
enabled = false
provider = "anthropic"           # can differ from annotation provider
model = "claude-sonnet-4-6"     # user selects the model for refinement
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
    "files": ["Dockerfile", ".env.dev", "src/schemas/user.ts", "scripts/deploy.sh"],
    "sub_groups": [
      {
        "name": "Infrastructure",
        "category": "Infrastructure",
        "files": ["Dockerfile", ".env.dev"]
      },
      {
        "name": "Schemas",
        "category": "Schema",
        "files": ["src/schemas/user.ts"]
      },
      {
        "name": "Scripts",
        "category": "Script",
        "files": ["scripts/deploy.sh"]
      }
    ],
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
- **Flow replay**: `r` to enter/exit replay mode, step through a group's files in data flow order
- **Review workflow**: `x` to toggle flow as reviewed, `c` to add comment (context-sensitive: code selection / file / group), `C` to copy all comments
- **Clipboard**: `y` to copy current file's absolute path, `Y` to copy all paths in current flow
- **Code comments**: select lines in Monaco diff viewer → inline comment button appears → comment anchored to line range with code snippet
- ~~**Risk heatmap**: visual indicator of which groups need most attention~~ (hidden — Phase 9)

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
Complete. Git diff extraction, tree-sitter AST parsing (TS/JS + Python), symbol graph, entrypoint detection, semantic clustering, review ranking, data flow tracing, JSON output, CLI, config support, test restructuring. See [completed tasks](./tasks/diff-analyzer-completed.md#phase-1-core-engine-week-1-2).

### Phase 2: Data Flow Depth (Week 2-3)
Complete. Heuristic inference, framework detection (30+), call chain tracing, shared IR with language-agnostic types, declarative tree-sitter query engine with `.scm` files, IR-based refactor of all analysis layers, config file support. See [completed tasks](./tasks/diff-analyzer-completed.md#phase-2-data-flow-depth-week-2-3).

### Phase 3: Tauri App (Week 3-4)
Complete. Tauri v2 + React 19 three-panel app, Monaco diff viewer, React Flow graph visualization, keyboard navigation (j/k/J/K), git auto-discovery with branch dropdowns, PR preview diff mode, LLM controls settings panel, refinement UX with original/refined toggle, 50 Playwright E2E tests. See [completed tasks](./tasks/diff-analyzer-completed.md#phase-3-tauri-app-week-3-4).

### Phase 4: LLM Integration (Week 4-5)
Complete. Anthropic/OpenAI/Gemini providers with structured outputs, Pass 1 overview + Pass 2 deep analysis, LLM refinement with split/merge/re-rank/reclassify, API key configuration, context window management, VCR caching, full CLI with all flags. See [completed tasks](./tasks/diff-analyzer-completed.md#phase-4-llm-integration-week-4-5).

### Phase 5: VS Code Extension (Week 5-6)
Complete. Extension scaffold, CLI binary invocation, activity bar + tree view, webview annotations panel, 9 commands with j/k/J/K keybindings, native diff viewer integration, 68 unit tests. See [completed tasks](./tasks/diff-analyzer-completed.md#phase-5-vs-code-extension-week-5-6).

### Phase 6: Polish & Integration (Week 6-7)
Complete. Beyond Compare launcher (5 diff tools), risk heatmap treemap, flow replay mode, rayon parallelism + caching, comprehensive error handling, README documentation, Clippy strict deny wall. See [completed tasks](./tasks/diff-analyzer-completed.md#phase-6-polish--integration-week-6-7).

### Phase 7: Synthetic Eval Suite (Future)
Complete. 5 synthetic fixture codebases, expected output baselines, 6 deterministic scoring functions, VCR caching for LLM calls, LLM-as-judge evaluator, eval harness CLI (`flowdiff eval`), HTML eval dashboard. Avg score: 0.89. See [completed tasks](./tasks/diff-analyzer-completed.md#phase-7-synthetic-eval-suite-future).

### Phase 8: Hardening (Future)
Complete. Automated audits of Rust core, query engine + .scm files, LLM providers, Tauri app, VS Code extension, cross-layer integration, and security. 16 security findings (10 fixed), 29 adversarial integration tests, 1,565 total tests passing. See [completed tasks](./tasks/diff-analyzer-completed.md#phase-8-hardening-future).

### Phase 9: UX Polish & Review Workflow

Focused on making the Tauri app a productive daily-driver for code review. Fixes packaging issues, adds review workflow features (comments, tick-off), and improves editor integration.

#### 9.1 Fix app icon packaging
The macOS dock icon shows a generic "exec" label on a dark square instead of the AI-generated flowdiff icon (directed-graph with teal/lavender nodes on dark indigo background). The icon was generated correctly (Phase 3) and `cargo tauri icon` produced all platform sizes, but the built app doesn't display it.

- [x] Debug why the generated `.icns` / PNG icons aren't being used in the built app bundle
- [x] Verify `tauri.conf.json` `icon` field points to the correct icon paths
- [x] Rebuild and confirm the dock icon, app switcher (Cmd+Tab), and Finder all show the correct flowdiff icon
- [x] Verify on macOS — `.icns` file is correctly embedded in the `.app` bundle's `Contents/Resources/`

#### 9.2 Update LLM model lists
Update the model selector dropdowns in the Tauri settings panel and the config documentation to reflect the latest available models (March 2026).

- [x] **Anthropic:** `claude-opus-4-6`, `claude-sonnet-4-6`, `claude-haiku-4-5`
- [x] **OpenAI:** `gpt-5.4`, `gpt-5.4-mini`, `gpt-4.1`, `o4-mini`, `o3`
- [x] **Gemini:** `gemini-3.1-pro-preview`, `gemini-3-flash-preview`, `gemini-2.5-flash`
- [x] Update model dropdowns in settings panel (provider → model mapping)
- [x] Update context window sizes for new models in `llm/mod.rs`
- [x] Update default model in `.flowdiff.toml` config reference
- [x] Update reasoning model detection for new model IDs (o4-mini, gpt-5.4)

#### 9.3 API key input in frontend
Currently API keys require env vars or `.flowdiff.toml` config. Add a text input field in the Tauri settings panel so users can paste their API key directly.

- [x] Add masked text input (password field) in the settings panel under the API key status section
- [x] "Save" button persists the key to `.flowdiff.toml` under `[llm] key = "..."` (or provider-specific section)
- [x] Key is masked after entry (show last 4 chars only, e.g. `sk-...abcd`)
- [x] "Clear" button to remove the stored key
- [x] Precedence maintained: `key_cmd` > pasted key in config > env var
- [x] Status indicator updates immediately after saving (green "Configured via config file")
- [x] Tauri IPC command: `save_api_key` — writes key to `.flowdiff.toml`, `clear_api_key` — removes it

#### 9.4 Hide risk heatmap
Remove the risk heatmap (squarified treemap) from the right panel UI entirely. The component and code remain in the codebase for future re-enablement, but are not rendered.

- [x] Remove `<RiskHeatmap>` from the right panel render tree (comment out or conditional render with `false`)
- [x] Remove the "Risk heatmap colors render correctly" Playwright test assertion (or skip it)
- [x] Keep `RiskHeatmap.tsx` component and CSS — do not delete, just hide

#### 9.5 Flow tick-off (mark as reviewed)
Add a way to mark flow groups as "reviewed / I'm happy with this" during a review session. Session-only state (not persisted).

- [x] Checkbox or checkmark icon on each flow group in the left panel
- [x] Click to toggle reviewed state — checked groups get a subtle visual treatment (e.g. muted opacity, green checkmark badge, or strikethrough on the group name)
- [x] Reviewed count shown in a summary (e.g. "3/7 flows reviewed" at the top or bottom of left panel)
- [x] Keyboard shortcut: `x` to toggle reviewed state on the currently selected group
- [x] Reviewed state is session-only — resets on page reload or new analysis
- [x] Reviewed groups stay in place (don't move to bottom) — the reviewer might want to revisit

#### 9.6 Right-click file → copy absolute path
Add a context menu on files in the left panel tree view.

- [x] Right-click on any file in the flow group tree → context menu appears
- [x] "Copy File Path" option copies the **absolute** file path to clipboard (e.g. `/Users/james/project/src/services/auth.ts`)
- [x] Use the repo path + relative file path to construct the absolute path
- [x] Toast/notification confirming "Path copied to clipboard"
- [x] Keyboard shortcut: `y` to copy the path of the currently selected file (vim-style yank)

#### 9.7 Open in external editors
Add buttons in the center panel toolbar to open the current file in external editors.

- [x] Toolbar row above the Monaco diff viewer with editor launch buttons
- [x] Supported editors with recognizable icons:
  - **VS Code** — `code` command, VS Code icon
  - **Cursor** — `cursor` command, Cursor icon
  - **Terminal** — open folder in default terminal, terminal icon
- [x] Each button opens the **specific file** currently displayed in the diff viewer
- [x] Uses Tauri `shell:allow-open` or IPC command to spawn the editor process
- [x] IPC command: `open_in_editor(editor: string, file_path: string)` — constructs and executes the appropriate CLI command (`code /path/to/file`, `cursor /path/to/file`, `open -a Terminal /path/to/folder`)
- [x] Graceful fallback if editor is not installed (e.g. `cursor` not found → show tooltip "Cursor not installed")
- [x] Editor icons styled to match Catppuccin theme (small, 20x20px, subtle)

#### 9.8 Copy entire flow (all file paths)
One-click button on each flow group to copy all file paths in that flow.

- [x] "Copy Flow Paths" button (clipboard icon) on each flow group header in the left panel, or as a right-click context menu option on the group
- [x] Copies all file paths in the group as **absolute paths**, one per line, in flow order
- [x] Format: raw paths, no metadata, no group name — ready to paste to an AI agent
  ```
  /Users/james/project/src/routes/auth.ts
  /Users/james/project/src/services/auth-service.ts
  /Users/james/project/src/repositories/user-repo.ts
  ```
- [x] Toast/notification confirming "N file paths copied to clipboard"
- [x] Keyboard shortcut: `Y` (shift+y) to copy all paths in the currently selected group

#### 9.9 Review comments
Add the ability to leave comments on flow groups, individual files, and specific code blocks during review. Three comment scopes: **group-level**, **file-level**, and **code-level** (line range selection). Comments are persisted to the `.flowdiff/` folder so they survive app restarts within the same review session.

**Comment creation:**
- [x] `c` keyboard shortcut opens a comment input — context-sensitive:
  - If a **code block is selected** (highlighted lines in Monaco): comment is attached to that line range
  - If a **file** is selected (no code highlight): comment is attached to that file
  - If a **group** is selected (no file): comment is attached to the group
- [x] **Code-level comments (drag to select):** click and drag in the Monaco diff viewer to select/highlight a range of lines → a "Comment" button appears inline (like GitHub PR review) → click to open comment input anchored to that line range
- [x] Inline text input appears:
  - For code-level: below the selected lines in the Monaco editor gutter area
  - For file/group-level: below the selected item in the left panel (or as a small modal)
- [x] Enter to save, Escape to cancel
- [x] Comments shown as small note icons on the file/group in the left panel tree
- [x] Code-level comments shown as gutter annotations in the Monaco diff viewer (colored marker in the gutter + expandable comment bubble)
- [x] Click the note icon or gutter marker to view/edit the comment
- [x] Right-click → "Delete Comment" to remove

**Comment display:**
- [x] Comments visible in the right panel annotations section when the commented file/group is selected
- [x] Code-level comments rendered inline in the Monaco diff viewer with highlighted line range background (subtle accent color) + comment bubble below the selection
- [x] Comment count badge on groups that have comments (on the group itself or its files)
- [x] Visual: Catppuccin-styled comment bubbles with subtle accent border

**Copy all comments:**
- [x] "Copy All Comments" button in the left panel header (or bottom toolbar)
- [x] Copies all comments as a formatted list with absolute file paths
- [x] **Code-level comments include the selected code snippet** in the export:
  ```
  /Users/james/project/src/routes/auth.ts:42-58
  ```typescript
  async function validateEmail(email: string) {
    // TODO: add proper validation
    return email.includes('@');
  }
  ```
  > Missing input validation on the email field — this should use a proper email regex

  /Users/james/project/src/services/auth-service.ts
  > Should we add rate limiting here?

  Flow: "Auth & Session Management"
  > Overall looks good but needs error handling review
  ```
- [x] Format designed for pasting to an AI agent as review feedback — includes the actual code being commented on so the agent has full context
- [x] Keyboard shortcut: `C` (shift+c) to copy all comments

**Persistence:**
- [x] Comments stored as JSON in `.flowdiff/comments.json` (alongside cache and other flowdiff state)
- [x] Keyed by analysis hash (so comments are scoped to a specific diff/analysis run)
- [x] Format:
  ```json
  {
    "analysis_hash": "abc123",
    "comments": [
      {
        "type": "code",
        "group_id": "group_1",
        "file_path": "src/auth.ts",
        "start_line": 42,
        "end_line": 58,
        "selected_code": "async function validateEmail(email: string) {\n  // TODO: add proper validation\n  return email.includes('@');\n}",
        "text": "Missing input validation — should use a proper email regex",
        "created_at": "2026-03-20T14:30:00Z"
      },
      {
        "type": "file",
        "group_id": "group_1",
        "file_path": "src/auth.ts",
        "start_line": null,
        "end_line": null,
        "selected_code": null,
        "text": "Missing validation",
        "created_at": "2026-03-20T14:30:00Z"
      },
      {
        "type": "group",
        "group_id": "group_1",
        "file_path": null,
        "start_line": null,
        "end_line": null,
        "selected_code": null,
        "text": "Needs error handling review",
        "created_at": "2026-03-20T14:31:00Z"
      }
    ]
  }
  ```
- [x] Loaded on app start if analysis hash matches
- [x] Tauri IPC commands: `save_comment`, `delete_comment`, `load_comments`, `export_comments`

#### 9.10 Flowchart node click → collapse graph + open file
When a user clicks a node in the React Flow graph, it should collapse the graph section within the right panel and open that file in the diff viewer. This creates a smooth "explore graph → dive into file" workflow.

- [x] Click a React Flow node → file opens in the center panel Monaco diff viewer (existing behavior)
- [x] Additionally: the React Flow graph section in the right panel **collapses** (animates to a thin bar or accordion header showing "Flow Graph ▶")
- [x] The right panel reclaims the space — annotations/details section expands to fill
- [x] Click the collapsed graph header to re-expand it
- [x] Graph collapse state resets when switching to a different group

### Phase 10: UX Fixes & Bug Fixes

Focused on fixing usability issues, keyboard shortcut conflicts, and UI bugs discovered during daily use.

#### 10.1 Flow graph: navigate on node click without closing
When clicking a node in the React Flow graph, navigate to that file in the diff viewer but do NOT close/collapse the graph. The current Phase 9.10 behavior (collapse on click) is too aggressive — users want to explore the graph and jump between nodes without losing the visual context.

- [x] Remove the auto-collapse behavior when clicking a React Flow node
- [x] Keep the navigation behavior: clicking a node still selects the file and updates the diff viewer
- [x] Graph stays open and visible after node click — user can click multiple nodes to explore
- [x] Manual collapse still works via the accordion header toggle

#### 10.2 Fix c/C keyboard shortcuts — Monaco read-only conflict
Pressing `c` or `C` currently shows "Cannot edit in read-only editor" because the keypress reaches Monaco before the app's keyboard handler intercepts it.

- [x] Intercept `c` and `C` keypresses at the app level before they reach Monaco
- [x] `c` opens comment input (context-sensitive: code selection → code comment, file → file comment, group → group comment) — already specified in 9.9 but not working due to Monaco capture
- [x] `C` copies all comments — already specified in 9.9 but not working due to Monaco capture
- [x] Verify no other single-key shortcuts are being swallowed by Monaco's read-only editor

#### 10.3 Make "Copy All Comments" more visible
The "Copy All Comments" button is hard to discover in the current UI.

- [x] Move or duplicate the button to a more prominent location (e.g. sticky footer bar in the left panel, or a toolbar button with clear label)
- [x] Add a visual badge showing comment count (e.g. "Copy All Comments (5)")
- [x] Consider a floating action button or persistent toolbar element that's always visible when comments exist
- [x] Keyboard hint tooltip showing `C` shortcut on hover

#### 10.4 Fix group/reviewed count after LLM refinement
Bug: After LLM refinement changes the number of groups, the UI shows stale counts like "4/3 reviewed" where the denominator doesn't match the actual group count.

- [x] When refinement completes and group structure changes, reset the reviewed state
- [x] Update the total group count in the "N/M reviewed" display to reflect the new group count
- [x] Clear per-group reviewed checkmarks since the groups have been reorganized
- [x] Show a brief toast: "Groups updated by refinement — review state reset"

#### 10.5 Auto-center/fit-all when opening flow graph
When the flow graph component opens, it should automatically zoom and pan to show all nodes.

- [x] Call `fitView()` (React Flow API) when the graph section is expanded or when switching to a new group
- [x] Add appropriate padding so nodes aren't flush against edges
- [x] Animate the fit-view transition for a smooth experience
- [x] Respect any user zoom/pan after initial fit — don't re-fit on every render

#### 10.6 Consolidate "Open With" into single dropdown
Replace the separate editor buttons in the diff viewer toolbar with a single "Open With" dropdown.

- [x] Single "Open With" dropdown button in the toolbar with a dropdown arrow
- [x] Dropdown lists available editors: VS Code, Cursor, Zed, Vim, Terminal
- [x] Each option has an icon and label
- [x] **Actually opens the file** — use Tauri `shell.open` or `Command` API to execute: `code <path>`, `cursor <path>`, `zed <path>`, `vim <path>` (in terminal), `open -a Terminal <folder>`
- [x] Remove the "Would open..." placeholder behavior — execute the real command
- [x] Detect which editors are installed (check if command exists) and only show installed ones
- [x] Remember last-used editor choice in session

#### 10.7 Adversarial edge cases: circular refs & import graph semantics
Create adversarial test fixtures to stress-test the clustering algorithm with degenerate dependency patterns. Use LLM-based evaluation to verify improvements.

- [x] **Circular imports**: A→B→C→A cycles — verify no infinite loops, groups are still meaningful
- [x] **Diamond dependencies**: A→B, A→C, B→D, C→D — verify D is assigned to the correct group
- [x] **Barrel file explosion**: `index.ts` re-exporting 50+ modules — verify barrel files don't distort grouping
- [x] **Re-export chains**: A re-exports B which re-exports C — verify edge resolution traces through
- [x] **Self-referencing modules**: File imports from itself (aliased paths) — verify no crash
- [x] **Deeply nested transitive deps**: 10+ levels of transitive imports — verify depth limiting works
- [x] **Hub-and-spoke**: One file imported by 30+ others — verify it doesn't pull everything into one group
- [x] **Orphan clusters**: Groups of files connected to each other but not to any entrypoint — verify they form their own infrastructure group, not silently dropped
- [x] **Cross-language imports**: Python calling a compiled Rust module, TS importing WASM — verify graceful handling
- [x] Create test fixtures for each case with expected grouping output
- [x] Run LLM refinement on each case and score with eval suite — compare deterministic vs refined groupings
- [x] Add adversarial fixtures to the regression test suite

### Phase 11: Multi-Language Support

Expand tree-sitter language support beyond TypeScript/JavaScript and Python. The architecture supports this declaratively — adding a language requires writing `.scm` query files for `imports`, `exports`, `definitions`, `calls`, and `assignments` under `crates/flowdiff-core/queries/{lang}/`, plus updating the `Language` enum in `ast.rs` and adding the tree-sitter grammar dependency.

#### 11.1 Tier 1 — Must have

**Go:**
- [x] Add `tree-sitter-go` grammar dependency to `Cargo.toml`
- [x] Add `Language::Go` variant to enum in `ast.rs`
- [x] Write `.scm` query files: `queries/go/imports.scm`, `exports.scm`, `definitions.scm`, `calls.scm`, `assignments.scm`
- [x] Handle Go-specific patterns: package imports, struct methods, interface implementations, goroutine spawning, channel send/receive
- [x] Entrypoint detection: `func main()`, `http.HandleFunc`, `http.Handle`, gin/echo/chi router patterns, `cobra.Command`
- [x] Framework detection: net/http, Gin, Echo, Chi, Fiber, gRPC, Cobra, GORM, sqlx
- [x] Tests: import resolution (relative packages, module paths), function/method extraction, struct definitions, interface edges, call graph across packages
- [x] Integration test: synthetic Go HTTP API app with handler→service→repo pattern

**Rust:**
- [x] Add `tree-sitter-rust` grammar dependency to `Cargo.toml`
- [x] Add `Language::Rust` variant to enum in `ast.rs`
- [x] Write `.scm` query files: `queries/rust/imports.scm`, `definitions.scm`, `calls.scm`, `assignments.scm`
- [x] Handle Rust-specific patterns: `use`/`pub` visibility, trait definitions, `impl` blocks, macro definitions, `async fn`, struct/enum/const/static definitions
- [x] Entrypoint detection: `fn main()`, `#[tokio::main]`, actix-web/axum route handlers, `#[test]`, clap derive patterns
- [x] Framework detection: Actix-web, Axum, Rocket, Warp, Tokio, Diesel, SQLx, SeaORM, Clap, Tauri commands, Serde, Tower, Tonic, Tracing
- [x] Tests: use path resolution, use list/alias/glob/crate imports, trait/struct/enum definitions, async fn extraction, macro definition detection, impl method extraction, call site detection, data flow tracing (25 unit tests)
- [x] Integration test: synthetic Rust axum API with handler→service→repo pattern + test file detection

#### 11.2 Tier 2 — High value

**Java:**
- [x] Add `tree-sitter-java` grammar dependency
- [x] Add `Language::Java` variant
- [x] Write `.scm` query files for Java
- [x] Handle: package/import statements, class hierarchy (extends/implements), annotations, generics (ignore for grouping), method overloading
- [x] Entrypoint detection: `public static void main`, `@RestController`/`@RequestMapping`, `@SpringBootApplication`, JUnit `@Test`
- [x] Framework detection: Spring Boot, Spring MVC, JPA/Hibernate, Maven/Gradle project structure
- [x] Tests: import resolution, class/interface extraction, annotation detection, inheritance edges
- [x] Integration test: synthetic Spring Boot REST API

**C#:**
- [x] Add `tree-sitter-c-sharp` grammar dependency
- [x] Add `Language::CSharp` variant
- [x] Write `.scm` query files for C#
- [x] Handle: namespace/using statements, class hierarchy, interfaces, attributes, async/await, partial classes
- [x] Entrypoint detection: `static void Main`, `[ApiController]`/`[HttpGet]`, `[TestMethod]`/`[Fact]`, minimal API `app.MapGet`
- [x] Framework detection: ASP.NET Core, Entity Framework, xUnit/NUnit, Blazor
- [x] Tests: using resolution, class/interface extraction, attribute detection, namespace edges
- [x] Integration test: synthetic ASP.NET Core Web API

**PHP:**
- [x] Add `tree-sitter-php` grammar dependency
- [x] Add `Language::Php` variant
- [x] Write `.scm` query files for PHP
- [x] Handle: `use`/`namespace` statements, class hierarchy, traits, interfaces, type hints
- [x] Entrypoint detection: Laravel route definitions, controller methods, artisan commands, PHPUnit tests
- [x] Framework detection: Laravel, Symfony, WordPress, Composer autoload, Eloquent ORM, Doctrine
- [x] Tests: namespace/use resolution, class extraction, trait usage, route detection
- [x] Integration test: synthetic Laravel REST API with controller→service→model pattern

**Ruby:**
- [x] Add `tree-sitter-ruby` grammar dependency
- [x] Add `Language::Ruby` variant
- [x] Write `.scm` query files for Ruby
- [x] Handle: `require`/`require_relative`, module/class hierarchy, mixins (`include`/`extend`), blocks/procs
- [x] Entrypoint detection: Rails route definitions, controller actions, Rake tasks, RSpec `describe`/`it`
- [x] Framework detection: Rails, Sinatra, RSpec, ActiveRecord, Sidekiq
- [x] Tests: require resolution, class/module extraction, mixin edges, route detection
- [x] Integration test: synthetic Rails REST API with controller→service→model pattern

#### 11.3 Tier 3 — Nice to have

**Kotlin:**
- [x] Add `tree-sitter-kotlin-ng` grammar dependency
- [x] Add `Language::Kotlin` variant
- [x] Write `.scm` query files for Kotlin
- [x] Handle: package/import (regular, aliased, wildcard), data classes, sealed classes, extension functions, object declarations, typealias, property declarations
- [x] Entrypoint detection: `fun main()`, Ktor route handlers (`get`/`post`/`put`/`delete`), Spring Boot controllers, JUnit `@Test`
- [x] Framework detection: Ktor, Spring Boot (Kotlin), Exposed, Jetpack Compose, Kotlin Coroutines, Kotlin Serialization, JUnit, Kotest, MockK, Koin, Retrofit, OkHttp, Arrow, Dagger/Hilt
- [x] Tests and integration test: synthetic Ktor API (6 unit tests + 2 integration tests)

**Swift:**
- [x] Add `tree-sitter-swift` grammar dependency
- [x] Add `Language::Swift` variant
- [x] Write `.scm` query files for Swift
- [x] Handle: `import`, class/struct/enum/protocol hierarchy, extensions, closures, `@main`
- [x] Entrypoint detection: `@main`, SwiftUI `App` protocol, `XCTestCase`
- [x] Framework detection: SwiftUI, Vapor, UIKit, XCTest
- [x] Tests and integration test: synthetic Vapor API

**C/C++:**
- [x] Add `tree-sitter-c` and `tree-sitter-cpp` grammar dependencies
- [x] Add `Language::C` and `Language::Cpp` variants
- [x] Write `.scm` query files for C and C++
- [x] Handle: `#include` (header resolution is hard — use heuristics), function declarations, class hierarchy (C++), namespaces (C++), templates (ignore for grouping)
- [x] Entrypoint detection: `int main()`, test framework macros (`TEST`, `TEST_F`)
- [x] Framework detection: Google Test, Catch2, CMake project structure
- [x] Tests and integration test: synthetic C++ project

**Scala:**
- [x] Add `tree-sitter-scala` grammar dependency
- [x] Add `Language::Scala` variant
- [x] Write `.scm` query files for Scala
- [x] Handle: package/import, class/trait/object hierarchy, case classes, implicits, pattern matching
- [x] Entrypoint detection: `def main`, `extends App`, Akka HTTP routes, Play Framework controllers, ScalaTest
- [x] Framework detection: Akka, Play Framework, ZIO, Cats Effect, ScalaTest, Slick
- [x] Tests and integration test: synthetic Akka HTTP API

## 12. Incremental Parse Caching (Performance) — **NEXT PRIORITY**

Tree-sitter parsing is the main bottleneck in the analysis pipeline. Each file is parsed **twice** (once for imports/definitions/calls in `parse_with_queries()`, once for assignments/arguments in `extract_data_flow()`), and no results are cached between runs. With 1500+ tests and growing, this adds up fast.

Goal: cache deterministic intermediate results so repeated/unchanged inputs skip expensive work.

### 12.1 Deduplicate per-file double parse

**Problem:** `parse_with_queries()` and `extract_data_flow()` both call `parse_tree()` independently, creating two `Parser` instances and two full tree-sitter parses for the same source.

- [x] Refactor `parse_to_ir()` in `pipeline.rs` to call `parse_tree()` once and pass the resulting `tree_sitter::Tree` to both `parse_with_queries()` and `extract_data_flow()`
- [x] Update method signatures to accept an optional pre-parsed tree
- [x] Benchmark: measured ~1.2-1.3x speedup (TS: 947→777µs, Py: 659→509µs) — parse is only part of per-file cost; query execution dominates

### 12.2 Content-addressed IrFile cache

**Problem:** Re-running analysis on the same file content re-parses and re-extracts everything from scratch.

- [x] Add `sha2` content hash of source → `IrFile` cache (in-memory `DashMap<[u8; 32], IrFile>`) — `IrCache` type in `pipeline.rs`
- [x] Key: `SHA-256(file_path + "\0" + source_content)` — content-addressed, so identical content = cache hit regardless of branch/ref
- [x] Integrate into `parse_to_ir()` and `parse_all_to_ir()`: check cache before parsing via `Option<&IrCache>` parameter
- [x] Thread the cache through all callers (cross_layer_audit, benchmarks, unit tests) — pass `None` when not needed, `Some(&cache)` when sharing
- [x] For tests: `IrCache` uses `DashMap` (thread-safe, lock-free reads) — shareable across rayon parallel parsing
- [x] For CLI: populate cache per invocation (single-run benefit from dedup; cross-run benefit requires disk cache in 12.4)
- [x] 12 new unit tests: cache hit/miss, different content/path, partial hit, cross-batch sharing, byte-identical verification, Send+Sync

### 12.3 Lazy QueryEngine initialization per language

**Problem:** `QueryEngine::new()` compiles `.scm` queries for all 12+ languages upfront, even if the diff only contains TypeScript files.

- [x] Change query compilation from eager (all languages in constructor) to lazy (compile on first use per language)
- [x] Use `OnceCell<CompiledQuery>` or equivalent for each language's query set — `once_cell::sync::OnceCell<LanguageQueries>` with `get_or_try_init()` in `get_lang_queries()`
- [x] Benchmark: measure `QueryEngine::new()` time before/after — `new()` is now ~348 ns (near-instant), first parse compiles only the needed language (TS: ~13.4 ms, Python: ~5.3 ms), subsequent parses: ~569 µs

### 12.4 Disk-persistent IrFile cache (optional, for CLI)

**Problem:** CLI invocations don't share state. Re-running `flowdiff analyze` on the same branch re-parses all files.

- [x] Serialize `IrFile` cache to `.flowdiff/cache/ir/{content_hash}.bincode` — `DiskIrCache` in `pipeline.rs` using `bincode` v1 serialization
- [x] On startup, load existing cache entries; on shutdown, write new entries — `DiskIrCache::load()` reads `.bincode` files into `IrCache`; `DiskIrCache::flush()` writes new entries
- [x] Add cache size limit (e.g. 100MB) with LRU eviction — `evict_lru()` sorts by mtime, removes oldest files until under limit
- [x] Add `--no-cache` flag to bypass — `AnalyzeArgs::no_cache` in CLI, skips `DiskIrCache::load()` when set
- [x] Invalidation: content-addressed, so no explicit invalidation needed — stale entries just get evicted by LRU
- [x] 11 new tests: load empty, flush creates files, roundtrip, byte-identical roundtrip, no-cache flag, LRU eviction, malformed files, non-bincode files, idempotent flush, multi-run accumulation, readonly dir

### 12.5 Test harness optimizations

**Problem:** E2E tests create fresh `QueryEngine` instances per test and re-parse identical fixture files.

- [x] Create `OnceLock<QueryEngine>` shared across test binaries — file-level `shared_test_engine()` in `query_engine.rs`, module-level `shared_engine()` in `pipeline.rs` tests, `shared_engine()` in `tests/helpers/mod.rs`, `OnceLock` in `eval/fixtures.rs` and `cross_layer_audit.rs`
- [x] Share the IrFile cache via `OnceLock<IrCache>` — `shared_cache()` in `pipeline.rs` tests and `tests/helpers/mod.rs` (content-addressed, no cross-test pollution)
- [x] Measure: **4x wall-clock speedup** (30.4s → 7.6s), **19x user time reduction** (259.5s → 13.7s) for `cargo test --package flowdiff-core`

### 12.6 Test profile optimization

**Problem:** Tests compile and run with default `[profile.dev]` settings — no optimizations at all.

- [x] Add `[profile.test] opt-level = 1` to root `Cargo.toml` — trades slightly longer compile for 10-30% faster runtime
- [x] Document recommended test invocation: `cargo test -- --test-threads=$(nproc)` (tests are isolated via TempDirs, safe to parallelize)

### 12.7 Reuse Parser instances per language

**Problem:** `parse_tree()` in `query_engine.rs` creates a new `tree_sitter::Parser` on every call. Across the test suite this means thousands of unnecessary allocations.

- [x] Store a `Parser` per language using `thread_local!` with `RefCell<HashMap<Language, Parser>>` — each rayon thread gets its own set of parsers, avoiding contention
- [x] Reuse across all `parse_tree()` calls for the same language — `parse_tree()` now takes a `Language` key, looks up the thread-local parser, creates one on first use
- [x] Verify thread safety: `Parser` is `Send` but `!Sync`, so cannot be stored in `QueryEngine` (shared via `&` across rayon threads); thread-local storage gives each worker its own parsers with zero synchronization overhead. 6 new tests: same-language reuse, multi-language reuse, rayon parallel, parse_tree_for_path reuse, extract_data_flow reuse, fresh-vs-reused result identity

### 12.8 Parallelize graph building

**Problem:** `SymbolGraph::build()` and `build_from_ir()` in `graph.rs` run sequentially — Phase 1 (add nodes) and Phase 2 (add edges) iterate files one at a time with no parallelism.

- [x] Phase 1: use rayon to collect node data per file in parallel, then merge into graph single-threaded
- [x] Phase 2: use rayon to compute edges per file in parallel, then add to graph single-threaded
- [x] Benchmark: 1.3x at 50 files, 1.8x at 100 files (rayon overhead dominates at small synthetic IR scale; real codebases with heavier parse results will see larger gains)

### 12.9 Efficient flow pattern matching

**Problem:** Heuristic flow analysis in `flow.rs` matches call sites against 100+ pattern strings (DB_WRITE_METHODS, EVENT_EMIT_METHODS, etc.) using `.contains()` per pattern per call — O(patterns × calls) substring searches.

- [x] Replace linear `.contains()` scans with Aho-Corasick automaton for substring matching (SQL keywords, DB keywords, ORM names) and `HashSet` for exact/suffix matching (log patterns, method suffixes, non-DB receivers)
- [x] Add `aho-corasick` crate dependency
- [x] Build automatons and hash sets once via `OnceLock`, reuse across all files. Also eliminated `format!()` allocations in framework detection and HTTP/config pattern matching
- [x] Benchmark added: flow_analysis/heuristic_patterns at 20/50/100 files with 37 mixed call sites per file (criterion)

### 12.10 Verification

- [x] All existing tests still pass (1638 tests: 1426 unit + 212 integration)
- [x] Add a benchmark test (criterion) for: graph_build_from_ir (20/50/100 files, parallel vs serial) and flow_analysis/heuristic_patterns (20/50/100 files with 37 mixed call sites each)
- [x] Cache hit/miss logging behind `FLOWDIFF_CACHE_DEBUG=1` env var (per-operation HIT/MISS lines + summary stats to stderr)
- [x] No behavior change: cached results are byte-identical to uncached results (verified by `ir_cache_cached_result_byte_identical` test)

---

## 13. Testing Plan

### 13.1 Test Convention

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

### 13.2 Test Infrastructure

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

### 13.3 Unit Tests — Core Engine

All spec-required unit tests are implemented. Total: 85 tests across 8 layers.

#### Git Layer (`git.rs`) — ✅ All 9 implemented
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

#### AST Layer (`ast.rs`) — ✅ All 11 implemented
| Test | What it verifies |
|------|-----------------|
| `test_parse_ts_imports` | Extracts named, default, and namespace imports from TypeScript |
| `test_parse_ts_exports` | Extracts named, default, and re-exports |
| `test_parse_ts_functions` | Extracts function declarations, arrow functions, methods |
| `test_parse_ts_call_sites` | Identifies function call expressions with resolved targets |
| `test_parse_python_imports` | Handles `import x`, `from x import y`, relative imports |
| `test_parse_python_functions` | Extracts functions, methods, decorators |
| `test_parse_python_class_hierarchy` | Detects class inheritance |
| `test_parse_rust_modules` | Handles `mod`, `use`, `pub` visibility (currently graceful fallback — Rust parsing not yet wired to tree-sitter queries) |
| `test_parse_unknown_language` | Falls back gracefully for unsupported file types |
| `test_changed_symbols_detection` | Correctly identifies which symbols were added/modified/removed between old and new AST |
| `test_large_file_performance` | Parses a 10K+ line file within acceptable time (<500ms) |

#### Graph Layer (`graph.rs`) — ✅ All 10 implemented
| Test | What it verifies |
|------|-----------------|
| `test_build_import_edges` | Creates correct `imports` edges between files |
| `test_build_call_edges` | Creates `calls` edges from call site analysis |
| `test_build_extends_edges` | Creates `extends` edges from class inheritance (via IR path; AST path is stub) |
| `test_cyclic_imports` | Handles circular dependencies without infinite loop |
| `test_cross_package_edges` | Resolves imports across monorepo package boundaries (via WorkspaceMap) |
| `test_dynamic_imports` | Handles `import()` / `require()` dynamic imports (no crash, well-formed graph) |
| `test_reexport_chains` | Traces through barrel files (`index.ts` re-exports) |
| `test_graph_node_count` | Correct vertex count for known fixture |
| `test_graph_edge_count` | Correct edge count for known fixture |
| `test_graph_serialization_roundtrip` | Graph → JSON → Graph is lossless |

#### Flow Layer (`flow.rs`) — ✅ All 9 implemented
| Test | What it verifies |
|------|-----------------|
| `test_trace_param_flow` | Traces a parameter from function A through call to function B |
| `test_trace_return_value` | Tracks return value from callee back to caller |
| `test_trace_variable_assignment` | Follows `const x = foo(); bar(x)` chains |
| `test_heuristic_db_write` | Detects `.save()`, `.insert()`, `INSERT INTO` as persistence |
| `test_heuristic_http_handler` | Detects HTTP handler registration patterns (app.on, app.listen) as event handling |
| `test_heuristic_event_emission` | Detects `.emit()`, `.publish()`, `.send()` as emission edges |
| `test_heuristic_config_read` | Detects `process.env`, `os.environ` as config reads |
| `test_no_false_positive_heuristics` | Common patterns that look like but aren't DB writes/handlers (arrays, Map/Set, JSON, Promise, localStorage) |
| `test_flow_depth_limit` | Tracing stops at configurable depth to prevent runaway (4-node chain, depth 1/2/10) |

#### Cluster Layer (`cluster.rs`) — ✅ All 9 implemented
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

#### Rank Layer (`rank.rs`) — ✅ All 10 implemented
| Test | What it verifies |
|------|-----------------|
| `test_risk_scoring_schema_change` | DB migration or schema file change → high risk |
| `test_risk_scoring_auth` | Auth/security file changes → high risk |
| `test_risk_scoring_test_only` | Test-only changes → low risk |
| `test_centrality_hub_node` | File imported by many others → high centrality |
| `test_centrality_leaf_node` | Leaf file with no dependents → low centrality |
| `test_surface_area` | More changed lines → higher surface area score |
| `test_composite_score` | Weighted combination produces expected ranking |
| `test_custom_weights` | Config-provided weights override defaults (custom weights flip ranking order) |
| `test_ranking_stability` | Same input → same ranking (deterministic) |
| `test_single_group_ranking` | One group still gets a valid score |

#### Config Layer (`config.rs`) — ✅ All 7 implemented
| Test | What it verifies |
|------|-----------------|
| `test_parse_valid_config` | Parses well-formed `.flowdiff.toml` |
| `test_missing_config` | Works fine without config file (auto-detect) |
| `test_partial_config` | Handles config with only some sections |
| `test_invalid_config` | Clear error message on malformed TOML |
| `test_entrypoint_globs` | Glob patterns in config resolve to correct files |
| `test_ignore_patterns` | Ignored files excluded from analysis |
| `test_custom_layer_names` | Layer names from config used in group naming |

#### Output Layer (`output.rs`) — ✅ All 5 implemented
| Test | What it verifies |
|------|-----------------|
| `test_json_schema_compliance` | Output matches documented JSON schema exactly |
| `test_mermaid_generation` | Valid Mermaid syntax generated for flow graphs (tested via `test_mermaid_basic_flow` + 6 edge/label/dedup tests) |
| `test_empty_annotations_field` | `annotations` is `null` when LLM not used (verified in both struct and JSON) |
| `test_output_file_write` | `-o` flag writes to file correctly (temp file write + read-back roundtrip) |
| `test_stdout_output` | Default outputs to stdout (buffer write, validates JSON, newline, pretty-print) |

### 13.4 Unit Tests — LLM Layer

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

### 13.5 Integration Tests — End-to-End Pipeline

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

### 13.6 Snapshot Tests

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

### 13.7 Property-Based Tests

Use `proptest` crate for fuzzing graph construction and ranking.

| Property | Description |
|----------|-------------|
| Every changed file appears in exactly one group | No file lost, no file duplicated |
| Group file order is topologically valid | BFS-tree ordering: flow_position monotonically non-decreasing, and for edges where source has strictly smaller flow_position than target, source appears first. Implemented as `prop_group_file_order_topologically_valid` in `cluster.rs` |
| Ranking scores are in [0.0, 1.0] | No score exceeds bounds |
| Ranking is total order | No two groups have identical rank (tie-break is deterministic) |
| Empty diff → empty groups | No phantom groups from empty input |
| Single file diff → single group | Minimal case always works |
| Graph with no edges → all infrastructure | Disconnected files go to infrastructure group |
| Determinism | `analyze(X) == analyze(X)` for any input (run 10 times) |

### 13.8 Performance Benchmarks

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

### 13.9 Tauri App Tests

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

### 13.10 Tauri App — Playwright E2E Tests

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

### 13.11 VS Code Extension Tests

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

### 13.12 LLM Integration Tests (Live, Optional)

These tests hit real APIs and are gated behind `FLOWDIFF_RUN_LIVE_LLM_TESTS=1`.

| Test | What it verifies |
|------|-----------------|
| `test_live_anthropic_pass1` | Real Anthropic API call returns valid structured output |
| `test_live_openai_pass1` | Real OpenAI API call returns valid structured output |
| `test_live_anthropic_pass2` | Deep analysis returns file-level annotations |
| `test_live_reasoning_model` | Extended thinking / o3 models produce richer output |
| `test_live_structured_output_compliance` | Response conforms to schema (no hallucinated fields) |

### 13.13 Regression Test Suite

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

### 13.14 CI Pipeline

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

### 13.15 Manual Acceptance Testing Checklist

Before each release, run through manually:

- [x] Clone a real project with 50+ file PR, run `flowdiff analyze --base main` — 78-file diff (HEAD~30), 12 groups, Rust+TS detected
- [ ] Verify flow groups intuitively match the PR's logical changes *(requires human review)*
- [ ] Verify files within each group are in reasonable data flow order *(requires human review)*
- [ ] Open Tauri app, navigate all three panels *(requires GUI)*
- [ ] Keyboard nav (j/k/J/K) works smoothly *(requires GUI)*
- [ ] Monaco diff viewer shows correct old/new with syntax highlighting *(requires GUI)*
- [ ] Mermaid graph renders and matches the flow group *(requires GUI)*
- [x] Click "Annotate" → LLM returns structured annotations — verified via CLI --annotate on Python project, returns overall_summary + per-group annotations
- [ ] Annotations display in right panel *(requires GUI)*
- [ ] VS Code extension: run `flowdiff.analyze`, verify tree view populates *(requires VS Code)*
- [ ] VS Code: click file in tree → native diff editor opens *(requires VS Code)*
- [ ] VS Code: `flowdiff.nextFile` advances through flow *(requires VS Code)*
- [x] Run on a Python project — verify tree-sitter + heuristics work — detected Python, FastAPI+Flask frameworks, found automate_tasks HttpRoute entrypoint
- [x] Run on a monorepo — verify cross-package edges resolve — synthetic 3-package TS monorepo (@monorepo/shared-types, @monorepo/api-client, @monorepo/backend), workspace map built from package.json, 9 cross-package edges detected (import + call edges across packages), shared-types pulled into flow group via workspace resolution
- [x] Run with no config file — auto-detection works — produces valid JSON output with 0 config
- [x] Run with `.flowdiff.toml` — overrides apply correctly — config loads, validates (rejects invalid provider), ignore patterns filter parsing
- [x] Run on empty diff — graceful "no changes" message — returns JSON with 0 files, 0 groups, no error
- [x] Performance: 100-file diff completes in under 15 seconds — 78-file diff completes in 65ms (release build)
