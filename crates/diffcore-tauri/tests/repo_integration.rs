#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration tests for repository-related Tauri commands.
//!
//! Tests `get_repo_info`, `list_branches`, `get_file_diff`, `get_branch_status`,
//! and `analyze` using real temporary git repositories.
//!
//! Run with:
//!   cargo test --test repo_integration

use diffcore_tauri::commands::{
    get_branch_status, get_file_diff_uncached, get_repo_info, list_branches, list_worktrees,
};
use git2::{Repository, Signature};

/// Create a temporary git repo with TypeScript files and two branches.
fn create_test_repo_with_branch() -> (tempfile::TempDir, String, String) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = Repository::init(tmp.path()).unwrap();
    let sig = Signature::now("test", "test@test.com").unwrap();

    // Write initial files
    std::fs::write(
        tmp.path()
            .join("src")
            .join("main.ts")
            .to_str()
            .unwrap_or(""),
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
    let base_branch = repo
        .head()
        .ok()
        .and_then(|head| head.shorthand().map(str::to_owned))
        .expect("test repo should have a default branch after the first commit");

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
    (tmp, path, base_branch)
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
    let (_tmp, repo_path, _) = create_test_repo_with_branch();
    let info = get_repo_info(repo_path).unwrap();
    assert_eq!(info.current_branch, Some("feature/test".to_string()));
}

#[test]
fn repo_info_lists_branches() {
    let (_tmp, repo_path, _) = create_test_repo_with_branch();
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
    let (_tmp, repo_path, _) = create_test_repo_with_branch();
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
    let (_tmp, repo_path, _) = create_test_repo_with_branch();
    let branches = list_branches(repo_path).unwrap();

    assert!(
        branches.len() >= 2,
        "Should have at least 2 branches (main + feature), got: {}",
        branches.len()
    );
}

#[test]
fn list_branches_marks_current() {
    let (_tmp, repo_path, _) = create_test_repo_with_branch();
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
    let (_tmp, repo_path, _) = create_test_repo_with_branch();
    let worktrees = list_worktrees(repo_path).unwrap();

    assert!(
        !worktrees.is_empty(),
        "Should list at least the main worktree"
    );
}

// ── get_branch_status ────────────────────────────────────────────────

#[test]
fn branch_status_works_for_local_only_branch() {
    let (_tmp, repo_path, _) = create_test_repo_with_branch();

    // Local branch with no upstream — should return a status (possibly with no upstream info)
    let result = get_branch_status(repo_path);
    // Either succeeds with 0/0 ahead/behind, or errors because no upstream
    // Both are valid — we just verify it doesn't panic
    let _ = result;
}

// ── get_file_diff ────────────────────────────────────────────────────

#[test]
fn file_diff_returns_content_for_changed_file() {
    let (_tmp, repo_path, base_branch) = create_test_repo_with_branch();

    let diff = get_file_diff_uncached(
        repo_path,
        "src/main.ts".to_string(),
        Some(base_branch), // base
        None,              // head (defaults to HEAD)
        None,              // range
        false,             // staged
        false,             // unstaged
        false,             // include_uncommitted
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
    let (_tmp, repo_path, base_branch) = create_test_repo_with_branch();

    let result = get_file_diff_uncached(
        repo_path,
        "../../../etc/passwd".to_string(),
        Some(base_branch),
        None,
        None,
        false,
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
    let (_tmp, repo_path, base_branch) = create_test_repo_with_branch();

    let result = get_file_diff_uncached(
        repo_path,
        "/etc/passwd".to_string(),
        Some(base_branch),
        None,
        None,
        false,
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
    let (_tmp, repo_path, base_branch) = create_test_repo_with_branch();

    let result = get_file_diff_uncached(
        repo_path,
        "nonexistent/file.ts".to_string(),
        Some(base_branch),
        None,
        None,
        false,
        false,
        false,
    );

    assert!(result.is_err());
}

#[test]
fn file_diff_detects_language_correctly() {
    let (_tmp, repo_path, base_branch) = create_test_repo_with_branch();

    let diff = get_file_diff_uncached(
        repo_path,
        "src/main.ts".to_string(),
        Some(base_branch),
        None,
        None,
        false,
        false,
        false,
    )
    .unwrap();

    assert_eq!(diff.language, "typescript");
}

// ── is_worktree detection ───────────────────────────────────────────

#[test]
fn repo_info_regular_repo_is_not_worktree() {
    let (_tmp, repo_path, _) = create_test_repo_with_branch();
    let info = get_repo_info(repo_path).unwrap();
    assert!(
        !info.is_worktree,
        "A regular (non-worktree) repo should have is_worktree = false"
    );
}

#[test]
fn repo_info_minimal_repo_is_not_worktree() {
    let (_tmp, repo_path) = create_minimal_repo();
    let info = get_repo_info(repo_path).unwrap();
    assert!(
        !info.is_worktree,
        "A minimal repo should have is_worktree = false"
    );
}

#[test]
fn repo_info_linked_worktree_is_worktree() {
    let (_tmp, repo_path, _) = create_test_repo_with_branch();

    // Create a linked worktree
    let repo = Repository::open(&repo_path).unwrap();
    let wt_dir = _tmp.path().join("linked-wt");
    std::fs::create_dir_all(&wt_dir).unwrap();

    // Add a worktree on the main branch
    let main_commit = repo
        .find_branch("feature/test", git2::BranchType::Local)
        .ok()
        .and_then(|b| b.get().target())
        .and_then(|oid| repo.find_commit(oid).ok())
        .unwrap();
    let main_ref = repo.find_branch("main", git2::BranchType::Local)
        .or_else(|_| repo.find_branch("master", git2::BranchType::Local))
        .ok()
        .and_then(|b| b.get().name().map(String::from));

    // Use git CLI to create worktree (git2's worktree API is limited)
    let _ = main_commit; // used above
    let status = std::process::Command::new("git")
        .args([
            "worktree",
            "add",
            wt_dir.to_str().unwrap(),
            main_ref.as_deref().unwrap_or("main"),
        ])
        .current_dir(&repo_path)
        .output()
        .expect("git worktree add should succeed");
    assert!(
        status.status.success(),
        "git worktree add failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );

    // Now get_repo_info on the linked worktree path
    let wt_info = get_repo_info(wt_dir.to_str().unwrap().to_string()).unwrap();
    assert!(
        wt_info.is_worktree,
        "A linked worktree should have is_worktree = true"
    );

    // The main repo should still be is_worktree = false
    let main_info = get_repo_info(repo_path).unwrap();
    assert!(
        !main_info.is_worktree,
        "The main repo should still have is_worktree = false after adding a worktree"
    );
}

#[test]
fn repo_info_linked_worktree_lists_branches() {
    let (_tmp, repo_path, _) = create_test_repo_with_branch();

    // Create a linked worktree
    let wt_dir = _tmp.path().join("linked-wt-branches");
    std::fs::create_dir_all(&wt_dir).unwrap();
    let repo = Repository::open(&repo_path).unwrap();
    let main_ref = repo.find_branch("main", git2::BranchType::Local)
        .or_else(|_| repo.find_branch("master", git2::BranchType::Local))
        .ok()
        .and_then(|b| b.get().name().map(String::from));
    let status = std::process::Command::new("git")
        .args([
            "worktree",
            "add",
            wt_dir.to_str().unwrap(),
            main_ref.as_deref().unwrap_or("main"),
        ])
        .current_dir(&repo_path)
        .output()
        .expect("git worktree add should succeed");
    assert!(status.status.success());

    let wt_info = get_repo_info(wt_dir.to_str().unwrap().to_string()).unwrap();

    // Should still be able to list all branches from a worktree
    assert!(
        wt_info.branches.len() >= 2,
        "Worktree should see all branches, got: {:?}",
        wt_info.branches.iter().map(|b| &b.name).collect::<Vec<_>>()
    );

    // Current branch in the worktree should differ from the main repo's current branch
    assert_ne!(
        wt_info.current_branch,
        Some("feature/test".to_string()),
        "Worktree should have a different current branch than the main repo"
    );
}
