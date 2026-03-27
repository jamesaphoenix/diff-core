#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use flowdiff_tauri::activity_stream::{spawn_sse_server, ActivityEntry, ActivityManager};
use flowdiff_tauri::commands::AppState;
use serde_json::json;
use tokio::time::{sleep, Duration};

#[tokio::test]
async fn sse_endpoint_replays_history_and_streams_completion() {
    let manager = Arc::new(ActivityManager::new());
    let base_url = spawn_sse_server(Arc::clone(&manager)).unwrap();
    let job = manager
        .create_job("overview", "codex", "default", "Summarizing PR")
        .await;

    job.emit(ActivityEntry::info(
        "flowdiff",
        "Preparing overview request",
        Some("flowdiff.prepare".to_string()),
    ))
    .await;

    let later_job = job.clone();
    tokio::spawn(async move {
        sleep(Duration::from_millis(75)).await;
        later_job
            .emit(ActivityEntry::info(
                "codex",
                "Codex started working",
                Some("turn.started".to_string()),
            ))
            .await;
        later_job
            .complete(
                "overview",
                json!({
                    "overall_summary": "Ready",
                    "groups": [],
                    "suggested_review_order": [],
                }),
            )
            .await;
    });

    let response = reqwest::get(format!("{}/llm/jobs/{}/events", base_url, job.job_id()))
        .await
        .unwrap();
    assert!(response.status().is_success());
    let body = response.text().await.unwrap();

    assert!(
        body.contains("event: job_started"),
        "missing job_started event: {}",
        body
    );
    assert!(
        body.contains("Preparing overview request"),
        "missing initial activity: {}",
        body
    );
    assert!(
        body.contains("Codex started working"),
        "missing streamed activity: {}",
        body
    );
    assert!(
        body.contains("event: completed"),
        "missing completed event: {}",
        body
    );
    assert!(
        body.contains("\"result_kind\":\"overview\""),
        "missing result payload: {}",
        body
    );
}

#[tokio::test]
async fn sse_endpoint_returns_not_found_for_unknown_job() {
    let manager = Arc::new(ActivityManager::new());
    let base_url = spawn_sse_server(manager).unwrap();

    let response = reqwest::get(format!("{}/llm/jobs/missing/events", base_url))
        .await
        .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
}

#[test]
fn app_state_create_llm_job_lazily_starts_sse_server() {
    let state = AppState::new();
    let (job, start) = state
        .create_llm_job("overview", "codex", "default", "Summarizing PR")
        .unwrap();

    assert!(
        start.stream_url.contains("/llm/jobs/"),
        "stream url should target the job SSE endpoint: {}",
        start.stream_url
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async {
        let delayed_job = job.clone();
        tokio::spawn(async move {
            sleep(Duration::from_millis(75)).await;
            delayed_job
                .complete(
                    "overview",
                    json!({
                        "overall_summary": "Ready",
                        "groups": [],
                        "suggested_review_order": [],
                    }),
                )
                .await;
        });

        let response = reqwest::get(&start.stream_url).await.unwrap();
        assert!(response.status().is_success());
        let body = response.text().await.unwrap();
        assert!(
            body.contains("event: job_started"),
            "lazy-started SSE endpoint should replay job_started: {}",
            body
        );
        assert!(
            body.contains("event: completed"),
            "lazy-started SSE endpoint should stream completion: {}",
            body
        );
    });
}
