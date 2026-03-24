# flowdiff — a semantic diff layer

**flowdiff** is a semantic diff layer for code review. Git gives you syntactic diffs — what text changed in which files. flowdiff adds meaning on top — what those changes *mean*, how they relate, and what order a human should read them in. Built for the era of AI agents producing 50–100 file PRs.

Three levels of semantics, each building on the last:

1. **Structural** (free, deterministic) — builds a symbol graph from tree-sitter ASTs, detects entrypoints (HTTP routes, CLI commands, queue consumers, Effect.ts services, etc.), clusters changed files into flow groups via forward reachability, traces data flow across call chains.
2. **Heuristic** (free, deterministic) — framework detection (Express, Next.js, FastAPI, Effect.ts, 30+ frameworks), risk scoring, review ordering by composite score (risk/centrality/surface-area/uncertainty).
3. **LLM refinement** (paid, optional) — Anthropic, OpenAI, or Gemini reads the actual diff content and refines groupings: split coincidental coupling, merge scattered refactors, re-rank by semantic review order, reclassify misplaced files. Evaluator-optimizer loop scores v1 vs v2, keeps whichever is better.

## Architecture

- **Rust core** (`flowdiff-core`) — all analysis logic. Shared IR maps tree-sitter ASTs from any language into language-agnostic types via declarative `.scm` query files. Adding a new language = writing `.scm` files, zero Rust code.
- **CLI** (`flowdiff-cli`) — `flowdiff analyze --base main`, JSON output, `--refine` for LLM refinement, `--annotate` for LLM narration.
- **Tauri desktop app** — three-panel UI: flow groups | Monaco diff viewer | annotations/Mermaid graph. Keyboard-driven (j/k/J/K).
- **VS Code extension** — thin shell over CLI. Tree view + native diff editor + webview annotations.

## Core Features (implemented)

- **Git layer** — branch comparison, commit ranges, staged/unstaged diffs via git2
- **AST layer** — tree-sitter parsing for TS/JS + Python via declarative `.scm` query files
- **Shared IR** — language-agnostic types: IrFile, IrFunctionDef, IrTypeDef, IrImport/IrExport, IrCallExpression, IrAssignment with destructuring patterns (object, array, tuple, nested, rest/spread)
- **Symbol graph** — directed graph (petgraph) with imports/calls/extends/instantiates/reads/writes/emits/handles edges
- **Entrypoint detection** — HTTP routes, CLI commands, queue consumers, cron jobs, test files, React pages, event handlers, Effect.ts services (HttpApi, @effect/cli, Queue/PubSub, Schedule, Stream/Hub, Effect.Service/Layer)
- **Semantic clustering** — forward reachability from entrypoints, shared file assignment by shortest path, infrastructure group for unreachable files
- **Review ranking** — composite score: risk (0.35) + centrality (0.25) + surface area (0.20) + uncertainty (0.20)
- **Data flow tracing** — variable assignment tracking, call argument extraction, cross-file data flow edges, heuristic inference (DB writes, event emission, config reads, HTTP calls, logging)
- **Framework detection** — auto-detect Express, Next.js, React, FastAPI, Flask, Django, Prisma, Effect.ts, 30+ frameworks
- **LLM annotation** — Pass 1 (overview) + Pass 2 (per-group deep analysis) via Anthropic, OpenAI, Gemini with structured outputs
- **VCR caching** — record/replay LLM calls for deterministic CI
- **LLM-as-judge** — evaluator that scores analysis quality across 5 criteria
- **Eval suite** — 5 synthetic fixture codebases, deterministic scoring, 0.89 avg score
- **Config** — `.flowdiff.toml` with entrypoint globs, layer names, ignore patterns, LLM settings, refinement settings

## Tests

1987 tests: unit (co-located `#[cfg(test)]`), integration (`tests/` directory), property-based (proptest), snapshot (insta), regression (`tests/regressions.rs`), live LLM (gated behind `FLOWDIFF_RUN_LIVE_LLM_TESTS=1`), VCR replay, CLI arg parsing + config override tests, eval harness. Playwright E2E planned for Tauri app.

## Specs

All specs live in `specs/`. When creating or updating a spec, always add or update its entry in [`specs/readme.md`](./specs/readme.md) — that file is the index of all specs.
