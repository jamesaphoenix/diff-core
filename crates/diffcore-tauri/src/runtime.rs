//! Process-wide background runtime for LLM jobs and SSE emission.
//!
//! Replaces `tauri::async_runtime` so command logic runs identically under the
//! desktop app and the headless web server.

use std::sync::OnceLock;

use tokio::runtime::{Builder, Runtime};

// ponytail: one global 2-thread runtime for all background jobs; per-job
// runtimes or a configurable pool if LLM concurrency ever needs tuning.
pub fn background() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        #[allow(clippy::expect_used)]
        Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("diffcore-bg")
            .enable_all()
            .build()
            .expect("failed to build background runtime")
    })
}
