//! Filename stem utilities for matching test files to their implementations.

/// Extract bare filename stem (no directory, no extension, no test suffix).
/// Returns empty string for common infra filenames to prevent cascade-merging
/// in monorepos (e.g., 37 package.json files would merge all groups via stem "package").
pub(super) fn bare_stem(path: &str) -> String {
    let filename = path.rsplit('/').next().unwrap_or(path);
    let base = filename
        .replace(".test.", ".")
        .replace(".spec.", ".")
        .replace(".vitest.", ".")
        .replace("_test.", ".")
        .replace(".e2e.", ".")
        .replace(".integration-test.", ".");
    let stem = base.rsplit_once('.').map(|(s, _)| s).unwrap_or(&base);
    let stem = stem.strip_prefix("test_").unwrap_or(stem);
    let mut lower = stem.to_lowercase();
    // Strip Java/PHP/C# test suffixes: FooTest → Foo, FooTests → Foo, FooSpec → Foo
    if lower.ends_with("tests") {
        lower.truncate(lower.len() - 5);
    } else if lower.ends_with("test") {
        lower.truncate(lower.len() - 4);
    } else if lower.ends_with("spec") {
        lower.truncate(lower.len() - 4);
    }

    // Exclude common infra filenames that appear in many packages
    // to prevent cascade-merging in monorepos.
    if matches!(
        lower.as_str(),
        "package"
            | "changelog"
            | "readme"
            | "license"
            | "index"
            | "mod"
            | "lib"
            | "main"
            | "init"
            | "__init__"
            | "version"
            | "setup"
            | "config"
            | "tsconfig"
    ) {
        return String::new();
    }

    lower
}

/// Extract the "stem" that a test file and its impl share.
/// "sort.rs" and "sort_test.rs" both have stem "sort".
/// "controller.spec.ts" and "controller.ts" both have stem "controller".
pub(super) fn test_impl_stem(path: &str) -> String {
    let filename = path.rsplit('/').next().unwrap_or(path);
    let dir = if path.contains('/') {
        &path[..path.rfind('/').unwrap_or(0)]
    } else {
        ""
    };

    // Remove test suffixes to get the base name
    let base = filename
        .replace(".test.", ".")
        .replace(".spec.", ".")
        .replace(".vitest.", ".")
        .replace("_test.", ".")
        .replace(".e2e.", ".")
        .replace(".integration-test.", ".")
        .replace("_bench_test.", ".");

    // Remove extension
    let stem = base.rsplit_once('.').map(|(s, _)| s).unwrap_or(&base);

    // Also strip "test_" prefix (Python convention)
    let stem = stem.strip_prefix("test_").unwrap_or(stem);

    // Strip Java/PHP/C# suffixes: FooTest → Foo, FooTests → Foo
    let stem = stem
        .strip_suffix("Tests")
        .or_else(|| stem.strip_suffix("Test"))
        .or_else(|| stem.strip_suffix("Spec"))
        .unwrap_or(stem);

    // Combine directory context with stem for uniqueness
    // Use the last 2 directory components + stem
    let dir_parts: Vec<&str> = dir.split('/').collect();
    let context = if dir_parts.len() >= 2 {
        dir_parts[dir_parts.len() - 2..].join("/")
    } else {
        dir.to_string()
    };

    format!("{}:{}", context, stem)
}

/// Check if a file is a test file — by filename pattern OR directory.
pub(super) fn is_test_file_name(path: &str) -> bool {
    let lower = path.to_lowercase();
    let filename = lower.rsplit('/').next().unwrap_or(&lower);

    // Filename patterns
    if filename.contains(".test.")
        || filename.contains(".spec.")
        || filename.contains(".vitest.")
        || filename.contains("_test.")
        || filename.starts_with("test_")
        || filename.contains(".e2e.")
        || filename.contains(".integration-test.")
        || filename.contains("_bench_test.")
    {
        return true;
    }

    // Java/PHP/C# convention: FooTest.java, FooTests.java, FooSpec.java
    let stem = filename
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(filename);
    if stem.ends_with("test") || stem.ends_with("tests") || stem.ends_with("spec") {
        return true;
    }

    // Directory patterns: files in tests/, test/, __tests__/, testscripts/, specs/ directories
    if lower.contains("/tests/")
        || lower.contains("/test/")
        || lower.contains("/__tests__/")
        || lower.contains("/testscripts/")
        || lower.contains("/testdata/")
        || lower.contains("/specs/")
        || lower.starts_with("tests/")
        || lower.starts_with("test/")
        || lower.starts_with("testscripts/")
        || lower.starts_with("specs/")
    {
        return true;
    }

    false
}
