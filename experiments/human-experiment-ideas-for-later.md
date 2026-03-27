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

**Schedule: 10 MACRO → 10 MICRO → 10 GROWING_DATA → repeat**

**Current phase: GROWING_DATA toward 107 repos. Next MACRO cycle: pie-in-the-sky ideas.**

### Macro (GLOBAL) — pie-in-the-sky research ideas
These are ambitious, research-grade approaches. Each may take multiple experiments.

1. [ ] **Graph community detection (Louvain/Leiden)** — treat symbol graph as weighted network, find natural communities. Replace BFS-from-entrypoints with data-driven clustering. Crate: `petgraph` + community detection algorithm.
2. [ ] **Spectral clustering on embeddings** — build similarity graph from 768-dim file embeddings, compute Laplacian eigenvectors, k-means on spectral space. Discovers non-convex clusters that centroid methods miss.
3. [ ] **Learned merge policy** — use 90+ golden repos as training data. For each pair of groups, extract features (directory overlap, embedding cosine, graph connectivity, stem match, file count ratio). Train a simple classifier (logistic regression / decision tree) to predict "should merge". Apply to new repos.
4. [ ] **Co-change mining from git history** — `git log --name-only` to build co-change frequency matrix. Files that change together in past commits → same group. Signal independent of code content.
5. [ ] **Multi-signal fusion** — combine graph edges + embedding similarity + directory proximity + filename similarity + co-change into a single weighted similarity matrix. Cluster on the fused matrix. Currently signals are applied sequentially; fusion could find groups no single signal detects.
6. [ ] **Hierarchical agglomerative clustering** — build distance matrix from import graph edge weights. Ward/average linkage to form hierarchy. Cut at optimal level for group count. More principled than BFS+consolidation.
7. [ ] **Feature generation pipeline** — extract per-file features: path depth, extension, directory tokens, import count, change size (additions+deletions), embedding vector. Feed into clustering algorithm (HDBSCAN, k-means, DBSCAN).
8. [ ] **GNN on symbol graph** — message-passing neural network on the file/symbol graph to learn file representations. Train on golden corpus labels. Would capture transitive dependencies that BFS misses.
9. [ ] **Active learning loop** — use golden failures as hard negatives. For each failing same_group/infra constraint, extract the feature vector and add to training set. Iteratively improve the merge/classify decision boundary.
10. [ ] **Hybrid BFS + embedding tiebreaker** — when BFS assigns a file to multiple entrypoints at equal distance, use embedding similarity to the group centroid to break the tie. Currently uses ep_idx (arbitrary).

### Micro (LOCAL) — targeted heuristics
Quick wins from specific failure patterns. Each is one experiment.

1. [ ] `lib.rs` / `mod.rs` → infra when file has <5 lines (just re-exports)
2. [ ] `.tmpl` / `.gohtml` → same group as Go handler matching by path stem
3. [ ] `coverage-final.json` / `*.snap` → Generated infra (test artifacts)
4. [ ] `examples/` directory files → feature code (not infra) unless config
5. [ ] `.sql` files in `db/` or `migrations/` → infra (schema/seeds)
6. [ ] `.properties` files → infra when not in src/main/ (Java convention)
7. [ ] `Makefile` in subdirectories → infra
8. [ ] `.proto` generated `.pb.go`/`.pb.rs` → always infra (Generated)
9. [ ] `conftest.py` → context-aware (test fixture vs config)
10. [ ] `_test.go` files in `vendor/` → always infra

### Growing Data — ongoing corpus expansion
- Target: 107 repos for Round 4 gate (91 currently, 16 more needed)
- Re-classify octospark via sub-agents (fix 1894 non-infra failures)
- Add Guava Java (172 files, needs divide-and-conquer)
- Continue diversifying: more C, C++, Dart, Elixir repos

### Refactoring
- [ ] **Split cluster.rs into modules** — classify.rs, merge.rs, rescue.rs, stem.rs (see task #1)

### Schedule rule
- MACRO ideas are research projects — may span multiple experiments each
- MICRO ideas are one-shot — test, keep/revert, move on
- After GROWING_DATA: pick the most promising MACRO idea and deep-dive

## Phase 0: Corpus Expansion Ideas

- Add Java repo (Spring Boot)
- Add C# repo (.NET)
- Create synthetic repos for edge cases
- Add more Rust repos (tokio, serde)
