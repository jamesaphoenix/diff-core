use diffcore_core::llm;
use diffcore_core::llm::LlmError;

#[test]
fn interactive_cli_timeout_is_long_enough_for_repo_agents() {
    assert!(
        llm::INTERACTIVE_CLI_TIMEOUT_SECS >= 3_600,
        "interactive CLI timeout regressed to {} seconds",
        llm::INTERACTIVE_CLI_TIMEOUT_SECS
    );
}

#[test]
fn claude_and_codex_share_the_same_interactive_timeout_budget() {
    assert_eq!(
        llm::claude_cli::cli_timeout_secs(),
        llm::INTERACTIVE_CLI_TIMEOUT_SECS
    );
    assert_eq!(
        llm::codex_cli::cli_timeout_secs(),
        llm::INTERACTIVE_CLI_TIMEOUT_SECS
    );
}

#[test]
fn timeout_error_message_reports_the_full_interactive_budget() {
    let message = LlmError::Timeout(llm::INTERACTIVE_CLI_TIMEOUT_SECS).to_string();

    assert!(message.contains("3600"));
    assert!(message.contains("timed out"));
}
