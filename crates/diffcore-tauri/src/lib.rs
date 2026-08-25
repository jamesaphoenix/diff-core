#[cfg(all(feature = "desktop", feature = "web"))]
compile_error!("features `desktop` and `web` are mutually exclusive: the web dispatcher constructs the shim State, tauri commands need the real one");

// Re-export commands module for integration tests.
pub mod activity_stream;
pub mod commands;
pub mod runtime;
pub mod state_shim;
#[cfg(feature = "web")]
pub mod web_server;
