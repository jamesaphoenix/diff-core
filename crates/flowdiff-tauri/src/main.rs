// Prevents an additional console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::todo)]
#![deny(clippy::unimplemented)]
#![deny(clippy::dbg_macro)]
#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]

mod commands;

use commands::AppState;

/// Set the macOS dock icon programmatically.
/// This is needed because `cargo tauri dev` runs the binary directly (not as a `.app` bundle),
/// so macOS shows a generic "exec" icon in the dock. Setting it via NSApplication ensures
/// the correct icon appears in both dev and production modes.
#[cfg(target_os = "macos")]
fn set_macos_dock_icon() {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    // Safety: this function is called from the Tauri setup hook which runs on the main thread.
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };

    let icon_bytes = include_bytes!("../icons/icon.png");
    let data = NSData::with_bytes(icon_bytes);
    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
        let app = NSApplication::sharedApplication(mtm);
        unsafe { app.setApplicationIconImage(Some(&image)) };
    }
}

fn main() {
    if let Err(e) = tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(AppState::new())
        .setup(|_app| {
            #[cfg(target_os = "macos")]
            set_macos_dock_icon();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::analyze,
            commands::get_last_analysis,
            commands::get_mermaid,
            commands::get_file_diff,
            commands::annotate_overview,
            commands::annotate_group,
            commands::list_branches,
            commands::list_worktrees,
            commands::get_branch_status,
            commands::get_repo_info,
            commands::check_api_key,
            commands::get_llm_settings,
            commands::save_llm_settings,
            commands::save_api_key,
            commands::clear_api_key,
            commands::refine_groups,
            commands::open_in_editor,
            commands::check_editors_available,
            commands::save_comment,
            commands::delete_comment,
            commands::load_comments,
            commands::export_comments,
            commands::get_ignore_paths,
            commands::save_ignore_paths,
        ])
        .run(tauri::generate_context!())
    {
        log::error!("Fatal: flowdiff failed to start: {}", e);
        std::process::exit(1);
    }
}
