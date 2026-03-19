//! Shared test utilities for flowdiff-core integration tests.
//!
//! This module provides common helpers used across multiple integration test files:
//! - `repo_builder` — programmatically create test git repos and run the analysis pipeline
//! - `graph_assertions` — custom assertions for graph structures and analysis output
//! - `llm_helpers` — LLM test utilities (env loading, sample requests, live test gating)

pub mod graph_assertions;
pub mod llm_helpers;
pub mod repo_builder;
