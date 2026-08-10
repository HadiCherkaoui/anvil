// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Drop-guard for one running update.
//!
//! Holds onto the per-server lock entry, the `watch::Sender` that feeds
//! the update WS, and a clone of the bus map; on `Drop` it removes both
//! entries so a panic or early return cannot leave stale state behind.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use crate::AppState;

use super::orchestrator::UpdatePhase;

/// Stores `err` as the latest failure reason for `server_id`.
///
/// Called by FSM failure handlers immediately before
/// `guard.emit(UpdatePhase::Failed)` so the update WS can include the
/// reason in its `done{result: failed*}` frame. A subsequent
/// [`UpdateGuard::try_acquire`] for the same server clears it.
pub fn set_update_error(state: &AppState, server_id: &str, err: String) {
    if let Ok(mut map) = state.update_errors.lock() {
        map.insert(server_id.to_owned(), err);
    }
}

/// Removes and returns the last error for `server_id`, if any.
#[must_use]
pub fn take_update_error(state: &AppState, server_id: &str) -> Option<String> {
    state
        .update_errors
        .lock()
        .ok()
        .and_then(|mut m| m.remove(server_id))
}

/// How long a terminal phase stays readable in `update_terminals` after
/// the FSM completes — long enough that a UI that opened a stream right
/// after a 202 still sees the result, short enough that stale rows from
/// an old run don't surface for a fresh subscription.
const RECENT_TERMINAL_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// Records `phase` as the latest terminal for `server_id`. Called by the
/// FSM right before [`UpdateGuard::emit`] for a terminal phase so a
/// late-connecting WS client can still see the result via
/// [`recent_terminal`].
pub fn record_terminal(state: &AppState, server_id: &str, phase: super::orchestrator::UpdatePhase) {
    if !is_terminal(phase) {
        return;
    }
    if let Ok(mut m) = state.update_terminals.lock() {
        m.insert(server_id.to_owned(), (phase, std::time::Instant::now()));
    }
}

/// Returns the last terminal for `server_id` if it landed within
/// `RECENT_TERMINAL_TTL`, garbage-collecting older entries on the way.
#[must_use]
pub fn recent_terminal(
    state: &AppState,
    server_id: &str,
) -> Option<super::orchestrator::UpdatePhase> {
    let mut m = state.update_terminals.lock().ok()?;
    // Drop stale entries opportunistically — keeps the map bounded
    // without a dedicated reaper task.
    m.retain(|_, (_, at)| at.elapsed() < RECENT_TERMINAL_TTL);
    m.get(server_id).map(|(p, _)| *p)
}

fn is_terminal(p: super::orchestrator::UpdatePhase) -> bool {
    matches!(
        p,
        super::orchestrator::UpdatePhase::Succeeded
            | super::orchestrator::UpdatePhase::Failed
            | super::orchestrator::UpdatePhase::RolledBack
    )
}

/// Type alias for the panel-wide map of last-error strings keyed by server id.
pub type UpdateErrorMap = Arc<Mutex<HashMap<String, String>>>;

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
    /// Clears any stale `update_errors` entry for this server so a fresh run
    /// starts with no leftover failure reason.
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
        errors: &UpdateErrorMap,
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
        if let Ok(mut errs) = errors.lock() {
            errs.remove(server_id);
        }
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
        // close; then take ourselves out of both maps (one lock at a time,
        // never both held).
        self.sender.take();
        if let Ok(mut buses) = self.buses.lock() {
            buses.remove(&self.server_id);
        }
        if let Ok(mut locks) = self.locks.lock() {
            locks.remove(&self.server_id);
        }
        // `update_errors` is intentionally NOT cleared here: the WS handler
        // reads the failure reason AFTER the FSM drops the guard; clearing on
        // Drop would race that read. The next `try_acquire` clears it.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh_errors() -> UpdateErrorMap {
        Arc::new(Mutex::new(HashMap::new()))
    }

    #[test]
    fn try_acquire_succeeds_first_time() {
        let locks = Arc::new(Mutex::new(HashSet::new()));
        let buses = Arc::new(Mutex::new(HashMap::new()));
        let errors = fresh_errors();
        let g = UpdateGuard::try_acquire("a", locks.clone(), buses.clone(), &errors);
        assert!(g.is_some());
        assert!(locks.lock().unwrap().contains("a"));
        assert!(buses.lock().unwrap().contains_key("a"));
    }

    #[test]
    fn try_acquire_blocks_concurrent_for_same_id() {
        let locks = Arc::new(Mutex::new(HashSet::new()));
        let buses = Arc::new(Mutex::new(HashMap::new()));
        let errors = fresh_errors();
        let _first = UpdateGuard::try_acquire("a", locks.clone(), buses.clone(), &errors).unwrap();
        let second = UpdateGuard::try_acquire("a", locks, buses, &errors);
        assert!(second.is_none());
    }

    #[test]
    fn drop_removes_lock_and_bus_entries() {
        let locks = Arc::new(Mutex::new(HashSet::new()));
        let buses = Arc::new(Mutex::new(HashMap::new()));
        let errors = fresh_errors();
        {
            let _g = UpdateGuard::try_acquire("a", locks.clone(), buses.clone(), &errors).unwrap();
        }
        assert!(!locks.lock().unwrap().contains("a"));
        assert!(!buses.lock().unwrap().contains_key("a"));
    }

    #[test]
    fn emit_lands_on_subscriber() {
        let locks = Arc::new(Mutex::new(HashSet::new()));
        let buses = Arc::new(Mutex::new(HashMap::new()));
        let errors = fresh_errors();
        let g = UpdateGuard::try_acquire("a", locks, buses.clone(), &errors).unwrap();
        let mut rx = buses.lock().unwrap().get("a").cloned().unwrap();
        g.emit(UpdatePhase::BackingUp);
        assert_eq!(*rx.borrow_and_update(), UpdatePhase::BackingUp);
    }

    #[test]
    fn try_acquire_clears_stale_error_for_same_server() {
        let locks = Arc::new(Mutex::new(HashSet::new()));
        let buses = Arc::new(Mutex::new(HashMap::new()));
        let errors = fresh_errors();
        errors
            .lock()
            .unwrap()
            .insert("a".to_owned(), "stale".to_owned());
        let _g = UpdateGuard::try_acquire("a", locks, buses, &errors).unwrap();
        assert!(!errors.lock().unwrap().contains_key("a"));
    }
}
