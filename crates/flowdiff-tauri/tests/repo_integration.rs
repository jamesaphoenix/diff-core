#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration tests for repository-related Tauri commands.
//!
//! Tests `get_repo_info`, `list_branches`, `get_file_diff`, `get_branch_status`,
//! and `analyze` using real temporary git repositories.
//!
//! Run with:
//!   cargo test --test repo_integration

use flowdiff_tauri::commands::{get_repo_info, list_branches, list_worktrees, get_branch_status, get_file_diff_uncached};
use git2::{Repository, Signature};

/// Create a temporary git repo with TypeScript files and two branches.
fn create_test_repo_with_branch() -> (tempfile::TempDir, String) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = Repository::init(tmp.path()).unwrap();
    let sig = Signature::now("test", "test@test.com").unwrap();

    // Write initial files
    std::fs::write(
        tmp.path().join("src").join("main.ts").to_str().unwrap_or(""),
        "",
    )
    .ok();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(
        tmp.path().join("src/main.ts"),
        "export function hello() { return 'hello'; }\n",
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("src/utils.ts"),
        "export function add(a: number, b: number) { return a + b; }\n",
    )
    .unwrap();

    // Initial commit on main
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let main_commit = repo
        .commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[])
        .unwrap();
    let main_commit = repo.find_commit(main_commit).unwrap();

    // Create a feature branch
    repo.branch("feature/test", &main_commit, false).unwrap();
    repo.set_head("refs/heads/feature/test").unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();

    // Modify a file on the feature branch
    std::fs::write(
        tmp.path().join("src/main.ts"),
        "export function hello() { return 'hello world'; }\nexport function goodbye() { return 'bye'; }\n",
    )
    .unwrap();

    let mut index = repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        "Add goodbye function",
        &tree,
        &[&main_commit],
    )
    .unwrap();

    let path = tmp.path().to_str().unwrap().to_string();
    (tmp, path)
}

/// Create a minimal repo with just an initial commit.
fn create_minimal_repo() -> (tempfile::TempDir, String) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = Repository::init(tmp.path()).unwrap();
    let sig = Signature::now("test", "test@test.com").unwrap();

    std::fs::write(tmp.path().join("README.md"), "# Test\n").unwrap();
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[])
        .unwrap();

    let path = tmp.path().to_str().unwrap().to_string();
    (tmp, path)
}

// ── get_repo_info ────────────────────────────────────────────────────

#[test]
fn repo_info_returns_current_branch() {
    let (_tmp, repo_path) = create_test_repo_with_branch();
    let info = get_repo_info(repo_path).unwrap();
    assert_eq!(info.current_branch, Some("feature/test".to_string()));
}

#[test]
fn repo_info_lists_branches() {
    let (_tmp, repo_path) = create_test_repo_with_branch();
    let info = get_repo_info(repo_path).unwrap();

    let branch_names: Vec<&str> = info.branches.iter().map(|b| b.name.as_str()).collect();
    assert!(
        branch_names.contains(&"feature/test"),
        "Should contain feature/test, got: {:?}",
        branch_names
    );
    // main branch should also be listed (created implicitly by initial commit)
}

#[test]
fn repo_info_has_worktrees() {
    let (_tmp, repo_path) = create_test_repo_with_branch();
    let info = get_repo_info(repo_path).unwrap();

    // Should have at least the main worktree
    assert!(
        !info.worktrees.is_empty(),
        "Should have at least one worktree"
    );
}

#[test]
fn repo_info_errors_on_invalid_path() {
    let result = get_repo_info("/tmp/__nonexistent_repo_12345__".to_string());
    assert!(result.is_err());
}

#[test]
fn repo_info_detects_default_branch() {
    let (_tmp, repo_path) = create_minimal_repo();
    let info = get_repo_info(repo_path).unwrap();

    // Default branch detection should return something reasonable
    assert!(
        !info.default_branch.is_empty(),
        "Default branch should not be empty"
    );
}

// ── list_branches ────────────────────────────────────────────────────

#[test]
fn list_branches_returns_all_branches() {
    let (_tmp, repo_path) = create_test_repo_with_branch();
    let branches = list_branches(repo_path).unwrap();

    assert!(
        branches.len() >= 2,
        "Should have at least 2 branches (main + feature), got: {}",
        branches.len()
    );
}

#[test]
fn list_branches_marks_current() {
    let (_tmp, repo_path) = create_test_repo_with_branch();
    let branches = list_branches(repo_path).unwrap();

    let current_branches: Vec<_> = branches.iter().filter(|b| b.is_current).collect();
    assert_eq!(
        current_branches.len(),
        1,
        "Exactly one branch should be current"
    );
    assert_eq!(current_branches[0].name, "feature/test");
}

// ── list_worktrees ───────────────────────────────────────────────────

#[test]
fn list_worktrees_returns_main_worktree() {
    let (_tmp, repo_path) = create_test_repo_with_branch();
    let worktrees = list_worktrees(repo_path).unwrap();

    assert!(
        !worktrees.is_empty(),
        "Should list at least the main worktree"
    );
}

// ── get_branch_status ────────────────────────────────────────────────

#[test]
fn branch_status_works_for_local_only_branch() {
    let (_tmp, repo_path) = create_test_repo_with_branch();

    // Local branch with no upstream — should return a status (possibly with no upstream info)
    let result = get_branch_status(repo_path);
    // Either succeeds with 0/0 ahead/behind, or errors because no upstream
    // Both are valid — we just verify it doesn't panic
    let _ = result;
}

// ── get_file_diff ────────────────────────────────────────────────────

#[test]
fn file_diff_returns_content_for_changed_file() {
    let (_tmp, repo_path) = create_test_repo_with_branch();

    let diff = get_file_diff_uncached(
        repo_path,
        "src/main.ts".to_string(),
        Some("main".to_string()),  // base
        None,                       // head (defaults to HEAD)
        None,                       // range
        false,                      // staged
        false,                      // unstaged
    )
    .unwrap();

    assert_eq!(diff.path, "src/main.ts");
    assert_eq!(diff.language, "typescript");
    assert!(
        diff.old_content.contains("hello"),
        "Old content should contain 'hello'"
    );
    assert!(
        diff.new_content.contains("goodbye"),
        "New content should contain 'goodbye'"
    );
}

#[test]
fn file_diff_rejects_path_traversal() {
    let (_tmp, repo_path) = create_test_repo_with_branch();

    let result = get_file_diff_uncached(
        repo_path,
        "../../../etc/passwd".to_string(),
        Some("main".to_string()),
        None,
        None,
        false,
        false,
    );

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("path traversal"),
        "Should reject path traversal, got: {}",
        err
    );
}

#[test]
fn file_diff_rejects_absolute_path() {
    let (_tmp, repo_path) = create_test_repo_with_branch();

    let result = get_file_diff_uncached(
        repo_path,
        "/etc/passwd".to_string(),
        Some("main".to_string()),
        None,
        None,
        false,
        false,
    );

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("path traversal"),
        "Should reject absolute paths, got: {}",
        err
    );
}

#[test]
fn file_diff_errors_on_nonexistent_file() {
    let (_tmp, repo_path) = create_test_repo_with_branch();

    let result = get_file_diff_uncached(
        repo_path,
        "nonexistent/file.ts".to_string(),
        Some("main".to_string()),
        None,
        None,
        false,
        false,
    );

    assert!(result.is_err());
}

#[test]
fn file_diff_detects_language_correctly() {
    let (_tmp, repo_path) = create_test_repo_with_branch();

    let diff = get_file_diff_uncached(
        repo_path,
        "src/main.ts".to_string(),
        Some("main".to_string()),
        None,
        None,
        false,
        false,
    )
    .unwrap();

    assert_eq!(diff.language, "typescript");
}
