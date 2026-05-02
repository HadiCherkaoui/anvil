//! Anvil — k8s-native Minecraft server panel.
//!
//! This crate exposes both an `anvil` binary and a small library surface
//! used by integration tests. Only items that need to be exercised from
//! `tests/*.rs` are public.

use std::fmt;

use kube::Client;

pub mod config;
pub mod db;
pub mod error;
pub mod k8s;
pub mod routes;
#[cfg(any(feature = "serve-dir", feature = "embed"))]
pub mod static_serve;

pub use routes::{router, stateless_router};

/// State shared across handlers.
///
/// Cheap to clone — `Client` and `String` both wrap reference-counted
/// internals — so axum's `State` extractor is fine to use everywhere.
#[derive(Clone)]
pub struct AppState {
    /// Live Kubernetes client (in-cluster SA token *or* local kubeconfig).
    pub kube: Client,
    /// Namespace where managed Minecraft resources live.
    pub mc_namespace: String,
}

// `kube::Client` doesn't impl `Debug`, so the derive on `AppState` would
// fail. Hand-rolling the impl keeps the `missing_debug_implementations`
// lint happy while still hiding the client's internals.
impl fmt::Debug for AppState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppState")
            .field("kube", &"<kube::Client>")
            .field("mc_namespace", &self.mc_namespace)
            .finish()
    }
}
