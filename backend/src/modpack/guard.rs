//! Drop-guard for one running update.
//!
//! Holds onto the per-server lock entry, the `watch::Sender` that feeds
//! the update WS, and a clone of the bus map; on `Drop` it removes both
//! entries so a panic or early return cannot leave stale state behind.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use super::orchestrator::UpdatePhase;

/// RAII handle owned by the spawned update task.
///
/// Keeping the [`watch::Sender`] alive lets WS clients subscribe to phase
/// transitions; dropping it fires `closed()` on every receiver.
#[derive(Debug)]
pub struct UpdateGuard {
    server_id: String,
    locks: Arc<Mutex<HashSet<String>>>,
    buses: Arc<Mutex<HashMap<String, watch::Receiver<UpdatePhase>>>>,
    sender: Option<watch::Sender<UpdatePhase>>,
}

impl UpdateGuard {
    /// Tries to acquire the lock for `server_id`. Returns `None` if another
    /// update is already running for the same server.
    ///
    /// # Panics
    ///
    /// Panics if either inner Mutex is poisoned (would mean another thread
    /// panicked while holding it — recoverable only by restarting the panel).
    #[must_use]
    pub fn try_acquire(
        server_id: &str,
        locks: Arc<Mutex<HashSet<String>>>,
        buses: Arc<Mutex<HashMap<String, watch::Receiver<UpdatePhase>>>>,
    ) -> Option<Self> {
        {
            let mut guard = locks.lock().expect("update_locks poisoned");
            if guard.contains(server_id) {
                return None;
            }
            guard.insert(server_id.to_owned());
        }
        let (tx, rx) = watch::channel(UpdatePhase::Queued);
        buses
            .lock()
            .expect("update_phase_buses poisoned")
            .insert(server_id.to_owned(), rx);
        Some(Self {
            server_id: server_id.to_owned(),
            locks,
            buses,
            sender: Some(tx),
        })
    }

    /// Sends the next phase to all WS subscribers. Errors are non-fatal —
    /// they only signal that no subscribers are listening yet.
    pub fn emit(&self, phase: UpdatePhase) {
        if let Some(tx) = self.sender.as_ref() {
            // `send` returns Err only when there are no receivers; we keep
            // the latest value either way (`watch::Sender` retains it for
            // late subscribers via `borrow_and_update`).
            let _ = tx.send(phase);
        }
    }

    /// Returns the server id this guard owns the lock for.
    #[must_use]
    pub fn server_id(&self) -> &str {
        &self.server_id
    }
}

impl Drop for UpdateGuard {
    fn drop(&mut self) {
        // Drop the sender first so existing WS subscribers see the channel
        // close; then take ourselves out of both maps. Lock order:
        // buses → locks (matches `try_acquire`).
        self.sender.take();
        if let Ok(mut buses) = self.buses.lock() {
            buses.remove(&self.server_id);
        }
        if let Ok(mut locks) = self.locks.lock() {
            locks.remove(&self.server_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_acquire_succeeds_first_time() {
        let locks = Arc::new(Mutex::new(HashSet::new()));
        let buses = Arc::new(Mutex::new(HashMap::new()));
        let g = UpdateGuard::try_acquire("a", locks.clone(), buses.clone());
        assert!(g.is_some());
        assert!(locks.lock().unwrap().contains("a"));
        assert!(buses.lock().unwrap().contains_key("a"));
    }

    #[test]
    fn try_acquire_blocks_concurrent_for_same_id() {
        let locks = Arc::new(Mutex::new(HashSet::new()));
        let buses = Arc::new(Mutex::new(HashMap::new()));
        let _first = UpdateGuard::try_acquire("a", locks.clone(), buses.clone()).unwrap();
        let second = UpdateGuard::try_acquire("a", locks, buses);
        assert!(second.is_none());
    }

    #[test]
    fn drop_removes_lock_and_bus_entries() {
        let locks = Arc::new(Mutex::new(HashSet::new()));
        let buses = Arc::new(Mutex::new(HashMap::new()));
        {
            let _g = UpdateGuard::try_acquire("a", locks.clone(), buses.clone()).unwrap();
        }
        assert!(!locks.lock().unwrap().contains("a"));
        assert!(!buses.lock().unwrap().contains_key("a"));
    }

    #[test]
    fn emit_lands_on_subscriber() {
        let locks = Arc::new(Mutex::new(HashSet::new()));
        let buses = Arc::new(Mutex::new(HashMap::new()));
        let g = UpdateGuard::try_acquire("a", locks, buses.clone()).unwrap();
        let mut rx = buses.lock().unwrap().get("a").cloned().unwrap();
        g.emit(UpdatePhase::BackingUp);
        assert_eq!(*rx.borrow_and_update(), UpdatePhase::BackingUp);
    }
}
