//! Shared log setup for the CLI, web server and desktop app.

use std::io::IsTerminal;
use std::path::PathBuf;

use tracing_subscriber::fmt::writer::BoxMakeWriter;
use tracing_subscriber::EnvFilter;

/// Install the global subscriber. Idempotent: later calls are no-ops.
///
/// `RUST_LOG` sets the filter (default `info`), `DIFFCORE_LOG_FORMAT=json`
/// switches renderer. Output goes to stderr when it is a terminal, otherwise
/// to `DIFFCORE_LOG_FILE` or `fallback_file` — GUI bundles discard stderr.
pub fn init(fallback_file: Option<PathBuf>) {
    let raw = std::env::var("RUST_LOG").unwrap_or_default();
    let (spec, bad_filter) = resolve_filter(&raw);
    let filter = EnvFilter::new(&spec);

    let path = resolve_sink(
        std::env::var_os("DIFFCORE_LOG_FILE").map(PathBuf::from),
        fallback_file,
        std::io::stderr().is_terminal(),
    );
    let mut open_error = None;
    let file = path.as_ref().and_then(|path| {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::File::options()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| open_error = Some(format!("{}: {e}", path.display())))
            .ok()
    });

    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(
            file.is_none()
                && std::io::stderr().is_terminal()
                && std::env::var_os("NO_COLOR").is_none(),
        );
    let builder = match file {
        Some(file) => builder.with_writer(BoxMakeWriter::new(std::sync::Mutex::new(file))),
        None => builder.with_writer(BoxMakeWriter::new(std::io::stderr)),
    };

    let _ = if std::env::var("DIFFCORE_LOG_FORMAT").as_deref() == Ok("json") {
        builder.json().flatten_event(true).try_init()
    } else {
        builder.compact().try_init()
    };

    if bad_filter {
        tracing::warn!("ignoring unparseable RUST_LOG {raw:?}, using `info`");
    }
    if let Some(error) = open_error {
        tracing::warn!("cannot open log file {error}; logging to stderr");
    }
}

/// Log destination: `None` means stderr.
///
/// A terminal is the better sink when there is one; `fallback` exists for GUI
/// launches (Finder, `.desktop`, Windows release) that discard stderr.
fn resolve_sink(
    explicit: Option<PathBuf>,
    fallback: Option<PathBuf>,
    stderr_is_terminal: bool,
) -> Option<PathBuf> {
    match explicit {
        Some(path) => Some(path),
        None if stderr_is_terminal => None,
        None => fallback,
    }
}

/// Effective filter spec, and whether `raw` was set but unparseable.
///
/// An empty `RUST_LOG` means "unset", not "silence everything" — a blank value
/// in a `.env` or CI matrix must not blind the process to its own errors.
fn resolve_filter(raw: &str) -> (String, bool) {
    let raw = raw.trim();
    if raw.is_empty() {
        ("info".to_string(), false)
    } else if EnvFilter::try_new(raw).is_err() {
        ("info".to_string(), true)
    } else {
        (raw.to_string(), false)
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_filter, resolve_sink};
    use std::path::PathBuf;

    #[test]
    fn a_terminal_wins_over_the_gui_fallback_file() {
        let explicit = || Some(PathBuf::from("/explicit.log"));
        let fallback = || Some(PathBuf::from("/fallback.log"));

        // Desktop app launched from a terminal: log where the user is looking.
        assert_eq!(resolve_sink(None, fallback(), true), None);
        // Desktop app launched from Finder/.desktop: stderr goes nowhere.
        assert_eq!(resolve_sink(None, fallback(), false), fallback());
        // CLI: stderr either way.
        assert_eq!(resolve_sink(None, None, true), None);
        assert_eq!(resolve_sink(None, None, false), None);
        // An explicit request always wins.
        assert_eq!(resolve_sink(explicit(), fallback(), true), explicit());
        assert_eq!(resolve_sink(explicit(), None, false), explicit());
    }

    #[test]
    fn filter_falls_back_to_info_unless_the_spec_is_usable() {
        assert_eq!(resolve_filter(""), ("info".to_string(), false));
        assert_eq!(resolve_filter("   "), ("info".to_string(), false));
        assert_eq!(resolve_filter("ir_cache=trace"), ("ir_cache=trace".to_string(), false));
        assert_eq!(resolve_filter("ir_cahce=trce=x"), ("info".to_string(), true));
    }
}
