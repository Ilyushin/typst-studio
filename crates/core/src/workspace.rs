//! Management of several editor windows in one process.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rustc_hash::FxHashMap;

use crate::Session;

/// Identifies one editor window.
pub type SessionId = u64;

/// Holds the compiler sessions of all open windows.
///
/// Each session sits behind its own lock, so two windows can compile at the
/// same time. The `comemo` cache underneath is process-global and shared, which
/// is a feature: two windows on the same project reuse each other's work.
#[derive(Default)]
pub struct Workspace {
    sessions: Mutex<FxHashMap<SessionId, Arc<Mutex<Session>>>>,
    next: AtomicU64,
}

impl Workspace {
    /// Creates an empty workspace.
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens a new session rooted at the given directory.
    pub fn create(&self, root: PathBuf) -> SessionId {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let mut sessions = self.sessions.lock().unwrap();
        sessions.insert(id, Arc::new(Mutex::new(Session::new(root))));
        Self::rebalance_cache(&sessions);
        id
    }

    /// Looks up a session.
    pub fn get(&self, id: SessionId) -> Option<Arc<Mutex<Session>>> {
        self.sessions.lock().unwrap().get(&id).cloned()
    }

    /// Closes a session, returning whether it existed.
    pub fn close(&self, id: SessionId) -> bool {
        let mut sessions = self.sessions.lock().unwrap();
        let existed = sessions.remove(&id).is_some();
        Self::rebalance_cache(&sessions);
        existed
    }

    /// Every open session, for work that concerns all windows.
    pub fn all(&self) -> Vec<Arc<Mutex<Session>>> {
        self.sessions.lock().unwrap().values().cloned().collect()
    }

    /// The number of open sessions.
    pub fn len(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }

    /// Whether no session is open.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Tells every session how many windows share the cache.
    ///
    /// Eviction counts compilations process-wide, so with several windows a
    /// document's cached layout would age out while its own window sits idle.
    /// Scaling the retention by the number of windows keeps each one as warm as
    /// it would be alone.
    fn rebalance_cache(sessions: &FxHashMap<SessionId, Arc<Mutex<Session>>>) {
        let peers = sessions.len().max(1);
        for session in sessions.values() {
            if let Ok(mut session) = session.lock() {
                session.set_peers(peers);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_are_independent() {
        let workspace = Workspace::new();
        let a = workspace.create(std::env::temp_dir());
        let b = workspace.create(std::env::temp_dir());

        {
            let session = workspace.get(a).unwrap();
            let mut session = session.lock().unwrap();
            session.world().open(None, "= One".into()).unwrap();
            session.preview();
        }
        {
            let session = workspace.get(b).unwrap();
            let mut session = session.lock().unwrap();
            session
                .world()
                .open(None, "= Two\n\n#pagebreak()\n\nSecond".into())
                .unwrap();
            session.preview();
        }

        let one = workspace.get(a).unwrap();
        let two = workspace.get(b).unwrap();
        assert_eq!(one.lock().unwrap().page_count(), 1);
        assert_eq!(two.lock().unwrap().page_count(), 2);
    }

    #[test]
    fn closing_removes_the_session() {
        let workspace = Workspace::new();
        let id = workspace.create(std::env::temp_dir());
        assert_eq!(workspace.len(), 1);

        assert!(workspace.close(id));
        assert!(workspace.is_empty());
        assert!(workspace.get(id).is_none());
        assert!(!workspace.close(id), "closing twice must not succeed");
    }
}
