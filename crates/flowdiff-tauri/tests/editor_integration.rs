#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Integration tests for editor-related Tauri commands.
//!
//! Tests the full flow of:
//! - Editor detection (`check_editors_available`)
//! - File opening (`open_in_editor`) with various editors and edge cases
//! - macOS app bundle detection (on macOS only)
//!
//! Run with:
//!   cargo test --test editor_integration

use flowdiff_tauri::commands::{check_editors_available, open_in_editor};

// ── Editor Detection ─────────────────────────────────────────────────

#[test]
fn editor_detection_returns_all_five_ids() {
    let result = check_editors_available();
    let expected_ids = ["vscode", "cursor", "zed", "vim", "terminal"];
    for id in &expected_ids {
        assert!(
            result.contains_key(*id),
            "check_editors_available missing key: {}",
            id
        );
    }
}

#[test]
fn editor_detection_terminal_always_available() {
    let result = check_editors_available();
    assert_eq!(
        result["terminal"], true,
        "terminal should always be available"
    );
}

#[test]
fn editor_detection_values_are_booleans() {
    let result = check_editors_available();
    // Every value must be a bool (true/false), which is guaranteed by the type,
    // but we verify the map is non-empty and well-formed.
    assert_eq!(result.len(), 5);
    for (id, available) in &result {
        // Just verify we can read them without panic
        let _ = format!("{}: {}", id, available);
    }
}

#[test]
fn editor_detection_is_consistent_across_calls() {
    // Two successive calls should return the same result (no flaky detection)
    let result1 = check_editors_available();
    let result2 = check_editors_available();
    assert_eq!(result1, result2, "Editor detection should be deterministic");
}

// ── macOS App Bundle Detection ───────────────────────────────────────

#[cfg(target_os = "macos")]
mod macos_detection {
    use flowdiff_tauri::commands::check_editors_available;

    #[test]
    fn detects_at_least_terminal() {
        let result = check_editors_available();
        // On any macOS machine, terminal should be available
        assert!(result["terminal"]);
    }

    #[test]
    fn detection_agrees_with_filesystem() {
        let result = check_editors_available();

        // Cross-check: if VS Code app exists, detection should agree
        let vscode_exists = std::path::Path::new("/Applications/Visual Studio Code.app").exists();
        if vscode_exists {
            assert!(
                result["vscode"],
                "VS Code app bundle exists but detection says false"
            );
        }

        let cursor_exists = std::path::Path::new("/Applications/Cursor.app").exists();
        if cursor_exists {
            assert!(
                result["cursor"],
                "Cursor app bundle exists but detection says false"
            );
        }

        let zed_exists = std::path::Path::new("/Applications/Zed.app").exists();
        if zed_exists {
            assert!(
                result["zed"],
                "Zed app bundle exists but detection says false"
            );
        }
    }
}

// ── Open In Editor ───────────────────────────────────────────────────

#[test]
fn open_in_editor_rejects_nonexistent_file() {
    let result = open_in_editor(
        "vscode".to_string(),
        "/tmp/__flowdiff_integration_test_nonexistent_12345__".to_string(),
    );
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("File not found"),
        "Expected file-not-found error, got: {}",
        err
    );
}

#[test]
fn open_in_editor_rejects_unknown_editor() {
    let tmp = std::env::temp_dir().join("flowdiff_inttest_unknown_editor");
    std::fs::write(&tmp, "test content").unwrap();

    let result = open_in_editor(
        "nonexistent_editor_xyz".to_string(),
        tmp.to_str().unwrap().to_string(),
    );
    std::fs::remove_file(&tmp).ok();

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("Unknown editor"),
        "Expected unknown-editor error, got: {}",
        err
    );
}

#[test]
fn open_in_editor_accepts_all_known_editors_with_valid_file() {
    let tmp = std::env::temp_dir().join("flowdiff_inttest_known_editors");
    std::fs::write(&tmp, "test content").unwrap();
    let path = tmp.to_str().unwrap().to_string();

    // Each known editor should either succeed (editor installed) or fail with
    // a launch error — but never "Unknown editor"
    for editor in &["vscode", "cursor", "zed", "vim", "terminal"] {
        let result = open_in_editor(editor.to_string(), path.clone());
        if let Err(e) = &result {
            let msg = e.to_string();
            assert!(
                !msg.contains("Unknown editor"),
                "'{}' incorrectly treated as unknown: {}",
                editor,
                msg
            );
        }
    }

    std::fs::remove_file(&tmp).ok();
}

#[test]
fn open_in_editor_with_directory_path_for_terminal() {
    // Terminal editor should accept directory paths (opens terminal in that dir)
    let dir = std::env::temp_dir();
    let result = open_in_editor(
        "terminal".to_string(),
        dir.to_str().unwrap().to_string(),
    );
    // Should succeed (terminal is always available)
    assert!(result.is_ok(), "Terminal should open directory: {:?}", result);
}

#[test]
fn open_in_editor_with_spaces_in_path() {
    let dir = std::env::temp_dir().join("flowdiff inttest dir with spaces");
    std::fs::create_dir_all(&dir).ok();
    let file = dir.join("test file.txt");
    std::fs::write(&file, "test content with spaces").unwrap();

    // Should not crash or error on file-exists check
    let result = open_in_editor(
        "terminal".to_string(),
        file.to_str().unwrap().to_string(),
    );
    // Terminal opens the parent dir, should work
    assert!(
        result.is_ok(),
        "Terminal should handle paths with spaces: {:?}",
        result
    );

    std::fs::remove_file(&file).ok();
    std::fs::remove_dir(&dir).ok();
}

#[test]
fn open_in_editor_with_unicode_filename() {
    let tmp = std::env::temp_dir().join("flowdiff_inttest_ünîcödé.txt");
    std::fs::write(&tmp, "unicode content").unwrap();

    // Should not crash on unicode paths
    let result = open_in_editor(
        "terminal".to_string(),
        tmp.to_str().unwrap().to_string(),
    );
    assert!(
        result.is_ok(),
        "Terminal should handle unicode paths: {:?}",
        result
    );

    std::fs::remove_file(&tmp).ok();
}
