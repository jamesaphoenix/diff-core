//! Analysis result caching based on diff content hashing.
//!
//! Caches the JSON-serialized `AnalysisOutput` keyed by a SHA-256 hash of the
//! diff inputs (base SHA, head SHA, sorted file paths). On subsequent runs with
//! the same diff, the cached result is returned instantly without re-parsing or
//! re-analyzing.
//!
//! Cache location: `<repo>/.flowdiff/cache/` (gitignored by convention).

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
    workdir.join(".flowdiff").join("cache")
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

#[cfg(test)]
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
        let dir = tmp.path().join(".flowdiff").join("cache");
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
        let dir = tmp.path().join(".flowdiff").join("cache");
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
        let dir = tmp.path().join(".flowdiff").join("cache");
        std::fs::create_dir_all(&dir).unwrap();

        // Write empty file
        let path = dir.join("empty.json");
        std::fs::write(&path, "").unwrap();

        assert!(load_cached(tmp.path(), "empty").is_none());
    }
}
