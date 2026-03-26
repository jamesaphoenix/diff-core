# Human Experiment Ideas for Later

Ideas queued for testing in the autoresearch loop. Move items to `experiments.jsonl` when tested.

## Phase 3: LLM Refinement Experiments

### Parameters to sweep

1. **Number of iterations** — test 1, 2, 3, 5, 10, 15 refinement passes
   - More iterations = more expensive but potentially better grouping
   - Diminishing returns after some threshold — find it
   - Cap at 10-15 iterations

2. **Model** — compare across providers
   - Anthropic: claude-sonnet-4-6, claude-opus-4-6, claude-haiku-4-5
   - OpenAI: gpt-4.1, gpt-4.1-mini, o4-mini
   - Google: gemini-2.5-flash, gemini-2.5-pro

3. **Provider** — same model class across different providers
   - Track: golden_score delta vs deterministic-only
   - Track: token count, latency, estimated cost per run

4. **Codex SDK vs Agent SDK** — compare orchestration approaches
   - [OpenAI Codex SDK](https://developers.openai.com/codex/sdk) — code execution + reasoning
   - [Anthropic Agent SDK](https://platform.claude.com/docs/en/agent-sdk/) — tool use + structured output
   - Hypothesis: Agent SDK with tool use for reading diffs may outperform raw prompt-based refinement
   - Test: same grouping task, same model tier, different SDK orchestration

### Experiment matrix

| Variable | Values to test |
|----------|---------------|
| iterations | 1, 2, 3, 5, 10, 15 |
| model | claude-sonnet-4-6, gpt-4.1, gemini-2.5-flash |
| provider | anthropic, openai, gemini |
| orchestration | native refinement, codex-sdk, agent-sdk |

### Metrics to track per experiment

- `model`: model name
- `prompt_version`: prompt template ID
- `iterations`: number of refinement passes
- `avg_golden_score`: score WITH refinement
- `avg_golden_score_deterministic_only`: score WITHOUT refinement (baseline=0.98)
- `delta_vs_deterministic`: the lift from LLM refinement
- `per_repo_scores`: per-repo breakdown
- `token_count`: total tokens used
- `estimated_cost_usd`: cost estimate
- `latency_seconds`: wall clock time

### Success criteria

- delta_vs_deterministic > 0 (any improvement)
- Cost < $1 per repo analysis
- Latency < 60s per repo
- No regression on repos that already PASS

## Global Optimization Ideas (generic approaches, not hardcoded heuristics)

### 1. Embeddings + Cosine Similarity for file grouping
- Use open-source code embeddings (e.g., `jina-embeddings-v3`, `voyage-code-3`, `CodeBERT`, `UniXcoder`) to embed each file's diff
- Compute pairwise cosine similarity between file diffs
- Cluster files with high similarity into the same group
- **Why:** This is a generic signal that doesn't need language-specific heuristics — semantically similar diffs naturally cluster
- **Open-source options:** `sentence-transformers` (Python), `fastembed` (Rust), Ollama local models
- **Experiment:** Compare embedding-based grouping vs current deterministic grouping vs LLM refinement
- **Cost:** Local embeddings are free; API embeddings are cheap ($0.001/1K tokens)

### 2. Compiler API / Language Server Protocol for IR
- Use TypeScript's `tsserver` / `tsc --declaration` for precise import resolution
- Use `rust-analyzer` LSP for Rust `use crate::` path resolution
- Use `gopls` for Go same-package implicit imports
- **Why:** Compiler APIs give exact import graphs — no heuristic guessing. This replaces our `.scm` tree-sitter queries with ground-truth resolution.
- **Experiment:** For TS repos, run `tsserver` to get the import graph, compare golden scores vs tree-sitter-only
- **Integration:** Modify IR layer to accept LSP-sourced edges alongside tree-sitter edges

### 3. Generic optimization approaches (vs hardcoded heuristics)
- Current algorithm is a stack of hardcoded heuristics (SMALL_GROUP_THRESHOLD, MAX_MERGE_BUCKET_SIZE, is_config_like_filename, etc.)
- Each heuristic helps some repos but may hurt others
- **Alternative approaches:**
  - Learned weights: use the golden corpus to learn optimal weights for different signals
  - Graph-based: treat the diff as a weighted graph (import edges, directory proximity, file stem similarity, embedding similarity) and use community detection (Louvain, spectral clustering)
  - Multi-signal fusion: combine tree-sitter graph, embeddings, directory proximity, naming conventions into a single similarity matrix, then cluster

## Phase 2b: Import Graph Ideas

- Rust `use crate::` path resolution → file paths
- Go same-package implicit imports
- TypeScript monorepo cross-package imports (`@org/package`)
- Python relative imports (`from . import`)

## Micro/Macro Experiment Queue (alternate between them)

**Counter: next = MACRO #1** (after 3 consecutive LOCAL: #59, #60, #61)

### Micro (LOCAL) — targeted heuristics
1. [ ] `.changeset/` directory → infra (Effect.ts repos)
2. [ ] `version.go` / `version.ts` → `is_config_like_filename` (auto-bumped constants)
3. [ ] `__init__.py` context-aware — only config if <10 lines or non-test dir
4. [ ] Cross-directory stem match: `django/forms/fields.py` ↔ `tests/forms_tests/*/test_*.py`
5. [ ] `go.mod` + `go.sum` always same group (post-processing merge)

### Macro (GLOBAL) — generic approaches
1. [ ] **Diff-based embeddings** — embed change hunks not full content, cluster by change similarity
2. [ ] **Graph community detection (Louvain)** — community detection on symbol graph via petgraph
3. [ ] **Co-change mining** — `git log --name-only` co-change frequencies as grouping signal
4. [ ] **Multi-signal fusion** — weighted similarity matrix (graph + embeddings + directory + name)
5. [ ] **Learned merge policy** — train on 47 golden repos, extract pairwise features, learn merges

### Alternation rule
- Pick from Macro list first (counter says MACRO), then switch to Micro, etc.
- After testing an item, mark it `[x]` and update the counter line.
- If an item is blocked (e.g., needs API keys), skip to next in same list.

## Phase 0: Corpus Expansion Ideas

- Add Java repo (Spring Boot)
- Add C# repo (.NET)
- Create synthetic repos for edge cases
- Add more Rust repos (tokio, serde)
