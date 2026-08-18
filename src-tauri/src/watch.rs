//! Watching project files for changes made outside the editor.

use tauri::{AppHandle, Emitter, Manager};
use typst_kit::watcher::Watcher;
use typst_studio_core::{SessionId, Workspace};

/// The event the frontend listens for.
pub const FILES_CHANGED: &str = "files-changed";

/// Watches the files the given session compiles, until the session is closed.
///
/// Runs on its own thread because [`Watcher::wait`] blocks. The thread notices a
/// closed session only after the next file system event, so it can outlive the
/// session for a while; it holds nothing but an app handle, and exits without
/// touching anything.
pub fn watch(app: AppHandle, session: SessionId) {
    std::thread::spawn(move || {
        let Ok(mut watcher) = Watcher::new(None) else { return };

        loop {
            // Watch what the last compilation actually read, so an included
            // file is watched but an unrelated one is not.
            {
                let workspace = app.state::<Workspace>();
                let Some(handle) = workspace.get(session) else { return };
                let Ok(mut guard) = handle.lock() else { return };
                if watcher.update(guard.world().dependencies()).is_err() {
                    return;
                }
            }

            if watcher.wait().is_err() {
                return;
            }

            if app.emit(FILES_CHANGED, session).is_err() {
                return;
            }
        }
    });
}
