## Eval Suite

See also [docs/grouping-overhaul.md](/Users/jamesaphoenix/Desktop/projects/just-understanding-data/diffcore/docs/grouping-overhaul.md) for the broader handoff covering deterministic grouping changes, refinement plumbing, verification state, and next-step recommendations.

The repo eval harness now supports two layers of scoring:

1. Structural heuristics
2. Repo-specific goldens

Structural heuristics score whether a run stays within a configurable group cap, keeps infrastructure under control, avoids singleton explosions, and accounts for every changed file exactly once.

Repo-manifest scoring uses analyzed files after ignore filtering for those structural checks. The raw diff count is still recorded separately so ignored paths do not make file-accounting or infra ratios fail spuriously.

Goldens let you encode what "good" looks like for a real diff:

- `group_count_min` / `group_count_max`
- `same_group`
- `separate_group`
- `infrastructure`
- `non_infrastructure`

This is exposed through `diffcore eval --manifest <path>`.

## Manifests

- [eval/repositories.example.toml](/Users/jamesaphoenix/Desktop/projects/just-understanding-data/diffcore/eval/repositories.example.toml)
  Focused local example with a pinned historical `octospark-services` diff and an inline golden.
- [eval/repositories.public-oss.example.toml](/Users/jamesaphoenix/Desktop/projects/just-understanding-data/diffcore/eval/repositories.public-oss.example.toml)
  Seed corpus with 20 targets and balanced language coverage: five TypeScript repos, five Python repos, five Go repos, and five Rust repos.

## Scoring

When no goldens are present, the overall score is driven entirely by the structural metrics.

When goldens are present, the score shifts weight toward expectation satisfaction so regressions like "cassette files split into separate singleton groups" show up directly in the eval output instead of hiding inside an acceptable group count.

The text and HTML reports now include a dedicated golden column and list any failed golden constraints.

## Suggested Workflow

1. Clone the public corpus to a stable local root.
2. Replace moving branches with pinned commit ranges for each target.
3. Add a small number of high-value golden checks per repo.
4. Run `diffcore eval --manifest ...` after deterministic or LLM-grouping changes.
5. Expand the goldens as failures reveal new fragmentation patterns.
