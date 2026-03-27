#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
//! Live CLI-backed activity tests.
//!
//! These tests verify that Codex CLI and Claude Code emit activity updates while
//! flowdiff requests structured output. They are gated behind
//! `FLOWDIFF_RUN_LIVE_LLM_TESTS=1` and skip cleanly when the corresponding CLI
//! is not installed or authenticated on the local machine.

mod helpers;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use flowdiff_core::config::LlmConfig;
use flowdiff_core::llm;
use flowdiff_core::llm::ActivityUpdate;
use flowdiff_core::llm::LlmProvider;
use helpers::llm_helpers::{load_env, sample_pass1_request, should_run_live};

fn repo_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

async fn capture_cli_activity(
    provider: Box<dyn LlmProvider>,
) -> (
    flowdiff_core::llm::schema::Pass1Response,
    Vec<ActivityUpdate>,
) {
    let updates: Arc<Mutex<Vec<ActivityUpdate>>> = Arc::new(Mutex::new(Vec::new()));
    let updates_for_callback = Arc::clone(&updates);

    let response = llm::with_activity_callback(
        Arc::new(move |update| {
            updates_for_callback.lock().unwrap().push(update);
        }),
        async move {
            provider
                .annotate_overview(&sample_pass1_request())
                .await
                .unwrap()
        },
    )
    .await;

    let updates = updates.lock().unwrap().clone();
    (response, updates)
}

#[tokio::test]
async fn test_live_codex_cli_activity_stream() {
    if !should_run_live() {
        eprintln!("Skipping live Codex CLI test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1 to run)");
        return;
    }
    load_env();

    let status = llm::codex_cli::detect_status();
    if !status.authenticated {
        eprintln!("Skipping Codex CLI activity test (Codex CLI not installed or not logged in)");
        return;
    }

    let config = LlmConfig {
        provider: Some("codex".to_string()),
        model: Some("default".to_string()),
        ..Default::default()
    };
    let provider = llm::create_provider_for_workdir(&config, Some(repo_root().as_path())).unwrap();

    let (response, updates) = capture_cli_activity(provider).await;

    assert!(
        !response.overall_summary.is_empty(),
        "Codex should return structured overview output"
    );
    assert!(
        !updates.is_empty(),
        "Codex should emit activity updates while working"
    );
    assert!(
        updates.iter().any(|update| update.source == "codex"),
        "Expected Codex-sourced updates, got: {:?}",
        updates
            .iter()
            .map(|update| format!("{}: {}", update.source, update.message))
            .collect::<Vec<_>>()
    );
    assert!(
        updates.iter().any(|update| {
            update.message.contains("Started Codex session")
                || update.message.contains("Codex started working")
                || update.message.contains("Codex finished")
                || update.message.contains("Codex is running")
        }),
        "Expected meaningful Codex progress updates, got: {:?}",
        updates
            .iter()
            .map(|update| update.message.clone())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn test_live_claude_cli_activity_stream() {
    if !should_run_live() {
        eprintln!("Skipping live Claude Code test (set FLOWDIFF_RUN_LIVE_LLM_TESTS=1 to run)");
        return;
    }
    load_env();

    let status = llm::claude_cli::detect_status();
    if !status.authenticated {
        eprintln!(
            "Skipping Claude Code activity test (Claude Code not installed or not logged in)"
        );
        return;
    }

    let config = LlmConfig {
        provider: Some("claude".to_string()),
        model: Some("default".to_string()),
        ..Default::default()
    };
    let provider = llm::create_provider_for_workdir(&config, Some(repo_root().as_path())).unwrap();

    let (response, updates) = capture_cli_activity(provider).await;

    assert!(
        !response.overall_summary.is_empty(),
        "Claude should return structured overview output"
    );
    assert!(
        !updates.is_empty(),
        "Claude should emit activity updates while working"
    );
    assert!(
        updates.iter().any(|update| update.source == "claude"),
        "Expected Claude-sourced updates, got: {:?}",
        updates
            .iter()
            .map(|update| format!("{}: {}", update.source, update.message))
            .collect::<Vec<_>>()
    );
    assert!(
        updates.iter().any(|update| {
            update.message.contains("Claude session initialized")
                || update.message.contains("Claude completed")
                || update.message.contains("Claude retrying")
                || update.message.contains("Claude used")
                || update.message == "OK"
        }),
        "Expected meaningful Claude progress updates, got: {:?}",
        updates
            .iter()
            .map(|update| update.message.clone())
            .collect::<Vec<_>>()
    );
}
