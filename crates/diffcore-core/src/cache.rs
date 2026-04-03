//! Analysis result caching based on diff content hashing.
//!
//! Caches the JSON-serialized `AnalysisOutput` keyed by a SHA-256 hash of the
//! diff inputs (base SHA, head SHA, sorted file paths). On subsequent runs with
//! the same diff, the cached result is returned instantly without re-parsing or
//! re-analyzing.
//!
//! Cache location: `<repo>/.diffcore/cache/` (gitignored by convention).

use log::warn;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use crate::git::DiffResult;
use crate::types::AnalysisOutput;

/// Compute a deterministic cache key from a diff result.
///
/// The key is a hex-encoded SHA-256 hash of:
/// - base_sha (or "none")
/// - head_sha (or "none")
/// - sorted file paths joined by newlines
///
/// This ensures the cache is invalidated when any file is added/removed
/// or when the base/head refs change.
pub fn compute_cache_key(diff_result: &DiffResult) -> String {
    let mut hasher = Sha256::new();

    hasher.update(diff_result.base_sha.as_deref().unwrap_or("none"));
    hasher.update(b"\n");
    hasher.update(diff_result.head_sha.as_deref().unwrap_or("none"));
    hasher.update(b"\n");

    let mut paths: Vec<&str> = diff_result.files.iter().map(|f| f.path()).collect();
    paths.sort();
    for path in &paths {
        hasher.update(path.as_bytes());
        hasher.update(b"\n");
    }

    hex::encode(hasher.finalize())
}

/// Resolve the cache directory for a repo working directory.
fn cache_dir(workdir: &Path) -> PathBuf {
    workdir.join(".diffcore").join("cache")
}

/// Try to load a cached analysis result for the given diff.
///
/// Returns `None` if no cache entry exists or if the entry is malformed.
/// Logs a warning if the cache file exists but cannot be deserialized.
pub fn load_cached(workdir: &Path, cache_key: &str) -> Option<AnalysisOutput> {
    let path = cache_dir(workdir).join(format!("{}.json", cache_key));
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return None, // File doesn't exist — normal cache miss
    };
    match serde_json::from_str(&content) {
        Ok(output) => Some(output),
        Err(e) => {
            warn!(
                "Cache entry at {} is malformed and will be ignored: {}",
                path.display(),
                e
            );
            None
        }
    }
}

/// Store an analysis result in the cache.
///
/// Creates the cache directory if it doesn't exist. Caching is best-effort
/// and should never block analysis — failures are logged as warnings.
pub fn store_cached(workdir: &Path, cache_key: &str, output: &AnalysisOutput) {
    let dir = cache_dir(workdir);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn!("Failed to create cache directory {}: {}", dir.display(), e);
        return;
    }

    let path = dir.join(format!("{}.json", cache_key));
    match serde_json::to_string(output) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                warn!("Failed to write cache entry {}: {}", path.display(), e);
            }
        }
        Err(e) => {
            warn!("Failed to serialize analysis for caching: {}", e);
        }
    }
}

/// Clear all cached analysis results for a repo.
pub fn clear_cache(workdir: &Path) -> std::io::Result<()> {
    let dir = cache_dir(workdir);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

// ── Global refinement cache (~/.diffcore/cache/refinements/) ──

/// Resolve the global refinement cache directory.
fn refinement_cache_dir() -> Option<PathBuf> {
    std::env::var_os("DIFFCORE_REFINEMENT_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| {
                PathBuf::from(home)
                    .join(".diffcore")
                    .join("cache")
                    .join("refinements")
            })
        })
}

/// Load a cached refinement result for the given analysis cache key.
///
/// Returns the raw JSON string so the caller can deserialize into its own type
/// (e.g. `RefinementResult` which lives in the Tauri crate, not core).
pub fn load_cached_refinement(cache_key: &str) -> Option<String> {
    let dir = refinement_cache_dir()?;
    let path = dir.join(format!("{}.json", cache_key));
    match std::fs::read_to_string(&path) {
        Ok(c) if !c.is_empty() => Some(c),
        Ok(_) => None,
        Err(_) => None,
    }
}

/// Store a refinement result in the global cache.
///
/// Accepts raw JSON so the Tauri crate can serialize its own `RefinementResult`.
pub fn store_cached_refinement(cache_key: &str, json: &str) {
    let Some(dir) = refinement_cache_dir() else {
        warn!("Cannot determine HOME directory for refinement cache");
        return;
    };
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn!(
            "Failed to create refinement cache directory {}: {}",
            dir.display(),
            e
        );
        return;
    }
    let path = dir.join(format!("{}.json", cache_key));
    if let Err(e) = std::fs::write(&path, json) {
        warn!(
            "Failed to write refinement cache entry {}: {}",
            path.display(),
            e
        );
    }
}

/// Clear all cached refinement results.
pub fn clear_refinement_cache() -> std::io::Result<()> {
    if let Some(dir) = refinement_cache_dir() {
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
    }
    Ok(())
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
    use crate::git::{DiffResult, FileDiff, FileStatus};
    use crate::types::{AnalysisOutput, AnalysisSummary, DiffSource, DiffType};

    fn make_diff_result(
        base_sha: Option<&str>,
        head_sha: Option<&str>,
        paths: &[&str],
    ) -> DiffResult {
        let files = paths
            .iter()
            .map(|p| FileDiff {
                old_path: None,
                new_path: Some(p.to_string()),
                status: FileStatus::Added,
                hunks: vec![],
                old_content: None,
                new_content: Some("content".to_string()),
                is_binary: false,
                additions: 1,
                deletions: 0,
            })
            .collect();
        DiffResult {
            files,
            base_sha: base_sha.map(|s| s.to_string()),
            head_sha: head_sha.map(|s| s.to_string()),
        }
    }

    fn make_analysis_output() -> AnalysisOutput {
        AnalysisOutput {
            version: "1.0.0".to_string(),
            diff_source: DiffSource {
                diff_type: DiffType::BranchComparison,
                base: Some("main".to_string()),
                head: Some("HEAD".to_string()),
                base_sha: Some("abc123".to_string()),
                head_sha: Some("def456".to_string()),
            },
            summary: AnalysisSummary {
                total_files_changed: 2,
                total_groups: 1,
                languages_detected: vec!["typescript".to_string()],
                frameworks_detected: vec![],
            },
            groups: vec![],
            infrastructure_group: None,
            annotations: None,
        }
    }

    #[test]
    fn cache_key_deterministic() {
        let diff = make_diff_result(Some("abc"), Some("def"), &["a.ts", "b.ts"]);
        let key1 = compute_cache_key(&diff);
        let key2 = compute_cache_key(&diff);
        assert_eq!(key1, key2);
    }

    #[test]
    fn cache_key_different_shas() {
        let diff1 = make_diff_result(Some("abc"), Some("def"), &["a.ts"]);
        let diff2 = make_diff_result(Some("abc"), Some("ghi"), &["a.ts"]);
        assert_ne!(compute_cache_key(&diff1), compute_cache_key(&diff2));
    }

    #[test]
    fn cache_key_different_files() {
        let diff1 = make_diff_result(Some("abc"), Some("def"), &["a.ts"]);
        let diff2 = make_diff_result(Some("abc"), Some("def"), &["a.ts", "b.ts"]);
        assert_ne!(compute_cache_key(&diff1), compute_cache_key(&diff2));
    }

    #[test]
    fn cache_key_order_independent() {
        let diff1 = make_diff_result(Some("abc"), Some("def"), &["b.ts", "a.ts"]);
        let diff2 = make_diff_result(Some("abc"), Some("def"), &["a.ts", "b.ts"]);
        assert_eq!(compute_cache_key(&diff1), compute_cache_key(&diff2));
    }

    #[test]
    fn cache_key_none_shas() {
        let diff = make_diff_result(None, None, &["a.ts"]);
        let key = compute_cache_key(&diff);
        assert!(!key.is_empty());
        assert_eq!(key.len(), 64); // SHA-256 hex length
    }

    #[test]
    fn cache_key_empty_files() {
        let diff = make_diff_result(Some("abc"), Some("def"), &[]);
        let key = compute_cache_key(&diff);
        assert_eq!(key.len(), 64);
    }

    #[test]
    fn store_and_load_cached() {
        let tmp = tempfile::tempdir().unwrap();
        let output = make_analysis_output();
        let key = "test_cache_key_12345";

        // Should return None before caching
        assert!(load_cached(tmp.path(), key).is_none());

        // Store
        store_cached(tmp.path(), key, &output);

        // Load
        let loaded = load_cached(tmp.path(), key);
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.version, "1.0.0");
        assert_eq!(loaded.summary.total_files_changed, 2);
    }

    #[test]
    fn load_cached_nonexistent() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(load_cached(tmp.path(), "nonexistent").is_none());
    }

    #[test]
    fn clear_cache_removes_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let output = make_analysis_output();
        store_cached(tmp.path(), "key1", &output);
        store_cached(tmp.path(), "key2", &output);

        assert!(load_cached(tmp.path(), "key1").is_some());
        clear_cache(tmp.path()).unwrap();
        assert!(load_cached(tmp.path(), "key1").is_none());
        assert!(load_cached(tmp.path(), "key2").is_none());
    }

    #[test]
    fn clear_cache_nonexistent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        // Should not error when cache dir doesn't exist
        assert!(clear_cache(tmp.path()).is_ok());
    }

    #[test]
    fn store_cached_creates_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("deep").join("nested");
        // Parent doesn't exist yet
        store_cached(&deep, "key", &make_analysis_output());
        assert!(load_cached(&deep, "key").is_some());
    }

    #[test]
    fn cache_key_hex_encoded() {
        let diff = make_diff_result(Some("abc"), Some("def"), &["a.ts"]);
        let key = compute_cache_key(&diff);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn load_cached_malformed_json_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".diffcore").join("cache");
        std::fs::create_dir_all(&dir).unwrap();

        // Write invalid JSON to cache file
        let path = dir.join("bad_key.json");
        std::fs::write(&path, "{ this is not valid json }").unwrap();

        // Should return None (malformed cache entry)
        assert!(load_cached(tmp.path(), "bad_key").is_none());
    }

    #[test]
    fn load_cached_wrong_schema_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".diffcore").join("cache");
        std::fs::create_dir_all(&dir).unwrap();

        // Write valid JSON but wrong schema
        let path = dir.join("wrong_schema.json");
        std::fs::write(&path, r#"{"version": 42, "unexpected": true}"#).unwrap();

        // Should return None (doesn't match AnalysisOutput schema)
        assert!(load_cached(tmp.path(), "wrong_schema").is_none());
    }

    #[test]
    fn store_cached_readonly_dir_does_not_panic() {
        // Attempting to store to a non-writable location should not panic.
        // On macOS/Linux, /proc or a read-only path works; on all systems,
        // an impossible path name should trigger an error gracefully.
        store_cached(
            std::path::Path::new("/dev/null/impossible"),
            "key",
            &make_analysis_output(),
        );
        // No panic = success (error is logged as warning)
    }

    #[test]
    fn load_cached_empty_file_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".diffcore").join("cache");
        std::fs::create_dir_all(&dir).unwrap();

        // Write empty file
        let path = dir.join("empty.json");
        std::fs::write(&path, "").unwrap();

        assert!(load_cached(tmp.path(), "empty").is_none());
    }

    // ── Refinement cache tests ──

    /// Helper: set DIFFCORE_REFINEMENT_CACHE_DIR to a temp path for isolated testing.
    fn with_refinement_cache_dir<F: FnOnce()>(dir: &Path, f: F) {
        std::env::set_var("DIFFCORE_REFINEMENT_CACHE_DIR", dir.as_os_str());
        f();
        std::env::remove_var("DIFFCORE_REFINEMENT_CACHE_DIR");
    }

    #[test]
    fn refinement_cache_store_and_load() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ref_cache");
        let json = r#"{"refined_groups":[],"provider":"test","model":"m1","had_changes":true}"#;

        with_refinement_cache_dir(&dir, || {
            // Should be empty initially
            assert!(load_cached_refinement("test_key").is_none());

            // Store
            store_cached_refinement("test_key", json);

            // Load
            let loaded = load_cached_refinement("test_key");
            assert!(loaded.is_some());
            assert_eq!(loaded.unwrap(), json);
        });
    }

    #[test]
    fn refinement_cache_different_keys_isolated() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ref_cache");

        with_refinement_cache_dir(&dir, || {
            store_cached_refinement("key_a", r#"{"a": 1}"#);
            store_cached_refinement("key_b", r#"{"b": 2}"#);

            assert_eq!(load_cached_refinement("key_a").unwrap(), r#"{"a": 1}"#);
            assert_eq!(load_cached_refinement("key_b").unwrap(), r#"{"b": 2}"#);
            assert!(load_cached_refinement("key_c").is_none());
        });
    }

    #[test]
    fn refinement_cache_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ref_cache");

        with_refinement_cache_dir(&dir, || {
            store_cached_refinement("key", r#"{"v": 1}"#);
            store_cached_refinement("key", r#"{"v": 2}"#);

            assert_eq!(load_cached_refinement("key").unwrap(), r#"{"v": 2}"#);
        });
    }

    #[test]
    fn refinement_cache_empty_file_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ref_cache");
        std::fs::create_dir_all(&dir).unwrap();

        // Write empty file
        let path = dir.join("empty_key.json");
        std::fs::write(&path, "").unwrap();

        with_refinement_cache_dir(&dir, || {
            assert!(load_cached_refinement("empty_key").is_none());
        });
    }

    #[test]
    fn refinement_cache_clear() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ref_cache");

        with_refinement_cache_dir(&dir, || {
            store_cached_refinement("key1", r#"{"x": 1}"#);
            store_cached_refinement("key2", r#"{"x": 2}"#);

            assert!(load_cached_refinement("key1").is_some());
            clear_refinement_cache().unwrap();
            assert!(load_cached_refinement("key1").is_none());
            assert!(load_cached_refinement("key2").is_none());
        });
    }

    #[test]
    fn refinement_cache_clear_nonexistent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("nonexistent_ref_cache");

        with_refinement_cache_dir(&dir, || {
            // Should not error when cache dir doesn't exist
            assert!(clear_refinement_cache().is_ok());
        });
    }

    #[test]
    fn refinement_cache_creates_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("deep").join("nested").join("ref_cache");

        with_refinement_cache_dir(&dir, || {
            store_cached_refinement("key", r#"{"deep": true}"#);
            assert_eq!(
                load_cached_refinement("key").unwrap(),
                r#"{"deep": true}"#
            );
        });
    }

    #[test]
    fn refinement_cache_store_readonly_does_not_panic() {
        let dir = PathBuf::from("/dev/null/impossible/ref_cache");
        with_refinement_cache_dir(&dir, || {
            // Should not panic, just log warning
            store_cached_refinement("key", r#"{"fail": true}"#);
        });
    }
}
