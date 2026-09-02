//! Deterministic review metadata — the heuristic floor from `specs/group-metadata.md` §3.1.
//!
//! Populates the three `FlowGroup` fields that can be derived honestly without a model:
//! `risk` (bucketed from `risk_score`), `group_type` (path conventions) and `impact`
//! (how far the group's files spread across the tree). `description`, `invariant`,
//! `review_focus` and `complexity` are deliberately left empty: a wrong value in any of
//! them misdirects a reviewer, and no free signal is good enough to fill them.

use std::collections::HashSet;

use crate::cluster::classify_by_convention;
use crate::cluster::stem::is_test_file_name;
use crate::types::{FlowGroup, GroupType, ImpactScope, InfraCategory, Risk};

/// Fill the deterministic metadata fields on every group.
///
/// `risk_score` and `review_order` are read-only here — see spec §1.3.
pub fn apply_heuristic_metadata(groups: &mut [FlowGroup]) {
    for group in groups.iter_mut() {
        let paths: Vec<&str> = group.files.iter().map(|f| f.path.as_str()).collect();
        group.risk = Some(bucket_risk(group.risk_score));
        group.group_type = infer_group_type(&paths);
        group.impact = Some(infer_impact(&paths));
    }
}

fn bucket_risk(score: f64) -> Risk {
    if score >= 0.75 {
        Risk::Critical
    } else if score >= 0.55 {
        Risk::High
    } else if score >= 0.35 {
        Risk::Medium
    } else {
        Risk::Low
    }
}

fn infer_group_type(paths: &[&str]) -> Option<GroupType> {
    if paths.is_empty() {
        return None;
    }

    for (matcher, group_type) in [
        (is_ci_path as fn(&str) -> bool, GroupType::Ci),
        (is_build_path, GroupType::Build),
        (is_doc_path, GroupType::Docs),
        (is_test_file_name, GroupType::Test),
    ] {
        if paths.iter().all(|path| matcher(path)) {
            return Some(group_type);
        }
    }

    None
}

fn is_ci_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    let filename = lower.rsplit('/').next().unwrap_or(&lower);

    lower.contains(".github/workflows/")
        || lower.contains(".github/actions/")
        || lower.contains(".circleci/")
        || lower.contains(".buildkite/")
        || matches!(
            filename,
            ".gitlab-ci.yml"
                | "jenkinsfile"
                | ".travis.yml"
                | "azure-pipelines.yml"
                | "bitbucket-pipelines.yml"
                | ".pre-commit-config.yaml"
        )
}

fn is_build_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    let filename = lower.rsplit('/').next().unwrap_or(&lower);

    if matches!(
        filename,
        "package.json"
            | "package-lock.json"
            | "yarn.lock"
            | "pnpm-lock.yaml"
            | "pnpm-workspace.yaml"
            | "cargo.toml"
            | "cargo.lock"
            | "go.mod"
            | "go.sum"
            | "requirements.txt"
            | "pipfile"
            | "pipfile.lock"
            | "pyproject.toml"
            | "poetry.lock"
            | "setup.py"
            | "setup.cfg"
            | "gemfile"
            | "gemfile.lock"
            | "composer.json"
            | "composer.lock"
            | "pom.xml"
            | "build.sbt"
            | "package.swift"
            | "makefile"
            | "cmakelists.txt"
            | "build.rs"
            | "flake.nix"
            | "flake.lock"
            | "shell.nix"
            | "default.nix"
            | ".dockerignore"
    ) {
        return true;
    }

    filename.starts_with("dockerfile")
        || filename.starts_with("docker-compose")
        || filename.starts_with("tsconfig")
        || filename.starts_with("build.gradle")
        || filename.starts_with("webpack.")
        || filename.starts_with("vite.")
        || filename.starts_with("rollup.")
        || filename.starts_with("esbuild.")
        || filename.starts_with("babel.")
        || filename.ends_with(".mk")
        || filename.ends_with(".csproj")
}

fn is_doc_path(path: &str) -> bool {
    classify_by_convention(path) == InfraCategory::Documentation
}

/// Fan-out of the group's own files across the tree: one directory is `Local`, several
/// directories under one module root is `Module`, two module roots is `CrossCutting`,
/// three or more is `System`.
///
/// `FlowGroup::edges` cannot answer this — `cluster::bfs::collect_internal_edges` keeps
/// only edges whose endpoints are both inside the group, so no group edge ever crosses a
/// group boundary, and the symbol graph is gone by the time groups are finalized.
fn infer_impact(paths: &[&str]) -> ImpactScope {
    let dirs: HashSet<&str> = paths.iter().map(|p| parent_dir(p)).collect();
    let roots: HashSet<&str> = paths.iter().map(|p| module_root(p)).collect();

    match (roots.len(), dirs.len()) {
        (roots, _) if roots >= 3 => ImpactScope::System,
        (roots, _) if roots >= 2 => ImpactScope::CrossCutting,
        (_, dirs) if dirs >= 2 => ImpactScope::Module,
        _ => ImpactScope::Local,
    }
}

fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// The unit a change is "inside": the first path segment, or two segments deep for the
/// usual monorepo container directories.
fn module_root(path: &str) -> &str {
    let mut parts = path.split('/');
    let Some(first) = parts.next().filter(|segment| !segment.is_empty()) else {
        return "";
    };
    if !matches!(
        first,
        "apps" | "packages" | "services" | "workers" | "libs" | "modules" | "crates"
    ) {
        return first;
    }
    match parts.next() {
        Some(second) if !second.is_empty() => &path[..first.len() + 1 + second.len()],
        _ => first,
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
mod tests {
    use super::*;
    use crate::types::{ChangeStats, FileChange, FileRole};

    fn group(id: &str, risk_score: f64, paths: &[&str]) -> FlowGroup {
        FlowGroup {
            id: id.to_string(),
            name: id.to_string(),
            risk_score,
            files: paths
                .iter()
                .map(|path| FileChange {
                    path: (*path).to_string(),
                    flow_position: 0,
                    role: FileRole::Service,
                    changes: ChangeStats {
                        additions: 1,
                        deletions: 0,
                    },
                    symbols_changed: vec![],
                })
                .collect(),
            ..Default::default()
        }
    }

    fn metadata_for(paths: &[&str]) -> FlowGroup {
        let mut groups = vec![group("g1", 0.0, paths)];
        apply_heuristic_metadata(&mut groups);
        groups.remove(0)
    }

    // ── risk bucketing ──

    #[test]
    fn risk_buckets_span_the_score_range() {
        for (score, expected) in [
            (0.0, Risk::Low),
            (0.34, Risk::Low),
            (0.35, Risk::Medium),
            (0.54, Risk::Medium),
            (0.55, Risk::High),
            (0.74, Risk::High),
            (0.75, Risk::Critical),
            (1.0, Risk::Critical),
        ] {
            let mut groups = vec![group("g1", score, &["src/a.ts"])];
            apply_heuristic_metadata(&mut groups);
            assert_eq!(groups[0].risk, Some(expected), "score {score}");
        }
    }

    #[test]
    fn risk_score_and_review_order_are_untouched() {
        let mut groups = vec![group("g1", 0.62, &["src/a.ts"])];
        groups[0].review_order = 7;
        apply_heuristic_metadata(&mut groups);
        assert_eq!(groups[0].risk_score, 0.62);
        assert_eq!(groups[0].review_order, 7);
    }

    // ── group_type ──

    #[test]
    fn all_test_files_are_a_test_group() {
        assert_eq!(
            metadata_for(&["tests/auth.rs", "src/user.test.ts", "pkg/user_test.go"]).group_type,
            Some(GroupType::Test)
        );
    }

    #[test]
    fn all_docs_are_a_docs_group() {
        assert_eq!(
            metadata_for(&["README.md", "docs/guide.mdx"]).group_type,
            Some(GroupType::Docs)
        );
    }

    #[test]
    fn workflow_files_are_a_ci_group() {
        assert_eq!(
            metadata_for(&[".github/workflows/ci.yml", ".circleci/config.yml"]).group_type,
            Some(GroupType::Ci)
        );
    }

    #[test]
    fn manifests_and_dockerfiles_are_a_build_group() {
        assert_eq!(
            metadata_for(&["Cargo.toml", "crates/core/Cargo.toml", "Dockerfile"]).group_type,
            Some(GroupType::Build)
        );
    }

    #[test]
    fn mixed_files_have_no_group_type() {
        assert_eq!(metadata_for(&["src/auth.ts", "tests/auth.rs"]).group_type, None);
        assert_eq!(metadata_for(&["src/auth.ts", "src/user.ts"]).group_type, None);
    }

    #[test]
    fn empty_group_has_no_group_type() {
        assert_eq!(metadata_for(&[]).group_type, None);
    }

    // ── impact ──

    #[test]
    fn single_directory_is_local() {
        assert_eq!(
            metadata_for(&["src/auth/login.ts", "src/auth/token.ts"]).impact,
            Some(ImpactScope::Local)
        );
    }

    #[test]
    fn several_directories_in_one_root_are_module_scoped() {
        assert_eq!(
            metadata_for(&["src/auth/login.ts", "src/http/router.ts"]).impact,
            Some(ImpactScope::Module)
        );
    }

    #[test]
    fn two_roots_are_cross_cutting() {
        assert_eq!(
            metadata_for(&["api/handler.ts", "web/page.tsx"]).impact,
            Some(ImpactScope::CrossCutting)
        );
    }

    #[test]
    fn sibling_groups_in_one_directory_stay_local() {
        let mut groups = vec![
            group("g1", 0.0, &["src/routes/health.ts"]),
            group("g2", 0.0, &["src/routes/users.ts"]),
        ];
        apply_heuristic_metadata(&mut groups);
        assert_eq!(groups[0].impact, Some(ImpactScope::Local));
        assert_eq!(groups[1].impact, Some(ImpactScope::Local));
    }

    #[test]
    fn three_roots_are_system_wide() {
        assert_eq!(
            metadata_for(&["api/a.ts", "web/b.ts", "worker/c.ts"]).impact,
            Some(ImpactScope::System)
        );
    }

    #[test]
    fn monorepo_packages_are_separate_roots() {
        assert_eq!(
            metadata_for(&["packages/ui/button.tsx", "packages/core/index.ts"]).impact,
            Some(ImpactScope::CrossCutting)
        );
        assert_eq!(
            metadata_for(&["packages/ui/button.tsx", "packages/ui/card.tsx"]).impact,
            Some(ImpactScope::Local)
        );
    }

    // ── fields the heuristic floor must not fill ──

    #[test]
    fn llm_only_fields_stay_empty() {
        let g = metadata_for(&["src/auth/login.ts"]);
        assert_eq!(g.description, None);
        assert_eq!(g.invariant, None);
        assert_eq!(g.complexity, None);
        assert!(g.review_focus.is_empty());
    }
}
