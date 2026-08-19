//! Fetching the Universe package index in the background.
//!
//! The index is two megabytes and the download blocks, so it happens off the
//! main thread and its absence never holds anything up: completion works
//! without it and improves once it lands.

use std::sync::OnceLock;

use ecow::EcoString;
use tauri::{AppHandle, Emitter, Manager};
use typst::syntax::package::PackageSpec;
use typst_studio_core::{SessionId, Workspace, fetch_package_index};

/// The event announcing that import completion got better.
pub const PACKAGES_READY: &str = "packages-ready";

/// The index, fetched once per process and shared by every window.
#[derive(Default)]
pub struct PackageIndex(OnceLock<Vec<(PackageSpec, Option<EcoString>)>>);

impl PackageIndex {
    /// The packages, if the download has finished.
    pub fn get(&self) -> Option<&[(PackageSpec, Option<EcoString>)]> {
        self.0.get().map(Vec::as_slice)
    }
}

/// Gives a newly created session the index, if it is already here.
pub fn apply_to(app: &AppHandle, session: SessionId) {
    let index = app.state::<PackageIndex>();
    let Some(packages) = index.get() else { return };

    let workspace = app.state::<Workspace>();
    if let Some(handle) = workspace.get(session)
        && let Ok(mut guard) = handle.lock()
    {
        guard.world().set_packages(packages.to_vec());
    }
}

/// Downloads the index on a background thread and hands it to every window.
///
/// Does nothing if it has already been fetched. A failure is not reported to
/// the user: import completion is a convenience, and the compiler reports a
/// missing network properly when a package is actually needed.
pub fn fetch(app: AppHandle) {
    if app.state::<PackageIndex>().get().is_some() {
        return;
    }

    std::thread::spawn(move || {
        let Ok(packages) = fetch_package_index() else { return };

        let index = app.state::<PackageIndex>();
        if index.0.set(packages.clone()).is_err() {
            // Another window won the race; its copy is just as good.
            return;
        }

        for handle in app.state::<Workspace>().all() {
            if let Ok(mut guard) = handle.lock() {
                guard.world().set_packages(packages.clone());
            }
        }

        let _ = app.emit(PACKAGES_READY, packages.len());
    });
}
