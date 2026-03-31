//! Shared test utilities for diffcore-core integration tests.
//!
//! This module provides common helpers used across multiple integration test files:
//! - `repo_builder` — programmatically create test git repos and run the analysis pipeline
//! - `graph_assertions` — custom assertions for graph structures and analysis output
//! - `llm_helpers` — LLM test utilities (env loading, sample requests, live test gating)
//! - `shared_engine` / `shared_cache` — process-wide shared QueryEngine and IrCache

pub mod graph_assertions;
pub mod llm_helpers;
pub mod repo_builder;

use std::sync::OnceLock;

/// Shared `QueryEngine` instance across all integration tests.
///
/// Lazy query compilation per language happens once and is reused by every test
/// in the same test binary, avoiding redundant `.scm` compilation.
pub fn shared_engine() -> &'static diffcore_core::query_engine::QueryEngine {
    static ENGINE: OnceLock<diffcore_core::query_engine::QueryEngine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        diffcore_core::query_engine::QueryEngine::new().expect("shared QueryEngine init")
    })
}

/// Shared `IrCache` instance across all integration tests.
///
/// Content-addressed (SHA-256 of path + source), so no cross-test pollution.
/// Duplicate fixture files parsed by multiple tests will hit the cache.
pub fn shared_cache() -> &'static diffcore_core::pipeline::IrCache {
    static CACHE: OnceLock<diffcore_core::pipeline::IrCache> = OnceLock::new();
    CACHE.get_or_init(diffcore_core::pipeline::IrCache::new)
}
