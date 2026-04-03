#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration tests for the review comment CRUD cycle.
//!
//! Tests the full lifecycle: save → load → delete → export
//! using real git repositories and the `.diffcore/comments.json` file.
//! Also tests branch-based comment caching in `~/.diffcore/cache/comments/`.
//!
//! Run with:
//!   cargo test --test comment_integration

use diffcore_tauri::commands::{
    delete_comment, delete_comment_cached, export_comments, load_comments,
    load_comments_cached, save_comment, save_comment_cached, ReviewComment,
};
use git2::{Repository, Signature};
use std::path::PathBuf;

/// Create a temporary git repo with an initial commit.
fn create_test_repo() -> (tempfile::TempDir, String) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = Repository::init(tmp.path()).unwrap();

    // Need at least one commit for the repo to be valid
    let sig = Signature::now("test", "test@test.com").unwrap();
    let tree_id = repo.index().unwrap().write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[])
        .unwrap();

    let path = tmp.path().to_str().unwrap().to_string();
    (tmp, path)
}

fn make_comment(id: &str, comment_type: &str, text: &str) -> ReviewComment {
    ReviewComment {
        id: id.to_string(),
        comment_type: comment_type.to_string(),
        group_id: "group_1".to_string(),
        file_path: Some("src/main.ts".to_string()),
        start_line: Some(10),
        end_line: Some(15),
        selected_code: Some("const x = 1;".to_string()),
        text: text.to_string(),
        created_at: "2026-03-21T10:00:00Z".to_string(),
    }
}

// ── Save & Load ──────────────────────────────────────────────────────

#[test]
fn save_and_load_single_comment() {
    let (_tmp, repo_path) = create_test_repo();
    let hash = "abc123".to_string();

    let comment = make_comment("c1", "code", "Needs error handling");
    save_comment(repo_path.clone(), hash.clone(), comment.clone()).unwrap();

    let loaded = load_comments(repo_path, hash).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, "c1");
    assert_eq!(loaded[0].text, "Needs error handling");
    assert_eq!(loaded[0].start_line, Some(10));
    assert_eq!(loaded[0].end_line, Some(15));
    assert_eq!(loaded[0].selected_code, Some("const x = 1;".to_string()));
}

#[test]
fn save_multiple_comments_preserves_order() {
    let (_tmp, repo_path) = create_test_repo();
    let hash = "abc123".to_string();

    save_comment(
        repo_path.clone(),
        hash.clone(),
        make_comment("c1", "code", "First"),
    )
    .unwrap();
    save_comment(
        repo_path.clone(),
        hash.clone(),
        make_comment("c2", "file", "Second"),
    )
    .unwrap();
    save_comment(
        repo_path.clone(),
        hash.clone(),
        make_comment("c3", "group", "Third"),
    )
    .unwrap();

    let loaded = load_comments(repo_path, hash).unwrap();
    assert_eq!(loaded.len(), 3);
    assert_eq!(loaded[0].id, "c1");
    assert_eq!(loaded[1].id, "c2");
    assert_eq!(loaded[2].id, "c3");
}

#[test]
fn load_with_wrong_hash_returns_empty() {
    let (_tmp, repo_path) = create_test_repo();

    save_comment(
        repo_path.clone(),
        "hash_a".to_string(),
        make_comment("c1", "code", "Comment for hash A"),
    )
    .unwrap();

    // Loading with a different hash should return empty (fresh analysis)
    let loaded = load_comments(repo_path, "hash_b".to_string()).unwrap();
    assert!(
        loaded.is_empty(),
        "Different hash should yield empty comments"
    );
}

#[test]
fn load_from_nonexistent_file_returns_empty() {
    let (_tmp, repo_path) = create_test_repo();

    // No comments saved yet — should return empty, not error
    let loaded = load_comments(repo_path, "any_hash".to_string()).unwrap();
    assert!(loaded.is_empty());
}

// ── Delete ───────────────────────────────────────────────────────────

#[test]
fn delete_removes_specific_comment() {
    let (_tmp, repo_path) = create_test_repo();
    let hash = "abc123".to_string();

    save_comment(
        repo_path.clone(),
        hash.clone(),
        make_comment("c1", "code", "Keep"),
    )
    .unwrap();
    save_comment(
        repo_path.clone(),
        hash.clone(),
        make_comment("c2", "code", "Delete me"),
    )
    .unwrap();
    save_comment(
        repo_path.clone(),
        hash.clone(),
        make_comment("c3", "code", "Keep too"),
    )
    .unwrap();

    delete_comment(repo_path.clone(), hash.clone(), "c2".to_string()).unwrap();

    let loaded = load_comments(repo_path, hash).unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].id, "c1");
    assert_eq!(loaded[1].id, "c3");
}

#[test]
fn delete_nonexistent_comment_is_noop() {
    let (_tmp, repo_path) = create_test_repo();
    let hash = "abc123".to_string();

    save_comment(
        repo_path.clone(),
        hash.clone(),
        make_comment("c1", "code", "Only one"),
    )
    .unwrap();

    // Deleting a comment that doesn't exist should succeed (no-op)
    delete_comment(
        repo_path.clone(),
        hash.clone(),
        "nonexistent_id".to_string(),
    )
    .unwrap();

    let loaded = load_comments(repo_path, hash).unwrap();
    assert_eq!(loaded.len(), 1);
}

#[test]
fn delete_all_comments_leaves_empty_list() {
    let (_tmp, repo_path) = create_test_repo();
    let hash = "abc123".to_string();

    save_comment(
        repo_path.clone(),
        hash.clone(),
        make_comment("c1", "code", "One"),
    )
    .unwrap();
    save_comment(
        repo_path.clone(),
        hash.clone(),
        make_comment("c2", "code", "Two"),
    )
    .unwrap();

    delete_comment(repo_path.clone(), hash.clone(), "c1".to_string()).unwrap();
    delete_comment(repo_path.clone(), hash.clone(), "c2".to_string()).unwrap();

    let loaded = load_comments(repo_path, hash).unwrap();
    assert!(loaded.is_empty());
}

// ── Export ────────────────────────────────────────────────────────────

#[test]
fn export_empty_comments_returns_empty_string() {
    let (_tmp, repo_path) = create_test_repo();

    let result = export_comments(repo_path, "hash_empty".to_string()).unwrap();
    assert!(
        result.is_empty(),
        "Exporting no comments should return empty string, got: '{}'",
        result
    );
}

#[test]
fn export_includes_comment_text() {
    let (_tmp, repo_path) = create_test_repo();
    let hash = "abc123".to_string();

    save_comment(
        repo_path.clone(),
        hash.clone(),
        make_comment("c1", "code", "This needs validation"),
    )
    .unwrap();

    let exported = export_comments(repo_path, hash).unwrap();
    assert!(
        exported.contains("This needs validation"),
        "Export should include comment text, got: '{}'",
        exported
    );
}

#[test]
fn export_includes_file_path() {
    let (_tmp, repo_path) = create_test_repo();
    let hash = "abc123".to_string();

    save_comment(
        repo_path.clone(),
        hash.clone(),
        make_comment("c1", "code", "Check this"),
    )
    .unwrap();

    let exported = export_comments(repo_path, hash).unwrap();
    assert!(
        exported.contains("src/main.ts"),
        "Export should include file path, got: '{}'",
        exported
    );
}

#[test]
fn export_includes_code_snippet() {
    let (_tmp, repo_path) = create_test_repo();
    let hash = "abc123".to_string();

    save_comment(
        repo_path.clone(),
        hash.clone(),
        make_comment("c1", "code", "Review this"),
    )
    .unwrap();

    let exported = export_comments(repo_path, hash).unwrap();
    assert!(
        exported.contains("const x = 1;"),
        "Export should include selected code, got: '{}'",
        exported
    );
}

// ── File Persistence ─────────────────────────────────────────────────

#[test]
fn comments_persisted_to_diffcore_directory() {
    let (_tmp, repo_path) = create_test_repo();
    let hash = "abc123".to_string();

    save_comment(
        repo_path.clone(),
        hash,
        make_comment("c1", "code", "Persisted"),
    )
    .unwrap();

    // Verify the .diffcore/comments.json file was created
    let comments_path = PathBuf::from(&repo_path)
        .join(".diffcore")
        .join("comments.json");
    assert!(
        comments_path.exists(),
        "comments.json should exist at {:?}",
        comments_path
    );

    // Verify it's valid JSON
    let content = std::fs::read_to_string(&comments_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(parsed.is_object());
    assert!(parsed["comments"].is_array());
    assert_eq!(parsed["comments"].as_array().unwrap().len(), 1);
}

// ── Comment Types ────────────────────────────────────────────────────

#[test]
fn all_comment_types_roundtrip() {
    let (_tmp, repo_path) = create_test_repo();
    let hash = "abc123".to_string();

    // Save one of each type
    let code_comment = make_comment("c1", "code", "Code comment");
    let mut file_comment = make_comment("c2", "file", "File comment");
    file_comment.start_line = None;
    file_comment.end_line = None;
    file_comment.selected_code = None;

    let mut group_comment = make_comment("c3", "group", "Group comment");
    group_comment.file_path = None;
    group_comment.start_line = None;
    group_comment.end_line = None;
    group_comment.selected_code = None;

    save_comment(repo_path.clone(), hash.clone(), code_comment).unwrap();
    save_comment(repo_path.clone(), hash.clone(), file_comment).unwrap();
    save_comment(repo_path.clone(), hash.clone(), group_comment).unwrap();

    let loaded = load_comments(repo_path, hash).unwrap();
    assert_eq!(loaded.len(), 3);
    assert_eq!(loaded[0].comment_type, "code");
    assert_eq!(loaded[0].start_line, Some(10));
    assert_eq!(loaded[1].comment_type, "file");
    assert_eq!(loaded[1].start_line, None);
    assert_eq!(loaded[2].comment_type, "group");
    assert_eq!(loaded[2].file_path, None);
}

// ── Error Cases ──────────────────────────────────────────────────────

#[test]
fn save_comment_to_invalid_repo_path_errors() {
    let result = save_comment(
        "/tmp/__nonexistent_repo_path_12345__".to_string(),
        "hash".to_string(),
        make_comment("c1", "code", "Should fail"),
    );
    assert!(result.is_err());
}

#[test]
fn load_comments_from_invalid_repo_path_errors() {
    let result = load_comments(
        "/tmp/__nonexistent_repo_path_12345__".to_string(),
        "hash".to_string(),
    );
    assert!(result.is_err());
}

// ══════════════════════════════════════════════════════════════════════
// Branch-based comment cache tests (~/.diffcore/cache/comments/)
// ══════════════════════════════════════════════════════════════════════

/// Helper: create a test repo on a specific branch.
fn create_test_repo_on_branch(branch_name: &str) -> (tempfile::TempDir, String) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = Repository::init(tmp.path()).unwrap();
    let sig = Signature::now("test", "test@test.com").unwrap();

    std::fs::write(tmp.path().join("file.txt"), "content").unwrap();
    let mut index = repo.index().unwrap();
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let commit = repo
        .commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[])
        .unwrap();

    // Create and checkout the target branch (skip if already on it)
    let commit = repo.find_commit(commit).unwrap();
    let current_branch = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(str::to_owned));
    if current_branch.as_deref() != Some(branch_name) {
        repo.branch(branch_name, &commit, false).unwrap();
        repo.set_head(&format!("refs/heads/{}", branch_name))
            .unwrap();
    }

    let path = tmp.path().to_str().unwrap().to_string();
    (tmp, path)
}

/// Helper: set the cache dir env var for isolated testing.
fn with_comment_cache_dir<F: FnOnce()>(dir: &std::path::Path, f: F) {
    std::env::set_var("DIFFCORE_COMMENT_CACHE_DIR", dir.as_os_str());
    f();
    std::env::remove_var("DIFFCORE_COMMENT_CACHE_DIR");
}

// ── Save & Load by branch ───────────────────────────────────────────

#[test]
fn cached_save_and_load_single_comment() {
    let cache_tmp = tempfile::tempdir().unwrap();
    let (_repo_tmp, repo_path) = create_test_repo_on_branch("feature-a");

    with_comment_cache_dir(cache_tmp.path(), || {
        let comment = make_comment("c1", "code", "Branch comment");
        save_comment_cached(repo_path.clone(), comment.clone()).unwrap();

        let loaded = load_comments_cached(repo_path.clone()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "c1");
        assert_eq!(loaded[0].text, "Branch comment");
    });
}

#[test]
fn cached_comments_isolated_by_branch() {
    let cache_tmp = tempfile::tempdir().unwrap();
    let (_repo_tmp, repo_path) = create_test_repo_on_branch("branch-a");

    with_comment_cache_dir(cache_tmp.path(), || {
        // Save on branch-a
        save_comment_cached(
            repo_path.clone(),
            make_comment("c1", "code", "On branch A"),
        )
        .unwrap();

        // Switch to branch-b
        let repo = Repository::discover(&repo_path).unwrap();
        let sig = Signature::now("test", "test@test.com").unwrap();
        let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("branch-b", &head_commit, true).unwrap();
        repo.set_head("refs/heads/branch-b").unwrap();

        // Save on branch-b
        save_comment_cached(
            repo_path.clone(),
            make_comment("c2", "code", "On branch B"),
        )
        .unwrap();

        // Load on branch-b — should only see branch-b comments
        let loaded_b = load_comments_cached(repo_path.clone()).unwrap();
        assert_eq!(loaded_b.len(), 1);
        assert_eq!(loaded_b[0].id, "c2");
        assert_eq!(loaded_b[0].text, "On branch B");

        // Switch back to branch-a
        repo.set_head("refs/heads/branch-a").unwrap();

        // Load on branch-a — should only see branch-a comments
        let loaded_a = load_comments_cached(repo_path.clone()).unwrap();
        assert_eq!(loaded_a.len(), 1);
        assert_eq!(loaded_a[0].id, "c1");
        assert_eq!(loaded_a[0].text, "On branch A");
    });
}

#[test]
fn cached_comments_persist_across_reload() {
    let cache_tmp = tempfile::tempdir().unwrap();
    let (_repo_tmp, repo_path) = create_test_repo_on_branch("persist-test");

    with_comment_cache_dir(cache_tmp.path(), || {
        save_comment_cached(
            repo_path.clone(),
            make_comment("c1", "code", "Persisted"),
        )
        .unwrap();
        save_comment_cached(
            repo_path.clone(),
            make_comment("c2", "file", "Also persisted"),
        )
        .unwrap();
    });

    // Simulate app restart: re-enter the cache dir context
    with_comment_cache_dir(cache_tmp.path(), || {
        let loaded = load_comments_cached(repo_path.clone()).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].text, "Persisted");
        assert_eq!(loaded[1].text, "Also persisted");
    });
}

#[test]
fn cached_delete_removes_specific_comment() {
    let cache_tmp = tempfile::tempdir().unwrap();
    let (_repo_tmp, repo_path) = create_test_repo_on_branch("delete-test");

    with_comment_cache_dir(cache_tmp.path(), || {
        save_comment_cached(repo_path.clone(), make_comment("c1", "code", "Keep")).unwrap();
        save_comment_cached(repo_path.clone(), make_comment("c2", "code", "Delete me")).unwrap();
        save_comment_cached(repo_path.clone(), make_comment("c3", "code", "Keep too")).unwrap();

        delete_comment_cached(repo_path.clone(), "c2".to_string()).unwrap();

        let loaded = load_comments_cached(repo_path.clone()).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "c1");
        assert_eq!(loaded[1].id, "c3");
    });
}

#[test]
fn cached_delete_nonexistent_is_noop() {
    let cache_tmp = tempfile::tempdir().unwrap();
    let (_repo_tmp, repo_path) = create_test_repo_on_branch("delete-noop");

    with_comment_cache_dir(cache_tmp.path(), || {
        save_comment_cached(repo_path.clone(), make_comment("c1", "code", "Only one")).unwrap();
        delete_comment_cached(repo_path.clone(), "nonexistent".to_string()).unwrap();

        let loaded = load_comments_cached(repo_path.clone()).unwrap();
        assert_eq!(loaded.len(), 1);
    });
}

#[test]
fn cached_worktree_shares_comments_with_main_repo() {
    let cache_tmp = tempfile::tempdir().unwrap();
    let (_repo_tmp, repo_path) = create_test_repo_on_branch("main");
    // Use a subdirectory inside a tempdir for the worktree path (must not exist yet)
    let wt_parent = tempfile::tempdir().unwrap();
    let wt_dir = wt_parent.path().join("wt-checkout");

    with_comment_cache_dir(cache_tmp.path(), || {
        // Create a worktree on a new branch (don't force — "main" is HEAD)
        let repo = Repository::discover(&repo_path).unwrap();
        let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
        let branch = repo.branch("wt-branch", &head_commit, false).unwrap();
        let branch_ref = branch.into_reference();
        let branch_ref_name = branch_ref.name().unwrap();
        repo.worktree(
            "wt-test",
            &wt_dir,
            Some(
                git2::WorktreeAddOptions::new()
                    .reference(Some(&repo.find_reference(branch_ref_name).unwrap())),
            ),
        )
        .unwrap();

        let wt_path = wt_dir.to_str().unwrap().to_string();

        // Save a comment from the worktree
        save_comment_cached(wt_path.clone(), make_comment("c1", "code", "From worktree")).unwrap();

        // Load from worktree — should see it
        let loaded_wt = load_comments_cached(wt_path.clone()).unwrap();
        assert_eq!(loaded_wt.len(), 1);
        assert_eq!(loaded_wt[0].text, "From worktree");

        // The main repo on a DIFFERENT branch should NOT see the worktree's comments
        // (main is on "main", worktree is on "wt-branch")
        let loaded_main = load_comments_cached(repo_path.clone()).unwrap();
        assert!(
            loaded_main.is_empty(),
            "Main repo on 'main' branch should not see worktree 'wt-branch' comments"
        );
    });
}

#[test]
fn cached_multiple_comments_preserves_order() {
    let cache_tmp = tempfile::tempdir().unwrap();
    let (_repo_tmp, repo_path) = create_test_repo_on_branch("order-test");

    with_comment_cache_dir(cache_tmp.path(), || {
        save_comment_cached(repo_path.clone(), make_comment("c1", "code", "First")).unwrap();
        save_comment_cached(repo_path.clone(), make_comment("c2", "file", "Second")).unwrap();
        save_comment_cached(repo_path.clone(), make_comment("c3", "group", "Third")).unwrap();

        let loaded = load_comments_cached(repo_path.clone()).unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].id, "c1");
        assert_eq!(loaded[1].id, "c2");
        assert_eq!(loaded[2].id, "c3");
    });
}

#[test]
fn cached_load_from_empty_cache_returns_empty() {
    let cache_tmp = tempfile::tempdir().unwrap();
    let (_repo_tmp, repo_path) = create_test_repo_on_branch("empty-test");

    with_comment_cache_dir(cache_tmp.path(), || {
        let loaded = load_comments_cached(repo_path.clone()).unwrap();
        assert!(loaded.is_empty());
    });
}

#[test]
fn cached_all_comment_types_roundtrip() {
    let cache_tmp = tempfile::tempdir().unwrap();
    let (_repo_tmp, repo_path) = create_test_repo_on_branch("types-test");

    with_comment_cache_dir(cache_tmp.path(), || {
        let code_comment = make_comment("c1", "code", "Code comment");
        let mut file_comment = make_comment("c2", "file", "File comment");
        file_comment.start_line = None;
        file_comment.end_line = None;
        file_comment.selected_code = None;
        let mut group_comment = make_comment("c3", "group", "Group comment");
        group_comment.file_path = None;
        group_comment.start_line = None;
        group_comment.end_line = None;
        group_comment.selected_code = None;

        save_comment_cached(repo_path.clone(), code_comment).unwrap();
        save_comment_cached(repo_path.clone(), file_comment).unwrap();
        save_comment_cached(repo_path.clone(), group_comment).unwrap();

        let loaded = load_comments_cached(repo_path.clone()).unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[0].comment_type, "code");
        assert_eq!(loaded[0].start_line, Some(10));
        assert_eq!(loaded[1].comment_type, "file");
        assert_eq!(loaded[1].start_line, None);
        assert_eq!(loaded[2].comment_type, "group");
        assert_eq!(loaded[2].file_path, None);
    });
}
