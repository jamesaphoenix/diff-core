## Step 1: Read these files first (in this order)

1. CLAUDE.md — project summary, architecture, core features
2. specs/readme.md — spec index
3. specs/diff-analyzer.md — the full spec (sections 1-12). Find the first phase with unchecked `- [ ]` tasks

## Step 2: Read the code you'll need for the current phase

Phase 3 (Tauri App):
- crates/diffcore-core/src/lib.rs — public API surface
- crates/diffcore-core/src/types.rs — FlowGroup, FileChange, Symbol, Edge
- crates/diffcore-core/src/output.rs — AnalysisOutput JSON schema (IPC contract)
- crates/diffcore-core/src/llm/schema.rs — Pass1Response, Pass2Response, Annotations
- crates/diffcore-tauri/ui/src/ — existing React components
- Cargo.toml — workspace members

Phase 4 (LLM Integration):
- crates/diffcore-core/src/llm/mod.rs — LlmProvider trait
- crates/diffcore-core/src/llm/anthropic.rs, openai.rs, gemini.rs — provider implementations
- crates/diffcore-core/src/llm/vcr.rs — VCR caching
- crates/diffcore-core/src/config.rs — .diffcore.toml structure

Phase 5 (VS Code Extension):
- crates/diffcore-cli/src/main.rs — CLI interface (extension spawns this)
- crates/diffcore-core/src/output.rs — JSON output schema
- specs/diff-analyzer.md sections 9, 12.10 — VS Code spec + tests

Phase 6 (Polish):
- crates/diffcore-core/src/lib.rs — all pub exports (for clippy deny wall)
- crates/diffcore-core/src/pipeline.rs — pipeline entry points

Phase 7 (Eval Suite):
- crates/diffcore-core/src/llm/judge.rs — LLM-as-judge
- crates/diffcore-core/src/llm/vcr.rs — VCR caching
- crates/diffcore-core/tests/eval_suite.rs — existing eval tests

## Step 3: Pick the most important unchecked task and implement it

CRITICAL: Complete phases sequentially (Phase 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8). Do NOT skip ahead to a later phase while earlier phases have unchecked tasks. Finish all tasks in the current phase before moving on.

## Housekeeping

- before starting work, check if `target/` is over 5GB: `du -sh target/ 2>/dev/null`. If over 5GB, run `cargo clean` to prune build artifacts before proceeding

## Rules

- author property based tests or unit tests (which ever is best)
- after making the changes to the files run the tests
- update the implementation plan when the task is done
- when tests pass, commit and push to deploy the changes
- for LLM integration tests that need API keys, set ANTHROPIC_API_KEY / OPENAI_API_KEY env vars or create a .env file in the repo root
- when working on Phase 7 (Synthetic Eval Suite), read these references first for eval architecture patterns:
  - https://understandingdata.com/posts/evaluator-optimizer-evolutionary-search/
  - https://understandingdata.com/posts/autonomous-loops-need-benchmarks/
