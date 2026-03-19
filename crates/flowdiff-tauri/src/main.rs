// Prevents an additional console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use commands::AppState;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(AppState::new())
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running flowdiff");
}
