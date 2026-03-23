#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration tests for the ignore/exclude paths feature.
//!
//! Tests `get_ignore_paths` and `save_ignore_paths` using real temporary
//! git repositories and `.flowdiff.toml` config files.
//!
//! Run with:
//!   cargo test --test ignore_paths_integration

use flowdiff_tauri::commands::{get_ignore_paths, save_ignore_paths};
use git2::{Repository, Signature};
use std::path::PathBuf;

/// Create a temporary git repo with an initial commit.
fn create_test_repo() -> (tempfile::TempDir, String) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = Repository::init(tmp.path()).unwrap();

    let sig = Signature::now("test", "test@test.com").unwrap();
    let tree_id = repo.index().unwrap().write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[])
        .unwrap();

    let path = tmp.path().to_str().unwrap().to_string();
    (tmp, path)
}

/// Create a test repo with a pre-existing `.flowdiff.toml` containing ignore paths.
fn create_test_repo_with_config(ignore_paths: &[&str]) -> (tempfile::TempDir, String) {
    let (tmp, path) = create_test_repo();

    let paths_toml: Vec<String> = ignore_paths.iter().map(|p| format!("\"{}\"", p)).collect();
    let config = format!(
        "[ignore]\npaths = [{}]\n",
        paths_toml.join(", ")
    );
    std::fs::write(tmp.path().join(".flowdiff.toml"), config).unwrap();

    (tmp, path)
}

// ── get_ignore_paths ─────────────────────────────────────────────────

#[test]
fn get_returns_empty_when_no_config() {
    let (_tmp, repo_path) = create_test_repo();
    let paths = get_ignore_paths(Some(repo_path)).unwrap();
    assert!(paths.is_empty());
}

#[test]
fn get_returns_empty_when_config_has_no_ignore_section() {
    let (_tmp, repo_path) = create_test_repo();

    // Write a config with only LLM settings, no [ignore] section
    let config_path = PathBuf::from(&repo_path).join(".flowdiff.toml");
    std::fs::write(&config_path, "[llm]\nprovider = \"anthropic\"\n").unwrap();

    let paths = get_ignore_paths(Some(repo_path)).unwrap();
    assert!(paths.is_empty());
}

#[test]
fn get_returns_configured_paths() {
    let (_tmp, repo_path) =
        create_test_repo_with_config(&["dist/**", "**/*.generated.ts", "node_modules/**"]);

    let paths = get_ignore_paths(Some(repo_path)).unwrap();
    assert_eq!(paths.len(), 3);
    assert_eq!(paths[0], "dist/**");
    assert_eq!(paths[1], "**/*.generated.ts");
    assert_eq!(paths[2], "node_modules/**");
}

#[test]
fn get_with_none_repo_path_returns_defaults() {
    let paths = get_ignore_paths(None).unwrap();
    assert!(paths.is_empty(), "No repo path should return default empty config");
}

// ── save_ignore_paths ────────────────────────────────────────────────

#[test]
fn save_creates_config_file() {
    let (_tmp, repo_path) = create_test_repo();

    let config_path = PathBuf::from(&repo_path).join(".flowdiff.toml");
    assert!(!config_path.exists(), "Config should not exist before save");

    save_ignore_paths(
        repo_path,
        vec!["build/**".to_string()],
    )
    .unwrap();

    assert!(config_path.exists(), "Config should be created after save");
}

#[test]
fn save_and_load_roundtrip() {
    let (_tmp, repo_path) = create_test_repo();

    let patterns = vec![
        "dist/**".to_string(),
        "**/*.min.js".to_string(),
        "coverage/**".to_string(),
    ];

    save_ignore_paths(repo_path.clone(), patterns.clone()).unwrap();
    let loaded = get_ignore_paths(Some(repo_path)).unwrap();

    assert_eq!(loaded, patterns);
}

#[test]
fn save_overwrites_existing_paths() {
    let (_tmp, repo_path) =
        create_test_repo_with_config(&["old_pattern/**"]);

    // Verify old pattern is present
    let paths = get_ignore_paths(Some(repo_path.clone())).unwrap();
    assert_eq!(paths, vec!["old_pattern/**"]);

    // Save new patterns — should completely replace
    save_ignore_paths(
        repo_path.clone(),
        vec!["new_pattern/**".to_string(), "other/**".to_string()],
    )
    .unwrap();

    let updated = get_ignore_paths(Some(repo_path)).unwrap();
    assert_eq!(updated, vec!["new_pattern/**", "other/**"]);
}

#[test]
fn save_empty_list_clears_paths() {
    let (_tmp, repo_path) =
        create_test_repo_with_config(&["dist/**", "build/**"]);

    // Clear all paths
    save_ignore_paths(repo_path.clone(), vec![]).unwrap();

    let loaded = get_ignore_paths(Some(repo_path)).unwrap();
    assert!(loaded.is_empty());
}

#[test]
fn save_preserves_other_config_sections() {
    let (_tmp, repo_path) = create_test_repo();

    // Write config with LLM settings
    let config_path = PathBuf::from(&repo_path).join(".flowdiff.toml");
    std::fs::write(
        &config_path,
        "[llm]\nprovider = \"openai\"\nmodel = \"gpt-4.1\"\n\n[ignore]\npaths = [\"old/**\"]\n",
    )
    .unwrap();

    // Save new ignore paths
    save_ignore_paths(
        repo_path.clone(),
        vec!["new/**".to_string()],
    )
    .unwrap();

    // Verify LLM section is preserved
    let content = std::fs::read_to_string(&config_path).unwrap();
    assert!(
        content.contains("openai"),
        "LLM provider should be preserved, got:\n{}",
        content
    );
    assert!(
        content.contains("gpt-4.1"),
        "LLM model should be preserved, got:\n{}",
        content
    );
}

// ── Error Cases ──────────────────────────────────────────────────────

#[test]
fn save_to_invalid_repo_path_errors() {
    let result = save_ignore_paths(
        "/tmp/__nonexistent_repo_path_12345__".to_string(),
        vec!["dist/**".to_string()],
    );
    assert!(result.is_err());
}

#[test]
fn save_to_non_git_directory_errors() {
    let tmp = tempfile::tempdir().unwrap();
    // No git init — just a plain directory
    let result = save_ignore_paths(
        tmp.path().to_str().unwrap().to_string(),
        vec!["dist/**".to_string()],
    );
    assert!(result.is_err());
}

// ── Glob Pattern Varieties ───────────────────────────────────────────

#[test]
fn save_and_load_various_glob_patterns() {
    let (_tmp, repo_path) = create_test_repo();

    let patterns = vec![
        "dist/**".to_string(),           // directory glob
        "**/*.generated.ts".to_string(), // extension glob with double star
        "*.lock".to_string(),             // root-level wildcard
        "migrations/**".to_string(),     // specific directory
        "src/**/*.test.ts".to_string(),  // nested with extension
        ".env*".to_string(),              // dotfile pattern
    ];

    save_ignore_paths(repo_path.clone(), patterns.clone()).unwrap();
    let loaded = get_ignore_paths(Some(repo_path)).unwrap();

    assert_eq!(loaded, patterns);
}

#[test]
fn save_duplicate_patterns_preserved() {
    let (_tmp, repo_path) = create_test_repo();

    // The command doesn't deduplicate — that's the UI's job
    let patterns = vec![
        "dist/**".to_string(),
        "dist/**".to_string(),
    ];

    save_ignore_paths(repo_path.clone(), patterns.clone()).unwrap();
    let loaded = get_ignore_paths(Some(repo_path)).unwrap();

    assert_eq!(loaded, patterns);
}

// ── Multiple Save Cycles ─────────────────────────────────────────────

#[test]
fn multiple_saves_each_replace_previous() {
    let (_tmp, repo_path) = create_test_repo();

    save_ignore_paths(repo_path.clone(), vec!["a/**".to_string()]).unwrap();
    assert_eq!(
        get_ignore_paths(Some(repo_path.clone())).unwrap(),
        vec!["a/**"]
    );

    save_ignore_paths(repo_path.clone(), vec!["b/**".to_string(), "c/**".to_string()]).unwrap();
    assert_eq!(
        get_ignore_paths(Some(repo_path.clone())).unwrap(),
        vec!["b/**", "c/**"]
    );

    save_ignore_paths(repo_path.clone(), vec![]).unwrap();
    assert!(get_ignore_paths(Some(repo_path)).unwrap().is_empty());
}

#[test]
fn save_then_add_incrementally() {
    let (_tmp, repo_path) = create_test_repo();

    // Simulate the UI pattern: load existing, append, save
    save_ignore_paths(repo_path.clone(), vec!["dist/**".to_string()]).unwrap();

    let mut current = get_ignore_paths(Some(repo_path.clone())).unwrap();
    current.push("build/**".to_string());
    save_ignore_paths(repo_path.clone(), current).unwrap();

    let loaded = get_ignore_paths(Some(repo_path)).unwrap();
    assert_eq!(loaded, vec!["dist/**", "build/**"]);
}

#[test]
fn save_then_remove_one() {
    let (_tmp, repo_path) = create_test_repo();

    save_ignore_paths(
        repo_path.clone(),
        vec!["a/**".to_string(), "b/**".to_string(), "c/**".to_string()],
    )
    .unwrap();

    // Simulate the UI pattern: load, filter out one, save
    let mut current = get_ignore_paths(Some(repo_path.clone())).unwrap();
    current.retain(|p| p != "b/**");
    save_ignore_paths(repo_path.clone(), current).unwrap();

    let loaded = get_ignore_paths(Some(repo_path)).unwrap();
    assert_eq!(loaded, vec!["a/**", "c/**"]);
}

// ── Config File Content ──────────────────────────────────────────────

#[test]
fn config_file_contains_ignore_paths() {
    let (_tmp, repo_path) = create_test_repo();

    save_ignore_paths(
        repo_path.clone(),
        vec!["dist/**".to_string(), "*.lock".to_string()],
    )
    .unwrap();

    let config_path = PathBuf::from(&repo_path).join(".flowdiff.toml");
    let content = std::fs::read_to_string(&config_path).unwrap();

    assert!(
        content.contains("[ignore]"),
        "Config should have [ignore] section, got:\n{}",
        content
    );
    assert!(
        content.contains("dist/**"),
        "Config should contain dist/** pattern, got:\n{}",
        content
    );
    assert!(
        content.contains("*.lock"),
        "Config should contain *.lock pattern, got:\n{}",
        content
    );
}
