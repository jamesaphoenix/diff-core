## Grouping Overhaul

This document is the handoff for the grouping/eval work completed on 2026-03-26.

It exists so another agent can continue the work without reconstructing context from terminal logs.

## Goal

Reduce pathological group explosion, enforce a configurable maximum group count, support iterative LLM refinement on top of deterministic grouping, and make regressions measurable across real repositories.

## What Shipped

### 1. Deterministic grouping cap and consolidation

Deterministic clustering now supports a hard cap via `ClusterOptions.max_groups`, defaulting to `200`.

After the initial entrypoint reachability pass, clustering now performs consolidation passes that:

- merge related small groups by topic
- prefer file-path-derived topics over noisy symbol names
- split camelCase and PascalCase topic tokens
- avoid broad stop-word collisions
- fall back to broader directory merges only when still above the cap

This is the main change that prevents pathological runs from producing thousands of semantic groups.

### 2. Config and CLI controls

The group cap is now configurable through:

- `.diffcore.toml` via `[clustering].max_groups`
- `diffcore analyze --max-groups <n>`

Iterative refinement is now configurable through:

- `.diffcore.toml` via `[llm.refinement].max_iterations`
- `diffcore analyze --refine-iterations <n>`

### 3. Iterative refinement plumbing

LLM refinement now runs as a loop rather than a single hard-coded pass. The deterministic groups remain the base state, and refinement patches that state iteratively.

This is important for the next phase: explicit regroup/patch workflows with many workers/subagents.

### 4. Repo eval harness

A real-repo eval harness now exists behind:

```bash
diffcore eval --manifest <manifest.toml>
```

The harness supports:

- structural metrics
- repo-specific golden constraints
- balanced language validation at the corpus level
- text, JSON, and HTML reporting

Structural metrics currently include:

- analyzed file count
- raw diff file count
- ignored file count
- duplicate diff-entry count
- group count
- infra ratio
- singleton ratio
- max/average group size
- file-accounting correctness

Goldens currently support:

- `group_count_min`
- `group_count_max`
- `same_group`
- `separate_group`
- `infrastructure`
- `non_infrastructure`

### 5. Cache semantics fix

`diffcore analyze --no-cache` now bypasses both:

- the disk-persistent IR cache
- the analysis-output cache

Before this fix, `--no-cache` still allowed stale cached analysis JSON to mask grouping changes, which was unacceptable for tuning work.

### 6. Repo-eval accounting fix

Repo eval scoring now uses unique analyzed file paths after ignore filtering, rather than raw diff-entry count.

This matters because the grouping pipeline deduplicates repeated file-path entries before assignment. Without that fix, a repo could satisfy every real invariant and still fail file-accounting.

## Verified State

### Octospark historical case

Pinned diff:

```text
2ef8528c4a5d60c045251ce5a270ec4993607327..884272b11751fea85e9703cf276114837e592147
```

Historical failure mode:

- ~3.4k changed files
- old cached run had `262` groups
- cassette/VCR-related files were fragmented into separate singleton-like groups

Current deterministic result:

- `3485` raw diff entries
- `3484` unique analyzed paths
- `145` groups
- `3196` infrastructure files
- cassette tests merged into `cassette related flows`

### Commands that passed

```bash
cargo test -p diffcore-core --lib
cargo test -p diffcore-cli
cargo run -p diffcore-cli -- eval --manifest eval/repositories.example.toml --format text
```

Expected eval result for the local octospark manifest:

```text
octospark-services-large-history | 3484 | 145 | 91.7% | 24.1% | 1.00 | 1.00 | PASS
```

## Files To Know

Core implementation:

- `crates/diffcore-core/src/cluster.rs`
- `crates/diffcore-core/src/config.rs`
- `crates/diffcore-core/src/eval/repos.rs`
- `crates/diffcore-cli/src/main.rs`
- `crates/diffcore-tauri/src/commands.rs`

Eval manifests and docs:

- `eval/repositories.example.toml`
- `eval/repositories.public-oss.example.toml`
- `docs/eval-suite.md`

Persisted working plan:

- `~/.codex/plans/2026-03-26-grouping-overhaul.md`

## Current Corpus State

There is now a seeded public corpus manifest with `20` targets and balanced language coverage:

- `5` TypeScript
- `5` Python
- `5` Go
- `5` Rust

Important caveat:

- only `octospark-services` is currently fully pinned and goldened
- most public entries are still seed targets with placeholder local paths
- the next agent should treat the public corpus manifest as a scaffold, not a finished benchmark suite

## Recommended Architecture For Next Phase

Use deterministic grouping as the inner engine and keep it local.

Then add an explicit regroup/patch surface on top of that for LLM workers.

If you want many workers, handoffs, traces, and orchestration, use Agents SDK outside the engine rather than making Agents SDK the grouping engine itself.

Practical recommendation:

- native `diffcore` deterministic pass first
- optional local/native refinement loop second
- Agents SDK as outer orchestration for large multi-agent refinement runs
- Codex/Codex-MCP workers as implementation agents inside that orchestration

Reasoning:

- deterministic grouping stays reproducible and cheap
- LLM workers can patch concrete grouping state instead of replacing it
- evals remain meaningful because the base algorithm is stable
- traces and handoffs become optional orchestration, not a requirement for basic analysis

## Highest-Value Next Steps

### 1. Build a real golden corpus beyond octospark

For each seeded public repo:

- clone locally
- replace moving branch assumptions with pinned commit ranges
- choose 1-3 historically interesting diffs
- add a few high-value goldens per diff

Good first golden patterns:

- files that must stay together
- files that must stay separate
- files that must not fall into infrastructure
- upper and lower group-count bounds

### 2. Add an explicit regroup/patch surface

The next agent should add a first-class refinement patch format rather than relying only on internal refinement logic.

Likely operations:

- merge groups
- split a group
- rename a group
- move file to infrastructure
- move file out of infrastructure
- rerank groups

This will make large multi-agent refinement workflows much cleaner.

### 3. Surface `max_groups` in the Tauri UI

The backend supports the cap already, but the user-facing settings UI does not yet expose it as a first-class control.

If this matters in product usage, add it to the settings surface rather than keeping it config/CLI-only.

### 4. Improve large artifact/fixure directory handling

The deterministic pass is much better now, but there is still room to collapse massive cassette/fixture trees more intelligently without over-grouping unrelated test assets.

Focus on:

- cassette trees
- fixture directories
- generated coverage artifacts
- large migration runs

### 5. Package the workflow like `domain-scan`

The user explicitly wants something closer to the `domain-scan` pattern:

- prompt installs/upgrades the binary
- bundles skills
- enables high-juice refinement flows quickly

A follow-up agent should turn the refinement/eval workflow into an installable or upgradable bundle rather than leaving it as repo-local knowledge.

## Suggested Pickup Sequence For The Next Agent

1. Read this file.
2. Read `docs/eval-suite.md`.
3. Read `eval/repositories.example.toml`.
4. Read `eval/repositories.public-oss.example.toml`.
5. Read `crates/diffcore-core/src/eval/repos.rs`.
6. Read `crates/diffcore-core/src/cluster.rs`.
7. Run the octospark eval manifest to confirm local state.
8. Pick one of the five next-step tracks above and continue from there.

## Quick Commands

Fresh deterministic octospark run:

```bash
cargo run -p diffcore-cli -- analyze \
  --repo /Users/jamesaphoenix/Desktop/projects/just-understanding-data/octospark-services \
  --range 2ef8528c4a5d60c045251ce5a270ec4993607327..884272b11751fea85e9703cf276114837e592147 \
  --max-groups 200 \
  --no-cache \
  -o /tmp/diffcore-octospark-history-analysis.json
```

Local repo eval:

```bash
cargo run -p diffcore-cli -- eval \
  --manifest eval/repositories.example.toml \
  --format text
```

## Open Risks

- The public corpus is seeded but not yet pinned end-to-end.
- The app still lacks a first-class UI control for the grouping cap.
- The refinement loop is iterative, but the explicit patch protocol for many-worker regrouping still needs to be designed.
- The deterministic pass still deserves more tuning for huge artifact-heavy diffs.
