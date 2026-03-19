//! VCR (Video Cassette Recorder) caching layer for LLM calls.
//!
//! Records real LLM responses on first run, replays from cache on subsequent runs.
//! Enables deterministic, fast, free CI runs of LLM-annotated tests.
//!
//! Cache is keyed by (provider, model, request hash, prompt template hash).
//! When prompt templates change, cached entries are automatically invalidated.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use super::schema::{
    JudgeRequest, JudgeResponse, Pass1Request, Pass1Response, Pass2Request, Pass2Response,
    RefinementRequest, RefinementResponse,
};
use super::{
    judge_system_prompt, pass1_system_prompt, pass2_system_prompt, refinement_system_prompt,
    LlmError, LlmProvider,
};

/// VCR operating mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcrMode {
    /// Record real LLM responses to cache (overwrites existing).
    Record,
    /// Replay from cache only; fail if entry not found.
    Replay,
    /// Use cache if available, otherwise call real provider and cache result.
    Auto,
}

/// A cached LLM response entry stored on disk.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CacheEntry<T> {
    /// Provider name that produced this response.
    pub provider: String,
    /// Model that produced this response.
    pub model: String,
    /// SHA-256 hash of the serialized request.
    pub request_hash: String,
    /// SHA-256 hash of the prompt template at recording time.
    pub prompt_template_hash: String,
    /// ISO 8601 timestamp when this entry was recorded.
    pub recorded_at: String,
    /// The cached response.
    pub response: T,
}

/// VCR provider wrapping a real LLM provider with disk-based caching.
pub struct VcrProvider {
    inner: Box<dyn LlmProvider>,
    cache_dir: PathBuf,
    mode: VcrMode,
}

impl VcrProvider {
    /// Create a new VCR provider wrapping an inner provider.
    ///
    /// `cache_dir` is the directory where cached responses are stored.
    /// It will be created if it doesn't exist.
    pub fn new(inner: Box<dyn LlmProvider>, cache_dir: PathBuf, mode: VcrMode) -> Self {
        Self {
            inner,
            cache_dir,
            mode,
        }
    }

    /// Compute the SHA-256 hash of arbitrary bytes, returned as hex string.
    fn sha256_hex(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        hex::encode(hasher.finalize())
    }

    /// Compute a cache key from provider, model, request, and prompt template.
    pub fn cache_key(
        provider: &str,
        model: &str,
        request_json: &str,
        prompt_template: &str,
    ) -> String {
        let combined = format!(
            "provider={}\nmodel={}\nrequest={}\ntemplate={}",
            provider, model, request_json, prompt_template
        );
        Self::sha256_hex(combined.as_bytes())
    }

    /// Get the current prompt template hash for Pass 1.
    pub fn pass1_template_hash() -> String {
        Self::sha256_hex(pass1_system_prompt().as_bytes())
    }

    /// Get the current prompt template hash for Pass 2.
    pub fn pass2_template_hash() -> String {
        Self::sha256_hex(pass2_system_prompt().as_bytes())
    }

    /// Get the current prompt template hash for judge evaluation.
    pub fn judge_template_hash() -> String {
        Self::sha256_hex(judge_system_prompt().as_bytes())
    }

    /// Get the current prompt template hash for refinement.
    pub fn refinement_template_hash() -> String {
        Self::sha256_hex(refinement_system_prompt().as_bytes())
    }

    /// Build the cache file path for a given pass type and cache key.
    fn cache_path(&self, pass_type: &str, cache_key: &str) -> PathBuf {
        self.cache_dir
            .join(format!("{}_{}.json", pass_type, &cache_key[..16]))
    }

    /// Read a cached entry from disk, validating the prompt template hash.
    fn read_cache<T: serde::de::DeserializeOwned>(
        &self,
        path: &Path,
        current_template_hash: &str,
    ) -> Option<T> {
        let data = std::fs::read_to_string(path).ok()?;
        let entry: CacheEntry<T> = serde_json::from_str(&data).ok()?;

        // Invalidate if prompt template has changed
        if entry.prompt_template_hash != current_template_hash {
            return None;
        }

        Some(entry.response)
    }

    /// Write a cache entry to disk.
    fn write_cache<T: serde::Serialize>(
        &self,
        path: &Path,
        request_hash: &str,
        prompt_template_hash: &str,
        response: &T,
    ) -> Result<(), LlmError> {
        std::fs::create_dir_all(&self.cache_dir).map_err(|e| {
            LlmError::ParseResponse(format!("Failed to create VCR cache dir: {}", e))
        })?;

        let entry = CacheEntry {
            provider: self.inner.name().to_string(),
            model: self.inner.model().to_string(),
            request_hash: request_hash.to_string(),
            prompt_template_hash: prompt_template_hash.to_string(),
            recorded_at: chrono::Utc::now().to_rfc3339(),
            response,
        };

        let json = serde_json::to_string_pretty(&entry).map_err(|e| {
            LlmError::ParseResponse(format!("Failed to serialize VCR cache entry: {}", e))
        })?;

        std::fs::write(path, json).map_err(|e| {
            LlmError::ParseResponse(format!("Failed to write VCR cache file: {}", e))
        })?;

        Ok(())
    }

    /// List all cache entries in the cache directory.
    pub fn list_entries(&self) -> Vec<PathBuf> {
        std::fs::read_dir(&self.cache_dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
            .collect()
    }

    /// Clear all cache entries.
    pub fn clear_cache(&self) -> std::io::Result<()> {
        if self.cache_dir.exists() {
            std::fs::remove_dir_all(&self.cache_dir)?;
        }
        Ok(())
    }

    /// Return the current VCR mode.
    pub fn mode(&self) -> VcrMode {
        self.mode
    }

    /// Return a reference to the cache directory.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }
}

#[async_trait]
impl LlmProvider for VcrProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn model(&self) -> &str {
        self.inner.model()
    }

    fn max_context_tokens(&self) -> usize {
        self.inner.max_context_tokens()
    }

    async fn annotate_overview(
        &self,
        request: &Pass1Request,
    ) -> Result<Pass1Response, LlmError> {
        let request_json = serde_json::to_string(request).map_err(|e| {
            LlmError::ParseResponse(format!("Failed to serialize request for VCR key: {}", e))
        })?;
        let template_hash = Self::pass1_template_hash();
        let key = Self::cache_key(self.inner.name(), self.inner.model(), &request_json, &template_hash);
        let path = self.cache_path("pass1", &key);

        match self.mode {
            VcrMode::Replay => {
                self.read_cache::<Pass1Response>(&path, &template_hash)
                    .ok_or_else(|| {
                        LlmError::ParseResponse(format!(
                            "VCR replay: no cached entry at {}",
                            path.display()
                        ))
                    })
            }
            VcrMode::Record => {
                let response = self.inner.annotate_overview(request).await?;
                self.write_cache(&path, &key, &template_hash, &response)?;
                Ok(response)
            }
            VcrMode::Auto => {
                if let Some(cached) = self.read_cache::<Pass1Response>(&path, &template_hash) {
                    return Ok(cached);
                }
                let response = self.inner.annotate_overview(request).await?;
                self.write_cache(&path, &key, &template_hash, &response)?;
                Ok(response)
            }
        }
    }

    async fn annotate_group(
        &self,
        request: &Pass2Request,
    ) -> Result<Pass2Response, LlmError> {
        let request_json = serde_json::to_string(request).map_err(|e| {
            LlmError::ParseResponse(format!("Failed to serialize request for VCR key: {}", e))
        })?;
        let template_hash = Self::pass2_template_hash();
        let key = Self::cache_key(self.inner.name(), self.inner.model(), &request_json, &template_hash);
        let path = self.cache_path("pass2", &key);

        match self.mode {
            VcrMode::Replay => {
                self.read_cache::<Pass2Response>(&path, &template_hash)
                    .ok_or_else(|| {
                        LlmError::ParseResponse(format!(
                            "VCR replay: no cached entry at {}",
                            path.display()
                        ))
                    })
            }
            VcrMode::Record => {
                let response = self.inner.annotate_group(request).await?;
                self.write_cache(&path, &key, &template_hash, &response)?;
                Ok(response)
            }
            VcrMode::Auto => {
                if let Some(cached) = self.read_cache::<Pass2Response>(&path, &template_hash) {
                    return Ok(cached);
                }
                let response = self.inner.annotate_group(request).await?;
                self.write_cache(&path, &key, &template_hash, &response)?;
                Ok(response)
            }
        }
    }

    async fn evaluate_quality(
        &self,
        request: &JudgeRequest,
    ) -> Result<JudgeResponse, LlmError> {
        let request_json = serde_json::to_string(request).map_err(|e| {
            LlmError::ParseResponse(format!("Failed to serialize request for VCR key: {}", e))
        })?;
        let template_hash = Self::judge_template_hash();
        let key = Self::cache_key(self.inner.name(), self.inner.model(), &request_json, &template_hash);
        let path = self.cache_path("judge", &key);

        match self.mode {
            VcrMode::Replay => {
                self.read_cache::<JudgeResponse>(&path, &template_hash)
                    .ok_or_else(|| {
                        LlmError::ParseResponse(format!(
                            "VCR replay: no cached entry at {}",
                            path.display()
                        ))
                    })
            }
            VcrMode::Record => {
                let response = self.inner.evaluate_quality(request).await?;
                self.write_cache(&path, &key, &template_hash, &response)?;
                Ok(response)
            }
            VcrMode::Auto => {
                if let Some(cached) = self.read_cache::<JudgeResponse>(&path, &template_hash) {
                    return Ok(cached);
                }
                let response = self.inner.evaluate_quality(request).await?;
                self.write_cache(&path, &key, &template_hash, &response)?;
                Ok(response)
            }
        }
    }

    async fn refine_groups(
        &self,
        request: &RefinementRequest,
    ) -> Result<RefinementResponse, LlmError> {
        let request_json = serde_json::to_string(request).map_err(|e| {
            LlmError::ParseResponse(format!("Failed to serialize request for VCR key: {}", e))
        })?;
        let template_hash = Self::refinement_template_hash();
        let key =
            Self::cache_key(self.inner.name(), self.inner.model(), &request_json, &template_hash);
        let path = self.cache_path("refinement", &key);

        match self.mode {
            VcrMode::Replay => self
                .read_cache::<RefinementResponse>(&path, &template_hash)
                .ok_or_else(|| {
                    LlmError::ParseResponse(format!(
                        "VCR replay: no cached entry at {}",
                        path.display()
                    ))
                }),
            VcrMode::Record => {
                let response = self.inner.refine_groups(request).await?;
                self.write_cache(&path, &key, &template_hash, &response)?;
                Ok(response)
            }
            VcrMode::Auto => {
                if let Some(cached) =
                    self.read_cache::<RefinementResponse>(&path, &template_hash)
                {
                    return Ok(cached);
                }
                let response = self.inner.refine_groups(request).await?;
                self.write_cache(&path, &key, &template_hash, &response)?;
                Ok(response)
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::print_stdout, clippy::print_stderr)]
mod tests {
    use super::*;
    use crate::llm::schema::{
        JudgeCriterionScore, JudgeSourceFile, Pass1GroupAnnotation, Pass1GroupInput,
        Pass2FileAnnotation, Pass2FileInput, RefinementGroupInput,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tempfile::TempDir;

    /// A mock LLM provider that counts calls and returns fixed responses.
    struct MockProvider {
        call_count: Arc<AtomicUsize>,
    }

    impl MockProvider {
        fn new(call_count: Arc<AtomicUsize>) -> Self {
            Self { call_count }
        }
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        fn name(&self) -> &str {
            "mock"
        }
        fn model(&self) -> &str {
            "mock-v1"
        }
        fn max_context_tokens(&self) -> usize {
            100_000
        }

        async fn annotate_overview(
            &self,
            _request: &Pass1Request,
        ) -> Result<Pass1Response, LlmError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(Pass1Response {
                groups: vec![Pass1GroupAnnotation {
                    id: "g1".to_string(),
                    name: "Mock group".to_string(),
                    summary: "Mock summary".to_string(),
                    review_order_rationale: "Mock rationale".to_string(),
                    risk_flags: vec!["mock_flag".to_string()],
                }],
                overall_summary: "Mock overall".to_string(),
                suggested_review_order: vec!["g1".to_string()],
            })
        }

        async fn annotate_group(
            &self,
            _request: &Pass2Request,
        ) -> Result<Pass2Response, LlmError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(Pass2Response {
                group_id: "g1".to_string(),
                flow_narrative: "Mock narrative".to_string(),
                file_annotations: vec![Pass2FileAnnotation {
                    file: "src/mock.ts".to_string(),
                    role_in_flow: "Mock role".to_string(),
                    changes_summary: "Mock changes".to_string(),
                    risks: vec![],
                    suggestions: vec![],
                }],
                cross_cutting_concerns: vec![],
            })
        }

        async fn evaluate_quality(
            &self,
            _request: &JudgeRequest,
        ) -> Result<JudgeResponse, LlmError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(JudgeResponse {
                criteria: vec![JudgeCriterionScore {
                    criterion: "group_coherence".to_string(),
                    score: 4,
                    explanation: "Mock evaluation".to_string(),
                }],
                overall_score: 4.0,
                failure_explanations: vec![],
                strengths: vec!["Mock strength".to_string()],
            })
        }

        async fn refine_groups(
            &self,
            _request: &RefinementRequest,
        ) -> Result<RefinementResponse, LlmError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(RefinementResponse {
                splits: vec![],
                merges: vec![],
                re_ranks: vec![],
                reclassifications: vec![],
                reasoning: "Mock: no refinements needed".to_string(),
            })
        }
    }

    fn sample_pass1() -> Pass1Request {
        Pass1Request {
            diff_summary: "test diff".to_string(),
            flow_groups: vec![Pass1GroupInput {
                id: "g1".to_string(),
                name: "Test group".to_string(),
                entrypoint: None,
                files: vec!["src/a.ts".to_string()],
                risk_score: 0.5,
                edge_summary: "a -> b".to_string(),
            }],
            graph_summary: "1 node".to_string(),
        }
    }

    fn sample_judge() -> JudgeRequest {
        JudgeRequest {
            analysis_json: r#"{"version":"1.0.0"}"#.to_string(),
            source_files: vec![JudgeSourceFile {
                path: "src/a.ts".to_string(),
                content: "export function a() {}".to_string(),
            }],
            diff_text: "+ new line".to_string(),
            fixture_name: "test fixture".to_string(),
        }
    }

    fn sample_pass2() -> Pass2Request {
        Pass2Request {
            group_id: "g1".to_string(),
            group_name: "Test group".to_string(),
            files: vec![Pass2FileInput {
                path: "src/a.ts".to_string(),
                diff: "+ new line".to_string(),
                new_content: None,
                role: "Entrypoint".to_string(),
            }],
            graph_context: "a -> b".to_string(),
        }
    }

    // ── Core Functionality Tests ──

    #[tokio::test]
    async fn test_record_and_replay_pass1() {
        let tmp = TempDir::new().unwrap();
        let call_count = Arc::new(AtomicUsize::new(0));
        let mock = MockProvider::new(call_count.clone());

        // Record
        let vcr = VcrProvider::new(Box::new(mock), tmp.path().to_path_buf(), VcrMode::Record);
        let response = vcr.annotate_overview(&sample_pass1()).await.unwrap();
        assert_eq!(response.groups[0].id, "g1");
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Replay with a new mock (should not be called)
        let call_count2 = Arc::new(AtomicUsize::new(0));
        let mock2 = MockProvider::new(call_count2.clone());
        let vcr2 = VcrProvider::new(Box::new(mock2), tmp.path().to_path_buf(), VcrMode::Replay);
        let replayed = vcr2.annotate_overview(&sample_pass1()).await.unwrap();
        assert_eq!(replayed.groups[0].id, "g1");
        assert_eq!(replayed.overall_summary, "Mock overall");
        assert_eq!(call_count2.load(Ordering::SeqCst), 0, "Should not call real provider in replay mode");
    }

    #[tokio::test]
    async fn test_record_and_replay_pass2() {
        let tmp = TempDir::new().unwrap();
        let call_count = Arc::new(AtomicUsize::new(0));
        let mock = MockProvider::new(call_count.clone());

        // Record
        let vcr = VcrProvider::new(Box::new(mock), tmp.path().to_path_buf(), VcrMode::Record);
        let response = vcr.annotate_group(&sample_pass2()).await.unwrap();
        assert_eq!(response.group_id, "g1");
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Replay
        let call_count2 = Arc::new(AtomicUsize::new(0));
        let mock2 = MockProvider::new(call_count2.clone());
        let vcr2 = VcrProvider::new(Box::new(mock2), tmp.path().to_path_buf(), VcrMode::Replay);
        let replayed = vcr2.annotate_group(&sample_pass2()).await.unwrap();
        assert_eq!(replayed.group_id, "g1");
        assert_eq!(replayed.flow_narrative, "Mock narrative");
        assert_eq!(call_count2.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_auto_mode_caches_on_miss() {
        let tmp = TempDir::new().unwrap();
        let call_count = Arc::new(AtomicUsize::new(0));
        let mock = MockProvider::new(call_count.clone());

        let vcr = VcrProvider::new(Box::new(mock), tmp.path().to_path_buf(), VcrMode::Auto);

        // First call: miss → calls provider, caches result
        let r1 = vcr.annotate_overview(&sample_pass1()).await.unwrap();
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        assert_eq!(r1.overall_summary, "Mock overall");

        // Second call: hit → reads from cache
        let r2 = vcr.annotate_overview(&sample_pass1()).await.unwrap();
        assert_eq!(call_count.load(Ordering::SeqCst), 1, "Should use cache on second call");
        assert_eq!(r1, r2);
    }

    #[tokio::test]
    async fn test_auto_mode_pass2_caches_on_miss() {
        let tmp = TempDir::new().unwrap();
        let call_count = Arc::new(AtomicUsize::new(0));
        let mock = MockProvider::new(call_count.clone());

        let vcr = VcrProvider::new(Box::new(mock), tmp.path().to_path_buf(), VcrMode::Auto);

        let r1 = vcr.annotate_group(&sample_pass2()).await.unwrap();
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        let r2 = vcr.annotate_group(&sample_pass2()).await.unwrap();
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        assert_eq!(r1, r2);
    }

    #[tokio::test]
    async fn test_replay_missing_entry_errors() {
        let tmp = TempDir::new().unwrap();
        let call_count = Arc::new(AtomicUsize::new(0));
        let mock = MockProvider::new(call_count.clone());

        let vcr = VcrProvider::new(Box::new(mock), tmp.path().to_path_buf(), VcrMode::Replay);
        let result = vcr.annotate_overview(&sample_pass1()).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            LlmError::ParseResponse(msg) => assert!(msg.contains("VCR replay")),
            other => panic!("Expected ParseResponse, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_replay_missing_pass2_errors() {
        let tmp = TempDir::new().unwrap();
        let call_count = Arc::new(AtomicUsize::new(0));
        let mock = MockProvider::new(call_count.clone());

        let vcr = VcrProvider::new(Box::new(mock), tmp.path().to_path_buf(), VcrMode::Replay);
        let result = vcr.annotate_group(&sample_pass2()).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            LlmError::ParseResponse(msg) => assert!(msg.contains("VCR replay")),
            other => panic!("Expected ParseResponse, got: {:?}", other),
        }
    }

    // ── Judge VCR Tests ──

    #[tokio::test]
    async fn test_record_and_replay_judge() {
        let tmp = TempDir::new().unwrap();
        let call_count = Arc::new(AtomicUsize::new(0));
        let mock = MockProvider::new(call_count.clone());

        // Record
        let vcr = VcrProvider::new(Box::new(mock), tmp.path().to_path_buf(), VcrMode::Record);
        let response = vcr.evaluate_quality(&sample_judge()).await.unwrap();
        assert_eq!(response.overall_score, 4.0);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Replay with a new mock (should not be called)
        let call_count2 = Arc::new(AtomicUsize::new(0));
        let mock2 = MockProvider::new(call_count2.clone());
        let vcr2 = VcrProvider::new(Box::new(mock2), tmp.path().to_path_buf(), VcrMode::Replay);
        let replayed = vcr2.evaluate_quality(&sample_judge()).await.unwrap();
        assert_eq!(replayed.overall_score, 4.0);
        assert_eq!(call_count2.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_auto_mode_judge_caches_on_miss() {
        let tmp = TempDir::new().unwrap();
        let call_count = Arc::new(AtomicUsize::new(0));
        let mock = MockProvider::new(call_count.clone());

        let vcr = VcrProvider::new(Box::new(mock), tmp.path().to_path_buf(), VcrMode::Auto);

        let r1 = vcr.evaluate_quality(&sample_judge()).await.unwrap();
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        let r2 = vcr.evaluate_quality(&sample_judge()).await.unwrap();
        assert_eq!(call_count.load(Ordering::SeqCst), 1, "Should use cache on second call");
        assert_eq!(r1, r2);
    }

    #[tokio::test]
    async fn test_replay_missing_judge_errors() {
        let tmp = TempDir::new().unwrap();
        let call_count = Arc::new(AtomicUsize::new(0));
        let mock = MockProvider::new(call_count.clone());

        let vcr = VcrProvider::new(Box::new(mock), tmp.path().to_path_buf(), VcrMode::Replay);
        let result = vcr.evaluate_quality(&sample_judge()).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            LlmError::ParseResponse(msg) => assert!(msg.contains("VCR replay")),
            other => panic!("Expected ParseResponse, got: {:?}", other),
        }
    }

    // ── Judge Template Hash ──

    #[test]
    fn test_judge_template_hash_deterministic() {
        let h1 = VcrProvider::judge_template_hash();
        let h2 = VcrProvider::judge_template_hash();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn test_judge_template_hash_differs_from_pass1_pass2() {
        let judge = VcrProvider::judge_template_hash();
        let pass1 = VcrProvider::pass1_template_hash();
        let pass2 = VcrProvider::pass2_template_hash();
        assert_ne!(judge, pass1);
        assert_ne!(judge, pass2);
    }

    // ── Different Requests Get Different Cache Entries ──

    #[tokio::test]
    async fn test_different_requests_different_cache() {
        let tmp = TempDir::new().unwrap();
        let call_count = Arc::new(AtomicUsize::new(0));
        let mock = MockProvider::new(call_count.clone());

        let vcr = VcrProvider::new(Box::new(mock), tmp.path().to_path_buf(), VcrMode::Auto);

        let req1 = sample_pass1();
        let mut req2 = sample_pass1();
        req2.diff_summary = "different diff".to_string();

        let _ = vcr.annotate_overview(&req1).await.unwrap();
        let _ = vcr.annotate_overview(&req2).await.unwrap();
        assert_eq!(call_count.load(Ordering::SeqCst), 2, "Different requests should both call provider");
    }

    // ── Cache Key Determinism ──

    #[test]
    fn test_cache_key_deterministic() {
        let k1 = VcrProvider::cache_key("anthropic", "claude-3", "request", "template");
        let k2 = VcrProvider::cache_key("anthropic", "claude-3", "request", "template");
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_cache_key_varies_by_provider() {
        let k1 = VcrProvider::cache_key("anthropic", "model", "request", "template");
        let k2 = VcrProvider::cache_key("openai", "model", "request", "template");
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_cache_key_varies_by_model() {
        let k1 = VcrProvider::cache_key("anthropic", "claude-3", "request", "template");
        let k2 = VcrProvider::cache_key("anthropic", "gpt-4o", "request", "template");
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_cache_key_varies_by_request() {
        let k1 = VcrProvider::cache_key("anthropic", "claude-3", "request_a", "template");
        let k2 = VcrProvider::cache_key("anthropic", "claude-3", "request_b", "template");
        assert_ne!(k1, k2);
    }

    #[test]
    fn test_cache_key_varies_by_template() {
        let k1 = VcrProvider::cache_key("anthropic", "claude-3", "request", "template_v1");
        let k2 = VcrProvider::cache_key("anthropic", "claude-3", "request", "template_v2");
        assert_ne!(k1, k2);
    }

    // ── SHA-256 ──

    #[test]
    fn test_sha256_hex_produces_valid_hex() {
        let hash = VcrProvider::sha256_hex(b"hello");
        assert_eq!(hash.len(), 64); // SHA-256 = 32 bytes = 64 hex chars
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_sha256_hex_known_value() {
        // SHA-256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        let hash = VcrProvider::sha256_hex(b"hello");
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    // ── Provider Delegation ──

    #[tokio::test]
    async fn test_delegates_name_model_tokens() {
        let tmp = TempDir::new().unwrap();
        let call_count = Arc::new(AtomicUsize::new(0));
        let mock = MockProvider::new(call_count);

        let vcr = VcrProvider::new(Box::new(mock), tmp.path().to_path_buf(), VcrMode::Auto);
        assert_eq!(vcr.name(), "mock");
        assert_eq!(vcr.model(), "mock-v1");
        assert_eq!(vcr.max_context_tokens(), 100_000);
    }

    // ── Cache Management ──

    #[tokio::test]
    async fn test_list_entries() {
        let tmp = TempDir::new().unwrap();
        let call_count = Arc::new(AtomicUsize::new(0));
        let mock = MockProvider::new(call_count);

        let vcr = VcrProvider::new(Box::new(mock), tmp.path().to_path_buf(), VcrMode::Record);
        assert!(vcr.list_entries().is_empty());

        let _ = vcr.annotate_overview(&sample_pass1()).await.unwrap();
        assert_eq!(vcr.list_entries().len(), 1);

        let _ = vcr.annotate_group(&sample_pass2()).await.unwrap();
        assert_eq!(vcr.list_entries().len(), 2);
    }

    #[tokio::test]
    async fn test_clear_cache() {
        let tmp = TempDir::new().unwrap();
        let call_count = Arc::new(AtomicUsize::new(0));
        let mock = MockProvider::new(call_count);

        let vcr = VcrProvider::new(Box::new(mock), tmp.path().to_path_buf(), VcrMode::Record);
        let _ = vcr.annotate_overview(&sample_pass1()).await.unwrap();
        assert_eq!(vcr.list_entries().len(), 1);

        vcr.clear_cache().unwrap();
        assert!(vcr.list_entries().is_empty());
    }

    // ── Mode Accessor ──

    #[test]
    fn test_mode_accessor() {
        let tmp = TempDir::new().unwrap();
        let call_count = Arc::new(AtomicUsize::new(0));
        let mock = MockProvider::new(call_count);

        let vcr = VcrProvider::new(Box::new(mock), tmp.path().to_path_buf(), VcrMode::Record);
        assert_eq!(vcr.mode(), VcrMode::Record);
    }

    // ── Cache Entry Serialization ──

    #[test]
    fn test_cache_entry_roundtrip() {
        let entry = CacheEntry {
            provider: "mock".to_string(),
            model: "mock-v1".to_string(),
            request_hash: "abc123".to_string(),
            prompt_template_hash: "def456".to_string(),
            recorded_at: "2026-03-19T00:00:00Z".to_string(),
            response: Pass1Response {
                groups: vec![],
                overall_summary: "test".to_string(),
                suggested_review_order: vec![],
            },
        };
        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: CacheEntry<Pass1Response> = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.provider, "mock");
        assert_eq!(deserialized.response.overall_summary, "test");
    }

    // ── Record Overwrites Existing Cache ──

    #[tokio::test]
    async fn test_record_overwrites_existing() {
        let tmp = TempDir::new().unwrap();
        let call_count = Arc::new(AtomicUsize::new(0));
        let mock = MockProvider::new(call_count.clone());

        let vcr = VcrProvider::new(Box::new(mock), tmp.path().to_path_buf(), VcrMode::Record);

        // Record twice with same request
        let _ = vcr.annotate_overview(&sample_pass1()).await.unwrap();
        let _ = vcr.annotate_overview(&sample_pass1()).await.unwrap();
        assert_eq!(call_count.load(Ordering::SeqCst), 2, "Record mode always calls provider");

        // Only one cache file (overwritten)
        assert_eq!(vcr.list_entries().len(), 1);
    }

    // ── Template Hash Changes Invalidate Cache ──

    #[test]
    fn test_template_hash_changes_invalidate() {
        let tmp = TempDir::new().unwrap();
        let cache_path = tmp.path().join("test_entry.json");

        // Write a cache entry with a different template hash
        let entry = CacheEntry {
            provider: "mock".to_string(),
            model: "mock-v1".to_string(),
            request_hash: "abc".to_string(),
            prompt_template_hash: "old_template_hash".to_string(),
            recorded_at: "2026-01-01T00:00:00Z".to_string(),
            response: Pass1Response {
                groups: vec![],
                overall_summary: "stale".to_string(),
                suggested_review_order: vec![],
            },
        };
        let json = serde_json::to_string_pretty(&entry).unwrap();
        std::fs::write(&cache_path, json).unwrap();

        // Try to read with a different template hash
        let call_count = Arc::new(AtomicUsize::new(0));
        let mock = MockProvider::new(call_count);
        let vcr = VcrProvider::new(Box::new(mock), tmp.path().to_path_buf(), VcrMode::Replay);

        let result = vcr.read_cache::<Pass1Response>(&cache_path, "new_template_hash");
        assert!(result.is_none(), "Should invalidate cache when template hash differs");

        // Same template hash should work
        let result = vcr.read_cache::<Pass1Response>(&cache_path, "old_template_hash");
        assert!(result.is_some());
        assert_eq!(result.unwrap().overall_summary, "stale");
    }

    // ── Cache Dir Accessor ──

    #[test]
    fn test_cache_dir_accessor() {
        let tmp = TempDir::new().unwrap();
        let call_count = Arc::new(AtomicUsize::new(0));
        let mock = MockProvider::new(call_count);

        let vcr = VcrProvider::new(Box::new(mock), tmp.path().to_path_buf(), VcrMode::Auto);
        assert_eq!(vcr.cache_dir(), tmp.path());
    }

    // ── Nonexistent Cache Dir Created Automatically ──

    #[tokio::test]
    async fn test_creates_cache_dir_on_write() {
        let tmp = TempDir::new().unwrap();
        let nested = tmp.path().join("deeply").join("nested").join("cache");
        assert!(!nested.exists());

        let call_count = Arc::new(AtomicUsize::new(0));
        let mock = MockProvider::new(call_count);

        let vcr = VcrProvider::new(Box::new(mock), nested.clone(), VcrMode::Record);
        let _ = vcr.annotate_overview(&sample_pass1()).await.unwrap();
        assert!(nested.exists());
        assert_eq!(vcr.list_entries().len(), 1);
    }

    // ── Property-Based Tests ──

    mod proptests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// SHA-256 output is always 64 hex chars regardless of input.
            #[test]
            fn sha256_always_64_hex(data in prop::collection::vec(any::<u8>(), 0..1000)) {
                let hash = VcrProvider::sha256_hex(&data);
                prop_assert_eq!(hash.len(), 64);
                prop_assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
            }

            /// Cache key is deterministic: same inputs → same key.
            #[test]
            fn cache_key_deterministic(
                provider in "[a-z]{3,10}",
                model in "[a-z0-9-]{3,20}",
                request in ".*",
                template in ".*",
            ) {
                let k1 = VcrProvider::cache_key(&provider, &model, &request, &template);
                let k2 = VcrProvider::cache_key(&provider, &model, &request, &template);
                prop_assert_eq!(k1, k2);
            }

            /// Different inputs produce different cache keys (collision resistance).
            #[test]
            fn cache_key_collision_resistance(
                provider in "[a-z]{3,10}",
                model in "[a-z0-9-]{3,20}",
                request_a in ".{1,100}",
                request_b in ".{1,100}",
                template in ".*",
            ) {
                prop_assume!(request_a != request_b);
                let k1 = VcrProvider::cache_key(&provider, &model, &request_a, &template);
                let k2 = VcrProvider::cache_key(&provider, &model, &request_b, &template);
                prop_assert_ne!(k1, k2);
            }

            /// CacheEntry<Pass1Response> roundtrips through JSON.
            #[test]
            fn cache_entry_serde_roundtrip(
                summary in ".*",
                provider_name in "[a-z]{3,10}",
            ) {
                let entry = CacheEntry {
                    provider: provider_name.clone(),
                    model: "test".to_string(),
                    request_hash: "hash".to_string(),
                    prompt_template_hash: "tmpl".to_string(),
                    recorded_at: "2026-01-01T00:00:00Z".to_string(),
                    response: Pass1Response {
                        groups: vec![],
                        overall_summary: summary.clone(),
                        suggested_review_order: vec![],
                    },
                };
                let json = serde_json::to_string(&entry).unwrap();
                let deser: CacheEntry<Pass1Response> = serde_json::from_str(&json).unwrap();
                prop_assert_eq!(&deser.provider, &provider_name);
                prop_assert_eq!(&deser.response.overall_summary, &summary);
            }

            /// sha256_hex never panics on arbitrary input.
            #[test]
            fn sha256_never_panics(data in prop::collection::vec(any::<u8>(), 0..5000)) {
                let _ = VcrProvider::sha256_hex(&data);
            }

            /// cache_key never panics on arbitrary strings.
            #[test]
            fn cache_key_never_panics(
                a in ".*",
                b in ".*",
                c in ".*",
                d in ".*",
            ) {
                let _ = VcrProvider::cache_key(&a, &b, &c, &d);
            }
        }
    }
}
