fn main() {
    // tauri context generation is only needed for the desktop binary; the web
    // build must not require tauri.conf.json processing.
    if std::env::var_os("CARGO_FEATURE_DESKTOP").is_some() {
        tauri_build::build()
    }
}
