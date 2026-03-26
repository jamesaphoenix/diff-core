# flowdiff autoresearch

Run the autonomous research loop for improving flowdiff's semantic grouping quality. Inspired by karpathy/autoresearch.

## Instructions

You are kicking off (or resuming) the flowdiff autoresearch loop. Each invocation runs ONE experiment. **Do NOT ask if you should continue** — you are fully autonomous.

### Step 1: Read context
Read these files to understand current state:
- `experiments/program.md` — full research instructions, tunable parameters, rules
- `experiments/experiments.jsonl` — experiment history (what's been tried, what worked)
- `eval/repositories.research.toml` — manifest index (defaults + `include_dir = "repos"`)
- `eval/repos/*.toml` — one file per repo with config + goldens (edit these for golden changes)

### Step 2: Setup (first run only)
If this is the first run of a session:
- Create branch: `git checkout -b autoresearch/<tag>` (e.g. `autoresearch/mar26`)
- Run baseline eval and record as experiment #0 if no baseline exists

### Step 3: Decide what to do

Based on the experiment history, pick the highest-priority phase with unfinished work:

**Phase 0: Expand corpus** (highest priority if coverage gaps exist)
- Check language counts in the manifest — each language should have 3-5 repos
- Check if any synthetic repos exist — if not, create one
- For real repos: clone from GitHub, find interesting diff range, pin with full SHAs, add `type = "real"`
- For synthetic repos: create under `flowdiff-eval-corpus/synthetic/<name>/`, add `type = "synthetic"` with tight goldens
- Clone destination: `~/Desktop/projects/just-understanding-data/flowdiff-eval-corpus/<language>/<repo>`

**Phase 1: Build goldens via sub-agents** (highest priority if goldens are sparse)
Use Claude Code sub-agents to generate golden constraints from diff content. For each repo that lacks goldens:

1. Get the diff content:
   ```bash
   git -C <repo_path> diff <base>..<head> --stat > /tmp/fd-<name>-stat.txt
   git -C <repo_path> diff <base>..<head> > /tmp/fd-<name>-diff.txt
   ```

2. Spawn a sub-agent (Agent tool) with this prompt:
   ```
   You are analyzing a git diff to determine ideal semantic groupings for code review.

   Read the diff output at /tmp/fd-<name>-diff.txt (or the diff content below).
   Also read the file list to understand the scope.

   For each changed file, determine:
   - What feature/module/API does this change belong to?
   - Which other changed files are part of the same logical change?
   - Is this file infrastructure/boilerplate (config, deps, CI) or a semantic change?

   Output golden constraints in this exact format:
   same_group = [
     ["path/to/file_a", "path/to/file_b"],  # reason: both part of X feature
   ]
   separate_group = [
     ["path/to/file_x", "path/to/file_y"],  # reason: unrelated changes
   ]
   infrastructure = [
     "path/to/config_file",  # reason: CI/config boilerplate
   ]
   non_infrastructure = [
     "path/to/feature_file",  # reason: core feature logic
   ]
   group_count_min = N  # minimum reasonable groups
   group_count_max = N  # maximum reasonable groups

   Be conservative: only assert constraints you're confident about from reading the code.
   Focus on the strongest signals: files that import each other, files that modify the same
   API/schema/feature, test files paired with their implementation.
   ```

3. Review the sub-agent's output, add constraints to the repo's file in `eval/repos/<name>.toml`
4. Run eval to verify: `cargo run -p flowdiff-cli -- eval --manifest eval/repositories.research.toml --format text 2>&1`
5. If a golden constraint fails, check whether the constraint is wrong (remove it) or flowdiff is wrong (keep it — that's what we're trying to improve)

**Phase 2: Improve grouping quality** (after goldens exist)
Two sub-tracks — pick based on WHY goldens are failing:

**2a: Parameter tuning** (files are parsed but land in wrong groups)
- Change ONE constant in `cluster.rs` or `rank.rs`
- git commit → run eval → keep or `git reset --hard HEAD~1`

**2b: Pipeline capability** (files aren't detected as related — missing entrypoints, edges, or AST data)
- Add entrypoint patterns (`entrypoint.rs`), `.scm` tree-sitter queries (`queries/`), graph edges, or framework detection (`flow.rs`)
- git commit → run eval → keep or `git reset --hard HEAD~1`
- Look at the worst-scoring repos — if a language gets 0 groups, it's a 2b problem

**Phase 3: Optimize LLM refinement** (after deterministic is tuned)
- Test models/prompts/iterations with VCR caching
- Compare golden scores: deterministic-only vs with-refinement
- Record model, prompt_version, prompt_hash, iterations, per-repo golden scores, token count, and estimated cost
- Goal: build a leaderboard of which model + prompt gives the best refinement lift

**Phase 4: Synthetic data** (ongoing, interleave with other phases)
- Create new fixtures in the eval system or synthetic test repos
- Add to manifest with `type = "synthetic"`

### Step 4: Run the experiment
- Make ONE change
- `git commit` the change
- Run eval: `cargo run -p flowdiff-cli -- eval --manifest eval/repositories.research.toml --format json 2>/dev/null > /tmp/fd-eval-result.json`
- Read the result file for key metrics

### Step 5: Record the result
Append one JSON line to `experiments/experiments.jsonl`. See `experiments/program.md` for the full schema per experiment type.
```json
{"id": N, "timestamp": "...", "hypothesis": "...", "type": "golden-generation|deterministic|llm|corpus-expansion|synthetic", ...}
```
Types: `golden-generation` (sub-agent generated goldens), `deterministic` (cluster/rank tuning), `llm` (refinement model+prompt), `corpus-expansion` (new repos), `synthetic` (fixture data), `crash` (failed experiments).

### Step 6: Keep or revert
- If improved: keep — advance the branch
- If equal or worse: `git reset --hard HEAD~1`
- If crashed: fix if trivial, otherwise revert and move on

**Never stop. Never ask permission. If stuck, think harder.**

## Key Numbers (baseline)
- 17 repos across 4 languages (TS, Python, Go, Rust), all `type = "real"`
- 0 synthetic repos (gap — Phase 0 should add some)
- 3 size tiers: small (13-40 files), medium (50-120), large (200-3500)
- Baseline avg_overall: 0.82, 2/17 PASS

## Arguments

- `$ARGUMENTS` - Optional: specific phase to work on ("corpus", "golden", "deterministic", "llm", "synthetic") or a specific hypothesis
