//! Orchestrated MC version change for non-modpack servers.
//!
//! Mirrors `orchestrator::run` shape: announce → stop → backup → swap → start
//! → verify, with auto-rollback on failure. Caller spawns this as a task and
//! it owns the [`UpdateGuard`] until completion. Only `vanilla`, `paper`, and
//! `modded` source kinds are accepted — modpack servers update via the
//! modpack orchestrator.

use anyhow::Result;
use tracing::{event, Level};

use crate::modpack::guard::UpdateGuard;
use crate::modpack::orchestrator::UpdatePhase;
use crate::AppState;

/// Kicks off the version-change FSM for `server_id`.
///
/// Long-running task: spawned by the route handler, runs until completion,
/// drops the [`UpdateGuard`] which releases the per-server lock + WS bus.
pub async fn run(
    state: AppState,
    server_id: String,
    new_mc: String,
    new_loader: Option<String>,
    guard: UpdateGuard,
) {
    let outcome = run_inner(&state, &server_id, &new_mc, new_loader.as_deref(), &guard).await;
    match outcome {
        Ok(()) => {
            guard.emit(UpdatePhase::Succeeded);
            event!(
                name: "anvil.version_change.succeeded",
                Level::INFO,
                server.id = %server_id,
                "version change succeeded",
            );
        }
        Err(err) => {
            event!(
                name: "anvil.version_change.failed",
                Level::ERROR,
                server.id = %server_id,
                err = %err,
                "version change failed",
            );
            guard.emit(UpdatePhase::Failed);
            // Rollback path lands in Task 3.
        }
    }
}

async fn run_inner(
    _state: &AppState,
    _server_id: &str,
    _new_mc: &str,
    _new_loader: Option<&str>,
    _guard: &UpdateGuard,
) -> Result<()> {
    anyhow::bail!("version_change FSM not yet implemented")
}
