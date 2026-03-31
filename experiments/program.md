# diffcore autoresearch

You are an autonomous research agent improving diffcore's semantic grouping quality. You run in a loop, making one experiment per iteration. Each experiment modifies one variable, measures the result, and records it in `experiments/experiments.jsonl`.

**NEVER STOP.** Once the experiment loop has begun, do NOT pause to ask the human if you should continue. Do NOT ask "should I keep going?" or "is this a good stopping point?". The human might be asleep or away and expects you to continue working *indefinitely* until you are manually stopped. You are autonomous. If you run out of ideas, think harder — re-read the in-scope files, try combining previous near-misses, try more radical changes, look at failing repos for patterns. The loop runs until the human interrupts you, period.

## The Problem

diffcore takes a git diff and clusters changed files into semantic "flow groups" for human review. Quality means:
- Related files land in the same group
- Unrelated files stay separate
- Infrastructure/boilerplate files are correctly identified
- Group count stays reasonable (not too many singletons, not one mega-group)
- Review ordering matches human intuition (riskiest first)

## Setup

At the start of a new research session:

1. **Create a branch**: `git checkout -b autoresearch/<tag>` from current main (e.g. `autoresearch/mar26`). The branch must not already exist.
2. **Read the in-scope files**: Read these for full context:
   - `experiments/experiments.jsonl` — what's been tried and what worked
   - `eval/repositories.research.toml` — corpus manifest with goldens
   - `crates/diffcore-core/src/cluster.rs` — deterministic grouping (the main thing you edit)
   - `crates/diffcore-core/src/rank.rs` — review ordering weights
3. **Verify corpus exists**: Check that `~/Desktop/projects/just-understanding-data/diffcore-eval-corpus/` contains the expected repos.
4. **Establish baseline**: Run eval and record the baseline as experiment #0 if one doesn't exist yet.
5. **Confirm and go**: Confirm setup looks good, then start the loop.

## What You Can Change

### 1. Deterministic grouping parameters (cluster.rs)

These are hardcoded constants in `crates/diffcore-core/src/cluster.rs`:

| Parameter | Current | Location | What it does |
|-----------|---------|----------|-------------|
| `max_groups` default | 200 | ClusterOptions::default() | Hard cap on group count |
| Forward BFS cost | 1 | assign_shared_files() | Cost per hop in forward direction |
| Reverse BFS cost | 2 | assign_shared_files() | Cost per hop in reverse direction (2x forward) |
| Small group threshold | 3 | is_small() | Max files to be "small" for merging |
| Dir merge depths | 4,3,2,1 | consolidate_groups() | Progressive directory merge depths |
| Min token length | 3 | normalize_topic_token() | Min chars for topic tokens |
| Stop words list | ~40 words | STOP_WORDS | Filtered from topic naming |

### 2. Ranking weights (rank.rs)

| Weight | Current | What it scores |
|--------|---------|---------------|
| risk | 0.35 | Schema/API/auth/DB changes |
| centrality | 0.25 | Graph connectedness |
| surface_area | 0.20 | Lines changed (log scale) |
| uncertainty | 0.20 | Novel/unfamiliar patterns |

Risk sub-weights: schema=0.3, api=0.25, auth=0.35, db_migration=0.3

### 3. LLM refinement parameters

Configurable via `.diffcore.toml` or CLI flags:
- `--refine` enables LLM refinement
- `--refine-model <model>` picks the model (claude-sonnet-4-6, gpt-4.1, gemini-2.5-flash)
- `--refine-iterations <n>` number of refinement passes
- Provider: anthropic, openai, gemini

### 4. Golden expectations

Per-target golden files live in `eval/repos/<name>.toml`. Each file must classify ALL changed files:

**Required (100% coverage enforced by `lint-goldens`):**
- `infrastructure = ["file"]` - config, deps, CI, generated files, lockfiles, docs
- `non_infrastructure = ["file"]` - feature code, business logic, tests, API handlers
- Every file in the diff MUST appear in exactly one of these lists
- Run `cargo run -p diffcore-cli -- lint-goldens --manifest eval/repositories.research.toml` to check

**Required:**
- `group_count_min` / `group_count_max` - reasonable bounds on group count

**Recommended (selective, high-confidence only):**
- `same_group = [["file_a", "file_b"]]` - files that must cluster together
- `separate_group = [["file_a", "file_b"]]` - files that must NOT be in same group
- `infrastructure = ["file"]` - files that must land in infra group
- `non_infrastructure = ["file"]` - files that must NOT be in infra group
- `group_count_min` / `group_count_max` - group count bounds

### 4b. ML-ready labels (additive)

The eval constraints above are necessary, but they are not sufficient for a strong ML dataset. New and upgraded targets should also carry richer labels in an additive section such as `[repos.ml]`.

Rules:
- **Do not replace or weaken** the existing `[repos.expectations]` fields. Keep eval constraints intact.
- Add ML labels **alongside** the current expectations.
- A target is **ML-ready** when every changed file belongs to exactly one semantic group in `[[repos.ml.groups]]`.
- Prefer a full semantic partition of the diff over sparse pairwise labels alone.

Recommended additive shape:
- `[repos.ml]`
  - `dataset_version`
  - `underlying_repo`
  - `range_id`
  - `label_source`
  - `review_status`
- `[[repos.ml.groups]]`
  - `id`
  - `kind` (`feature`, `infrastructure`, `refactor`, `release`, etc.)
  - `files = [...]`
- Optional per-file metadata for future ML work:
  - `role` (`impl`, `test`, `schema`, `migration`, `config`, `docs`, `generated`, `dependency`, `ci`)
  - `surface` (`api`, `domain`, `persistence`, `ui`, `tooling`)
  - `change_kind` (`feature`, `fix`, `refactor`, `release`, `generated-sync`)

The point of this section is to turn the corpus into a supervised dataset, not just a benchmark.

### 5. Pipeline capabilities

These are structural improvements to the analysis layer — not parameters, but code/data that determines what the pipeline can detect:

| File | What it does |
|------|-------------|
| `crates/diffcore-core/src/entrypoint.rs` | Entrypoint detection patterns (HTTP routes, CLI commands, tests, etc.) |
| `crates/diffcore-core/queries/*.scm` | Tree-sitter queries that extract symbols from source code |
| `crates/diffcore-core/src/flow.rs` | Framework detection (Express, FastAPI, Effect.ts, etc.) |
| `crates/diffcore-core/src/graph.rs` | Symbol graph construction and edge types |
| `crates/diffcore-core/src/ir.rs` | Shared IR types (IrFile, IrFunctionDef, IrImport, etc.) |
| `crates/diffcore-core/src/ast.rs` | AST parsing and language-specific extraction |

### 6. Repo type tag

Every repo in the manifest has a `type` field:
- `type = "real"` — cloned from a real OSS project on GitHub
- `type = "synthetic"` — generated programmatically to exercise specific edge cases

This distinction matters for reporting: real repos test generalization, synthetic repos test specific capabilities.

## The Experiment Loop

Each experiment runs the eval harness. The eval takes ~30s-2min depending on corpus size. You launch it as:

```bash
cargo run -p diffcore-cli -- eval --manifest eval/repositories.research.toml --format json 2>/dev/null > /tmp/fd-eval-result.json
```

**Redirect output to files.** Do NOT let long eval output flood your context. Read only the summary metrics you need (avg_overall, per-repo scores, golden failures).

**Time budget**: Each experiment should take < 5 minutes wall clock (including compile + eval). If a run exceeds 10 minutes, kill it and treat it as a crash.

LOOP FOREVER:

1. **Read state**: Check `experiments/experiments.jsonl` — what's the current best? What hasn't been tried?
2. **Pick phase**: Choose the highest-priority phase with unfinished work (see phases below).
3. **Form hypothesis**: Be specific. "Increase small group threshold from 3 to 5 to reduce singleton explosion on starship" — not "try tweaking parameters".
4. **Make the change**: Edit code, config, or golden file. Keep changes minimal and reversible.
5. **git commit**: Commit the change so you can revert cleanly.
6. **Run eval**:
   ```bash
   cargo run -p diffcore-cli -- eval --manifest eval/repositories.research.toml --format json 2>/dev/null > /tmp/fd-eval-result.json
   ```
7. **Read results**: Extract key metrics from the output file. Compare to the last entry in experiments.jsonl. Track these metrics:
   - `avg_overall` — average overall score across all repos
   - `min_overall` — worst-performing repo (prevents hiding regressions)
   - `avg_golden` — average golden score (the metric we care about most)
   - `repos_passing` — count of repos meeting all thresholds
8. **Record**: Append one JSON line to `experiments/experiments.jsonl` (see formats below). NOTE: do not commit experiments.jsonl — leave it untracked.
   Every Phase 2/3 experiment MUST include `"optimization_scope": "local"` or `"optimization_scope": "global"`:
   - **LOCAL**: Hardcoded heuristics targeting specific repos/patterns (config filename lists, extension checks, threshold constants, specific entrypoint patterns). Quick wins but risk overfitting.
   - **GLOBAL**: Generic approaches that improve grouping broadly without repo-specific logic (embeddings, graph algorithms, import resolution via LSP, learned weights, architectural changes like the rescue mechanism). Slower but more durable.

   After 3 consecutive LOCAL experiments, do at least 1 GLOBAL. This prevents the algorithm from becoming a pile of special cases.

   When reviewing experiment history, compare avg improvement per LOCAL vs GLOBAL experiment to see which approach delivers more value.
9. **Decide**:
   - If avg_overall improved (or golden coverage increased) and no repo regressed more than 0.05: **keep** — advance the branch.
   - If avg_overall is equal or worse: **discard** — `git reset --hard HEAD~1` to revert.
   - If any single repo regressed more than 0.10: **discard** even if avg improved — fix the regression first.
   - If the build crashed or eval errored: **crash** — try to fix if it's trivial, otherwise revert and move on.

**Simplicity criterion**: All else being equal, simpler is better. A small improvement (+0.01 avg_overall) that adds ugly complexity is not worth it. Conversely, removing code and getting equal or better results is a great outcome — that's a simplification win. When evaluating whether to keep a change, weigh the complexity cost against the improvement magnitude.

**Crashes**: If a run fails to compile or crashes at runtime, use your judgment: if it's a typo or easy fix, fix it and re-run. If the idea itself is fundamentally broken, log "crash" as the status and move on. Don't spiral on a broken idea for more than 2-3 attempts.

**When you're stuck**: Don't give up — think harder:
- Re-read `cluster.rs` and `rank.rs` for angles you haven't tried
- Look at the worst-scoring repos — what specific pattern are they failing on?
- Try combining two previous near-miss improvements
- Try more radical architectural changes (new merging strategies, different graph traversals)
- Look at the diff content of failing repos to understand why grouping is wrong
- Consider adding new entrypoint detection patterns for underserved languages

## Recording Results

### experiments.jsonl format

Each experiment gets one JSON line. Format depends on experiment type:

**Deterministic experiments** (cluster/rank tuning):
```json
{
  "id": 1,
  "timestamp": "2026-03-26T14:30:00Z",
  "hypothesis": "Increase small group threshold from 3 to 5",
  "variable": "cluster.rs:small_group_threshold",
  "old_value": "3",
  "new_value": "5",
  "type": "deterministic",
  "optimization_scope": "local",
  "results": {
    "octospark": {"groups": 145, "infra_ratio": 0.917, "singleton_ratio": 0.241, "golden_score": 1.0, "overall": 1.0},
    "starship": {"groups": 12, "infra_ratio": 0.15, "singleton_ratio": 0.08, "golden_score": 1.0, "overall": 0.95}
  },
  "avg_overall": 0.97,
  "baseline_comparison": "+0.02 avg overall",
  "decision": "keep",
  "notes": "Reduced singletons on starship from 15% to 8% without hurting octospark"
}
```

**LLM refinement experiments** (must include model + prompt tracking):
```json
{
  "id": 5,
  "timestamp": "2026-03-27T10:00:00Z",
  "hypothesis": "claude-sonnet-4-6 with explicit grouping prompt improves over deterministic",
  "type": "llm",
  "model": "claude-sonnet-4-6",
  "prompt_version": "v2-explicit-grouping",
  "prompt_hash": "abc123",
  "iterations": 2,
  "results": {},
  "avg_golden_score": 0.92,
  "avg_golden_score_deterministic_only": 0.85,
  "delta_vs_deterministic": "+0.07",
  "avg_overall": 0.92,
  "token_count": 12500,
  "estimated_cost_usd": 0.04,
  "baseline_comparison": "+0.07 avg golden vs deterministic-only",
  "decision": "keep",
  "notes": "Best model+prompt combo so far"
}
```

**Sub-agent golden generation**:
```json
{
  "id": 4,
  "timestamp": "2026-03-26T15:00:00Z",
  "hypothesis": "Generate goldens for alertmanager-tracing via diff analysis sub-agent",
  "type": "golden-generation",
  "repo_name": "alertmanager-tracing",
  "method": "sub-agent-diff-analysis",
  "constraints_added": {
    "same_group": 3,
    "separate_group": 1,
    "infrastructure": 2,
    "non_infrastructure": 4
  },
  "golden_score_before": 1.0,
  "golden_score_after": 0.65,
  "avg_overall": 0.78,
  "decision": "keep",
  "notes": "Sub-agent identified 3 same_group clusters from import analysis. diffcore fails 2/3 — tracing files scattered across singletons. This gives Phase 2 clear targets."
}
```

**Corpus expansion**:
```json
{
  "id": 3,
  "timestamp": "2026-03-26T16:00:00Z",
  "hypothesis": "Add synthetic deep-import-chain repo to test 5-hop reachability",
  "type": "corpus-expansion",
  "repo_name": "synthetic-deep-imports",
  "repo_type": "synthetic",
  "language": "typescript",
  "file_count": 15,
  "decision": "keep",
  "notes": "Created 5-hop import chain A->B->C->D->E, added golden: A and E must be in same group"
}
```

**Crashes**:
```json
{
  "id": 6,
  "timestamp": "2026-03-27T11:00:00Z",
  "hypothesis": "Double BFS reverse cost from 2 to 4",
  "type": "deterministic",
  "avg_overall": 0.0,
  "decision": "crash",
  "notes": "Compile error: type mismatch in assign_shared_files. Reverted after 2 fix attempts."
}
```

## The Five Phases

The loop cycles through these 5 phases. Each iteration picks the highest-priority phase that still has unfinished work.

### Phase 0: Expand corpus
**Priority: HIGHEST (when coverage gaps exist).** The corpus should have broad coverage across languages, size tiers, and diff patterns. Check for gaps and fill them.

Storage constraint:
- Local disk is effectively full for this project.
- Prefer mining additional pinned diff ranges from repos already cloned under `diffcore-eval-corpus/`.
- Treat each `eval/repos/*.toml` file as one eval target. Multiple targets can come from the same underlying repo checkout.

**Adding real eval targets:**
1. Identify coverage gaps — check language counts, size tier distribution, framework diversity
2. First look for an existing local repo checkout that can supply another interesting historical range
3. Find an interesting diff range: `git log --oneline | grep -iE "feat|refactor"`
4. Verify file count: `git diff --stat <base>..<head> | tail -3` (10-100 ideal)
5. Add a new eval target with `type = "real"`, full SHA hashes, and initial golden constraints
6. Run eval to verify: `cargo run -p diffcore-cli -- eval --manifest eval/repositories.research.toml --format text 2>&1`
7. Only if the local corpus is clearly insufficient for the missing coverage gap should you consider cloning a new repo

**Adding synthetic repos:**
1. Identify a specific edge case not covered by real repos (e.g., deep import chains, monorepo with 5+ features, pure rename refactor)
2. Create a temp directory under `diffcore-eval-corpus/synthetic/<name>/`
3. Build a minimal repo with known-good grouping (you know the correct answer)
4. Commit a base state, then commit the changes
5. Add to manifest with `type = "synthetic"` and tight golden constraints (since you know the answer)
6. Run eval to verify

**Coverage targets:**
- 3-5 underlying repos per language (TS, Python, Go, Rust) across real + synthetic
- Multiple pinned diff targets per strong underlying repo
- At least 2 synthetic repos exercising: deep import chains, monorepo multi-feature, pure refactor, mixed-language
- Each size tier (small/medium/large) should have targets from multiple languages

**When to run this phase:** Check corpus coverage at the start of each loop iteration. If any language has < 3 underlying repos, the corpus lacks enough total targets, or no synthetic repos exist, this phase takes priority. Prefer filling gaps by mining more historical ranges from existing local repos before adding another clone.

### Phase 1: Build goldens via sub-agents
**Priority: HIGHEST (when `lint-goldens` reports gaps).** Use Claude Code sub-agents to generate golden constraints by reading the actual diff content. **Every file in the diff must be classified** — `lint-goldens` enforces this.

**Why full coverage?** Without classifying every file, diffcore can silently misplace files and the eval won't notice. A repo with 11/102 classified files has 91 blind spots. Full coverage eliminates this.

**Why sub-agents?** Goldens need to represent ground truth about how a human would group these changes for review. An LLM reading the diff content can determine this from the code itself — which files modify the same API, which are part of the same feature, which are infrastructure. This is independent of what diffcore currently outputs.

**For each target without goldens or without ML-ready labels:**

1. **Extract the diff** for the sub-agent to read:
   ```bash
   git -C <repo_path> diff <base>..<head> --stat > /tmp/fd-<name>-stat.txt
   git -C <repo_path> diff <base>..<head> > /tmp/fd-<name>-diff.txt
   ```
   For large diffs (500+ files), use `--stat` only and let the sub-agent read individual files as needed.

2. **Spawn a Claude Code sub-agent** (Agent tool) that reads the diff and generates golden constraints plus additive ML labels. The sub-agent should:
   - Read the diff stat to understand scope
   - Read the full diff (or key sections for large diffs)
   - For each changed file, determine: what feature/module/API does this belong to?
   - Identify which files should be grouped together (same feature, test+impl, schema+migration)
   - Identify which files are infrastructure (CI, config, deps, boilerplate)
   - Output golden constraints in TOML format
   - Output a full `[[repos.ml.groups]]` partition so every changed file belongs to exactly one semantic group

3. **Sub-agent prompt template:**
   ```
   Analyze this git diff to determine the ideal semantic groupings for code review.

   Diff stat: [paste or file path]
   Full diff: [paste or file path]

   Determine:
   - same_group: sets of files that belong together (same feature, test+impl, API+handler)
   - separate_group: files that are definitely unrelated changes
   - infrastructure: config/CI/deps/boilerplate files
   - non_infrastructure: core feature files that must be in semantic groups
   - group_count_min/max: reasonable bounds on group count
   - repos.ml.groups: a full semantic partition of the changed files for ML training

   Rules:
   - Be conservative — only assert constraints you're confident about
   - Focus on strong signals: imports between files, same API/schema, test+impl pairs
   - Use relative paths from repo root
   - Preserve existing [repos.expectations] fields; add ML labels additively under [repos.ml]
   - Every changed file must appear in exactly one infrastructure/non_infrastructure label and exactly one [[repos.ml.groups]] files list
   - Output in TOML format matching the manifest schema
   ```

4. **Review and merge** the sub-agent's constraints into `eval/repos/<name>.toml`

5. **Run eval** to see how diffcore scores against the new goldens:
   ```bash
   cargo run -p diffcore-cli -- eval --manifest eval/repositories.research.toml --format text 2>&1
   ```

6. **If a golden constraint fails:** decide whether the constraint is wrong (remove it) or diffcore is wrong (keep it — failing goldens drive improvement in Phase 2).
7. **If the target is not ML-ready yet:** add or complete `repos.ml.groups` before moving on. Sparse pairwise constraints alone are not enough for training data.

**Work order:** Small diffs first (fast, easy for the sub-agent), then medium, then large.

**Important:** Goldens represent the IDEAL grouping, not what diffcore currently produces. A golden that diffcore fails against is a signal for improvement, not a bug in the golden.
For ML dataset work, the ideal output is a full semantic partition plus the sparse eval constraints, stored together additively in the same TOML.

### Phase 2: Improve grouping quality
**Priority: HIGH.** This phase has two sub-tracks: parameter tuning and pipeline capability improvements. Both aim to improve golden scores, but they fix different kinds of problems.

**2a: Parameter tuning** — sweep `cluster.rs` and `rank.rs` knobs. One variable per experiment.

Key experiments:
- Small group threshold (3 → 4, 5, 6) — does it reduce singleton explosion?
- BFS cost asymmetry (1/2 → 1/3, 2/3) — does it change shared file assignment quality?
- Directory merge depths (4,3,2,1 → 5,4,3,2,1 or 3,2,1) — more/fewer merge passes?
- Ranking weights (risk/centrality/surface_area/uncertainty) — which balance works best?
- Stop words list — add/remove words that cause bad topic merges
- Min token length (3 → 2, 4) — affects group naming quality

**2b: Pipeline capability improvements** — when parameter tuning can't fix the problem because the pipeline is missing data. These are structural changes to the analysis layer.

Key experiments:
- **Entrypoint detection patterns**: Add new patterns for underserved languages/frameworks. If Rust repos score 0 because no entrypoints are detected, no amount of tuning cluster.rs fixes that. Add patterns in `crates/diffcore-core/src/entrypoint.rs` or the relevant `.scm` query files.
- **AST/IR extraction**: Improve `.scm` tree-sitter queries to capture more symbols, imports, or call expressions. Files in `crates/diffcore-core/queries/`. Adding a new language = writing `.scm` files, zero Rust code.
- **Graph edges**: Add new edge types to the symbol graph (e.g., detecting that a Rust `mod.rs` re-exports its children, or that a Go `_test.go` file tests its sibling).
- **Framework detection**: Add detection for frameworks not yet supported. See `crates/diffcore-core/src/flow.rs` for existing patterns.

**How to decide 2a vs 2b:** Look at why a golden is failing. If the files are parsed and connected in the graph but land in the wrong group → 2a (tuning). If the files aren't even detected as related (missing from graph, no entrypoints, no edges) → 2b (capability).

Run eval across ALL repos after each change. Track per-repo regressions carefully — a change that helps large diffs might hurt small ones.

### Phase 3: Optimize LLM refinement
**Priority: MEDIUM.** Compare providers, models, prompts, and iteration counts using VCR caching.

Experiments:
- Model comparison: claude-sonnet-4-6 vs gpt-4.1 vs gemini-2.5-flash
- Iteration count: 1 vs 2 vs 3 refinement passes
- Prompt variations: test different system prompts, grouping instructions, output schemas
- Measure: does LLM refinement improve golden scores over deterministic-only?
- Cost tracking: record token counts and estimated cost per run

**Model + prompt leaderboard:**
Track which model/prompt combinations produce the best golden scores. Each LLM experiment in `experiments.jsonl` MUST record:
```json
{
  "type": "llm",
  "model": "claude-sonnet-4-6",
  "prompt_version": "v2-explicit-grouping",
  "prompt_hash": "abc123",
  "iterations": 2,
  "avg_golden_score": 0.92,
  "avg_golden_score_deterministic_only": 0.85,
  "delta_vs_deterministic": "+0.07",
  "per_repo_scores": {"calcom": 0.95, "starship": 0.88},
  "token_count": 12500,
  "estimated_cost_usd": 0.04
}
```

This builds a leaderboard over time so we can answer: "which model + prompt gives the best refinement lift against our goldens?"

**VCR workflow for LLM experiments:**
1. First run with a new model records cassettes (VCR auto mode)
2. Subsequent runs with same model/diff replay from cache — free
3. Only switch to a new model when you've exhausted the current one
4. When changing prompts, the VCR cache auto-invalidates (prompt template hash changes)

### Phase 4: Synthetic data and edge cases
**Priority: LOW (but do some early).** Create synthetic repos that exercise edge cases:

The existing fixture system (`crates/diffcore-core/src/eval/fixtures.rs`) builds temp git repos with known structures. Add new fixtures for:
- Very large diffs (500+ files) with known grouping
- Diffs with deep import chains (A → B → C → D)
- Monorepo diffs spanning 5+ unrelated features
- Pure refactors (rename-only, move-only)
- Mixed language diffs (JS + Python in one repo)

You can also create synthetic test repos by:
1. Creating a temp directory
2. Adding files that exercise specific patterns
3. Committing and running diffcore against it

### Size Tier Strategy

The corpus has 3 size tiers. Experiments should be validated across all tiers:

| Tier | Files | Purpose | Repos |
|------|-------|---------|-------|
| Small | 13-40 | Fast iteration, detailed goldens | calcom, fastapi, starship, alertmanager, minikube, nushell-testing |
| Medium | 50-120 | Balanced coverage | payload-eval, saleor, gitea-permissions, rust-analyzer, saleor-recent, gitea-recent |
| Large | 200-3500 | Stress test caps and consolidation | calcom-large, payload-large, gitea-large, nushell-large, octospark |

When tuning parameters:
- If a change helps large diffs but hurts small ones, it's probably wrong
- If a change helps all tiers, it's a real improvement
- Golden failures on any tier block the change

## Eval Corpus

The corpus uses a split manifest structure:
- `eval/repositories.research.toml` — index file with `[defaults]` and `include_dir = "repos"`
- `eval/repos/*.toml` — one file per eval target with config, thresholds, and golden expectations

To add a new eval target: create `eval/repos/<name>.toml`. To edit goldens: edit the target's file directly. No need to touch the index.

Target: 3-5 repos per language for variety.

### TypeScript (3-5 repos)
- **octospark-services** (~3.4k files) - pinned, goldened (private)
- **payload** (PayloadCMS) - headless CMS, rich plugin architecture
- **cal.com** - scheduling platform, complex monorepo

### Python (3-5 repos)
- **full-stack-fastapi-template** (22 files) - small but clean
- **saleor** - e-commerce platform, Django-based
- **httpx** - HTTP client library, focused codebase

### Go (3-5 repos)
- **alertmanager** (32 files) - distributed tracing feature
- **gitea** - Git forge, large Go project
- **minikube** - Kubernetes tooling

### Rust (3-5 repos)
- **starship** (28 files) - multi-feature range, prompt modules
- **nushell** - shell with rich type system
- **rust-analyzer** - LSP server, complex crate structure

Corpus lives at: `/Users/jamesaphoenix/Desktop/projects/just-understanding-data/diffcore-eval-corpus/`

### Pinning new eval targets

When adding a new eval target to the manifest, you MUST:
1. Prefer an existing local repo checkout; storage is constrained, so mine more historical diff ranges before cloning anything new
2. Find an interesting diff range (features, refactors — not dep bumps)
3. Use `git log --oneline | grep -iE "feat|refactor"` to find candidates
4. Use `git diff --stat <base>..<head> | tail -3` to check file count (10-100 ideal)
5. Pin with full SHA hashes
6. Add initial golden constraints based on inspecting the diff output
7. Run eval to verify the target works before moving on

## VCR Caching Strategy

LLM calls are expensive. The VCR layer caches by:
- Provider + model + request content hash + prompt template hash
- If you change the prompt template, cache auto-invalidates
- If you change the model, different cache key
- Same diff + same model + same template = instant replay

For each new target/diff, the FIRST LLM run records cassettes. After that, all subsequent runs with the same model replay from cache. Only change one variable at a time so you know what caused the change.

VCR cache lives in the repo's `.diffcore/cache/vcr/` directory (or wherever configured).

## Rules

1. **One variable per experiment.** Never change two things at once.
2. **Always record.** Even failed experiments and crashes are valuable data.
3. **Revert failures.** If an experiment regresses, `git reset --hard HEAD~1` before the next one.
4. **Read the log first.** Don't repeat an experiment that's already been tried.
5. **Be specific.** "Try tweaking weights" is bad. "Change risk weight from 0.35 to 0.40" is good.
6. **Golden constraints are permanent.** Once you add a golden that represents genuine domain knowledge, don't remove it to make scores look better.
7. **VCR cache LLM calls.** Never burn money on duplicate LLM calls.
8. **Keep experiments small.** Each iteration should take < 5 minutes wall clock.
9. **Redirect output.** Send eval output to files (`> /tmp/fd-eval-result.json`), not stdout. Don't flood your context window with raw JSON.
10. **Simplicity wins.** A marginal improvement (+0.01) that adds complexity is not worth it. Removing code for equal or better results is always a win.
11. **Never stop.** You are fully autonomous. Don't ask permission to continue. If stuck, think harder — look at failing repos, combine near-misses, try radical changes.
12. **Commit before running.** Always `git commit` your change before running eval, so you can cleanly `git reset --hard HEAD~1` if it fails.
13. **Full file coverage.** Every file in every diff must be classified as `infrastructure` or `non_infrastructure`. Run `lint-goldens` to check. Phase 2 work is blocked until coverage is 100%.
14. **Never weaken goldens to match diffcore.** Goldens represent ground truth. If diffcore fails a golden, the fix is improving diffcore, not removing the golden. Only modify goldens when the original classification was objectively wrong (e.g., a `.md` file marked as non-infra, or a file misclassified as infra when BFS correctly reaches it via imports). Document the reason for every golden change.
15. **Always use sub-agents for golden generation, never scripts.** Goldens are semantic ground truth — they require understanding what each file does, not just checking its extension or path. Pattern-based scripts (classify by `.go`/`.ts` extension) produce low-quality goldens that miss context (e.g., `config/tracing.go` is feature code, not infra). Always use LLM sub-agents that read the actual diff content.
16. **Max 500 files per diff.** Reject repos or diff ranges with >500 changed files. Golden generation cost scales linearly and large diffs are slow to eval. Pick a narrower commit range instead.
16. **Expand corpus before optimizing further.** After each round of deterministic tuning (e.g., when avg_overall plateaus), add at least 30 new eval targets before continuing optimization. Because storage is constrained, prefer mining more pinned diff ranges from repos already on disk rather than cloning more repos. This prevents overfitting to the current corpus. Tune on N targets, validate on N+30.

## Files To Know

**Experiment infra:**
- `experiments/experiments.jsonl` - experiment log (append-only, untracked)
- `eval/repositories.research.toml` - manifest index (defaults + `include_dir = "repos"`)
- `eval/repos/*.toml` - one file per eval target with config + goldens (edit these directly)

**Grouping algorithm (Phase 2a — parameter tuning):**
- `crates/diffcore-core/src/cluster.rs` - deterministic grouping constants
- `crates/diffcore-core/src/rank.rs` - review ordering weights

**Pipeline capabilities (Phase 2b — structural improvements):**
- `crates/diffcore-core/src/entrypoint.rs` - entrypoint detection patterns
- `crates/diffcore-core/queries/*.scm` - tree-sitter queries per language
- `crates/diffcore-core/src/flow.rs` - framework detection
- `crates/diffcore-core/src/graph.rs` - symbol graph and edge types
- `crates/diffcore-core/src/ir.rs` - shared IR types
- `crates/diffcore-core/src/ast.rs` - AST parsing

**Supporting:**
- `crates/diffcore-core/src/config.rs` - config schema
- `crates/diffcore-core/src/llm/vcr.rs` - VCR caching
- `crates/diffcore-core/src/eval/repos.rs` - repo eval harness
- `docs/grouping-overhaul.md` - prior work handoff

## Getting Started

If `experiments/experiments.jsonl` is empty or doesn't exist, start with the baseline: run eval as-is and record experiment #0 with the current scores. Then move to Phase 1 (golden generation via sub-agents) for repos that lack goldens.

If experiments already exist, read them, identify the most promising direction, and continue from there. Look at:
- Which repos have the worst scores? Why?
- Which experiments showed the biggest improvements? Can you push further in that direction?
- Are there repos without goldens? Generate them first (Phase 1).
- Are there coverage gaps in the corpus? Fill them (Phase 0).
