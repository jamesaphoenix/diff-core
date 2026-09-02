# Specs Index

| Spec | Description | Status |
|------|-------------|--------|
| [diff-analyzer](./diff-analyzer.md) | diffcore — semantic diff review tool with ranked data-flow grouping, Tauri app + VS Code extension | Active (Phase 12 complete — 7/18 acceptance tests passed, 11 require GUI/VS Code/human review) |
| [web-server-mode](./web-server-mode.md) | Host the Tauri UI as a browser web app via a headless axum binary (`diffcore-web`) for remote diff review | Active (v1 implemented) |
| [improved-clustering](./improved-clustering.md) | Reduce infrastructure bloat: path-based entrypoints for all languages, bidirectional BFS, infrastructure redefinition + sub-grouping | Complete (Phases 1-6 done — core types, bidirectional BFS, path-based entrypoints, sub-clustering, consumer updates, spec updates) |
| [group-metadata](./group-metadata.md) | Per-group review metadata (type, risk, impact, focus, invariant, description) on `FlowGroup`, populated by a heuristic floor plus an optional batched LLM pass | Planned (spec only) |

## Completed Tasks

| Archive | Description |
|---------|-------------|
| [diff-analyzer-completed](./tasks/diff-analyzer-completed.md) | Granular completed tasks from Phases 1-8 |
