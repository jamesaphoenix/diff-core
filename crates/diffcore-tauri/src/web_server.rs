//! Headless web server exposing the tauri command surface over HTTP.
//!
//! `POST /api/invoke/{cmd}` mirrors tauri IPC 1:1 (JSON body with camelCase
//! keys), `GET /api/health` signals web mode to the UI, the LLM job SSE routes
//! are merged in at the same paths the desktop app uses, and the built UI is
//! served as a static SPA.

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Request, State as AxumState};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::de::DeserializeOwned;
use serde_json::{json, Map, Value};
use tower_http::services::{ServeDir, ServeFile};

use crate::activity_stream;
use crate::commands::{self, AppState, CommandError};
use crate::state_shim::State;

pub struct WebOptions {
    pub host: String,
    pub port: u16,
    pub ui_dir: PathBuf,
    pub default_repo: Option<String>,
    /// Extra Host-header values to accept (e.g. a reverse-proxy domain).
    pub allowed_hosts: Vec<String>,
}

struct ServerState {
    app: AppState,
    default_repo: Option<String>,
}

/// Full router: API + SSE + static UI. Public for integration tests.
///
/// `allowed_hosts` guards against DNS rebinding and cross-origin requests:
/// requests with a Host or Origin header naming a host outside the set are
/// rejected. Headerless clients (curl, tests) pass. The guard targets
/// browsers, which always send both.
pub fn router(
    ui_dir: &std::path::Path,
    default_repo: Option<String>,
    allowed_hosts: HashSet<String>,
) -> Result<Router, String> {
    let app = AppState::new();
    // Relative base makes `stream_url` come out as "/llm/jobs/{id}/events",
    // served by this same process; also prevents the desktop localhost SSE
    // server from being spawned.
    {
        let mut base = app
            .activity_stream_base_url
            .lock()
            .map_err(|e| e.to_string())?;
        *base = Some(String::new());
    }

    let sse = activity_stream::sse_router(Arc::clone(&app.activity_manager));
    let index = ui_dir.join("index.html");
    let state = Arc::new(ServerState { app, default_repo });

    Ok(Router::new()
        .route("/api/health", get(health))
        .route("/api/invoke/:cmd", post(invoke))
        .with_state(state)
        .merge(sse)
        .fallback_service(ServeDir::new(ui_dir).fallback(ServeFile::new(index)))
        .layer(middleware::from_fn_with_state(
            Arc::new(allowed_hosts),
            host_guard,
        )))
}

/// Strip the port and brackets from a Host/Origin host segment.
fn bare_host(value: &str) -> &str {
    let value = value.strip_prefix("http://").unwrap_or(value);
    let value = value.strip_prefix("https://").unwrap_or(value);
    if let Some(v6) = value.strip_prefix('[') {
        return v6.split(']').next().unwrap_or(v6);
    }
    value.rsplit_once(':').map_or(value, |(host, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) {
            host
        } else {
            value
        }
    })
}

async fn host_guard(
    AxumState(allowed): AxumState<Arc<HashSet<String>>>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    for name in [header::HOST, header::ORIGIN] {
        if let Some(value) = headers.get(&name).and_then(|v| v.to_str().ok()) {
            if !allowed.contains(&bare_host(value).to_ascii_lowercase()) {
                return Err((StatusCode::FORBIDDEN, "host not allowed"));
            }
        }
    }
    Ok(next.run(request).await)
}

pub async fn serve(opts: WebOptions) -> Result<(), String> {
    let mut allowed: HashSet<String> = ["localhost", "127.0.0.1", "::1"]
        .into_iter()
        .map(str::to_string)
        .collect();
    allowed.insert(opts.host.trim_matches(['[', ']']).to_ascii_lowercase());
    allowed.extend(opts.allowed_hosts.iter().map(|h| h.to_ascii_lowercase()));
    let router = router(&opts.ui_dir, opts.default_repo, allowed)?;

    let addr: SocketAddr = format!("{}:{}", opts.host, opts.port)
        .parse()
        .map_err(|e| format!("invalid host/port: {e}"))?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("bind {addr}: {e}"))?;
    let bound = listener.local_addr().map_err(|e| e.to_string())?;
    log::info!("diffcore-web listening on http://{bound}");
    axum::serve(listener, router)
        .await
        .map_err(|e| e.to_string())
}

async fn health(AxumState(state): AxumState<Arc<ServerState>>) -> Json<Value> {
    Json(json!({ "ok": true, "default_repo": state.default_repo }))
}

type InvokeError = (StatusCode, String);

fn bad_args(msg: String) -> InvokeError {
    (StatusCode::BAD_REQUEST, msg)
}

fn unsupported(msg: &str) -> InvokeError {
    (StatusCode::NOT_IMPLEMENTED, msg.to_string())
}

impl From<CommandError> for InvokeError {
    fn from(err: CommandError) -> Self {
        (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
    }
}

type Args = Map<String, Value>;

/// Required argument, camelCase key (mirrors tauri IPC arg naming).
fn req<T: DeserializeOwned>(args: &mut Args, key: &str) -> Result<T, InvokeError> {
    let value = args
        .remove(key)
        .ok_or_else(|| bad_args(format!("missing argument: {key}")))?;
    serde_json::from_value(value).map_err(|e| bad_args(format!("invalid argument {key}: {e}")))
}

/// Optional / defaultable argument: absent or null becomes `T::default()`.
fn opt<T: DeserializeOwned + Default>(args: &mut Args, key: &str) -> Result<T, InvokeError> {
    match args.remove(key) {
        None | Some(Value::Null) => Ok(T::default()),
        Some(value) => serde_json::from_value(value)
            .map_err(|e| bad_args(format!("invalid argument {key}: {e}"))),
    }
}

fn ok<T: serde::Serialize>(value: T) -> Result<Value, InvokeError> {
    serde_json::to_value(value).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

async fn invoke(
    AxumState(state): AxumState<Arc<ServerState>>,
    Path(cmd): Path<String>,
    // Strict Json (correct content-type, valid JSON, rejected otherwise):
    // no-cors text/plain CSRF posts and malformed bodies both fail loudly.
    Json(body): Json<Value>,
) -> Result<Json<Value>, InvokeError> {
    let mut args = match body {
        Value::Object(map) => map,
        Value::Null => Map::new(),
        _ => return Err(bad_args("body must be a JSON object".into())),
    };

    // Async commands run directly on the server runtime.
    match cmd.as_str() {
        "resolve_pr_url" => {
            let url = req(&mut args, "url")?;
            return ok(commands::resolve_pr_url(url).await?).map(Json);
        }
        "cancel_refine_groups" => {
            let job_id = req(&mut args, "jobId")?;
            return ok(commands::cancel_refine_groups(job_id, State(&state.app)).await?).map(Json);
        }
        "annotate_overview" => {
            let (a, b, c) = llm_args(&mut args)?;
            return ok(commands::annotate_overview(a, b, c, State(&state.app)).await?).map(Json);
        }
        "refine_groups" => {
            let (a, b, c) = llm_args(&mut args)?;
            return ok(commands::refine_groups(a, b, c, State(&state.app)).await?).map(Json);
        }
        "annotate_group" => {
            let group_id = req(&mut args, "groupId")?;
            let d = diff_args(&mut args)?;
            let (_, provider, model) = llm_args(&mut args)?;
            return ok(commands::annotate_group(
                group_id,
                d.repo_path,
                d.base,
                d.head,
                d.range,
                d.staged,
                d.unstaged,
                d.include_uncommitted,
                provider,
                model,
                State(&state.app),
            )
            .await?)
            .map(Json);
        }
        _ => {}
    }

    // Sync commands (several are heavy: analysis, git, LLM cache IO) run on
    // the blocking pool.
    let result = tokio::task::spawn_blocking(move || dispatch_sync(&cmd, &mut args, &state.app))
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("task join error: {e}"),
            )
        })??;
    Ok(Json(result))
}

struct DiffArgs {
    repo_path: String,
    base: Option<String>,
    head: Option<String>,
    range: Option<String>,
    staged: bool,
    unstaged: bool,
    include_uncommitted: Option<bool>,
}

fn diff_args(args: &mut Args) -> Result<DiffArgs, InvokeError> {
    Ok(DiffArgs {
        repo_path: req(args, "repoPath")?,
        base: opt(args, "base")?,
        head: opt(args, "head")?,
        range: opt(args, "range")?,
        staged: opt(args, "staged")?,
        unstaged: opt(args, "unstaged")?,
        include_uncommitted: opt(args, "includeUncommitted")?,
    })
}

type LlmArgs = (Option<String>, Option<String>, Option<String>);

fn llm_args(args: &mut Args) -> Result<LlmArgs, InvokeError> {
    Ok((
        opt(args, "repoPath")?,
        opt(args, "llmProvider")?,
        opt(args, "llmModel")?,
    ))
}

fn dispatch_sync(cmd: &str, args: &mut Args, app: &AppState) -> Result<Value, InvokeError> {
    let state = State(app);
    match cmd {
        "analyze" => {
            let d = diff_args(args)?;
            let pr_preview = opt(args, "prPreview")?;
            ok(commands::analyze(
                d.repo_path,
                d.base,
                d.head,
                d.range,
                d.staged,
                d.unstaged,
                pr_preview,
                d.include_uncommitted,
                state,
            )?)
        }
        "get_last_analysis" => ok(commands::get_last_analysis(state)?),
        "get_mermaid" => ok(commands::get_mermaid(req(args, "groupId")?, state)?),
        "get_file_diff" => {
            let d = diff_args(args)?;
            let file_path = req(args, "filePath")?;
            ok(commands::get_file_diff(
                d.repo_path,
                file_path,
                d.base,
                d.head,
                d.range,
                d.staged,
                d.unstaged,
                d.include_uncommitted,
                state,
            )?)
        }
        "start_annotate_overview" => {
            let (a, b, c) = llm_args(args)?;
            ok(commands::start_annotate_overview(a, b, c, state)?)
        }
        "start_refine_groups" => {
            let (a, b, c) = llm_args(args)?;
            ok(commands::start_refine_groups(a, b, c, state)?)
        }
        "start_annotate_group" => {
            let group_id = req(args, "groupId")?;
            let d = diff_args(args)?;
            let (_, provider, model) = llm_args(args)?;
            ok(commands::start_annotate_group(
                group_id,
                d.repo_path,
                d.base,
                d.head,
                d.range,
                d.staged,
                d.unstaged,
                d.include_uncommitted,
                provider,
                model,
                state,
            )?)
        }
        "get_cached_refinement" => ok(commands::get_cached_refinement(
            opt(args, "repoPath")?,
            state,
        )?),
        "store_refinement_cache" => ok(commands::store_refinement_cache(
            req(args, "result")?,
            opt(args, "repoPath")?,
            state,
        )?),
        "list_branches" => ok(commands::list_branches(req(args, "repoPath")?)?),
        "list_worktrees" => ok(commands::list_worktrees(req(args, "repoPath")?)?),
        "get_branch_status" => ok(commands::get_branch_status(req(args, "repoPath")?)?),
        "get_repo_info" => ok(commands::get_repo_info(req(args, "repoPath")?)?),

        "check_api_key" => ok(commands::check_api_key(opt(args, "repoPath")?)?),
        "get_llm_settings" => ok(commands::get_llm_settings(opt(args, "repoPath")?)?),
        "save_llm_settings" => ok(commands::save_llm_settings(
            opt(args, "repoPath")?,
            req(args, "settings")?,
        )?),
        "save_api_key" => ok(commands::save_api_key(
            opt(args, "repoPath")?,
            req(args, "apiKey")?,
        )?),
        "clear_api_key" => ok(commands::clear_api_key(opt(args, "repoPath")?)?),
        "get_ignore_paths" => ok(commands::get_ignore_paths(opt(args, "repoPath")?)?),
        "save_ignore_paths" => ok(commands::save_ignore_paths(
            req(args, "repoPath")?,
            req(args, "paths")?,
        )?),
        "save_comment" => ok(commands::save_comment(
            req(args, "repoPath")?,
            req(args, "analysisHash")?,
            req(args, "comment")?,
        )?),
        "delete_comment" => ok(commands::delete_comment(
            req(args, "repoPath")?,
            req(args, "analysisHash")?,
            req(args, "commentId")?,
        )?),
        "load_comments" => ok(commands::load_comments(
            req(args, "repoPath")?,
            req(args, "analysisHash")?,
        )?),
        "export_comments" => ok(commands::export_comments(
            req(args, "repoPath")?,
            req(args, "analysisHash")?,
        )?),
        "save_comment_cached" => ok(commands::save_comment_cached(
            req(args, "repoPath")?,
            req(args, "comment")?,
        )?),
        "load_comments_cached" => ok(commands::load_comments_cached(req(args, "repoPath")?)?),
        "delete_comment_cached" => ok(commands::delete_comment_cached(
            req(args, "repoPath")?,
            req(args, "commentId")?,
        )?),
        "update_comment_cached" => ok(commands::update_comment_cached(
            req(args, "repoPath")?,
            req(args, "commentId")?,
            req(args, "newText")?,
        )?),
        "import_groups_manifest" => ok(commands::import_groups_manifest(
            req(args, "manifestPath")?,
            state,
        )?),
        "export_groups_manifest" => ok(commands::export_groups_manifest(
            req(args, "outputPath")?,
            state,
        )?),
        "unwatch_manifest" => ok(commands::unwatch_manifest(state)?),
        "unwatch_git_head" => ok(commands::unwatch_git_head(state)?),
        "watch_manifest" | "watch_git_head" => Err(unsupported(
            "file watching is desktop-only; refresh manually in web mode",
        )),
        "open_in_editor" | "check_editors_available" => {
            Err(unsupported("editor integration is desktop-only"))
        }
        _ => Err(unsupported("unknown command")),
    }
}
