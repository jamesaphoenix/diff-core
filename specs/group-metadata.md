# Group Review Metadata — Specification

Origin: [issue #13](https://github.com/jamesaphoenix/diff-core/issues/13) and the design discussion in its comments.

## Problem

A user who runs Analyze → Refine ends up with better groupings and no prose. To get any narrative they must take a second, separate, deliberate action — "Summarize PR" in the desktop app, or `--annotate` on the CLI — and nothing in the refine flow suggests that action exists.

Worse, the prose they eventually get answers the wrong question. `Pass1GroupAnnotation.summary` describes *what changed*. A reviewer opening a 60-file agent-authored PR needs to know *how to review this group*: how risky it is, what class of change it is, what property to verify while reading.

Three concrete defects:

1. **Discoverability** — group narrative exists only behind a manual second call that the primary flow never mentions.
2. **Wrong question** — narrative summaries restate mechanically observable facts instead of directing review attention.
3. **Separate keyed object** — annotations live in `Annotations.overview.groups`, keyed by group id, disjoint from the `FlowGroup` they describe. Consumers must join two structures to render one card.

## Goals

- Every group carries review metadata **on the group itself**, not in a side-car keyed object.
- Metadata is available on the free deterministic path, not only after a paid pass.
- The important bits render in a **fixed layout that can be glanced at**; detail sits below.
- Ranking and review order stay deterministic and reproducible.
- The golden eval corpus does not become model-version-sensitive.

---

## 1. Data Model

### 1.1 New enums (`diffcore-core/src/types.rs`)

```rust
pub enum GroupType { Feat, Fix, Perf, Refactor, Test, Docs, Build, Ci, Chore }

pub enum Risk { Low, Medium, High, Critical }

pub enum ImpactScope { Local, Module, CrossCutting, System }

pub enum ReviewComplexity { Trivial, Simple, Moderate, Complex }

pub enum ReviewFocus {
    Correctness, Security, Concurrency, Performance,
    DataIntegrity, Compatibility, ErrorHandling, ApiContract,
}
```

All derive `Debug, Clone, Serialize, Deserialize, PartialEq, JsonSchema`.

### 1.2 `FlowGroup` extension

```rust
pub struct FlowGroup {
    // existing fields unchanged
    pub id: String,
    pub name: String,
    pub entrypoint: Option<Entrypoint>,
    pub files: Vec<FileChange>,
    pub edges: Vec<FlowEdge>,
    pub risk_score: f64,
    pub review_order: u32,

    // new
    #[serde(default)]
    pub group_type: Option<GroupType>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub risk: Option<Risk>,
    #[serde(default)]
    pub impact: Option<ImpactScope>,
    #[serde(default)]
    pub complexity: Option<ReviewComplexity>,
    #[serde(default)]
    pub review_focus: Vec<ReviewFocus>,
    #[serde(default)]
    pub summary: Vec<String>,
    #[serde(default)]
    pub invariant: Option<String>,
}
```

All seven fields ship in the first cut. `#[serde(default)]` throughout means previously written analysis JSON still deserializes.

### 1.3 `risk_score` vs `Risk`

`risk_score: f64` remains deterministic and remains the **only** input to review ranking. `Risk` is a label derived from it. The metadata pass may override the label; it may never write the score.

Rationale: letting a model move the score makes review order non-reproducible and shifts the entire golden-eval baseline whenever a model version changes.

### 1.4 `summary` is the detail tier

`description` is the one-line caption that sits with the chips; `summary` is
what the reader drops to when the caption isn't enough. It answers "what does
this group achieve", **sized to the change**: a single entry when one sentence
covers it, more only when the group genuinely does several things, capped at
`MAX_SUMMARY_BULLETS` (5).

It is a `Vec<String>`, not a markdown string, which is what keeps §4.2's
no-markdown rule intact while still producing bullets. The list *is* the
structure — the UI renders one entry as prose and several as an unordered list,
with no parser and no renderer anywhere in the path. Models emit leading `-`
and `*` markers regardless of the schema, so `apply_metadata` strips them.

### 1.5 `invariant` is the point

`description` answers "what changed". `invariant` answers "what property should I verify while reading this". The second is what makes the panel worth looking at:

```text
FIX · HIGH RISK · CROSS-CUTTING · COMPLEX
Focus: concurrency, data-integrity

Move job claiming behind a Redis lock.

Invariant: Two workers must never successfully claim the same job.
```

Metadata must not restate LOC, file counts, or anything else already visible in the group card.

---

## 2. Pipeline Placement

### 2.1 A separate pass, not a refinement field

Refinement is patch-op shaped and its `RefinementResponse` contract is **unchanged** by this spec. It cannot carry per-group metadata, because it does not know the ids of the groups it produces: `apply_split` mints `group_refined_{n}` at apply time (`llm/refinement.rs:860`) and `apply_merge` reuses the first source id (`llm/refinement.rs:303`). A model answering the refinement prompt cannot key metadata to groups that do not yet exist.

The pipeline is therefore:

```text
deterministic clustering
        ↓
rank → risk_score → heuristic floor
        ↓
[optional] refinement ops → apply → re-score → heuristic floor again
        ↓
final groups  ──────→  metadata pass  ──→  FlowGroup fields
```

The floor runs twice on the refinement path, and that is load-bearing.
`apply_refinement` mints split products with `..Default::default()` and merged
groups with `risk_score: 0.0`, so after refinement the metadata is empty for
every group it touched and the score it derives from is wrong. `rank::rescore_groups`
recomputes `risk_score` (leaving `review_order` alone — refinement may have
re-ranked deliberately) and the floor is then re-applied on top. This also fixes
a pre-existing bug: before this spec, nothing re-scored after refinement, so
merged groups sorted as the least risky change in the diff.

The metadata pass consumes **final** groups. It runs whether or not refinement ran, which is what allows descriptions on a plain deterministic grouping.

The original issue framed the fix as "fold this into refinement instead of a separate call". The actual complaint was that the second step was *manual and undiscoverable*, not that it was a second call. An automatic second pass resolves it without welding two unrelated contracts together.

### 2.2 The pass re-runs after refinement

Refinement rebuilds groups, so their metadata goes with them: split products are
constructed with `..Default::default()` and merges start empty. The desktop
therefore fires the metadata pass twice — once after analyze, once after any
refinement that changed the grouping — rather than exposing a third button.

This needs `AppState::last_analysis` to be an `Arc<Mutex<..>>`. The desktop
refines through `start_refine_groups`, which spawns a `'static` background task
and cannot borrow `State<'_, AppState>`; before this change that task never wrote
its result back, so every command reading `last_analysis` — `describe_groups`,
and `annotate_overview` before it — kept answering about pre-refinement groups.

### 2.3 Metadata never re-derives from merge rules

When refinement merges a `Fix` group and a `Perf` group, the result is not computed by precedence rules over the inputs. The metadata pass sees the assembled final group and re-assesses every field from scratch.

---

## 3. Provenance

### 3.1 Heuristic floor

The metadata pass is optional and costs money. Fields that can be honestly inferred without a model are populated deterministically first:

| Field | Deterministic floor |
|-------|--------------------|
| `risk` | Bucketed from `risk_score` |
| `group_type` | Path conventions (`tests/**` → Test, `*.md` → Docs, `.github/**` → Ci, …) |
| `impact` | Spread of the group's files across module roots and directories |
| `description` | None — empty |
| `invariant` | None — empty |
| `review_focus` | None — empty |
| `summary` | None — empty |
| `complexity` | None — empty |

**`impact` is not derived from graph fan-out**, despite that being the obvious
reading. `collect_internal_edges` (`cluster/bfs.rs:78`) keeps only edges whose
endpoints are *both* inside the group, so `FlowGroup::edges` cannot cross a
group boundary by construction, and the `SymbolGraph` is gone by the time
groups are finalized. A cross-group proxy — promote to `CrossCutting` when
another group also touched the same directory — was prototyped and rejected: on
the `simple_express_app` fixture it labelled two sibling one-file route groups
as cross-cutting, the opposite of the truth. `impact` therefore counts distinct
module roots and directories within the group itself, and
`sibling_groups_in_one_directory_stay_local` pins the rejected behaviour.

`review_focus` has a tempting mapping from `RiskIndicators` (`has_auth_change` → Security). It is deliberately not used: the mapping is lossy enough to point reviewers at the wrong concern, and an empty chip row is better than a wrong one. `invariant` is the field where a heuristic guess is most harmful — a wrong invariant sends a reviewer hunting for a property that was never at stake.

### 3.2 LLM override

When the metadata pass runs, its output wins on every field, including the three with floors. The floor exists to make the free path useful, not to constrain the model.

Exception, permanently: `risk_score`. See §1.3.

If the eval later shows the deterministic `impact` beating the model's — plausible, since the heuristic reads real graph edges while the model reads a path list — pin `impact` then. Do not pin it pre-emptively.

---

## 4. Prompt and Response

### 4.1 Request

Per batch:

- **Full detail** for the batch's own groups: names, file paths, roles, diff content.
- **Read-only index of every final group** in the analysis: id, name, file paths, `risk_score`. No diff content.

The index is what makes cross-group judgment (`ImpactScope::CrossCutting`) possible from inside a batch. It is immutable input derived from the already-applied grouping, so batch ordering cannot affect any result.

Batches must never read other batches' *results*. That would make output depend on completion order, which under concurrent dispatch is nondeterministic, which breaks the structural assertions in §7.

### 4.2 Response constraints

Enforced in the prompt and in the JSON schema description, then re-enforced on the consuming side:

- `review_focus`: at most 3 entries.
- `invariant`: one sentence.
- `description`: one line.
- `summary`: at most 5 entries, each a bare sentence with no leading bullet marker.
- `description` and `invariant` are **plain text**, not markdown. Both are short enough to have no structure to mark up, and both are consumed by a fixed-layout glance panel and by downstream agents that do not benefit from stripping markup.

---

## 5. Batching and Concurrency

- Batch size from `MetadataConfig.batch_size`, default 20 groups per call.
- Batches dispatched concurrently.
- Results keyed by group id and re-sorted by `review_order` before serialization, so concurrency never reaches the output JSON.

Sizing by token estimate rather than group count is the correct eventual answer. Group count is an adequate proxy until the large-diff track demonstrates otherwise.

Above `LARGE_DIFF_PARTITION_THRESHOLD` (2000 files, `cluster/mod.rs:58`) the pass still runs, batched. The heuristic floor covers whatever it cannot reach.

---

## 6. Surfaces

### 6.1 Desktop

- **Left group list** — `group_type` and `risk` as chips in the group header. No description line; a description in a scan-list defeats scanning.
- **Right panel** — the fixed-layout block (chip row, `description`, `Invariant:`), then `summary` in its own section directly beneath, then existing detail.
  `summary` sits *outside* the fixed-layout block on purpose: its height varies
  from one to five entries, and reserving the worst case would defeat the glance
  row above it.
- **PR-level overview** — not rendered in the group panel; it competed with the
  group's own prose, which is the problem this spec exists to fix. It lives
  behind a `PR Overview` toggle in the annotations panel, and completing
  "Summarize PR" reveals it so the click has visible output. Selecting any group
  returns to the group view.

  Gating it on "no group is selected" does **not** work: analysis auto-selects
  the first group and nothing but dismissing an empty group ever clears the
  selection, so the overview would be unreachable and Summarize PR would spend
  money to display nothing.
- Absent fields are **omitted entirely**. No `—` placeholders, no "run refine to fill this in" nudges.
- `review_focus` truncates rather than wraps.

### 6.2 Firing policy

- **Desktop/web**: fires automatically after analyze via the `describe_groups` command, gated on the `metadata_enabled` setting. The backend re-checks `llm.metadata.enabled` itself, so a stale UI flag cannot force a paid call.
- **CLI**: explicit `--describe` flag. `diffcore analyze` in someone's CI must not start billing them silently.

  Pass 1 fires from the same analyze path, gated on the existing
  `annotations_enabled` setting, so the desktop is down to two LLM buttons:
  **Refine** (restructures groups — a real decision) and **Analyze This Flow**
  (Pass 2, billed per group — must stay manual). Both auto-fired passes call the
  plain commands rather than the streaming jobs: a streaming job sets
  `activityJob`, and the effect at `App.tsx:579` switches the right panel to the
  Activity tab on that, which would throw the user out of the groups they just
  analyzed. The consequence is that **Pass 1 no longer appears in the Activity
  tab** — Refine and Analyze This Flow still do.

### 6.3 CLI

- `--describe` runs the metadata pass.
- `--annotate` retains Pass 1 for the **PR-level overview only**.

---

## 7. Eval

Metadata is emitted and snapshotted but **non-gating**.

Asserted on every group, every run:
- Schema validity — enum variants parse, no unknown values.
- Cap compliance — `review_focus` ≤ 3, `invariant` single sentence, `description` single line.
- Presence — every group has whatever its provenance tier promises.

Not asserted: `description` and `invariant` content quality. Scoring prose against a golden string produces a corpus that goes red when a provider ships a new checkpoint, which trains everyone to ignore corpus failures.

---

## 8. Configuration

New `MetadataConfig` under `llm`, sibling to `RefinementConfig`:

```toml
[llm.metadata]
enabled = true       # default
provider = "anthropic"
model = "claude-haiku-4-5-20251001"
key_cmd = "op read op://vault/item/field"
batch_size = 20      # default
```

It gets its own provider/model rather than inheriting refinement's because the two passes want different models: refinement is a reasoning task, writing a one-line invariant is not.

---

## 9. Removals

### 9.1 `Pass1GroupAnnotation`

Deleted entirely. Its `summary` is superseded by `description`, its `risk_flags` by `risk` + `review_focus`, its `review_order_rationale` by `description`, and its per-group `name` duplicates the name the group already has.

`Pass1Response` retains only its PR-level overview fields.

This is the "separate keyed object" the issue discussion argued against: metadata belongs on the group, not in a structure a consumer has to join against it.

### 9.2 `RefinementConfig.max_iterations`

Dead. It is parsed, validated (`config.rs:353`), and merged across global/local config (`config.rs:487`), and read by no refinement code. It describes an evaluator-optimizer loop that does not exist: `judge.rs` exposes a standalone `run_judge_evaluation`, and the CLI refine path applies the refinement or logs a warning and keeps the deterministic groups (`main.rs:595`).

Delete the field and correct the `RefinementConfig` doc comment at `config.rs:170`, which describes the same nonexistent loop.

### 9.3 The `max_iterations` UI control

`LlmSettings.refinement_max_iterations` (Tauri IPC) and its "Max iterations"
number input in the desktop settings panel are removed along with the config
field. The control let a user pick a value between 1 and 10 that nothing ever
read.

### 9.4 Pass 1 consumers

Deleting `Pass1GroupAnnotation` breaks two surfaces that must be updated in the
same change, because `annotations.overview.groups` no longer exists in the
output JSON:

- **Desktop UI** — the "LLM Summary" panel block, and `copyPrDescription`, which
  built its `# Review Flow` section from per-group Pass 1 summaries. That now
  reads `FlowGroup::description`.
- **VS Code extension** — `renderPass1` in `webviewPanel.ts`, replaced by a
  `renderReviewMetadata` that reads the group's own fields.

### 9.5 CLAUDE.md

The overview claims refinement uses an "evaluator-optimizer loop [that] scores v1 vs v2, keeps whichever is better." It does not. Correct the text.

---

## 10. Compatibility

- `FlowGroup`'s new fields are all `#[serde(default)]`, so previously written analysis JSON deserializes unchanged.
- `RefinementResponse` is untouched, so the 8 cassettes under `crates/diffcore-core/tests/fixtures/vcr_adversarial_refinement/` stay valid. The metadata pass records its own.
- Deleting `Pass1GroupAnnotation` is a breaking change to the annotations JSON shape. Any Pass 1 cassettes need re-recording.

---

## 11. Phases

| Phase | Scope |
|-------|-------|
| 1 | Enums + `FlowGroup` fields + serde defaults. No behavior change. |
| 2 | Heuristic floor: `risk` bucketing, `group_type` path conventions, `impact` fan-out. Free path is now useful. |
| 3 | `MetadataConfig`, metadata pass, schema, batching, concurrency, result merge. |
| 4 | Desktop chip row + right-panel fixed layout; settings opt-out. |
| 5 | CLI `--describe`; `--annotate` narrowed to PR-level overview. |
| 6 | Removals (§9) and eval assertions (§7). |

## 12. Acceptance

1. `diffcore analyze` with no provider configured emits `risk`, `group_type`, and `impact` on every group.
2. The same run emits no `description`, `summary`, `invariant`, `review_focus`, or `complexity`.
2b. A group with a one-entry `summary` renders as prose; several entries render as bullets.
3. `diffcore analyze --describe` populates all eight on every group.
4. `--describe` without `--refine` produces metadata on deterministic groups.
5. Two runs of `--describe` over the same diff produce byte-identical field ordering.
6. `review_order` and `risk_score` are identical with and without `--describe`.
7. A ≥100-group analysis batches, and no group is missing metadata.
8. Analysis JSON written before this change still deserializes.
9. A group with no `invariant` renders no `Invariant:` label.
