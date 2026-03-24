# Improved Clustering & Infrastructure Pruning — Specification

## Problem

Too many files land in the infrastructure group. The current algorithm puts **any file not forward-reachable from a detected entrypoint** into a single flat "infrastructure" bucket. In real PRs this produces an infrastructure group containing 30-80% of changed files — schemas, controllers, services, utilities, docs — defeating the purpose of semantic grouping.

Three root causes:

1. **Narrow entrypoint detection** — TS/JS (and several other languages) lack path-based heuristics. Only files with explicit `router.get()` / decorator call sites are detected. Files in `/routes/`, `/handlers/`, `/controllers/` directories are missed.

2. **Forward-only BFS** — `compute_file_reachability` only follows outgoing edges. Files that import/depend on entrypoint-adjacent code (reverse direction) are unreachable.

3. **Infrastructure is a catch-all** — anything not reachable = infrastructure. No distinction between true infrastructure (Docker, CI, env configs) and code that just lacked a graph path.

## Goals

- **Reduce infrastructure group size by 70%+** for typical PRs
- **Redefine infrastructure** — only true infrastructure/config files belong there
- **Add an "Unclassified" category** for code files that lack graph paths but aren't infrastructure
- **Improve entrypoint detection** across all 14 supported languages with path-based heuristics
- **Bidirectional reachability** so downstream AND upstream dependencies are grouped
- **Sub-group remaining ungrouped files** by directory proximity and naming convention

---

## 1. Entrypoint Detection — Path-Based Heuristics (All Languages)

### 1.1 Current State

Entrypoint detection in `entrypoint.rs` varies by language:

| Language | Call-site detection | Path-based detection | Framework-import gating |
|----------|-------------------|---------------------|------------------------|
| TypeScript/JavaScript | `router.get()`, Next.js conventions | **Missing** | No |
| Python | `app.route()`, decorator patterns | `/views/`, `/routes/`, `/endpoints/` | Yes (web framework import required) |
| Go | `http.HandleFunc`, framework patterns | `/handler/`, `/handlers/`, `/routes/`, `/api/` | Yes (net/http or framework import) |
| Rust | `Router::new`, axum/actix patterns | **Partial** (handler modules only) | Yes |
| Java | Annotation patterns | **Missing** | No |
| C# | Attribute patterns | **Missing** | No |
| PHP | Route patterns | **Missing** | No |
| Ruby | Route patterns | **Missing** | No |
| Kotlin | Annotation patterns | **Missing** | No |
| Swift | Route patterns | **Missing** | No |
| C/C++ | **Missing** | **Missing** | No |
| Scala | Route patterns | **Missing** | No |

### 1.2 Design: Universal Path-Based Detection

Add a **two-tier** path-based detection system that works identically across all languages:

**Tier 1: Path + framework import** — if the file path suggests a route/handler AND the file imports from a known web/CLI framework, all exported/public functions become entrypoints.

**Tier 2: Strong path only** — if the file path is a very strong signal (e.g., `/routes/users.ts`, `/handlers/billing.go`, `*.controller.ts`), detect entrypoints even WITHOUT a framework import check. This catches files where the import was from an unchanged transitive dependency.

#### 1.2.1 HTTP Route Path Patterns (all languages)

Directory patterns (file path contains):
- `/routes/`, `/route/`
- `/handlers/`, `/handler/`
- `/controllers/`, `/controller/`
- `/endpoints/`, `/endpoint/`
- `/api/` (only when combined with framework import — too generic alone)
- `/views/` (Python/Ruby/PHP only)

File name patterns:
- `*.routes.*`, `*.route.*`
- `*.handler.*`, `*.handlers.*`
- `*.controller.*`, `*.controllers.*`
- `*.endpoint.*`, `*.endpoints.*`

Strong-signal file name patterns (Tier 2 — no import check needed):
- File contains `entrypoint` or `entrypoints` in name
- File matches `server.*` or `app.*` at the project root level

#### 1.2.2 CLI Command Path Patterns (all languages)

Directory patterns:
- `/commands/`, `/command/`
- `/cmd/` (Go convention)
- `/cli/`

File name patterns:
- `*.command.*`, `*.commands.*`
- `*.cli.*`

#### 1.2.3 Framework Import Tables

**Web frameworks by language:**

| Language | Framework imports |
|----------|-----------------|
| TypeScript/JavaScript | `express`, `fastify`, `@hapi/hapi`, `koa`, `@trpc/server`, `hono`, `@nestjs/common`, `@nestjs/core`, `next`, `nuxt`, `@remix-run/*`, `sveltekit`, `restify`, `polka`, `@effect/platform` |
| Python | `flask`, `fastapi`, `django`, `starlette`, `tornado`, `aiohttp`, `sanic`, `bottle`, `falcon`, `pyramid` |
| Go | `net/http`, `github.com/gin-gonic/gin`, `github.com/labstack/echo*`, `github.com/go-chi/chi*`, `github.com/gofiber/fiber*`, `github.com/gorilla/mux` |
| Rust | `actix_web`, `actix-web`, `axum`, `rocket`, `warp`, `hyper`, `tower` |
| Java | `org.springframework.web`, `javax.ws.rs`, `jakarta.ws.rs`, `io.javalin`, `io.micronaut.http`, `io.quarkus` |
| C# | `Microsoft.AspNetCore`, `System.Web.Http`, `Carter`, `ServiceStack` |
| PHP | `Illuminate\Routing` (Laravel), `Symfony\Component\Routing`, `Slim\App` |
| Ruby | `sinatra`, `rails`, `grape`, `hanami`, `roda` |
| Kotlin | `org.springframework.web`, `io.ktor`, `io.javalin`, `io.micronaut.http` |
| Swift | `Vapor`, `Kitura`, `Hummingbird` |
| Scala | `akka.http`, `http4s`, `play.api`, `zio.http`, `cask` |

**CLI frameworks by language:**

| Language | Framework imports |
|----------|-----------------|
| TypeScript/JavaScript | `commander`, `yargs`, `meow`, `cac`, `oclif`, `@effect/cli`, `inquirer`, `vorpal`, `caporal`, `clipanion` |
| Python | `argparse`, `click`, `typer`, `fire`, `docopt`, `plac` |
| Go | `github.com/spf13/cobra`, `github.com/urfave/cli*`, `flag` |
| Rust | `clap`, `structopt`, `argh`, `gumdrop` |
| Java | `picocli`, `commons-cli`, `jcommander`, `airline` |
| C# | `System.CommandLine`, `CommandLine`, `McMaster.Extensions.CommandLineUtils` |
| PHP | `symfony/console`, `league/climate` |
| Ruby | `thor`, `gli`, `optimist`, `slop` |
| Kotlin | `picocli`, `clikt`, `kotlinx.cli` |
| Swift | `ArgumentParser`, `SwiftCLI`, `Commander` |
| Scala | `scopt`, `decline`, `picocli` |

### 1.3 Implementation

In `entrypoint.rs`:

1. Add language-specific helper functions: `is_{lang}_web_framework_import(import) -> bool` and `is_{lang}_cli_framework_import(import) -> bool`
2. Add a shared helper: `is_route_handler_path(path) -> bool` and `is_strong_route_handler_path(path) -> bool` (language-agnostic path patterns)
3. At the end of each `detect_http_routes_{lang}` function, add path-based detection:
   ```
   if is_route_handler_path(path) && has_framework_import → add exported functions
   if is_strong_route_handler_path(path) → add exported functions (no import check)
   ```
4. Same pattern for `detect_cli_commands` per language

### 1.4 Acceptance Tests

| Test | Input | Expected |
|------|-------|----------|
| `ts_routes_dir_with_express` | File at `src/routes/users.ts` importing `express` | Detected as HTTP entrypoint |
| `ts_routes_dir_no_import` | File at `src/routes/users.ts` with no framework import | Detected (strong path) |
| `ts_controller_suffix` | `src/billing.controller.ts` importing `@nestjs/common` | Detected |
| `ts_entrypoints_in_name` | `src/command-entrypoints.ts` | Detected (strong path) |
| `go_handlers_dir` | `internal/handlers/auth.go` importing `net/http` | Detected |
| `python_views_flask` | `app/views/dashboard.py` importing `flask` | Detected |
| `java_controller_spring` | `com/api/UserController.java` importing `org.springframework.web` | Detected |
| `rust_handlers_axum` | `src/handlers/auth.rs` importing `axum` | Detected |
| `no_false_positive_utils` | `src/utils/helpers.ts` | NOT detected |
| `no_false_positive_api_types` | `src/api/types.ts` (no framework import, not strong path) | NOT detected |
| `cli_commands_dir` | `src/commands/deploy.ts` importing `commander` | Detected as CLI entrypoint |
| `cli_strong_path` | `src/commands/migrate.go` | Detected (strong path) |

---

## 2. Bidirectional Reachability

### 2.1 Current State

`compute_file_reachability` in `cluster.rs` does forward-only BFS (`Direction::Outgoing`). A file that imports FROM an entrypoint's group is unreachable.

### 2.2 Design

Add a second BFS pass using `Direction::Incoming` with a distance penalty, then merge results.

**Algorithm:**

```
fn compute_file_reachability(graph, entry_file, entry_symbol) -> HashMap<String, usize>:
    // Pass 1: Forward BFS (existing behavior)
    forward_distances = bfs(seed_nodes, Direction::Outgoing, cost_per_hop=1)

    // Pass 2: Reverse BFS (new)
    reverse_distances = bfs(seed_nodes, Direction::Incoming, cost_per_hop=2)

    // Merge: keep minimum distance for each file
    merged = merge(forward_distances, reverse_distances, min)
    return merged
```

The `cost_per_hop=2` for reverse edges means forward-reachable files are always preferred for group assignment (shorter distance), while reverse-reachable files serve as a fallback that prevents them from being dumped into infrastructure.

### 2.3 Why cost=2 for reverse?

Forward edges represent the natural data flow: entrypoint calls service, service calls repo. These files clearly belong to the entrypoint's group.

Reverse edges represent "this file depends on something in the group" — weaker signal. A higher cost ensures:
- Files reachable both ways use the forward (shorter) distance
- When multiple entrypoints compete, forward-reachable entrypoint wins
- Reverse-reachable files sort after forward-reachable files in flow position

### 2.4 Acceptance Tests

| Test | Setup | Expected |
|------|-------|----------|
| `reverse_reachable_not_infra` | File A has edge TO entrypoint file | A is in entrypoint's group, not infrastructure |
| `forward_preferred` | File reachable forward (dist 1) and reverse (dist 2) | Uses forward distance (1) |
| `reverse_only_grouped` | File only reachable via reverse edges | Grouped, not infrastructure |
| `mixed_multi_hop` | Complex graph with both directions | Correct distance calculation |
| `existing_tests_unchanged` | All existing cluster tests | Pass without modification |

---

## 3. Infrastructure Redefinition & Unclassified Groups

### 3.1 Current State

```rust
pub struct InfrastructureGroup {
    pub files: Vec<String>,
    pub reason: String,
}
```

One flat bucket. Every unreachable file = infrastructure. No sub-grouping.

### 3.2 Design: Three-Tier Ungrouped Classification

Files not assigned to any flow group go through a classification pipeline:

```
Ungrouped files
    │
    ├─→ [Convention classifier] → "Infrastructure" (true infra only)
    ├─→ [Convention classifier] → Named sub-groups (schemas, scripts, docs, etc.)
    ├─→ [Directory proximity] → Directory-based groups
    ├─→ [Import-edge clustering] → Connected component groups
    └─→ [Fallback] → "Unclassified"
```

#### 3.3 What IS Infrastructure

Infrastructure = files that configure, build, deploy, or operate the system but do NOT contain application logic.

| Category | Patterns |
|----------|----------|
| **Environment/Secrets** | `.env*`, `*.env`, `.env.dev`, `.env.prod`, `.env.local` |
| **Docker** | `Dockerfile*`, `docker-compose*`, `.dockerignore` |
| **CI/CD** | `.github/workflows/*`, `.gitlab-ci.yml`, `Jenkinsfile`, `.circleci/*`, `.travis.yml`, `azure-pipelines.yml`, `bitbucket-pipelines.yml` |
| **Container orchestration** | `k8s/*`, `kubernetes/*`, `helm/*`, `*.helmrelease.*` |
| **Terraform/IaC** | `terraform/*`, `*.tf`, `*.tfvars`, `pulumi/*`, `Pulumi.*`, `cdk/*`, `cloudformation/*` |
| **Package manager configs** | `package.json`, `Cargo.toml`, `go.mod`, `go.sum`, `requirements.txt`, `Pipfile`, `pyproject.toml`, `Gemfile`, `pom.xml`, `build.gradle*`, `*.csproj`, `Package.swift`, `build.sbt`, `composer.json` |
| **Lock files** | `package-lock.json`, `yarn.lock`, `pnpm-lock.yaml`, `Cargo.lock`, `Gemfile.lock`, `poetry.lock`, `composer.lock` |
| **Build tool configs** | `tsconfig*.json`, `webpack.*`, `vite.*`, `rollup.*`, `esbuild.*`, `babel.*`, `Makefile`, `CMakeLists.txt`, `*.mk`, `build.rs` |
| **Linter/formatter configs** | `.eslintrc*`, `.prettierrc*`, `.stylelintrc*`, `.editorconfig`, `.clang-format`, `rustfmt.toml`, `.rubocop.yml`, `.flake8`, `mypy.ini`, `.golangci.yml` |
| **IDE/editor** | `.vscode/*`, `.idea/*`, `.eclipse/*` |
| **MCP/tool configs** | `.mcp.json`, `.mcp/*`, `.tool-versions`, `.nvmrc`, `.node-version`, `.python-version`, `.ruby-version` |
| **Git configs** | `.gitignore`, `.gitattributes`, `.gitmodules` |
| **Misc config** | `*.toml` (at root), `*.yaml`/`*.yml` (at root, not in src dirs), `*.ini`, `*.cfg` |

#### 3.4 What is NOT Infrastructure

These should be **named sub-groups** or **unclassified**, not infrastructure:

| Category | Sub-group name | Patterns |
|----------|---------------|----------|
| **Schemas/Types** | "Schemas" | `/schemas/`, `/schema/`, `*.schema.*`, `*.dto.*`, `/types/` (when containing type definitions) |
| **Database/Migrations** | "Migrations" | `/migrations/`, `/migrate/`, `*.migration.*`, `/seeds/`, `/fixtures/` |
| **Scripts** | "Scripts" | `/scripts/`, `*.sh`, `*.bash`, `*.zsh`, `*.ps1` (that contain application logic or dev tooling) |
| **Documentation** | "Documentation" | `*.md`, `*.mdx`, `*.rst`, `/docs/`, `/documentation/` |
| **Tests** | "Test utilities" | Test fixtures, test helpers, test utilities that weren't detected as test entrypoints |
| **Generated code** | "Generated" | `/generated/`, `/__generated__/`, `*.generated.*`, `*.g.dart`, `*.pb.go` |
| **Deployment scripts** | "Deployment" | `/deploy/`, `/deployment/`, deploy shell scripts |

#### 3.5 Updated Types

```rust
/// Category for infrastructure sub-groups.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum InfraCategory {
    /// True infrastructure: Docker, CI/CD, env configs, build configs, package manager
    Infrastructure,
    /// Schema/type/DTO files
    Schema,
    /// Shell scripts and dev tooling scripts
    Script,
    /// Database migrations and seed files
    Migration,
    /// Deployment scripts and configs
    Deployment,
    /// Documentation files
    Documentation,
    /// Linter/formatter configs
    Lint,
    /// Test utilities, fixtures, helpers
    TestUtil,
    /// Generated code
    Generated,
    /// Files grouped by shared directory prefix
    DirectoryGroup,
    /// Files with no category match
    Unclassified,
}

/// A sub-group within the ungrouped files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InfraSubGroup {
    /// Human-readable name: "Schemas", "scripts/", "Configuration", etc.
    pub name: String,
    /// Classification category
    pub category: InfraCategory,
    /// Files in this sub-group
    pub files: Vec<String>,
}

/// Files not assigned to any flow group, organized into sub-groups.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InfrastructureGroup {
    /// Flat file list for backward compatibility with JSON consumers
    pub files: Vec<String>,
    /// Semantically organized sub-groups
    pub sub_groups: Vec<InfraSubGroup>,
    /// Reason these files weren't assigned to flow groups
    pub reason: String,
}
```

#### 3.6 Sub-Clustering Algorithm

```
fn sub_cluster_infra_files(files: &[String], graph: &SymbolGraph) -> Vec<InfraSubGroup>:
    remaining = set(files)
    sub_groups = []

    // Phase 1: Convention-based classification
    for each file in files:
        category = classify_by_convention(file)
        if category != Unclassified:
            add file to sub_groups[category]
            remove file from remaining

    // Phase 2: Import-edge clustering (for remaining files)
    // Build connected components among remaining files using graph edges
    components = connected_components(remaining, graph)
    for component in components where len > 1:
        name = generate_component_name(component)  // use common directory prefix
        add InfraSubGroup { name, category: DirectoryGroup, files: component }
        remove component files from remaining

    // Phase 3: Directory proximity (for remaining files)
    dir_groups = group_by_directory(remaining)
    for (dir, files) in dir_groups where len >= 2:
        add InfraSubGroup { name: dir, category: DirectoryGroup, files }
        remove files from remaining

    // Phase 4: Remaining → Unclassified
    if remaining is not empty:
        add InfraSubGroup { name: "Unclassified", category: Unclassified, files: remaining }

    return sub_groups
```

#### 3.7 Convention Classifier

```
fn classify_by_convention(path: &str) -> InfraCategory:
    // Infrastructure (strict — true infra only)
    if is_true_infrastructure(path):
        return Infrastructure

    // Schemas/Types
    if path matches /schemas/, /schema/, *.schema.*, *.dto.*, /types/ (with type defs):
        return Schema

    // Migrations
    if path matches /migrations/, /migrate/, *.migration.*, /seeds/, /fixtures/:
        return Migration

    // Scripts
    if extension in [.sh, .bash, .zsh, .ps1] or path matches /scripts/:
        return Script

    // Deployment
    if path matches /deploy/, /deployment/ and not matched by Infrastructure:
        return Deployment

    // Documentation
    if extension in [.md, .mdx, .rst, .txt] or path matches /docs/, /documentation/:
        return Documentation

    // Lint configs
    if path matches /lint/, /eslint/ or filename matches .eslint*, .prettier*, .stylelint*:
        return Lint

    // Test utilities
    if path matches /test-utils/, /test-helpers/, /__fixtures__/, /test/fixtures/:
        return TestUtil

    // Generated
    if path matches /generated/, /__generated__/, *.generated.*, *.g.dart, *.pb.go:
        return Generated

    return Unclassified
```

#### 3.8 Updated JSON Output

```json
{
  "infrastructure_group": {
    "files": ["Dockerfile", ".env.dev", "src/schemas/user.ts", "scripts/deploy.sh"],
    "sub_groups": [
      {
        "name": "Infrastructure",
        "category": "Infrastructure",
        "files": ["Dockerfile", ".env.dev"]
      },
      {
        "name": "Schemas",
        "category": "Schema",
        "files": ["src/schemas/user.ts"]
      },
      {
        "name": "Scripts",
        "category": "Script",
        "files": ["scripts/deploy.sh"]
      }
    ],
    "reason": "Not reachable from any detected entrypoint"
  }
}
```

### 3.9 Acceptance Tests

| Test | Input files | Expected sub-groups |
|------|-------------|-------------------|
| `only_true_infra` | `Dockerfile`, `.env.dev`, `tsconfig.json` | 1 group: Infrastructure |
| `schemas_separated` | `schemas/user.ts`, `schemas/billing.ts`, `Dockerfile` | Infrastructure + Schemas |
| `scripts_grouped` | `scripts/deploy.sh`, `scripts/setup.sh` | Scripts |
| `dir_proximity` | `mcp/langfuse.sh`, `mcp/spotlight.sh` | DirectoryGroup "mcp/" |
| `mixed_all_categories` | Mix of infra, schemas, scripts, docs, unclassified | Multiple sub-groups, each correctly categorized |
| `import_edge_clustering` | Two code files importing each other, both ungrouped | Merged into one DirectoryGroup |
| `single_unclassified` | `src/random-file.ts` with no matches | Unclassified |
| `empty_input` | No files | No sub-groups |
| `docs_grouped` | `docs/README.md`, `docs/setup.md` | Documentation |
| `migrations_grouped` | `migrations/001.sql`, `migrations/002.sql` | Migrations |

---

## 4. Tauri UI — Clickable Infrastructure Files & Sub-Group Rendering

### 4.1 Current State

Infrastructure files in the Tauri app are **not clickable**. Flow group files render with an `onClick` handler that calls `handleSelectFile(path)` to load the diff in the Monaco viewer. Infrastructure files render as plain `<span className="file-path">` with no click handler — the user can see the file names but cannot view their diffs.

**This is wrong.** Infrastructure/ungrouped files are still changed files in the PR. Reviewers need to inspect them too.

### 4.2 Design: Clickable Files

Make every infrastructure file clickable, identical to flow group files:

1. **Click handler** — each `<li>` in the infrastructure file list gets `onClick={() => handleSelectFile(file)}` which loads the diff into the Monaco viewer
2. **Selected state** — apply the `selected` CSS class when `selectedFile === file` so the highlight follows the cursor
3. **Keyboard navigation** — j/k should navigate within infrastructure files when the infrastructure group is focused

### 4.3 Design: Sub-Group Rendering

Replace the flat infrastructure file list with collapsible sub-groups:

```
Infrastructure (32 files)
  ▼ Infrastructure (5 files)     ← true infra: Docker, env, configs
      Dockerfile
      .env.dev
      tsconfig.json
      package.json
      pnpm-lock.yaml
  ▶ Schemas (12 files)           ← collapsed by default
  ▶ Scripts (3 files)
  ▶ Documentation (8 files)
  ▶ Unclassified (4 files)
```

Each sub-group:
- Has a collapsible header showing name + file count
- Files within are clickable (load diff)
- Sub-groups are collapsed by default, expand on click
- The parent "Infrastructure" section header shows total file count across all sub-groups

### 4.4 Implementation

In `App.tsx`, replace the infrastructure rendering block (lines ~1796-1834) with:

```tsx
{analysis?.infrastructure_group && analysis.infrastructure_group.sub_groups.length > 0 && (
  <div className="group-item infra-group">
    <div className="group-header" onClick={() => setInfraExpanded(prev => !prev)}>
      <span className="group-name">Ungrouped</span>
      <span className="risk-badge" data-risk="low">
        {analysis.infrastructure_group.files.length} files
      </span>
      <span>{infraExpanded ? "▲" : "▼"}</span>
    </div>
    {infraExpanded && analysis.infrastructure_group.sub_groups.map(sg => (
      <InfraSubGroupView
        key={sg.name}
        subGroup={sg}
        selectedFile={selectedFile}
        onSelectFile={handleSelectFile}
      />
    ))}
  </div>
)}
```

Each `InfraSubGroupView` renders a collapsible section with clickable files.

### 4.5 Acceptance Tests

| Test | Action | Expected |
|------|--------|----------|
| `infra_file_clickable` | Click an infrastructure file | Diff loads in Monaco viewer |
| `infra_file_selected_highlight` | Click an infrastructure file | File row gets `selected` class |
| `sub_groups_rendered` | Expand infrastructure section | Sub-groups shown with headers |
| `sub_group_collapsible` | Click sub-group header | Files toggle visibility |
| `keyboard_nav_in_infra` | Press j/k while infrastructure file selected | Navigates to next/prev file |

---

## 5. Implementation Plan

### Phase 1: Type Changes (types.rs) — DONE
1. ~~Add `InfraCategory` enum~~ (11 variants)
2. ~~Add `InfraSubGroup` struct~~
3. ~~Update `InfrastructureGroup` to include `sub_groups` field~~ (with backward-compat serde)
4. ~~Update test helpers~~ (all constructors updated across 7 files)
5. ~~Tests: serde roundtrip, backward-compat deserialization, property-based tests~~

### Phase 2: Bidirectional Reachability (cluster.rs) — DONE
1. ~~Modify `compute_file_reachability` to do two-pass BFS~~ (refactored into `bfs_pass` helper)
2. ~~Forward pass: `Direction::Outgoing`, cost=1~~
3. ~~Reverse pass: `Direction::Incoming`, cost=2~~
4. ~~Merge distance maps~~
5. ~~Add tests~~ (5 acceptance + 11 property-based: every-file-placed, forward/reverse chain distances, merge-picks-min, chain grouping, flow order, disconnected-infra, entry-distance-zero, reverse-flow-position-after-forward, multi-entrypoint-forward-preferred)

### Phase 3: Path-Based Entrypoint Detection (entrypoint.rs) — DONE
1. ~~Add shared path-matching helpers~~ (`is_route_handler_path`, `is_strong_route_handler_path`, `is_cli_command_path`, `has_filename_pattern`)
2. ~~Add per-language framework import helpers~~ (11 web + 4 CLI framework checkers)
3. ~~Add path-based detection~~ (`detect_path_based_http_routes`, `detect_path_based_cli_commands`)
4. ~~Add tests per language~~ (12 acceptance tests + helper unit tests + 8 property-based: strong⊂regular, case insensitivity, CLI/route disjointness, dot-delimited patterns, determinism)

### Phase 4: Infrastructure Sub-Clustering (cluster.rs) — DONE
1. ~~Add `classify_by_convention(path) -> InfraCategory`~~ (with `is_true_infrastructure` helper)
2. ~~Add `sub_cluster_infra_files(files, graph) -> Vec<InfraSubGroup>`~~ (4-phase pipeline)
3. ~~Wire into `cluster_files()`~~
4. ~~Add tests~~ (10 acceptance + 7 property-based: preserves-all, no-duplicates, classify-deterministic, sub-cluster-deterministic, files-sorted, convention-categories-match, realistic-paths-preserved)
5. ~~Fix sub-group ordering determinism~~ (sort sub_groups by name before returning)

### Phase 5: Consumer Updates
1. CLI output — no changes needed (JSON structure is backward compatible via `files` field)
2. Tauri UI — make infrastructure files clickable (load diff on click, selected state, keyboard nav)
3. Tauri UI — render sub-groups with collapsible headers instead of flat list
4. ~~LLM refinement — update `from_group_id: "infrastructure"` to work with sub-groups~~ (remove from sub-group on reclassify-out, classify into correct sub-group on reclassify-in, empty sub-group cleanup. 5 unit tests + 3 property-based: categorized-correctly, removes-from-all-sub-groups, sub-groups-consistent-after-reclassify)
5. LLM judge — no changes needed

### Phase 6: Update Existing Spec
1. Update `diff-analyzer.md` section 4.5 (Entrypoint Detection) to reference path-based heuristics
2. Update section 4.6 (Semantic Clustering) to reference bidirectional BFS
3. Update section 7 (JSON schema) to show new `infrastructure_group` shape

---

## 6. Non-Goals

- Parsing unchanged files for better graph connectivity (too expensive, save for v2)
- Workspace-aware import resolution improvements (separate concern)
- Custom user-defined infrastructure patterns in `.flowdiff.toml` (future enhancement)
- Renaming `infrastructure_group` field in JSON output (backward compat)

## 7. Risks

| Risk | Mitigation |
|------|------------|
| False positive entrypoints from path-based detection | Tier 1 (path + import) for ambiguous paths, Tier 2 only for very strong signals |
| Reverse BFS assigns files to wrong entrypoint | Higher cost (2x) ensures forward edges always win when both paths exist |
| Convention classifier miscategorizes files | Categories are conservative — when in doubt, file goes to Unclassified, not Infrastructure |
| Breaking JSON consumers | `files` flat list is preserved for backward compat; `sub_groups` is additive |
