use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener as StdTcpListener};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_stream::stream;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::Router;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
use tower_http::cors::{Any, CorsLayer};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub source: String,
    pub level: String,
    pub message: String,
    pub event_type: Option<String>,
    pub timestamp_ms: u64,
}

impl ActivityEntry {
    pub fn info(
        source: impl Into<String>,
        message: impl Into<String>,
        event_type: Option<String>,
    ) -> Self {
        Self {
            source: source.into(),
            level: "info".to_string(),
            message: message.into(),
            event_type,
            timestamp_ms: timestamp_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobEvent {
    JobStarted {
        job_id: String,
        operation: String,
        provider: String,
        model: String,
        title: String,
        timestamp_ms: u64,
    },
    Activity {
        job_id: String,
        entry: ActivityEntry,
    },
    Completed {
        job_id: String,
        result_kind: String,
        result: serde_json::Value,
        timestamp_ms: u64,
    },
    Failed {
        job_id: String,
        error: String,
        timestamp_ms: u64,
    },
}

impl JobEvent {
    fn event_name(&self) -> &'static str {
        match self {
            JobEvent::JobStarted { .. } => "job_started",
            JobEvent::Activity { .. } => "activity",
            JobEvent::Completed { .. } => "completed",
            JobEvent::Failed { .. } => "failed",
        }
    }

    fn is_terminal(&self) -> bool {
        matches!(self, JobEvent::Completed { .. } | JobEvent::Failed { .. })
    }
}

struct JobState {
    history: Vec<JobEvent>,
    sender: broadcast::Sender<JobEvent>,
}

#[derive(Default)]
pub struct ActivityManager {
    jobs: RwLock<HashMap<String, JobState>>,
}

impl ActivityManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn create_job(
        self: &Arc<Self>,
        operation: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
        title: impl Into<String>,
    ) -> JobHandle {
        let job_id = Uuid::new_v4().to_string();
        let operation = operation.into();
        let provider = provider.into();
        let model = model.into();
        let title = title.into();
        let (sender, _) = broadcast::channel(256);
        let started = JobEvent::JobStarted {
            job_id: job_id.clone(),
            operation,
            provider,
            model,
            title,
            timestamp_ms: timestamp_ms(),
        };

        let mut jobs = self.jobs.write().await;
        jobs.insert(
            job_id.clone(),
            JobState {
                history: vec![started],
                sender,
            },
        );

        JobHandle {
            manager: Arc::clone(self),
            job_id,
        }
    }

    pub async fn subscribe(
        &self,
        job_id: &str,
    ) -> Option<(Vec<JobEvent>, broadcast::Receiver<JobEvent>)> {
        let jobs = self.jobs.read().await;
        let job = jobs.get(job_id)?;
        Some((job.history.clone(), job.sender.subscribe()))
    }

    async fn push_event(&self, job_id: &str, event: JobEvent) {
        let mut jobs = self.jobs.write().await;
        if let Some(job) = jobs.get_mut(job_id) {
            job.history.push(event.clone());
            let _ = job.sender.send(event);
        }
    }
}

#[derive(Clone)]
pub struct JobHandle {
    manager: Arc<ActivityManager>,
    job_id: String,
}

impl JobHandle {
    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    pub async fn emit(&self, entry: ActivityEntry) {
        self.manager
            .push_event(
                &self.job_id,
                JobEvent::Activity {
                    job_id: self.job_id.clone(),
                    entry,
                },
            )
            .await;
    }

    pub async fn complete(&self, result_kind: impl Into<String>, result: serde_json::Value) {
        self.manager
            .push_event(
                &self.job_id,
                JobEvent::Completed {
                    job_id: self.job_id.clone(),
                    result_kind: result_kind.into(),
                    result,
                    timestamp_ms: timestamp_ms(),
                },
            )
            .await;
    }

    pub async fn fail(&self, error: impl Into<String>) {
        self.manager
            .push_event(
                &self.job_id,
                JobEvent::Failed {
                    job_id: self.job_id.clone(),
                    error: error.into(),
                    timestamp_ms: timestamp_ms(),
                },
            )
            .await;
    }
}

pub fn spawn_sse_server(manager: Arc<ActivityManager>) -> Result<String, std::io::Error> {
    let listener = StdTcpListener::bind(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)))?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;

    let router = Router::new()
        .route("/llm/jobs/:job_id/events", get(stream_job_events))
        .with_state(manager)
        .layer(CorsLayer::new().allow_origin(Any));

    thread::Builder::new()
        .name("flowdiff-activity-sse".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    log::error!("Failed to create SSE runtime: {}", error);
                    return;
                }
            };

            runtime.block_on(async move {
                let listener = match tokio::net::TcpListener::from_std(listener) {
                    Ok(listener) => listener,
                    Err(error) => {
                        log::error!("Failed to convert SSE listener: {}", error);
                        return;
                    }
                };

                if let Err(error) = axum::serve(listener, router).await {
                    log::error!("Activity SSE server failed: {}", error);
                }
            });
        })?;

    Ok(format!("http://127.0.0.1:{}", address.port()))
}

async fn stream_job_events(
    State(manager): State<Arc<ActivityManager>>,
    Path(job_id): Path<String>,
) -> Result<Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    let Some((history, mut receiver)) = manager.subscribe(&job_id).await else {
        return Err(StatusCode::NOT_FOUND);
    };

    let event_stream = stream! {
        for job_event in history {
            yield Ok(to_sse_event(&job_event));
            if job_event.is_terminal() {
                return;
            }
        }

        loop {
            match receiver.recv().await {
                Ok(job_event) => {
                    yield Ok(to_sse_event(&job_event));
                    if job_event.is_terminal() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
    };

    Ok(Sse::new(event_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(10))
            .text("keepalive"),
    ))
}

fn to_sse_event(job_event: &JobEvent) -> Event {
    Event::default()
        .event(job_event.event_name())
        .data(serde_json::to_string(job_event).unwrap_or_else(|_| "{}".to_string()))
}

fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
