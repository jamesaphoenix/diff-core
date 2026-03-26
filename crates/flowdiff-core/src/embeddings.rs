//! Local code embeddings for semantic file similarity.
//!
//! Uses fastembed with Jina Embeddings v2 Base Code (768 dimensions, 30 languages)
//! to embed file diff content and compute pairwise cosine similarity.
//! This provides a language-agnostic semantic signal for clustering that
//! complements the deterministic tree-sitter graph.
//!
//! Gated behind the `embeddings` feature flag to avoid pulling in ONNX Runtime
//! for users who don't need it.

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

/// Embed file diff contents using Jina Code embeddings (768 dimensions).
///
/// Each entry in `file_diffs` is `(file_path, diff_content)`.
/// Returns one `FileEmbedding` per input file.
///
/// The model is downloaded on first use (~200MB) and cached locally.
#[cfg(feature = "embeddings")]
pub fn embed_file_diffs(
    file_diffs: &[(String, String)],
) -> Result<Vec<FileEmbedding>, EmbeddingError> {
    if file_diffs.is_empty() {
        return Ok(vec![]);
    }

    let model = TextEmbedding::try_new(
        InitOptions::new(EmbeddingModel::JinaEmbeddingsV2BaseCode)
            .with_show_download_progress(true),
    )
    .map_err(|e| EmbeddingError::ModelLoad(e.to_string()))?;

    // Prepare texts — prefix with filename for context
    let texts: Vec<String> = file_diffs
        .iter()
        .map(|(path, diff)| {
            // Truncate very long diffs to fit in 8192 token context
            let truncated = if diff.len() > 16000 {
                &diff[..16000]
            } else {
                diff.as_str()
            };
            format!("// File: {}\n{}", path, truncated)
        })
        .collect();

    let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
    let vectors = model
        .embed(text_refs, None)
        .map_err(|e| EmbeddingError::Inference(e.to_string()))?;

    let embeddings = file_diffs
        .iter()
        .zip(vectors)
        .map(|((path, _), vec)| FileEmbedding {
            file_path: path.clone(),
            vector: vec,
        })
        .collect();

    Ok(embeddings)
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

    similarities.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap_or(std::cmp::Ordering::Equal));
    similarities
}

/// Group files by similarity threshold using single-linkage clustering.
/// Files with pairwise similarity >= threshold are merged into the same group.
pub fn cluster_by_similarity(
    embeddings: &[FileEmbedding],
    threshold: f32,
) -> Vec<Vec<String>> {
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
    let mut groups: std::collections::HashMap<usize, Vec<String>> = std::collections::HashMap::new();
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
            FileEmbedding { file_path: "a.rs".into(), vector: vec![1.0, 0.0, 0.0] },
            FileEmbedding { file_path: "b.rs".into(), vector: vec![0.99, 0.1, 0.0] },
            FileEmbedding { file_path: "c.rs".into(), vector: vec![0.0, 0.0, 1.0] },
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
            FileEmbedding { file_path: "a.rs".into(), vector: vec![1.0, 0.0] },
            FileEmbedding { file_path: "b.rs".into(), vector: vec![0.9, 0.1] },
            FileEmbedding { file_path: "c.rs".into(), vector: vec![0.0, 1.0] },
        ];
        let sims = pairwise_similarities(&embeddings);
        assert_eq!(sims.len(), 3);
        // First pair should have highest similarity (a-b)
        assert!(sims[0].similarity > sims[1].similarity);
    }
}
