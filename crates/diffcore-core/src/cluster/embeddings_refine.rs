//! Embedding-based refinement (optional, behind `embeddings` feature).

#[cfg(feature = "embeddings")]
use std::collections::HashMap;

#[cfg(feature = "embeddings")]
use crate::types::FileChange;

#[cfg(feature = "embeddings")]
use super::{ClusterResult, SMALL_GROUP_THRESHOLD};

/// Minimum cosine similarity between group centroids for embedding-based merging.
#[cfg(feature = "embeddings")]
const EMBEDDING_MERGE_THRESHOLD: f32 = 0.75;

/// Refine clustering using embedding similarity — merge-only strategy.
///
/// Uses centroid-based comparisons: O(groups² × dim). Scales to any repo size.
///
/// Deliberately does NOT rescue files from infrastructure — experiment #53 showed
/// that infra files (README, Cargo.toml, build.rs) share enough vocabulary with
/// source code to produce false rescues. Instead, only merges small groups whose
/// centroids are semantically similar, which is safe since both sides are already
/// classified as non-infrastructure.
///
/// `file_diffs` is a slice of `(file_path, content)` tuples for all changed files.
#[cfg(feature = "embeddings")]
pub fn refine_with_embeddings(
    mut result: ClusterResult,
    file_diffs: &[(String, String)],
) -> ClusterResult {
    use crate::embeddings::{cosine_similarity, EmbeddingCache};

    if file_diffs.is_empty() || result.groups.len() <= 1 {
        return result;
    }

    let cache = EmbeddingCache::default_cache();
    let embeddings = match crate::embeddings::embed_file_diffs_with_cache(file_diffs, &cache) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("Embedding computation failed, skipping refinement: {}", e);
            return result;
        }
    };

    // Build lookup: file_path -> embedding vector
    let embed_map: HashMap<String, Vec<f32>> = embeddings
        .into_iter()
        .map(|e| (e.file_path, e.vector))
        .collect();

    let dim = embed_map.values().next().map(|v| v.len()).unwrap_or(0);
    if dim == 0 {
        return result;
    }

    // Merge small groups with similar centroids — O(groups² × dim)
    let mut centroids: Vec<Vec<f32>> = result
        .groups
        .iter()
        .map(|g| compute_centroid(&embed_map, g.files.iter().map(|f| f.path.as_str()), dim))
        .collect();

    let mut merged = true;
    while merged {
        merged = false;
        let n = result.groups.len();
        let mut best_merge: Option<(usize, usize, f32)> = None;

        for i in 0..n {
            if result.groups[i].files.len() > SMALL_GROUP_THRESHOLD {
                continue;
            }
            for j in (i + 1)..n {
                if result.groups[j].files.len() > SMALL_GROUP_THRESHOLD {
                    continue;
                }
                let sim = cosine_similarity(&centroids[i], &centroids[j]);
                if sim >= EMBEDDING_MERGE_THRESHOLD {
                    match best_merge {
                        None => best_merge = Some((i, j, sim)),
                        Some((_, _, best)) if sim > best => {
                            best_merge = Some((i, j, sim));
                        }
                        _ => {}
                    }
                }
            }
        }

        if let Some((i, j, _)) = best_merge {
            let donor = result.groups.remove(j);
            centroids.remove(j);
            let receiver = &mut result.groups[i];
            for fc in donor.files {
                let pos = receiver.files.len() as u32;
                receiver.files.push(FileChange {
                    flow_position: pos,
                    ..fc
                });
            }
            receiver.edges.extend(donor.edges);
            centroids[i] = compute_centroid(
                &embed_map,
                receiver.files.iter().map(|f| f.path.as_str()),
                dim,
            );
            merged = true;
        }
    }

    result
}

/// Compute the centroid (element-wise mean) of embedding vectors for given file paths.
#[cfg(feature = "embeddings")]
fn compute_centroid<'a>(
    embed_map: &HashMap<String, Vec<f32>>,
    paths: impl Iterator<Item = &'a str>,
    dim: usize,
) -> Vec<f32> {
    let mut sum = vec![0.0_f32; dim];
    let mut count = 0u32;
    for path in paths {
        if let Some(vec) = embed_map.get(path) {
            for (s, v) in sum.iter_mut().zip(vec.iter()) {
                *s += v;
            }
            count += 1;
        }
    }
    if count > 0 {
        let inv = 1.0 / count as f32;
        for s in &mut sum {
            *s *= inv;
        }
    }
    sum
}
