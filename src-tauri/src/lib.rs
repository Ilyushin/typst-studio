//! Tauri shell: exposes the compilation core to the editor frontend.
//!
//! Each window owns one [`SessionId`]. The frontend creates a session on load,
//! feeds edits into it, and asks for pages to display.

mod commands;

use typst_studio_core::Workspace;

/// Runs the desktop application.
pub fn run() {
    tauri::Builder::default()
        .manage(Workspace::new())
        .invoke_handler(tauri::generate_handler![
            commands::create_session,
            commands::close_session,
            commands::open_document,
            commands::apply_edit,
            commands::compile,
            commands::page_svg,
            commands::open_window,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Typst Studio");
}
