//! File classification by convention — maps file paths to infrastructure categories.

use crate::types::{FileRole, InfraCategory};

/// Classify a file path into an infrastructure category by convention.
pub fn classify_by_convention(path: &str) -> InfraCategory {
    if is_true_infrastructure(path) {
        return InfraCategory::Infrastructure;
    }

    let lower = path.to_lowercase();
    let filename = lower.rsplit('/').next().unwrap_or(&lower);
    let ext = filename.rsplit('.').next().unwrap_or("");

    // Spring-style app config and bundled seed/schema resources are infra.
    if filename == "application.properties" {
        return InfraCategory::Infrastructure;
    }
    if filename.ends_with(".sql")
        && (lower.contains("/resources/db/") || lower.starts_with("db/"))
    {
        return InfraCategory::Migration;
    }

    // Schemas/Types — but NOT source code files in /types/ directories
    // (Go, Rust, TS packages named "types" contain core application types, not infra)
    let is_source_ext = matches!(
        ext,
        "go" | "rs"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "java"
            | "kt"
            | "rb"
            | "php"
            | "cs"
            | "swift"
            | "scala"
    );
    if (lower.contains("/schemas/")
        || lower.starts_with("schemas/")
        || lower.contains("/schema/")
        || lower.starts_with("schema/")
        || filename.contains(".schema.")
        || filename.contains(".dto."))
        // /types/ only counts as schema for non-source files (JSON schemas, etc.)
        || ((lower.contains("/types/") || lower.starts_with("types/")) && !is_source_ext)
    {
        return InfraCategory::Schema;
    }

    // Migrations
    if lower.contains("/migrations/")
        || lower.starts_with("migrations/")
        || lower.contains("/migrate/")
        || lower.starts_with("migrate/")
        || filename.contains(".migration.")
        || lower.contains("/seeds/")
        || lower.starts_with("seeds/")
        || lower.contains("/fixtures/")
        || lower.starts_with("fixtures/")
    {
        return InfraCategory::Migration;
    }

    // Scripts
    if matches!(ext, "sh" | "bash" | "zsh" | "ps1")
        || lower.contains("/scripts/")
        || lower.starts_with("scripts/")
    {
        return InfraCategory::Script;
    }

    // Deployment
    if (lower.contains("/deploy/")
        || lower.starts_with("deploy/")
        || lower.contains("/deployment/")
        || lower.starts_with("deployment/"))
        && !is_true_infrastructure(path)
    {
        return InfraCategory::Deployment;
    }

    // Documentation
    // Exception: top-level docs/content/ is site content in static site generators (Hugo),
    // not project documentation. Also skip www/docs/ which is often a docs website source.
    // But packages/docs/content/ (nested under a package) is still documentation.
    let is_site_content = lower.starts_with("docs/content/")
        || lower.starts_with("content/")
        || lower.starts_with("www/docs/");
    // .md files inside src/docs/ directories are inline API documentation
    // tied to source code (e.g., axum/src/docs/routing/with_state.md).
    // Also exempt resources/mdtest/ (ruff test specs written as .md).
    let is_inline_doc = matches!(ext, "md" | "mdx")
        && (lower.contains("/src/docs/") || lower.contains("/resources/mdtest/"));
    if !is_site_content
        && !is_inline_doc
        && (matches!(ext, "md" | "mdx" | "rst" | "txt")
            || lower.contains("/docs/")
            || lower.starts_with("docs/")
            || lower.contains("/documentation/")
            || lower.starts_with("documentation/"))
    {
        return InfraCategory::Documentation;
    }

    // Lint configs
    if filename.starts_with(".eslint")
        || filename.starts_with(".prettier")
        || filename.starts_with(".stylelint")
        || filename == ".editorconfig"
        || filename == ".clang-format"
        || filename == "rustfmt.toml"
        || filename == ".rubocop.yml"
        || filename == ".flake8"
        || filename == "mypy.ini"
        || filename == ".golangci.yml"
    {
        return InfraCategory::Lint;
    }

    // Test utilities
    if lower.contains("/test-utils/")
        || lower.contains("/test-helpers/")
        || lower.contains("/__fixtures__/")
        || lower.contains("/test/fixtures/")
        || lower.contains("/testutils/")
    {
        return InfraCategory::TestUtil;
    }

    // Generated code
    if lower.contains("/generated/")
        || lower.contains("/__generated__/")
        || filename.contains(".generated.")
        || filename.ends_with(".g.dart")
        || filename.ends_with(".pb.go")
    {
        return InfraCategory::Generated;
    }

    InfraCategory::Unclassified
}

/// Check if a file is true infrastructure (Docker, CI/CD, env configs, build configs, etc.).
pub(super) fn is_true_infrastructure(path: &str) -> bool {
    let lower = path.to_lowercase();
    let filename = lower.rsplit('/').next().unwrap_or(&lower);

    // Environment/Secrets
    if filename.starts_with(".env") || filename.ends_with(".env") {
        return true;
    }

    // Signing keys and binary infra
    if filename.ends_with(".snk") || filename.ends_with(".pfx") || filename.ends_with(".pem") {
        return true;
    }

    // Docker
    if filename.starts_with("dockerfile")
        || filename.starts_with("docker-compose")
        || filename == ".dockerignore"
    {
        return true;
    }

    // CI/CD + release tooling
    if lower.contains(".github/workflows/")
        || filename == ".gitlab-ci.yml"
        || filename == "jenkinsfile"
        || lower.contains(".circleci/")
        || filename == ".travis.yml"
        || filename == "azure-pipelines.yml"
        || filename == "bitbucket-pipelines.yml"
        || lower.contains(".changeset/")
        || lower.starts_with(".changeset/")
        || lower.contains(".changes/")
        || lower.starts_with(".changes/")
    {
        return true;
    }

    // Container orchestration
    if lower.contains("k8s/")
        || lower.contains("kubernetes/")
        || lower.contains("helm/")
        || filename.contains(".helmrelease.")
    {
        return true;
    }

    // Terraform/IaC
    if lower.contains("terraform/")
        || filename.ends_with(".tf")
        || filename.ends_with(".tfvars")
        || lower.contains("pulumi/")
        || filename.starts_with("pulumi.")
        || lower.contains("cdk/")
        || lower.contains("cloudformation/")
    {
        return true;
    }

    // Package manager configs
    if matches!(
        filename,
        "package.json"
            | "cargo.toml"
            | "go.mod"
            | "go.sum"
            | "requirements.txt"
            | "pipfile"
            | "pyproject.toml"
            | "gemfile"
            | "pom.xml"
            | "package.swift"
            | "build.sbt"
            | "composer.json"
    ) || filename.starts_with("build.gradle")
        || filename.ends_with(".csproj")
    {
        return true;
    }

    // Lock files
    if matches!(
        filename,
        "package-lock.json"
            | "yarn.lock"
            | "pnpm-lock.yaml"
            | "cargo.lock"
            | "gemfile.lock"
            | "poetry.lock"
            | "composer.lock"
    ) {
        return true;
    }

    // Build tool configs
    if filename.starts_with("tsconfig")
        || filename.starts_with("webpack.")
        || filename.starts_with("vite.")
        || filename.starts_with("rollup.")
        || filename.starts_with("esbuild.")
        || filename.starts_with("babel.")
        || filename == "makefile"
        || filename == "cmakelists.txt"
        || filename.ends_with(".mk")
        || filename == "build.rs"
    {
        return true;
    }

    // IDE/editor configs
    if lower.contains(".vscode/") || lower.contains(".idea/") || lower.contains(".eclipse/") {
        return true;
    }

    // MCP/tool configs
    if filename == ".mcp.json"
        || lower.contains(".mcp/")
        || filename == ".tool-versions"
        || filename == ".nvmrc"
        || filename == ".node-version"
        || filename == ".python-version"
        || filename == ".ruby-version"
    {
        return true;
    }

    // Git configs
    if matches!(
        filename,
        ".gitignore" | ".gitattributes" | ".gitmodules" | "codeowners"
    ) {
        return true;
    }

    false
}

/// Check if a filename looks like infrastructure/config even though it has a source extension.
/// Examples: settings.py, __init__.py, celeryconf.py, seed.ts, biome.json, urls.py
pub(super) fn is_config_like_filename(path: &str) -> bool {
    let lower = path.to_lowercase();
    let filename = lower.rsplit('/').next().unwrap_or(&lower);

    // Python/Django config files
    // Note: __init__.py removed — when changed in a diff, it usually contains
    // meaningful exports or version bumps (fastapi/__init__.py, requests/__init__.py).
    if matches!(
        filename,
        "settings.py"
            | "celeryconf.py"
            | "urls.py"
            | "wsgi.py"
            | "asgi.py"
            | "conftest.py"
            | "manage.py"
            | "setup.py"
            | "setup.cfg"
    ) {
        return true;
    }

    // PHP config tools (rector, phpstan, phpunit)
    if matches!(
        filename,
        "rector.php" | "phpstan.neon" | "phpunit.xml" | "phpunit.xml.dist"
    ) {
        return true;
    }

    // Version constant files (auto-bumped, not feature code)
    if matches!(
        filename,
        "version.go"
            | "version.ts"
            | "version.js"
            | "version.py"
            | "version.rb"
            | "__version__.py"
    ) || filename == "version_current.go"
    {
        return true;
    }

    // Generated conversion/deepcopy files (Kubernetes codegen)
    if filename.starts_with("zz_generated") {
        return true;
    }

    // JS/TS config files with source extensions
    if matches!(
        filename,
        "seed.ts"
            | "seed.js"
            | "biome.json"
            | "eslint.config.js"
            | "eslint.config.ts"
            | "eslint.config.mjs"
            | "vitest.config.ts"
            | "jest.config.ts"
            | "jest.config.js"
            | "webpack.config.ts"
            | "webpack.config.js"
            | "rollup.config.ts"
            | "rollup.config.js"
            | "vite.config.ts"
            | "vite.config.js"
            | "next.config.ts"
            | "next.config.js"
            | "next.config.mjs"
            | "tailwind.config.ts"
            | "tailwind.config.js"
            | "postcss.config.js"
            | "postcss.config.ts"
            | "build.ts"
            | "build.js"
            | "build.mjs"
            | "tsup.config.ts"
            | "esbuild.config.ts"
    ) {
        return true;
    }

    // Generic *.config.ts/js pattern (catches updates.config.ts, etc.)
    if filename.contains(".config.") {
        return true;
    }

    // Swagger/OpenAPI generated templates
    if lower.contains("/swagger/") || lower.contains("/openapi/") {
        return true;
    }

    // CSS theme files (infrastructure, not feature code)
    if lower.contains("/themes/") && filename.starts_with("theme-") {
        return true;
    }

    // Proto files (generated protobuf definitions)
    if filename.ends_with(".proto") || filename.ends_with(".pb.go") {
        return true;
    }

    // Files in test fixtures directories (not under src/)
    if lower.contains("/fixtures/") && !lower.contains("/src/") {
        return true;
    }

    // Scripts, packaging, and vendored dependency directories
    if lower.starts_with("scripts/")
        || lower.contains("/pkg/brew/")
        || lower.starts_with("pkg/brew/")
        || lower.starts_with("vendor/")
        || lower.contains("/vendor/")
        || lower.starts_with("third_party/")
        || lower.contains("/third_party/")
        || lower.starts_with("node_modules/")
    {
        return true;
    }

    // Migration test files (tests inside migration directories)
    if lower.contains("/migrations/") {
        return true;
    }

    false
}

/// Check if a documentation file should stay in infrastructure.
/// Files like README.md and CHANGELOG.md at the root are infra, as are direct
/// landing pages in nested docs sections such as `docs/user/*.rst`.
pub(super) fn is_top_level_doc(path: &str) -> bool {
    let lower = path.to_lowercase();
    let filename = lower.rsplit('/').next().unwrap_or(&lower);
    let depth = path.matches('/').count();

    // Direct docs section landing pages are usually navigational/release docs,
    // not semantic code-adjacent documentation bundles.
    if matches!(lower.as_str(), "docs/api.md" | "docs/api.mdx" | "docs/api.rst" | "docs/api.txt")
        || is_nested_docs_section_landing_page(&lower, "docs/user/")
        || is_nested_docs_section_landing_page(&lower, "docs/community/")
    {
        return true;
    }

    // Root-level docs
    if depth == 0
        && matches!(
            filename,
            "readme.md"
                | "changelog.md"
                | "changelog.rst"
                | "contributing.md"
                | "license.md"
                | "security.md"
                | "authors.md"
                | "code_of_conduct.md"
                | "changes.rst"
                | "history.md"
                | "releases.md"
        )
    {
        return true;
    }

    // CHANGELOG/README at any depth
    if matches!(filename, "readme.md" | "changelog.md" | "changelog.rst") {
        return true;
    }

    false
}

fn is_nested_docs_section_landing_page(lower: &str, prefix: &str) -> bool {
    let Some(rest) = lower.strip_prefix(prefix) else {
        return false;
    };

    !rest.contains('/')
        && matches!(
            rest.rsplit('.').next().unwrap_or(""),
            "md" | "mdx" | "rst" | "txt"
        )
}

/// Infer a file's role from its path using heuristic patterns.
pub(super) fn infer_file_role(path: &str) -> FileRole {
    let lower = path.to_lowercase();
    if lower.contains("handler") || lower.contains("controller") || lower.contains("route") {
        FileRole::Handler
    } else if lower.contains("service") {
        FileRole::Service
    } else if lower.contains("repo") || lower.contains("repository") || lower.contains("dal") {
        FileRole::Repository
    } else if lower.contains("model") || lower.contains("schema") || lower.contains("entity") {
        FileRole::Model
    } else if lower.contains("config") || lower.contains("setting") {
        FileRole::Config
    } else if lower.contains("test") || lower.contains("spec") {
        FileRole::Test
    } else if lower.contains("util") || lower.contains("helper") || lower.contains("lib") {
        FileRole::Utility
    } else {
        FileRole::Infrastructure
    }
}

/// Display name for an InfraCategory.
pub(crate) fn category_display_name(cat: &InfraCategory) -> String {
    match cat {
        InfraCategory::Infrastructure => "Infrastructure".to_string(),
        InfraCategory::Schema => "Schemas".to_string(),
        InfraCategory::Script => "Scripts".to_string(),
        InfraCategory::Migration => "Migrations".to_string(),
        InfraCategory::Deployment => "Deployment".to_string(),
        InfraCategory::Documentation => "Documentation".to_string(),
        InfraCategory::Lint => "Lint".to_string(),
        InfraCategory::TestUtil => "Test utilities".to_string(),
        InfraCategory::Generated => "Generated".to_string(),
        InfraCategory::DirectoryGroup => "Directory group".to_string(),
        InfraCategory::Unclassified => "Unclassified".to_string(),
    }
}
