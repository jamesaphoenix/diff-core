//! Local code embeddings for semantic file similarity.
//!
//! Uses fastembed with Jina Embeddings v2 Base Code (768 dimensions, 30 languages)
//! to embed file diff content and compute pairwise cosine similarity.
//! This provides a language-agnostic semantic signal for clustering that
//! complements the deterministic tree-sitter graph.
//!
//! Gated behind the `embeddings` feature flag to avoid pulling in ONNX Runtime
//! for users who don't need it.
//!
//! ## Caching
//!
//! Embeddings are cached to disk keyed by SHA-256(file_path + content).
//! Cache location: `~/.cache/diffcore/embeddings/` (shared across all repos).
//! This avoids recomputing embeddings during tests, eval loops, and repeated analyses.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[cfg(feature = "embeddings")]
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

/// A computed embedding vector for a file's diff content.
#[derive(Debug, Clone)]
pub struct FileEmbedding {
    pub file_path: String,
    pub vector: Vec<f32>,
}

/// Pairwise similarity between two files.
#[derive(Debug, Clone)]
pub struct FileSimilarity {
    pub file_a: String,
    pub file_b: String,
    pub similarity: f32,
}

/// Compute cosine similarity between two vectors.
/// Returns 0.0 if either vector has zero magnitude.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

// ── Embedding Cache ──────────────────────────────────────────────────────

/// Disk-based embedding cache.
///
/// Embeddings are stored as raw f32 binary files keyed by SHA-256 of
/// (file_path + "\0" + content). This avoids recomputing embeddings
/// for the same file content across runs.
pub struct EmbeddingCache {
    cache_dir: PathBuf,
}

impl EmbeddingCache {
    /// Create a new cache at the given directory. Creates the directory if it doesn't exist.
    pub fn new(cache_dir: &Path) -> Self {
        if let Err(e) = std::fs::create_dir_all(cache_dir) {
            log::warn!("Failed to create embedding cache dir: {}", e);
        }
        Self {
            cache_dir: cache_dir.to_path_buf(),
        }
    }

    /// Default cache location: `~/.cache/diffcore/embeddings/`
    pub fn default_cache() -> Self {
        let dir = dirs_fallback().join("diffcore").join("embeddings");
        Self::new(&dir)
    }

    /// Compute a cache key for a file path + content pair.
    fn cache_key(file_path: &str, content: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(file_path.as_bytes());
        hasher.update(b"\0");
        hasher.update(content.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Try to load a cached embedding vector.
    pub fn get(&self, file_path: &str, content: &str) -> Option<Vec<f32>> {
        let key = Self::cache_key(file_path, content);
        let path = self.cache_dir.join(&key);
        let data = std::fs::read(&path).ok()?;
        // Each f32 is 4 bytes
        if data.len() % 4 != 0 {
            return None;
        }
        let floats: Vec<f32> = data
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();
        Some(floats)
    }

    /// Store an embedding vector in the cache.
    pub fn put(&self, file_path: &str, content: &str, vector: &[f32]) {
        let key = Self::cache_key(file_path, content);
        let path = self.cache_dir.join(&key);
        let data: Vec<u8> = vector.iter().flat_map(|f| f.to_le_bytes()).collect();
        if let Err(e) = std::fs::write(&path, &data) {
            log::warn!("Failed to write embedding cache entry: {}", e);
        }
    }

    /// Number of cached entries.
    pub fn len(&self) -> usize {
        std::fs::read_dir(&self.cache_dir)
            .map(|entries| entries.count())
            .unwrap_or(0)
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clear all cached embeddings.
    pub fn clear(&self) {
        if let Ok(entries) = std::fs::read_dir(&self.cache_dir) {
            for entry in entries.flatten() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// Fallback for cache directory — uses `~/.cache` on all platforms.
fn dirs_fallback() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".cache")
    } else {
        PathBuf::from("/tmp")
    }
}

// ── Embedding computation ────────────────────────────────────────────────

/// Embed file diff contents using Jina Code embeddings (768 dimensions).
///
/// Each entry in `file_diffs` is `(file_path, diff_content)`.
/// Returns one `FileEmbedding` per input file.
///
/// The model is downloaded on first use (~200MB) and cached locally.
/// Uses the embedding cache to skip recomputation for previously seen content.
#[cfg(feature = "embeddings")]
pub fn embed_file_diffs(
    file_diffs: &[(String, String)],
) -> Result<Vec<FileEmbedding>, EmbeddingError> {
    let cache = EmbeddingCache::default_cache();
    embed_file_diffs_with_cache(file_diffs, &cache)
}

/// Embed file diffs with an explicit cache.
#[cfg(feature = "embeddings")]
pub fn embed_file_diffs_with_cache(
    file_diffs: &[(String, String)],
    cache: &EmbeddingCache,
) -> Result<Vec<FileEmbedding>, EmbeddingError> {
    if file_diffs.is_empty() {
        return Ok(vec![]);
    }

    // Check cache for each file
    let mut results: Vec<Option<FileEmbedding>> = Vec::with_capacity(file_diffs.len());
    let mut uncached_indices: Vec<usize> = Vec::new();

    for (i, (path, content)) in file_diffs.iter().enumerate() {
        if let Some(vector) = cache.get(path, content) {
            results.push(Some(FileEmbedding {
                file_path: path.clone(),
                vector,
            }));
        } else {
            results.push(None);
            uncached_indices.push(i);
        }
    }

    let cache_hits = file_diffs.len() - uncached_indices.len();
    if cache_hits > 0 {
        log::info!(
            "Embedding cache: {}/{} hits, {} to compute",
            cache_hits,
            file_diffs.len(),
            uncached_indices.len()
        );
    }

    // If all cached, return immediately
    if uncached_indices.is_empty() {
        return Ok(results.into_iter().flatten().collect());
    }

    // Load model and compute uncached embeddings
    let model = TextEmbedding::try_new(
        InitOptions::new(EmbeddingModel::JinaEmbeddingsV2BaseCode)
            .with_show_download_progress(true),
    )
    .map_err(|e| EmbeddingError::ModelLoad(e.to_string()))?;

    // Prepare texts for uncached files
    let texts: Vec<String> = uncached_indices
        .iter()
        .map(|&i| {
            let (path, diff) = &file_diffs[i];
            let max_bytes = 16000;
            let truncated = if diff.len() > max_bytes {
                let mut end = max_bytes;
                while end > 0 && !diff.is_char_boundary(end) {
                    end -= 1;
                }
                &diff[..end]
            } else {
                diff.as_str()
            };
            format!("// File: {}\n{}", path, truncated)
        })
        .collect();

    // Embed in batches of 64 to avoid ONNX memory blowup on large repos.
    const BATCH_SIZE: usize = 64;
    let mut all_vectors: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
    for chunk in texts.chunks(BATCH_SIZE) {
        let refs: Vec<&str> = chunk.iter().map(|s| s.as_str()).collect();
        let batch_vecs = model
            .embed(refs, None)
            .map_err(|e| EmbeddingError::Inference(e.to_string()))?;
        all_vectors.extend(batch_vecs);
    }

    // Fill in results and cache
    for (vec_idx, &file_idx) in uncached_indices.iter().enumerate() {
        let (path, content) = &file_diffs[file_idx];
        let vector = &all_vectors[vec_idx];

        cache.put(path, content, vector);

        results[file_idx] = Some(FileEmbedding {
            file_path: path.clone(),
            vector: vector.clone(),
        });
    }

    Ok(results.into_iter().flatten().collect())
}

/// Compute pairwise similarities between all file embeddings.
/// Returns pairs sorted by similarity (highest first).
pub fn pairwise_similarities(embeddings: &[FileEmbedding]) -> Vec<FileSimilarity> {
    let n = embeddings.len();
    let mut similarities = Vec::with_capacity(n * (n - 1) / 2);

    for i in 0..n {
        for j in (i + 1)..n {
            let sim = cosine_similarity(&embeddings[i].vector, &embeddings[j].vector);
            similarities.push(FileSimilarity {
                file_a: embeddings[i].file_path.clone(),
                file_b: embeddings[j].file_path.clone(),
                similarity: sim,
            });
        }
    }

    similarities.sort_by(|a, b| {
        b.similarity
            .partial_cmp(&a.similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    similarities
}

/// Build a lookup table of pairwise similarities for O(1) access.
pub fn similarity_matrix(embeddings: &[FileEmbedding]) -> HashMap<(String, String), f32> {
    let mut matrix = HashMap::new();
    for i in 0..embeddings.len() {
        for j in (i + 1)..embeddings.len() {
            let sim = cosine_similarity(&embeddings[i].vector, &embeddings[j].vector);
            let a = embeddings[i].file_path.clone();
            let b = embeddings[j].file_path.clone();
            matrix.insert((a.clone(), b.clone()), sim);
            matrix.insert((b, a), sim);
        }
    }
    matrix
}

/// Group files by similarity threshold using single-linkage clustering.
/// Files with pairwise similarity >= threshold are merged into the same group.
pub fn cluster_by_similarity(embeddings: &[FileEmbedding], threshold: f32) -> Vec<Vec<String>> {
    let n = embeddings.len();
    if n == 0 {
        return vec![];
    }

    // Union-Find for clustering
    let mut parent: Vec<usize> = (0..n).collect();

    fn find(parent: &mut [usize], i: usize) -> usize {
        if parent[i] != i {
            parent[i] = find(parent, parent[i]);
        }
        parent[i]
    }

    fn union(parent: &mut [usize], a: usize, b: usize) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent[ra] = rb;
        }
    }

    // Merge files with similarity above threshold
    for i in 0..n {
        for j in (i + 1)..n {
            let sim = cosine_similarity(&embeddings[i].vector, &embeddings[j].vector);
            if sim >= threshold {
                union(&mut parent, i, j);
            }
        }
    }

    // Collect groups
    let mut groups: HashMap<usize, Vec<String>> = HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        groups
            .entry(root)
            .or_default()
            .push(embeddings[i].file_path.clone());
    }

    let mut result: Vec<Vec<String>> = groups.into_values().collect();
    result.sort_by_key(|g| std::cmp::Reverse(g.len()));
    result
}

#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    #[error("Failed to load embedding model: {0}")]
    ModelLoad(String),
    #[error("Embedding inference failed: {0}")]
    Inference(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!((cosine_similarity(&a, &b)).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        assert!((cosine_similarity(&a, &b) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_empty() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }

    #[test]
    fn test_cosine_similarity_different_lengths() {
        assert_eq!(cosine_similarity(&[1.0], &[1.0, 2.0]), 0.0);
    }

    #[test]
    fn test_cluster_by_similarity() {
        let embeddings = vec![
            FileEmbedding {
                file_path: "a.rs".into(),
                vector: vec![1.0, 0.0, 0.0],
            },
            FileEmbedding {
                file_path: "b.rs".into(),
                vector: vec![0.99, 0.1, 0.0],
            },
            FileEmbedding {
                file_path: "c.rs".into(),
                vector: vec![0.0, 0.0, 1.0],
            },
        ];
        let groups = cluster_by_similarity(&embeddings, 0.9);
        // a.rs and b.rs should be clustered together (high similarity)
        // c.rs should be alone (orthogonal)
        assert_eq!(groups.len(), 2);
        let big_group = &groups[0];
        assert_eq!(big_group.len(), 2);
        assert!(big_group.contains(&"a.rs".to_string()));
        assert!(big_group.contains(&"b.rs".to_string()));
    }

    #[test]
    fn test_pairwise_similarities_ordering() {
        let embeddings = vec![
            FileEmbedding {
                file_path: "a.rs".into(),
                vector: vec![1.0, 0.0],
            },
            FileEmbedding {
                file_path: "b.rs".into(),
                vector: vec![0.9, 0.1],
            },
            FileEmbedding {
                file_path: "c.rs".into(),
                vector: vec![0.0, 1.0],
            },
        ];
        let sims = pairwise_similarities(&embeddings);
        assert_eq!(sims.len(), 3);
        // First pair should have highest similarity (a-b)
        assert!(sims[0].similarity > sims[1].similarity);
    }

    #[test]
    fn test_similarity_matrix_symmetric() {
        let embeddings = vec![
            FileEmbedding {
                file_path: "x.rs".into(),
                vector: vec![1.0, 0.5],
            },
            FileEmbedding {
                file_path: "y.rs".into(),
                vector: vec![0.5, 1.0],
            },
        ];
        let matrix = similarity_matrix(&embeddings);
        let ab = matrix.get(&("x.rs".to_string(), "y.rs".to_string()));
        let ba = matrix.get(&("y.rs".to_string(), "x.rs".to_string()));
        assert!(ab.is_some());
        assert!(ba.is_some());
        assert!((ab.unwrap_or(&0.0) - ba.unwrap_or(&0.0)).abs() < 1e-6);
    }

    #[test]
    fn test_embedding_cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap_or_else(|_| {
            // Fallback — create a temp dir manually
            let p = std::env::temp_dir().join("fd-embed-test");
            let _ = std::fs::create_dir_all(&p);
            tempfile::TempDir::new_in(&p).unwrap_or_else(|_| tempfile::TempDir::new().unwrap())
        });
        let cache = EmbeddingCache::new(dir.path());

        let vector = vec![1.0_f32, 2.0, 3.0, -0.5];
        assert!(cache.get("foo.rs", "fn main()").is_none());

        cache.put("foo.rs", "fn main()", &vector);
        let loaded = cache.get("foo.rs", "fn main()");
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.len(), 4);
        assert!((loaded[0] - 1.0).abs() < 1e-6);
        assert!((loaded[3] - (-0.5)).abs() < 1e-6);

        // Different content = different key = miss
        assert!(cache.get("foo.rs", "fn other()").is_none());

        assert_eq!(cache.len(), 1);
    }
}
