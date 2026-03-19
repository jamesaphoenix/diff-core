# flowdiff

A semantic diff layer for code review. Git gives you syntactic diffs — what text changed in which files. flowdiff adds meaning — what those changes *mean*, how they relate, and what order a human should read them in.

Built for the era of AI agents producing 50-100 file PRs.

## What it does

flowdiff transforms flat file diffs into ranked, semantically grouped review flows:

1. **Structural analysis** (free, deterministic) — builds a symbol graph from tree-sitter ASTs, detects entrypoints (HTTP routes, CLI commands, queue consumers, Effect.ts services, etc.), clusters changed files into flow groups via forward reachability, traces data flow across call chains.

2. **Heuristic scoring** (free, deterministic) — framework detection (Express, Next.js, FastAPI, Effect.ts, 30+ frameworks), risk scoring, review ordering by composite score (risk / centrality / surface area / uncertainty).

3. **LLM refinement** (paid, optional) — Anthropic, OpenAI, or Gemini reads the actual diff and refines groupings: split coincidental coupling, merge scattered refactors, re-rank by semantic review order. Evaluator-optimizer loop keeps whichever version scores better.

## Installation

### CLI

```bash
cargo install --path crates/flowdiff-cli
```

### Tauri Desktop App

```bash
cd crates/flowdiff-tauri/ui && npm install
cargo tauri build
```

The built app is in `target/release/bundle/`.

### VS Code Extension

```bash
cd extensions/vscode
npm install
npm run compile
# Then install the .vsix or use "Developer: Install Extension from Location"
```

## Usage

### CLI

```bash
# PR preview (default) — merge-base diff: main...HEAD
flowdiff analyze

# Branch comparison
flowdiff analyze --base main --head feature-branch

# Commit range
flowdiff analyze --range HEAD~5..HEAD

# Staged / unstaged changes
flowdiff analyze --staged
flowdiff analyze --unstaged

# Save to file
flowdiff analyze --base main -o review.json

# With LLM annotations (Pass 1 overview)
flowdiff analyze --base main --annotate

# With LLM refinement of groupings
flowdiff analyze --base main --refine
flowdiff analyze --base main --refine --refine-model gpt-4o

# Analyze a different repo
flowdiff analyze --base main --repo /path/to/repo

# Open a flow group in an external diff tool
flowdiff launch --tool bcompare --group group_1 --input review.json
flowdiff launch --tool meld --group group_2 --input review.json
```

Supported external diff tools: `bcompare` (Beyond Compare), `meld`, `kdiff3`, `code` (VS Code), `opendiff` (macOS FileMerge).

### Tauri Desktop App

Three-panel layout:

- **Left** — flow groups ranked by review score, expandable file tree
- **Center** — Monaco diff viewer with syntax highlighting
- **Right** — annotations, React Flow graph, risk heatmap

Keyboard navigation: `j`/`k` next/prev file, `J`/`K` next/prev group, `r` flow replay mode.

The app auto-discovers git branches, worktrees, and push status on launch. Default diff mode is PR preview (`main...HEAD`).

### VS Code Extension

Commands (available from the command palette):

| Command | Keybinding | Description |
|---------|-----------|-------------|
| `flowdiff.analyze` | — | Analyze current branch |
| `flowdiff.analyzeRange` | — | Analyze a commit range |
| `flowdiff.annotate` | — | Annotate with LLM |
| `flowdiff.nextFile` | `j` | Next file in flow |
| `flowdiff.prevFile` | `k` | Previous file in flow |
| `flowdiff.nextGroup` | `Shift+J` | Next flow group |
| `flowdiff.prevGroup` | `Shift+K` | Previous flow group |
| `flowdiff.openAnnotations` | — | Show annotations panel |

Settings:

- `flowdiff.binaryPath` — path to `flowdiff` binary (leave empty to use `$PATH`)
- `flowdiff.defaultBase` — default base branch for comparison (default: `"main"`)

## Configuration

Create `.flowdiff.toml` in your repo root. All fields are optional — flowdiff auto-detects languages, frameworks, and entrypoints by default.

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
paths = ["**/*.test.ts", "**/*.spec.ts", "migrations/**"]

[llm]
provider = "anthropic"  # "anthropic", "openai", or "gemini"
model = "claude-sonnet-4-6"
# API key via env var or command:
# key_cmd = "op read op://vault/flowdiff/api-key"

[llm.refinement]
enabled = false
provider = "anthropic"
model = "claude-sonnet-4-6"
max_iterations = 1  # 1 = single refinement, 2+ = iterative

[ranking]
# Override default review ordering weights
risk = 0.35
centrality = 0.25
surface_area = 0.20
uncertainty = 0.20
```

### API Key Resolution

flowdiff checks for API keys in this order:

1. `key_cmd` in `.flowdiff.toml` (e.g., `op read op://vault/flowdiff/api-key`)
2. `FLOWDIFF_API_KEY` environment variable
3. Provider-specific env var: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, or `GEMINI_API_KEY`

## Architecture

```
flowdiff/
├── crates/
│   ├── flowdiff-core/       # Library: all analysis logic
│   │   ├── src/
│   │   │   ├── git.rs       # Git diff extraction (git2)
│   │   │   ├── ast.rs       # Tree-sitter parsing
│   │   │   ├── ir.rs        # Language-agnostic intermediate representation
│   │   │   ├── query_engine.rs  # Declarative .scm query engine
│   │   │   ├── graph.rs     # Symbol graph (petgraph)
│   │   │   ├── flow.rs      # Data flow tracing + heuristics
│   │   │   ├── cluster.rs   # Semantic grouping
│   │   │   ├── rank.rs      # Review ordering
│   │   │   ├── entrypoint.rs    # Entrypoint detection
│   │   │   ├── pipeline.rs  # Pipeline entry points
│   │   │   ├── cache.rs     # SHA-256 based analysis caching
│   │   │   ├── config.rs    # .flowdiff.toml parsing
│   │   │   ├── output.rs    # JSON serialization
│   │   │   └── llm/         # LLM providers + VCR caching + judge
│   │   └── queries/         # Declarative .scm tree-sitter queries
│   │       ├── typescript/  # imports, exports, definitions, calls, assignments
│   │       └── python/      # imports, definitions, calls, assignments
│   ├── flowdiff-cli/        # Binary: CLI interface
│   └── flowdiff-tauri/      # Tauri desktop app
│       ├── src/             # Rust backend (IPC commands)
│       └── ui/              # React + Vite frontend
├── extensions/
│   └── vscode/              # VS Code extension
└── specs/                   # Design specifications
```

### Adding a New Language

flowdiff uses declarative tree-sitter query files. To add a new language:

1. Add `.scm` query files in `crates/flowdiff-core/queries/<language>/` (imports, definitions, calls, assignments)
2. Add the tree-sitter grammar crate to `Cargo.toml`
3. Register the language in the query engine

Zero Rust analysis code needed — the query engine maps `@capture` names to the shared IR types.

## JSON Output Schema

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
      "entrypoint": { "file": "src/routes/users.ts", "symbol": "POST", "type": "http_route" },
      "risk_score": 0.82,
      "review_order": 1,
      "files": [
        {
          "path": "src/routes/users.ts",
          "flow_position": 0,
          "role": "entrypoint",
          "changes": { "additions": 25, "deletions": 10 },
          "symbols_changed": ["POST", "validateUserInput"]
        }
      ],
      "edges": [
        {
          "from": "src/routes/users.ts::POST",
          "to": "src/services/user.ts::createUser",
          "type": "calls"
        }
      ]
    }
  ],
  "infrastructure_group": {
    "files": ["tsconfig.json", "package.json"],
    "reason": "Not reachable from any detected entrypoint"
  },
  "annotations": null
}
```

## Development

### Prerequisites

- Rust 1.75+
- Node.js 18+ (for Tauri UI and VS Code extension)
- System dependencies for tree-sitter and git2 (libgit2)

### Building

```bash
# Build all crates
cargo build

# Build CLI only
cargo build -p flowdiff-cli

# Build Tauri app
cd crates/flowdiff-tauri/ui && npm install
cargo tauri dev
```

### Testing

```bash
# Run all tests (1100+)
cargo test

# Run tests for a specific crate
cargo test -p flowdiff-core
cargo test -p flowdiff-cli

# Run VS Code extension tests
cd extensions/vscode && npm test

# Run Tauri Playwright E2E tests
cd crates/flowdiff-tauri/ui && npx playwright test

# Run live LLM integration tests (requires API keys)
FLOWDIFF_RUN_LIVE_LLM_TESTS=1 cargo test -p flowdiff-core -- --ignored
```

## License

MIT
