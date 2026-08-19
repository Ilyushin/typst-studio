//! Tauri shell: exposes the compilation core to the editor frontend.
//!
//! Each window owns one [`SessionId`]. The frontend creates a session on load,
//! feeds edits into it, and asks for pages to display.

mod commands;
mod index;
mod watch;

use typst_studio_core::Workspace;

/// Runs the desktop application.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Workspace::new())
        .manage(index::PackageIndex::default())
        .invoke_handler(tauri::generate_handler![
            commands::create_session,
            commands::close_session,
            commands::open_project,
            commands::project_files,
            commands::open_file,
            commands::set_compiled,
            commands::save,
            commands::is_dirty,
            commands::reload,
            commands::export,
            commands::open_document,
            commands::apply_edit,
            commands::compile,
            commands::page_svg,
            commands::complete,
            commands::tooltip,
            commands::jump_from_click,
            commands::jump_from_cursor,
            commands::open_window,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Typst Studio");
}
