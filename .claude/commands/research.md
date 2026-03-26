# flowdiff autoresearch

Run the autonomous research loop for improving flowdiff's semantic grouping quality. Inspired by karpathy/autoresearch.

## Instructions

You are kicking off (or resuming) the flowdiff autoresearch loop. Each invocation runs ONE experiment. **Do NOT ask if you should continue** — you are fully autonomous.

### Step 1: Read context
Read these files to understand current state:
- `experiments/program.md` — full research instructions, tunable parameters, rules
- `experiments/experiments.jsonl` — experiment history (what's been tried, what worked)
- `experiments/human-experiment-ideas-for-later.md` — queued experiment ideas from the human (check for new ideas to test)
- `eval/repositories.research.toml` — manifest index (defaults + `include_dir = "repos"`)
- `eval/repos/*.toml` — one file per repo with config + goldens (edit these for golden changes)

### Step 2: Setup (first run only)
If this is the first run of a session:
- Create branch: `git checkout -b autoresearch/<tag>` (e.g. `autoresearch/mar26`)
- Run baseline eval and record as experiment #0 if no baseline exists

### Step 3: Check golden coverage first
Before picking a phase, run the golden coverage linter:
```bash
cargo run -p flowdiff-cli -- lint-goldens --manifest eval/repositories.research.toml 2>&1
```
This reports which repos have unclassified files. **If any repo has < 100% file coverage, Phase 1 takes priority** — you must classify all files before the eval scores are meaningful.

The linter checks that every file in each diff appears in either `infrastructure` or `non_infrastructure` in the repo's golden expectations. Unclassified files are blind spots where flowdiff could silently misplace them.

### Step 4: Decide what to do

Based on the experiment history and lint results, pick the highest-priority phase:

**Phase 0: Expand corpus** (highest priority if coverage gaps exist)
- Check language counts in the manifest — each language should have 3-5 repos
- Check if any synthetic repos exist — if not, create one
- For real repos: clone from GitHub, find interesting diff range, pin with full SHAs, add `type = "real"`
- For synthetic repos: create under `flowdiff-eval-corpus/synthetic/<name>/`, add `type = "synthetic"` with tight goldens
- Clone destination: `~/Desktop/projects/just-understanding-data/flowdiff-eval-corpus/<language>/<repo>`

**Phase 1: Build goldens via sub-agents** (highest priority if lint-goldens reports gaps)
Use Claude Code sub-agents to generate golden constraints from diff content. **Every file in the diff must be classified as infrastructure or non_infrastructure** — this is enforced by `lint-goldens`.

**Sizing strategy:**
- **Small repos (< 100 files):** Classify directly or with one sub-agent
- **Medium repos (100-300 files):** One sub-agent with the file list
- **Large repos (300+ files):** Use **divide-and-conquer** — split files into chunks of ~50-100, send each chunk to a separate sub-agent in parallel, then merge all results. This avoids context overflow.

**Divide-and-conquer for large repos:**
1. Get the file list: `git diff --name-only > /tmp/fd-<name>-names.txt`
2. Split into N chunks: `split -l 80 /tmp/fd-<name>-names.txt /tmp/fd-<name>-chunk-`
3. Launch N sub-agents in parallel, each classifying their chunk
4. Merge all sub-agent results into the TOML file
5. Run `lint-goldens` to verify — fix any gaps recursively

For each repo that has unclassified files:

1. Get the diff file list and the unclassified paths from `lint-goldens` output.

2. Get the diff content:
   ```bash
   git -C <repo_path> diff <base>..<head> --stat > /tmp/fd-<name>-stat.txt
   git -C <repo_path> diff <base>..<head> --name-only > /tmp/fd-<name>-names.txt
   ```

3. Spawn a sub-agent (Agent tool) with this prompt:
   ```
   You are analyzing a git diff to determine ideal semantic groupings for code review.

   Read the diff output at /tmp/fd-<name>-diff.txt.
   Also read the file list at /tmp/fd-<name>-stat.txt.

   You MUST classify EVERY file in the diff as either infrastructure or non_infrastructure.
   Then additionally identify strong grouping relationships.

   For EVERY changed file, determine:
   - Is this file infrastructure (config, deps, CI, generated files, lockfiles, docs)
     or non_infrastructure (feature code, business logic, tests, API handlers)?
   - What feature/module/API does this change belong to?
   - Which other changed files are part of the same logical change?

   Output golden constraints in this exact format:

   # REQUIRED: classify every single file
   infrastructure = [
     "path/to/config_file",        # CI/config/generated
     "path/to/lockfile",           # dependency lock
   ]
   non_infrastructure = [
     "path/to/feature_file",       # core feature logic
     "path/to/test_file",          # test for feature
   ]

   # REQUIRED: reasonable group bounds
   group_count_min = N
   group_count_max = N

   # RECOMMENDED: strong grouping relationships
   same_group = [
     ["path/to/impl.go", "path/to/impl_test.go"],   # test+impl pair
   ]
   separate_group = [
     ["path/to/feature_a", "path/to/feature_b"],    # unrelated features
   ]

   Rules:
   - EVERY file must appear in exactly one of infrastructure or non_infrastructure
   - Be conservative with same_group/separate_group — only confident pairs
   - Focus on: test+impl pairs, API+handler, schema+migration
   - Use relative paths from repo root
   ```

4. Review the sub-agent's output, add constraints to `eval/repos/<name>.toml`

5. Run the linter to verify full coverage:
   ```bash
   cargo run -p flowdiff-cli -- lint-goldens --manifest eval/repositories.research.toml 2>&1
   ```
   **If any files are still unclassified, fix them before moving on.** The agent must recursively add missing classifications until lint-goldens passes for this repo.

6. Run eval to see how flowdiff scores:
   ```bash
   cargo run -p flowdiff-cli -- eval --manifest eval/repositories.research.toml --format text 2>&1
   ```

**Phase 2: Improve grouping quality** (after goldens have full coverage)
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

### Step 5: Run the experiment
- Make ONE change
- `git commit` the change
- Run eval: `cargo run -p flowdiff-cli -- eval --manifest eval/repositories.research.toml --format json 2>/dev/null > /tmp/fd-eval-result.json`
- Read the result file for key metrics

### Step 6: Record the result
Append one JSON line to `experiments/experiments.jsonl`. See `experiments/program.md` for the full schema per experiment type.
```json
{"id": N, "timestamp": "...", "hypothesis": "...", "type": "golden-generation|deterministic|llm|corpus-expansion|synthetic", ...}
```
Types: `golden-generation` (sub-agent generated goldens), `deterministic` (cluster/rank tuning), `llm` (refinement model+prompt), `corpus-expansion` (new repos), `synthetic` (fixture data), `crash` (failed experiments).

### Step 7: Keep or revert
- If improved: keep — advance the branch
- If equal or worse: `git reset --hard HEAD~1`
- If crashed: fix if trivial, otherwise revert and move on

**Never stop. Never ask permission. If stuck, think harder.**

## Key Commands
```bash
# Check golden file coverage (must pass before Phase 2 work)
cargo run -p flowdiff-cli -- lint-goldens --manifest eval/repositories.research.toml

# Run eval
cargo run -p flowdiff-cli -- eval --manifest eval/repositories.research.toml --format text

# Run eval (JSON for metrics extraction)
cargo run -p flowdiff-cli -- eval --manifest eval/repositories.research.toml --format json 2>/dev/null > /tmp/fd-eval-result.json
```

## Arguments

- `$ARGUMENTS` - Optional: specific phase to work on ("corpus", "golden", "deterministic", "llm", "synthetic") or a specific hypothesis
