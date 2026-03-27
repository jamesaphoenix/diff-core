#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration tests for the review comment CRUD cycle.
//!
//! Tests the full lifecycle: save → load → delete → export
//! using real git repositories and the `.flowdiff/comments.json` file.
//!
//! Run with:
//!   cargo test --test comment_integration

use flowdiff_tauri::commands::{
    delete_comment, export_comments, load_comments, save_comment, ReviewComment,
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
fn comments_persisted_to_flowdiff_directory() {
    let (_tmp, repo_path) = create_test_repo();
    let hash = "abc123".to_string();

    save_comment(
        repo_path.clone(),
        hash,
        make_comment("c1", "code", "Persisted"),
    )
    .unwrap();

    // Verify the .flowdiff/comments.json file was created
    let comments_path = PathBuf::from(&repo_path)
        .join(".flowdiff")
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
