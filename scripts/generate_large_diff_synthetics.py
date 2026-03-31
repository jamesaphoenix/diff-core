#!/usr/bin/env python3
"""Generate a separate synthetic large-diff corpus and target TOMLs.

This is intentionally isolated from the default live-repo experimentation
pipeline. It creates:

- synthetic git repos under ../diffcore-eval-corpus/synthetic/large-diff/
- target TOMLs under eval/repos-large-diff-synthetic/
- a manifest at eval/repositories.large-diff.synthetic.toml

Each synthetic repo contains 80 feature roots with 32 changed files each.
We create 5 commits that add 16 features per commit, then expose 4 pinned
diff ranges per repo:

- base..batch3   => 1536 files
- base..batch4   => 2048 files
- batch1..batch4 => 1536 files
- batch1..batch5 => 2048 files

That yields 20 deterministic large-diff targets from 5 underlying repos.
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import textwrap
from dataclasses import dataclass
from pathlib import Path


FILES_PER_FEATURE = 32
FEATURES_PER_BATCH = 16
TOTAL_BATCHES = 5
TOTAL_FEATURES = FEATURES_PER_BATCH * TOTAL_BATCHES


@dataclass(frozen=True)
class FeatureMeta:
    index: int
    slug: str
    ident: str
    pascal: str
    title: str
    root: str
    bucket: str | None = None


@dataclass(frozen=True)
class Scenario:
    name: str
    description: str
    root_style: str
    semantic_style: str
    support_style: str
    buckets: tuple[str, ...] = ()


@dataclass(frozen=True)
class FeatureBundle:
    semantic: list[tuple[str, str]]
    infrastructure: list[tuple[str, str]]


SCENARIOS = [
    Scenario(
        name="packages-services-clean",
        description="Monorepo packages with clear route/controller/service/repository chains.",
        root_style="packages-services",
        semantic_style="clean",
        support_style="package",
    ),
    Scenario(
        name="packages-workers-placeholder",
        description="Monorepo worker packages with repeated placeholder/index/constants test names.",
        root_style="packages-workers",
        semantic_style="placeholder",
        support_style="package",
    ),
    Scenario(
        name="apps-page-flow",
        description="Next.js style feature folders inside multiple apps with page-flow entrypoints.",
        root_style="apps",
        semantic_style="page-flow",
        support_style="feature",
        buckets=("web", "admin", "studio", "portal"),
    ),
    Scenario(
        name="modules-generated-heavy",
        description="Feature modules with generated/spec/schema churn surrounding semantic code.",
        root_style="modules-features",
        semantic_style="generated-heavy",
        support_style="generated",
    ),
    Scenario(
        name="domain-workflows",
        description="Service-domain roots with repeated workflow/activity/agent/job file names.",
        root_style="domains",
        semantic_style="domain",
        support_style="feature",
        buckets=(
            "content-service",
            "activity-stream",
            "social-media",
            "slideshow",
            "core-services",
        ),
    ),
]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--force",
        action="store_true",
        help="Delete and recreate the synthetic large-diff corpus and TOMLs.",
    )
    args = parser.parse_args()

    repo_root = Path(__file__).resolve().parents[1]
    corpus_root = repo_root.parent / "diffcore-eval-corpus" / "synthetic" / "large-diff"
    all_toml_root = repo_root / "eval" / "repos-large-diff-synthetic"
    gated_toml_root = repo_root / "eval" / "repos-large-diff-synthetic-gated"
    near_toml_root = repo_root / "eval" / "repos-large-diff-synthetic-near-threshold"
    gated_manifest_path = repo_root / "eval" / "repositories.large-diff.synthetic.toml"
    near_manifest_path = repo_root / "eval" / "repositories.large-diff.synthetic.near-threshold.toml"
    all_manifest_path = repo_root / "eval" / "repositories.large-diff.synthetic.all.toml"

    ensure_clean_dir(corpus_root, args.force)
    ensure_clean_dir(all_toml_root, args.force)
    ensure_clean_dir(gated_toml_root, args.force)
    ensure_clean_dir(near_toml_root, args.force)

    target_paths: list[Path] = []
    for scenario in SCENARIOS:
        repo_dir = corpus_root / scenario.name
        targets = build_scenario_repo(repo_dir, scenario)
        for target in targets:
            rendered = render_target_toml(target)
            filename = f"{target['name']}.toml"
            all_path = all_toml_root / filename
            all_path.write_text(rendered, encoding="utf-8")
            target_paths.append(all_path)
            if str(target["name"]).endswith("2048"):
                (gated_toml_root / filename).write_text(rendered, encoding="utf-8")
            else:
                (near_toml_root / filename).write_text(rendered, encoding="utf-8")

    gated_manifest_path.write_text(
        render_manifest(
            include_dir="repos-large-diff-synthetic-gated",
            title="diffcore synthetic large-diff corpus",
            description=(
                "Gated 2k+ synthetic targets. This is the primary large-diff "
                "synthetic pipeline because the coarse partitioning compromise "
                "only applies at 2000+ changed files."
            ),
        ),
        encoding="utf-8",
    )
    near_manifest_path.write_text(
        render_manifest(
            include_dir="repos-large-diff-synthetic-near-threshold",
            title="diffcore synthetic near-threshold corpus",
            description=(
                "Diagnostic 1.5k synthetic targets. These do not exercise the "
                "2k+ partition gate and should be tracked separately from the "
                "gated large-diff pipeline."
            ),
        ),
        encoding="utf-8",
    )
    all_manifest_path.write_text(
        render_manifest(
            include_dir="repos-large-diff-synthetic",
            title="diffcore synthetic large-diff corpus (all)",
            description=(
                "All generated 1.5k and 2k+ synthetic targets. Useful for broad "
                "exploration, but not the default gating manifest."
            ),
        ),
        encoding="utf-8",
    )

    print(f"Generated {len(target_paths)} large-diff synthetic targets.")
    print(f"Corpus:   {corpus_root}")
    print(f"Targets:  {all_toml_root}")
    print(f"Gated:    {gated_manifest_path}")
    print(f"Near:     {near_manifest_path}")
    print(f"All:      {all_manifest_path}")


def ensure_clean_dir(path: Path, force: bool) -> None:
    if path.exists():
        if not force:
            raise SystemExit(
                f"{path} already exists. Re-run with --force to recreate the synthetic corpus."
            )
        shutil.rmtree(path)
    path.mkdir(parents=True, exist_ok=True)


def build_scenario_repo(repo_dir: Path, scenario: Scenario) -> list[dict[str, object]]:
    repo_dir.mkdir(parents=True, exist_ok=True)
    git(repo_dir, "init", "-q")
    git(repo_dir, "config", "user.name", "diffcore synthetic generator")
    git(repo_dir, "config", "user.email", "diffcore@example.com")

    write_base_files(repo_dir, scenario)
    base_sha = commit_all(repo_dir, f"{scenario.name}: base workspace")

    commit_shas: list[str] = []
    for batch_index in range(TOTAL_BATCHES):
        start = batch_index * FEATURES_PER_BATCH + 1
        end = start + FEATURES_PER_BATCH
        for feature_index in range(start, end):
            bundle = build_feature_bundle(scenario, make_feature_meta(scenario, feature_index))
            for rel_path, content in bundle.semantic + bundle.infrastructure:
                write_text(repo_dir / rel_path, content)
        commit_shas.append(
            commit_all(
                repo_dir,
                f"{scenario.name}: add features {start:03d}-{end - 1:03d}",
            )
        )

    return [
        build_target(
            repo_dir=repo_dir,
            scenario=scenario,
            label="early-1536",
            base_sha=base_sha,
            head_sha=commit_shas[2],
            feature_range=range(1, 49),
        ),
        build_target(
            repo_dir=repo_dir,
            scenario=scenario,
            label="early-2048",
            base_sha=base_sha,
            head_sha=commit_shas[3],
            feature_range=range(1, 65),
        ),
        build_target(
            repo_dir=repo_dir,
            scenario=scenario,
            label="late-1536",
            base_sha=commit_shas[0],
            head_sha=commit_shas[3],
            feature_range=range(17, 65),
        ),
        build_target(
            repo_dir=repo_dir,
            scenario=scenario,
            label="late-2048",
            base_sha=commit_shas[0],
            head_sha=commit_shas[4],
            feature_range=range(17, 81),
        ),
    ]


def write_base_files(repo_dir: Path, scenario: Scenario) -> None:
    workspace = "apps/*" if scenario.root_style == "apps" else "packages/*\n  - modules/*"
    files = {
        "package.json": textwrap.dedent(
            f"""\
            {{
              "name": "diffcore-synthetic-{scenario.name}",
              "private": true,
              "version": "1.0.0",
              "type": "module"
            }}
            """
        ),
        "pnpm-workspace.yaml": f"packages:\n  - {workspace}\n",
        "tsconfig.base.json": textwrap.dedent(
            """\
            {
              "compilerOptions": {
                "target": "ES2022",
                "module": "ESNext",
                "moduleResolution": "Node",
                "jsx": "react-jsx"
              }
            }
            """
        ),
        "README.md": f"# {scenario.name}\n\nSynthetic large-diff workspace.\n",
        ".gitignore": "node_modules/\ndist/\ncoverage/\n",
    }
    for rel_path, content in files.items():
        write_text(repo_dir / rel_path, content)


def make_feature_meta(scenario: Scenario, feature_index: int) -> FeatureMeta:
    slug = f"feature-{feature_index:03d}"
    ident = f"feature{feature_index:03d}"
    pascal = f"Feature{feature_index:03d}"
    title = f"Feature {feature_index:03d}"
    bucket = None

    if scenario.root_style == "packages-services":
        root = f"packages/services/{slug}"
    elif scenario.root_style == "packages-workers":
        root = f"packages/workers/{slug}"
    elif scenario.root_style == "apps":
        bucket = scenario.buckets[(feature_index - 1) % len(scenario.buckets)]
        root = f"apps/{bucket}/src/features/{slug}"
    elif scenario.root_style == "modules-features":
        root = f"modules/features/{slug}"
    elif scenario.root_style == "domains":
        bucket = scenario.buckets[(feature_index - 1) % len(scenario.buckets)]
        root = f"{bucket}/{slug}"
    else:
        raise ValueError(f"Unsupported root style: {scenario.root_style}")

    return FeatureMeta(
        index=feature_index,
        slug=slug,
        ident=ident,
        pascal=pascal,
        title=title,
        root=root,
        bucket=bucket,
    )


def build_feature_bundle(scenario: Scenario, meta: FeatureMeta) -> FeatureBundle:
    semantic = build_semantic_files(scenario.semantic_style, meta)
    infrastructure = build_support_files(scenario.support_style, meta)
    assert len(semantic) == 8, (scenario.name, meta.slug, len(semantic))
    assert len(infrastructure) == 24, (scenario.name, meta.slug, len(infrastructure))
    assert len(semantic) + len(infrastructure) == FILES_PER_FEATURE
    return FeatureBundle(semantic=semantic, infrastructure=infrastructure)


def build_semantic_files(style: str, meta: FeatureMeta) -> list[tuple[str, str]]:
    if style == "clean":
        return clean_semantic_files(meta)
    if style == "placeholder":
        return placeholder_semantic_files(meta)
    if style == "page-flow":
        return page_flow_semantic_files(meta)
    if style == "generated-heavy":
        return generated_heavy_semantic_files(meta)
    if style == "domain":
        return domain_semantic_files(meta)
    raise ValueError(f"Unsupported semantic style: {style}")


def build_support_files(style: str, meta: FeatureMeta) -> list[tuple[str, str]]:
    if style == "package":
        return package_support_files(meta)
    if style == "feature":
        return feature_support_files(meta)
    if style == "generated":
        return generated_support_files(meta)
    raise ValueError(f"Unsupported support style: {style}")


def clean_semantic_files(meta: FeatureMeta) -> list[tuple[str, str]]:
    feature = meta.ident
    root = meta.root
    return [
        (
            f"{root}/src/route.ts",
            ts(
                f"""
                import {{ handle{meta.pascal} }} from "./controller";
                import {{ {feature}DefaultInput }} from "./constants";

                export function register{meta.pascal}Route() {{
                  return handle{meta.pascal}({feature}DefaultInput);
                }}
                """
            ),
        ),
        (
            f"{root}/src/controller.ts",
            ts(
                f"""
                import {{ create{meta.pascal}Record }} from "./service";
                import type {{ {meta.pascal}Input }} from "./types";

                export function handle{meta.pascal}(input: {meta.pascal}Input) {{
                  return create{meta.pascal}Record(input);
                }}
                """
            ),
        ),
        (
            f"{root}/src/service.ts",
            ts(
                f"""
                import {{ persist{meta.pascal}Record }} from "./repository";
                import {{ map{meta.pascal}Input }} from "./model";
                import type {{ {meta.pascal}Input, {meta.pascal}Record }} from "./types";

                export function create{meta.pascal}Record(input: {meta.pascal}Input): {meta.pascal}Record {{
                  const record = map{meta.pascal}Input(input);
                  persist{meta.pascal}Record(record);
                  return record;
                }}
                """
            ),
        ),
        (
            f"{root}/src/repository.ts",
            ts(
                f"""
                import type {{ {meta.pascal}Record }} from "./types";

                export function persist{meta.pascal}Record(record: {meta.pascal}Record) {{
                  return record.id;
                }}
                """
            ),
        ),
        (
            f"{root}/src/model.ts",
            ts(
                f"""
                import type {{ {meta.pascal}Input, {meta.pascal}Record }} from "./types";

                export function map{meta.pascal}Input(input: {meta.pascal}Input): {meta.pascal}Record {{
                  return {{ ...input, persisted: true }};
                }}
                """
            ),
        ),
        (
            f"{root}/src/types.ts",
            ts(
                f"""
                export type {meta.pascal}Input = {{
                  id: string;
                  name: string;
                }};

                export type {meta.pascal}Record = {meta.pascal}Input & {{
                  persisted: boolean;
                }};
                """
            ),
        ),
        (
            f"{root}/src/constants.ts",
            ts(
                f"""
                import type {{ {meta.pascal}Input }} from "./types";

                export const {feature}DefaultInput: {meta.pascal}Input = {{
                  id: "{meta.slug}",
                  name: "{meta.title}",
                }};
                """
            ),
        ),
        (
            f"{root}/src/index.ts",
            ts(
                f"""
                export {{ register{meta.pascal}Route }} from "./route";
                export {{ create{meta.pascal}Record }} from "./service";
                """
            ),
        ),
    ]


def placeholder_semantic_files(meta: FeatureMeta) -> list[tuple[str, str]]:
    feature = meta.ident
    root = meta.root
    return [
        (
            f"{root}/src/route.ts",
            ts(
                f"""
                import {{ handle{meta.pascal} }} from "./controller";
                import {{ {feature}DefaultInput }} from "./constants";

                export function register{meta.pascal}Route() {{
                  return handle{meta.pascal}({feature}DefaultInput);
                }}
                """
            ),
        ),
        (
            f"{root}/src/controller.ts",
            ts(
                f"""
                import {{ run{meta.pascal}Service }} from "./service";
                import type {{ {meta.pascal}Input }} from "./types";

                export function handle{meta.pascal}(input: {meta.pascal}Input) {{
                  return run{meta.pascal}Service(input);
                }}
                """
            ),
        ),
        (
            f"{root}/src/service.ts",
            ts(
                f"""
                import {{ persist{meta.pascal}Value }} from "./repository";
                import {{ {feature}DefaultInput }} from "./constants";
                import type {{ {meta.pascal}Input, {meta.pascal}Result }} from "./types";

                export function run{meta.pascal}Service(
                  input: {meta.pascal}Input = {feature}DefaultInput
                ): {meta.pascal}Result {{
                  return persist{meta.pascal}Value(input);
                }}
                """
            ),
        ),
        (
            f"{root}/src/repository.ts",
            ts(
                f"""
                import type {{ {meta.pascal}Input, {meta.pascal}Result }} from "./types";

                export function persist{meta.pascal}Value(input: {meta.pascal}Input): {meta.pascal}Result {{
                  return {{ ...input, persisted: true }};
                }}
                """
            ),
        ),
        (
            f"{root}/src/constants.ts",
            ts(
                f"""
                import type {{ {meta.pascal}Input }} from "./types";

                export const {feature}DefaultInput: {meta.pascal}Input = {{
                  id: "{meta.slug}",
                  label: "{meta.title}",
                }};
                """
            ),
        ),
        (
            f"{root}/src/types.ts",
            ts(
                f"""
                export type {meta.pascal}Input = {{
                  id: string;
                  label: string;
                }};

                export type {meta.pascal}Result = {meta.pascal}Input & {{
                  persisted: boolean;
                }};
                """
            ),
        ),
        (
            f"{root}/workers/worker.ts",
            ts(
                f"""
                import {{ run{meta.pascal}Service }} from "../src/service";

                export function run{meta.pascal}Worker() {{
                  return run{meta.pascal}Service();
                }}
                """
            ),
        ),
        (
            f"{root}/src/placeholder.ts",
            ts(
                f"""
                import {{ run{meta.pascal}Service }} from "./service";

                export function resolve{meta.pascal}Placeholder() {{
                  return run{meta.pascal}Service();
                }}
                """
            ),
        ),
    ]


def page_flow_semantic_files(meta: FeatureMeta) -> list[tuple[str, str]]:
    feature = meta.ident
    root = meta.root
    return [
        (
            f"{root}/page.tsx",
            ts(
                f"""
                import {{ load{meta.pascal}Data }} from "./loader";
                import {{ {meta.pascal}Panel }} from "./components/panel";

                export default function {meta.pascal}Page() {{
                  const data = load{meta.pascal}Data();
                  return <{meta.pascal}Panel items={{data}} />;
                }}
                """
            ),
        ),
        (
            f"{root}/loader.ts",
            ts(
                f"""
                import {{ fetch{meta.pascal}Data }} from "./api/client";
                import {{ select{meta.pascal}Items }} from "./state/selectors";

                export function load{meta.pascal}Data() {{
                  const store = fetch{meta.pascal}Data();
                  return select{meta.pascal}Items(store);
                }}
                """
            ),
        ),
        (
            f"{root}/api/client.ts",
            ts(
                f"""
                import {{ make{meta.pascal}Store }} from "../state/store";

                export function fetch{meta.pascal}Data() {{
                  return make{meta.pascal}Store();
                }}
                """
            ),
        ),
        (
            f"{root}/state/store.ts",
            ts(
                f"""
                import type {{ {meta.pascal}Store }} from "../state/types";

                export function make{meta.pascal}Store(): {meta.pascal}Store {{
                  return {{ id: "{meta.slug}", items: ["{meta.slug}-card", "{meta.slug}-panel"] }};
                }}
                """
            ),
        ),
        (
            f"{root}/state/selectors.ts",
            ts(
                f"""
                import type {{ {meta.pascal}Store }} from "./types";

                export function select{meta.pascal}Items(store: {meta.pascal}Store) {{
                  return store.items;
                }}
                """
            ),
        ),
        (
            f"{root}/components/card.tsx",
            ts(
                f"""
                export function {meta.pascal}Card(props: {{ label: string }}) {{
                  return <section>{{props.label}}</section>;
                }}
                """
            ),
        ),
        (
            f"{root}/components/panel.tsx",
            ts(
                f"""
                import {{ {meta.pascal}Card }} from "./card";

                export function {meta.pascal}Panel(props: {{ items: string[] }}) {{
                  return <div><{meta.pascal}Card label={{props.items[0] ?? "{meta.slug}"}} /></div>;
                }}
                """
            ),
        ),
        (f"{root}/state/types.ts", ts(f'export type {meta.pascal}Store = {{ id: string; items: string[] }};\n')),
    ]


def generated_heavy_semantic_files(meta: FeatureMeta) -> list[tuple[str, str]]:
    root = meta.root
    return [
        (
            f"{root}/src/route.ts",
            ts(
                f"""
                import {{ handle{meta.pascal} }} from "./controller";

                export function register{meta.pascal}Route() {{
                  return handle{meta.pascal}();
                }}
                """
            ),
        ),
        (
            f"{root}/src/controller.ts",
            ts(
                f"""
                import {{ run{meta.pascal}Service }} from "./service";

                export function handle{meta.pascal}() {{
                  return run{meta.pascal}Service();
                }}
                """
            ),
        ),
        (
            f"{root}/src/service.ts",
            ts(
                f"""
                import {{ persist{meta.pascal}Record }} from "./repository";
                import {{ run{meta.pascal}Workflow }} from "./workflow";
                import type {{ {meta.pascal}Payload }} from "./types";

                export function run{meta.pascal}Service(): {meta.pascal}Payload {{
                  const payload: {meta.pascal}Payload = {{ id: "{meta.slug}", state: "ready" }};
                  run{meta.pascal}Workflow(payload);
                  persist{meta.pascal}Record(payload);
                  return payload;
                }}
                """
            ),
        ),
        (
            f"{root}/src/repository.ts",
            ts(
                f"""
                import type {{ {meta.pascal}Payload }} from "./types";

                export function persist{meta.pascal}Record(payload: {meta.pascal}Payload) {{
                  return payload.id;
                }}
                """
            ),
        ),
        (
            f"{root}/src/workflow.ts",
            ts(
                f"""
                import {{ run{meta.pascal}Activity }} from "./activity";
                import type {{ {meta.pascal}Payload }} from "./types";

                export function run{meta.pascal}Workflow(payload: {meta.pascal}Payload) {{
                  return run{meta.pascal}Activity(payload);
                }}
                """
            ),
        ),
        (
            f"{root}/src/activity.ts",
            ts(
                f"""
                import type {{ {meta.pascal}Payload }} from "./types";

                export function run{meta.pascal}Activity(payload: {meta.pascal}Payload) {{
                  return payload.id;
                }}
                """
            ),
        ),
        (f"{root}/src/resolver.ts", ts(f'export function resolve{meta.pascal}Result() {{ return "{meta.slug}-resolved"; }}\n')),
        (f"{root}/src/types.ts", ts(f'export type {meta.pascal}Payload = {{ id: string; state: "ready" }};\n')),
    ]


def domain_semantic_files(meta: FeatureMeta) -> list[tuple[str, str]]:
    root = meta.root
    return [
        (
            f"{root}/api/route.ts",
            ts(
                f"""
                import {{ handle{meta.pascal} }} from "../src/controller";

                export function register{meta.pascal}Route() {{
                  return handle{meta.pascal}();
                }}
                """
            ),
        ),
        (
            f"{root}/src/controller.ts",
            ts(
                f"""
                import {{ run{meta.pascal}Service }} from "./service";

                export function handle{meta.pascal}() {{
                  return run{meta.pascal}Service();
                }}
                """
            ),
        ),
        (
            f"{root}/src/service.ts",
            ts(
                f"""
                import {{ persist{meta.pascal}Event }} from "./repository";
                import {{ run{meta.pascal}Workflow }} from "./workflow";

                export function run{meta.pascal}Service() {{
                  const workflow = run{meta.pascal}Workflow();
                  return persist{meta.pascal}Event(workflow);
                }}
                """
            ),
        ),
        (
            f"{root}/src/repository.ts",
            ts(
                f"""
                export function persist{meta.pascal}Event(value: string) {{
                  return "{meta.slug}:" + value;
                }}
                """
            ),
        ),
        (
            f"{root}/src/workflow.ts",
            ts(
                f"""
                import {{ run{meta.pascal}Activity }} from "./activity";
                import {{ load{meta.pascal}Agent }} from "./agent";

                export function run{meta.pascal}Workflow() {{
                  return run{meta.pascal}Activity(load{meta.pascal}Agent());
                }}
                """
            ),
        ),
        (
            f"{root}/src/activity.ts",
            ts(
                f"""
                export function run{meta.pascal}Activity(agent: string) {{
                  return agent;
                }}
                """
            ),
        ),
        (f"{root}/src/agent.ts", ts(f'export function load{meta.pascal}Agent() {{ return "{meta.slug}-agent"; }}\n')),
        (
            f"{root}/workers/job.ts",
            ts(
                f"""
                import {{ run{meta.pascal}Workflow }} from "../src/workflow";

                export function run{meta.pascal}Job() {{
                  return run{meta.pascal}Workflow();
                }}
                """
            ),
        ),
    ]


def package_support_files(meta: FeatureMeta) -> list[tuple[str, str]]:
    root = meta.root
    return [
        (f"{root}/package.json", package_json(meta)),
        (f"{root}/tsconfig.json", tsconfig_json()),
        (f"{root}/vitest.config.ts", ts("export default { test: { environment: \"node\" } };\n")),
        (f"{root}/eslint.config.ts", ts("export default [];\n")),
        (f"{root}/README.md", markdown(f"# {meta.title}\n\nPackage support docs.\n")),
        (f"{root}/CHANGELOG.md", markdown(f"# Changelog\n\nUpdated {meta.slug}.\n")),
        (f"{root}/generated/openapi.generated.ts", ts(f'export const {meta.ident}OpenApiVersion = "{meta.slug}-openapi";\n')),
        (f"{root}/generated/client.generated.ts", ts(f'export const {meta.ident}GeneratedClient = "{meta.slug}-client";\n')),
        (f"{root}/generated/types.generated.ts", ts(f'export const {meta.ident}GeneratedTypes = "{meta.slug}-types";\n')),
        (f"{root}/schema/request.schema.json", json_object(f"{meta.slug}-request")),
        (f"{root}/schema/response.schema.json", json_object(f"{meta.slug}-response")),
        (f"{root}/schema/events.schema.json", json_object(f"{meta.slug}-events")),
        (f"{root}/migrations/001_seed.sql", f"-- seed for {meta.slug}\ninsert into flows(id) values ('{meta.slug}');\n"),
        (f"{root}/scripts/release.sh", f"#!/bin/sh\nprintf 'release {meta.slug}\\n'\n"),
        (f"{root}/scripts/sync.sh", f"#!/bin/sh\nprintf 'sync {meta.slug}\\n'\n"),
        (f"{root}/openapi/spec.yaml", f"openapi: 3.1.0\ninfo:\n  title: {meta.title}\n"),
        (f"{root}/Dockerfile", f"FROM node:20-alpine\nWORKDIR /app/{meta.slug}\n"),
        (f"{root}/.env.example", f"FEATURE_ID={meta.slug}\n"),
        (f"{root}/deployment/review.yaml", f"name: {meta.slug}\nmode: review\n"),
        (f"{root}/fixtures/payload.json", json_object(f"{meta.slug}-fixture")),
        (f"{root}/test-utils/factory.ts", ts(f'export const {meta.ident}Factory = "{meta.slug}-factory";\n')),
        (f"{root}/__generated__/summary.generated.ts", ts(f'export const {meta.ident}Summary = "{meta.slug}-summary";\n')),
        (f"{root}/seed.ts", ts(f'export const {meta.ident}Seed = "{meta.slug}-seed";\n')),
        (f"{root}/feature.config.ts", ts(f'export const {meta.ident}Config = {{ feature: "{meta.slug}" }};\n')),
    ]


def feature_support_files(meta: FeatureMeta) -> list[tuple[str, str]]:
    return package_support_files(meta)


def generated_support_files(meta: FeatureMeta) -> list[tuple[str, str]]:
    return package_support_files(meta)


def build_target(
    repo_dir: Path,
    scenario: Scenario,
    label: str,
    base_sha: str,
    head_sha: str,
    feature_range: range,
) -> dict[str, object]:
    features = [make_feature_meta(scenario, index) for index in feature_range]
    semantic_paths: list[str] = []
    infra_paths: list[str] = []
    ml_groups: list[tuple[str, str, list[str]]] = []

    for meta in features:
        bundle = build_feature_bundle(scenario, meta)
        feature_semantic = sorted(path for path, _ in bundle.semantic)
        feature_infra = sorted(path for path, _ in bundle.infrastructure)
        semantic_paths.extend(feature_semantic)
        infra_paths.extend(feature_infra)
        ml_groups.append((meta.slug, "feature", feature_semantic))
        ml_groups.append((f"{meta.slug}-support", "infrastructure", feature_infra))

    semantic_paths.sort()
    infra_paths.sort()

    feature_count = len(features)
    total_files = len(semantic_paths) + len(infra_paths)
    assert total_files in (1536, 2048), total_files

    same_group = []
    if scenario.semantic_style != "domain":
        representative_indexes = [0, 1, feature_count // 2, feature_count - 1]
        for position in representative_indexes:
            meta = features[position]
            same_group.append(
                [
                    semantic_path(meta, scenario),
                    service_like_path(meta, scenario),
                    supporting_semantic_path(meta, scenario),
                ]
            )

    separate_group = [
        [semantic_path(features[0], scenario), semantic_path(features[1], scenario)],
        [service_like_path(features[0], scenario), service_like_path(features[-1], scenario)],
        [semantic_path(features[feature_count // 3], scenario), semantic_path(features[(feature_count // 3) + 1], scenario)],
    ]

    is_gated_large_diff = total_files >= 2000
    max_groups = 130 if is_gated_large_diff else int(feature_count * 1.5)
    max_group_density = 65.0 if is_gated_large_diff else 50.0
    return {
        "name": f"synthetic-large-diff-{scenario.name}-{label}",
        "scenario": scenario,
        "repo_dir": repo_dir,
        "base_sha": base_sha,
        "head_sha": head_sha,
        "notes": f"Synthetic large-diff track only. {scenario.description}",
        "thresholds": {
            "max_groups": max_groups,
            "max_infra_ratio": 0.80,
            "max_singleton_ratio": 0.60 if is_gated_large_diff else 0.20,
            "max_groups_per_1000_files": max_group_density,
        },
        "expectations": {
            "group_count_min": int(feature_count * 0.75),
            "group_count_max": max_groups,
            "same_group": same_group,
            "separate_group": separate_group,
            "infrastructure": infra_paths,
            "non_infrastructure": semantic_paths,
        },
        "ml": {
            "dataset_version": 1,
            "underlying_repo": scenario.name,
            "range_id": f"{scenario.name}-{label}",
            "label_source": "synthetic-generator",
            "groups": ml_groups,
        },
    }


def semantic_path(meta: FeatureMeta, scenario: Scenario) -> str:
    if scenario.semantic_style == "page-flow":
        return f"{meta.root}/page.tsx"
    if scenario.semantic_style == "domain":
        return f"{meta.root}/api/route.ts"
    return f"{meta.root}/src/route.ts"


def service_like_path(meta: FeatureMeta, scenario: Scenario) -> str:
    if scenario.semantic_style == "page-flow":
        return f"{meta.root}/loader.ts"
    if scenario.semantic_style == "placeholder":
        return f"{meta.root}/src/service.ts"
    if scenario.semantic_style == "domain":
        return f"{meta.root}/src/service.ts"
    return f"{meta.root}/src/service.ts"


def supporting_semantic_path(meta: FeatureMeta, scenario: Scenario) -> str:
    if scenario.semantic_style == "page-flow":
        return f"{meta.root}/api/client.ts"
    if scenario.semantic_style == "placeholder":
        return f"{meta.root}/src/controller.ts"
    if scenario.semantic_style == "domain":
        return f"{meta.root}/src/controller.ts"
    if scenario.semantic_style == "generated-heavy":
        return f"{meta.root}/src/workflow.ts"
    return f"{meta.root}/src/index.ts"


def render_target_toml(target: dict[str, object]) -> str:
    scenario: Scenario = target["scenario"]  # type: ignore[assignment]
    thresholds = target["thresholds"]  # type: ignore[assignment]
    expectations = target["expectations"]  # type: ignore[assignment]
    ml = target["ml"]  # type: ignore[assignment]
    repo_dir: Path = target["repo_dir"]  # type: ignore[assignment]

    lines: list[str] = [
        f"# Synthetic TypeScript large-diff target for {scenario.name}",
        "[[repos]]",
        f'name = "{target["name"]}"',
        'type = "synthetic"',
        f'path = "{repo_dir}"',
        'language = "typescript"',
        f'range = "{target["base_sha"]}..{target["head_sha"]}"',
        f'notes = "{escape_toml_string(target["notes"])}"',
        "",
        "[repos.thresholds]",
        f'max_groups = {thresholds["max_groups"]}',
        f'max_infra_ratio = {thresholds["max_infra_ratio"]:.2f}',
        f'max_singleton_ratio = {thresholds["max_singleton_ratio"]:.2f}',
        f'max_groups_per_1000_files = {thresholds["max_groups_per_1000_files"]:.1f}',
        "",
        "[repos.expectations]",
        f'group_count_min = {expectations["group_count_min"]}',
        f'group_count_max = {expectations["group_count_max"]}',
        "same_group = [",
    ]
    for group in expectations["same_group"]:
        lines.extend(render_nested_array(group, "  "))
    lines.append("]")
    lines.append("separate_group = [")
    for group in expectations["separate_group"]:
        lines.extend(render_nested_array(group, "  "))
    lines.append("]")
    lines.append("infrastructure = [")
    lines.extend(render_string_list(expectations["infrastructure"], "  "))
    lines.append("]")
    lines.append("non_infrastructure = [")
    lines.extend(render_string_list(expectations["non_infrastructure"], "  "))
    lines.append("]")
    lines.append("")
    lines.append("[repos.ml]")
    lines.append(f'dataset_version = {ml["dataset_version"]}')
    lines.append(f'underlying_repo = "{ml["underlying_repo"]}"')
    lines.append(f'range_id = "{ml["range_id"]}"')
    lines.append(f'label_source = "{ml["label_source"]}"')
    lines.append("")

    for group_id, kind, files in ml["groups"]:
        lines.append("[[repos.ml.groups]]")
        lines.append(f'id = "{group_id}"')
        lines.append(f'kind = "{kind}"')
        lines.append("files = [")
        lines.extend(render_string_list(files, "  "))
        lines.append("]")
        lines.append("")

    return "\n".join(lines).rstrip() + "\n"


def render_manifest(include_dir: str, title: str, description: str) -> str:
    return textwrap.dedent(
        f"""\
        # {title}
        #
        # Separate from:
        # - eval/repositories.research.toml (default live-repo experimentation)
        # - eval/repositories.large-diff.toml (real large-diff targets like octospark-services)
        #
        # {description}

        include_dir = "{include_dir}"

        [defaults]
        max_groups = 130
        max_infra_ratio = 0.80
        max_singleton_ratio = 0.60
        max_groups_per_1000_files = 65.0
        """
    )


def render_nested_array(values: list[str], indent: str) -> list[str]:
    lines = [f"{indent}["]
    lines.extend(render_string_list(values, indent + "  "))
    lines.append(f"{indent}],")
    return lines


def render_string_list(values: list[str], indent: str) -> list[str]:
    return [f'{indent}"{escape_toml_string(value)}",' for value in values]


def escape_toml_string(value: str) -> str:
    return value.replace("\\", "\\\\").replace('"', '\\"')


def package_json(meta: FeatureMeta) -> str:
    return textwrap.dedent(
        f"""\
        {{
          "name": "@synthetic/{meta.slug}",
          "private": true,
          "version": "1.0.0",
          "type": "module"
        }}
        """
    )


def tsconfig_json() -> str:
    return textwrap.dedent(
        """\
        {
          "extends": "../../tsconfig.base.json",
          "compilerOptions": {
            "outDir": "dist"
          }
        }
        """
    )


def json_object(name: str) -> str:
    return f'{{\n  "title": "{name}"\n}}\n'


def markdown(text: str) -> str:
    return text


def ts(source: str) -> str:
    return textwrap.dedent(source).strip() + "\n"


def write_text(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def commit_all(repo_dir: Path, message: str) -> str:
    git(repo_dir, "add", "-A")
    git(repo_dir, "commit", "-q", "-m", message)
    return git(repo_dir, "rev-parse", "HEAD").strip()


def git(repo_dir: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", *args],
        cwd=repo_dir,
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout


if __name__ == "__main__":
    main()
