//! Programmatic git repo builder and pipeline runner for integration tests.
//!
//! Re-exports from `diffcore_core::eval::fixtures` for backward compatibility
//! with existing integration tests.

pub use diffcore_core::eval::fixtures::{find_feature_branch, run_pipeline, RepoBuilder};
