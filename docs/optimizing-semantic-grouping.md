# Optimising Diffcore's AST/IR Pipeline and LLM Grouping Pass

[Diffcore](https://understandingdata.com/tools/diffcore/) exists because raw git diff is the wrong abstraction for reviewing large AI-generated pull requests. Git shows me which files changed. What I actually need is a higher-level answer: which changes belong to the same behavior, where execution starts, how the change fans out, and what should be reviewed first.

The optimisation work in Diffcore is centered on that gap. I am trying to group semantically similar changes without collapsing into fuzzy "these files kind of look related" clustering. The approach is deliberately hybrid:

1. A deterministic structural pass builds the strongest possible program-level model from the diff.
2. An LLM pass refines only the ambiguous cases that the structural pass cannot settle cleanly.
3. An eval loop measures whether each change actually improves grouping quality on real repositories.

## The Core Thesis

I do not want the LLM to invent review groups from scratch.

If grouping starts as a pure prompt over raw diff text, the result is expensive, hard to debug, and hard to reproduce. It also tends to over-index on naming similarity and miss the actual execution path through the codebase.

So the main optimisation strategy in Diffcore is:

- push as much semantic signal as possible into the AST/IR and graph layers
- make the deterministic grouping engine do the heavy lifting
- use the LLM as a constrained patch layer on top of that baseline

That keeps the system fast and reproducible while still giving me a way to recover from edge cases like scattered refactors, weak entrypoint signals, and misleading infrastructure buckets.

## AST to IR to Graph

The structural side starts with tree-sitter parsing and declarative query extraction. The parser layer reads changed files, identifies language-specific syntax, and extracts definitions, imports, exports, calls, and assignment patterns. The important design choice is that this does not stop at per-language AST traversal. The parser normalises everything into a shared intermediate representation so the rest of the engine can reason about TypeScript, Python, Go, Rust, and other languages through the same conceptual model.

That shared IR is the real optimisation surface.

Instead of saying "this is a TypeScript route file" or "this is a Python controller" in ten different hand-written ways downstream, I want one language-agnostic layer that captures:

- functions and exported symbols
- types and structural definitions
- imports and re-exports
- call expressions and data-flow-adjacent assignments
- binding patterns such as destructuring and tuple unpacking

Once the code is normalised into IR, Diffcore builds a symbol graph and enriches it with flow information. That graph is what lets grouping move beyond path proximity. Files are no longer related just because they sit in neighboring directories; they are related because they share entrypoints, imports, calls, and execution-adjacent edges.

This matters for semantically similar content because many large diffs are not physically local. A single behavior might span a route file, a service, a repository, a schema, and a migration. Those files can live far apart in the tree while still belonging to the same review packet. The AST/IR layer is what gives the grouping algorithm enough structure to recover that shape.

## Deterministic Grouping as the Main Engine

The current deterministic grouping pass works by detecting likely entrypoints, tracing reachability through the symbol graph, and assigning changed files to the nearest meaningful flow. That gives Diffcore its basic unit: the flow group.

The optimisation work here is about reducing the two main failure modes:

1. group explosion, where one logical change shatters into too many tiny groups
2. infrastructure collapse, where too many files end up in a catch-all bucket

The fixes are structural, not cosmetic:

- stronger entrypoint detection so real route, command, test, and worker files are recognised earlier
- better IR and graph coverage so more files are connected to a meaningful execution path
- bidirectional reachability so files that depend on an entrypoint-adjacent flow are not automatically discarded as unrelated
- convention-based infrastructure classification so docs, scripts, schemas, migrations, generated code, and true infra are not all treated as the same leftover category

This is the key idea: the closer I can get the deterministic pass to "mostly right," the more useful the LLM becomes. If the baseline groups already reflect real program structure, the LLM does not need to perform full semantic reconstruction. It only needs to repair the residual mistakes.

## The LLM Pass as a Structured Patch Layer

The LLM pass is intentionally narrow.

Instead of asking the model to re-cluster the entire diff from raw text, Diffcore gives it a structured view of the current grouping state and asks for specific operations:

- split a group
- merge groups
- re-rank review order
- reclassify misplaced files

This is a much better fit for the problem.

The deterministic pass is very good at extracting hard signals: imports, exported handlers, graph reachability, file roles, and basic execution order. The LLM is much better at the softer semantic questions:

- are these two files part of the same refactor even if the graph is weak?
- is this group actually two different reviewer tasks mixed together?
- should the schema be reviewed before the handler even if the graph shape is shallow?
- is this file "infrastructure" or is it actually part of the behavior change?

So the optimisation goal is not "replace the deterministic engine with AI." It is "give the model a strong structural prior, then let it make bounded semantic corrections."

That is how I want to group semantically similar content: structure first, semantic repair second.

## Why This Looks Like Autoresearch

This work maps very naturally onto the idea behind [karpathy/autoresearch](https://github.com/karpathy/autoresearch): treat research itself as an executable loop.

In Karpathy's framing, the agent runs an experiment, measures the result, keeps or discards the change, and repeats. The important shift is that the loop is not hidden in human intuition. It is made explicit in code and in a Markdown "program" that defines the research process.

Diffcore already fits that pattern well.

The repo contains an `experiments/program.md` that turns grouping work into an explicit experiment loop:

- pick one hypothesis
- change one variable
- run the eval harness
- compare against baseline
- record the result
- keep or revert

The difference is that the objective function is not training loss. My target is grouping quality:

- do related files land together?
- do unrelated files stay separate?
- does infrastructure stay under control?
- does the group count stay usable?
- does review order match human intuition?

That means Diffcore's equivalent of `autoresearch` is an evaluation-driven semantic clustering loop. The artefacts are different, but the rhythm is the same: formulate a hypothesis, run the system, score the outcome, and iterate quickly.

## What I Am Actually Optimising

At a practical level, I am tuning four layers together:

### 1. Representation quality

I want the AST and IR to preserve more of the semantics that matter for review grouping. Every missed import shape, entrypoint convention, destructuring pattern, or call relationship weakens the graph and forces more work onto the LLM.

### 2. Grouping quality

I want the deterministic pass to produce groups that are stable, interpretable, and close to how an experienced reviewer would naturally segment the PR.

### 3. Refinement quality

I want the LLM pass to operate on the boundary cases only, using structured patch operations rather than replacing the whole grouping output with opaque prose or ad hoc labels.

### 4. Evaluation quality

I want improvements to be measurable across a real corpus, not just "this looked better on one hand-picked diff." That is why the eval harness and repo-specific goldens matter so much. They turn grouping from taste into something I can optimise systematically.

## Implementation Map

For the concrete implementation behind this note, these are the most relevant files:

- `crates/diffcore-core/src/ast.rs`
- `crates/diffcore-core/src/query_engine.rs`
- `crates/diffcore-core/src/ir.rs`
- `crates/diffcore-core/src/graph.rs`
- `crates/diffcore-core/src/entrypoint.rs`
- `crates/diffcore-core/src/cluster.rs`
- `crates/diffcore-core/src/llm/refinement.rs`
- `crates/diffcore-core/src/eval/repos.rs`
- `experiments/program.md`

## The End State

The end state I am aiming for is a semantic review engine where the deterministic AST/IR pipeline does most of the intellectual work and the LLM acts as an optimiser over a constrained search space.

That is also why the connection to [Diffcore's public product page](https://understandingdata.com/tools/diffcore/) and to [autoresearch](https://github.com/karpathy/autoresearch) matters.

The product page explains the user-facing promise: turn raw diffs into review flows.

The research loop explains how I intend to keep improving that promise: not with vague prompt tinkering, but with a repeatable optimisation process around AST extraction, IR design, graph quality, structured refinement, and benchmarked evaluation.

In short: I am not trying to make Diffcore "more AI." I am trying to make its structural understanding strong enough that AI only has to solve the last mile.
