# diffcore — a semantic diff layer

This file is the canonical repo guidance for local agents. `CLAUDE.md` should symlink to this file.

**Diffcore** is a semantic diff layer for code review. Git gives you syntactic diffs — what text changed in which files. Diffcore adds meaning on top — what those changes *mean*, how they relate, and what order a human should read them in. Built for the era of AI agents producing 50–100 file PRs.

Three levels of semantics, each building on the last:

1. **Structural** (free, deterministic) — builds a symbol graph from tree-sitter ASTs, detects entrypoints (HTTP routes, CLI commands, queue consumers, Effect.ts services, etc.), clusters changed files into flow groups via forward reachability, traces data flow across call chains.
2. **Heuristic** (free, deterministic) — framework detection (Express, Next.js, FastAPI, Effect.ts, 30+ frameworks), risk scoring, review ordering by composite score (risk/centrality/surface-area/uncertainty).
3. **LLM refinement** (paid, optional) — Anthropic, OpenAI, or Gemini reads the actual diff content and refines groupings: split coincidental coupling, merge scattered refactors, re-rank by semantic review order, reclassify misplaced files. Evaluator-optimizer loop scores v1 vs v2, keeps whichever is better.

## Target Grouping Strategy

Diffcore is intentionally a hybrid system. The target architecture is not "heuristics or ML" and not "replace the engine with an LLM." It is:

1. **Structural + heuristic prior** — deterministic graph extraction, entrypoint detection, reachability, framework cues, and file-role heuristics produce the strongest cheap baseline possible.
2. **ML grouping layer** — a learned model trained on the growing golden eval corpus scores pairwise or file-to-group relationships and proposes better initial groupings than heuristics alone.
3. **LLM repair pass** — an optional, bounded refinement layer applies semantic patches on top of the deterministic/ML proposal for the ambiguous cases the cheaper layers still miss.

The intended end state is a mixture of heuristics, ML, and LLMs: heuristics for hard signals and reproducibility, ML for learned grouping patterns at scale, and LLMs for expensive edge-case cleanup.

## Agent Guidance

- Keep the deterministic structural and heuristic layers as the foundation.
- Add learned behavior on top of those signals, not instead of them.
- Preferred pipeline: structural/heuristic feature extraction -> ML grouping/scoring -> optional LLM refinement -> golden eval.
- Do not propose an end-to-end LLM grouper as the primary architecture.
- Train ML components against the pinned golden eval corpus.
- Split train/validation/test by repository to avoid leakage.
- Prefer pairwise scoring or file-to-group attachment models over freeform generation.
- Treat the LLM as a bounded patch layer that edits proposed groups, not a from-scratch clustering engine.
- Measure deterministic-only, ML-only, and ML+LLM results separately on the eval suite.
- Preserve the hybrid goal: heuristics for precision and reproducibility, ML for scalable learned grouping, and LLMs for the residual semantic edge cases.

## Large-Diff Track

Large-diff work is a separate evaluation track, not part of the default live-repo experimentation pipeline.

- Keep the main experimentation pipeline focused on the normal live-repo corpus.
- Evaluate large-diff-specific heuristics, metrics, and synthetic data in a separate large-diff manifest/pipeline.
- Use the separate large-diff track for `1k+` and especially `2k+` file diffs where coarse partitioning and density-style group metrics are needed.
- Primary real large-diff manifest: `eval/repositories.large-diff.toml`
- Primary gated synthetic large-diff manifest (`2k+` only): `eval/repositories.large-diff.synthetic.toml`
- Near-threshold synthetic diagnostic manifest (`~1.5k`, non-gating): `eval/repositories.large-diff.synthetic.near-threshold.toml`
- Do not let large-diff-specific compromises silently change the baseline behavior or scores for the main corpus.
- If a change is intended only for large repos, gate it explicitly by diff size and validate it in the separate large-diff pipeline first.

## Architecture

- **Rust core** (`diffcore-core`) — all analysis logic. Shared IR maps tree-sitter ASTs from any language into language-agnostic types via declarative `.scm` query files. Adding a new language = writing `.scm` files, zero Rust code.
- **CLI** (`diffcore-cli`) — `diffcore analyze --base main`, JSON output, `--refine` for LLM refinement, `--annotate` for LLM narration.
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
- **Config** — `.diffcore.toml` with entrypoint globs, layer names, ignore patterns, LLM settings, refinement settings

## Tests

1987 tests: unit (co-located `#[cfg(test)]`), integration (`tests/` directory), property-based (proptest), snapshot (insta), regression (`tests/regressions.rs`), live LLM (gated behind `DIFFCORE_RUN_LIVE_LLM_TESTS=1`), VCR replay, CLI arg parsing + config override tests, eval harness. Playwright E2E planned for Tauri app.

## Specs

All specs live in `specs/`. When creating or updating a spec, always add or update its entry in [`specs/readme.md`](./specs/readme.md) — that file is the index of all specs.
