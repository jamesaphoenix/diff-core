#![cfg(feature = "web")]
//! Integration tests for the diffcore-web HTTP surface.

use std::collections::HashSet;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tower::util::ServiceExt;

fn test_router(ui_dir: &std::path::Path, default_repo: Option<String>) -> axum::Router {
    let allowed: HashSet<String> = ["localhost".to_string(), "127.0.0.1".to_string()].into();
    diffcore_tauri::web_server::router(ui_dir, default_repo, allowed).unwrap()
}

fn ui_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "<html>diffcore</html>").unwrap();
    dir
}

fn test_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    std::fs::write(dir.path().join("a.txt"), "hello\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(std::path::Path::new("a.txt")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("test", "test@localhost").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();
    dir
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

fn invoke_request(cmd: &str, args: Value) -> Request<Body> {
    Request::post(format!("/api/invoke/{cmd}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(args.to_string()))
        .unwrap()
}

#[tokio::test]
async fn health_reports_ok_and_default_repo() {
    let ui = ui_dir();
    let router = test_router(ui.path(), Some("/srv/repo".into()));
    let response = router
        .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["default_repo"], json!("/srv/repo"));
}

#[tokio::test]
async fn invoke_get_repo_info_roundtrip() {
    let ui = ui_dir();
    let repo = test_repo();
    let router = test_router(ui.path(), None);
    let response = router
        .oneshot(invoke_request(
            "get_repo_info",
            json!({ "repoPath": repo.path().to_string_lossy() }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert!(body["current_branch"].is_string(), "unexpected body: {body}");
}

#[tokio::test]
async fn invoke_command_error_maps_to_500() {
    let ui = ui_dir();
    let router = test_router(ui.path(), None);
    let response = router
        .oneshot(invoke_request(
            "get_repo_info",
            json!({ "repoPath": "/nonexistent/nowhere" }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn invoke_missing_arg_maps_to_400() {
    let ui = ui_dir();
    let router = test_router(ui.path(), None);
    let response = router
        .oneshot(invoke_request("get_repo_info", json!({})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unknown_and_desktop_only_commands_map_to_501() {
    let ui = ui_dir();
    let router = test_router(ui.path(), None);
    for cmd in ["definitely_not_a_command", "watch_git_head", "open_in_editor"] {
        let response = router
            .clone()
            .oneshot(invoke_request(cmd, json!({ "repoPath": "x" })))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED, "{cmd}");
    }
}

#[tokio::test]
async fn spa_fallback_serves_index() {
    let ui = ui_dir();
    let router = test_router(ui.path(), None);
    for path in ["/", "/some/spa/route"] {
        let response = router
            .clone()
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{path}");
    }
}

#[tokio::test]
async fn foreign_host_or_origin_is_rejected() {
    let ui = ui_dir();
    let router = test_router(ui.path(), None);
    for (name, value) in [("host", "evil.example:4400"), ("origin", "http://evil.example")] {
        let request = Request::get("/api/health")
            .header(name, value)
            .body(Body::empty())
            .unwrap();
        let response = router.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{name}");
    }
    // allowed host (with port) still passes
    let request = Request::get("/api/health")
        .header("host", "localhost:4400")
        .header("origin", "http://127.0.0.1:4400")
        .body(Body::empty())
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn non_json_body_is_rejected() {
    let ui = ui_dir();
    let router = test_router(ui.path(), None);
    // text/plain (the no-cors CSRF shape) must not reach the dispatcher
    let request = Request::post("/api/invoke/clear_api_key")
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from("{}"))
        .unwrap();
    let response = router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    // malformed JSON errors instead of silently running with defaults
    let request = Request::post("/api/invoke/clear_api_key")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{not json"))
        .unwrap();
    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
