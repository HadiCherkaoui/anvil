//! Anvil binary entry point.
//!
//! Boots a Tokio runtime, initializes tracing, opens the `SQLite` pool,
//! builds the router, and serves it on the configured bind address.
//! SIGTERM / Ctrl-C trigger graceful shutdown so in-flight requests finish.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anvil::auth::OidcState;
use anvil::config::Config;
use anvil::modpack::{self, CurseForgeClient, ModrinthClient};
use anvil::{AppState, db, k8s, router};
use anyhow::{Context as _, Result};
use axum_extra::extract::cookie::Key;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::Mutex as AsyncMutex;
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;
use tracing::Level;
use tracing::event;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::from_env().context("loading configuration")?;
    init_tracing(&config.log_level);

    // Open the SQLite pool and run migrations. The pool is now exposed via
    // AppState so M2 handlers can read/write the `servers` and `audit_log`
    // tables.
    let pool = db::init(&config.database_url)
        .await
        .context("initializing database")?;

    let kube = k8s::try_default_client()
        .await
        .context("initializing kube client")?;
    let oidc = OidcState::new(
        config.oidc_issuer_url.clone(),
        config.oidc_client_id.clone(),
        config.oidc_client_secret.clone(),
        config.oidc_redirect_url.clone(),
    )
    .map_err(|e| anyhow::anyhow!("OIDC client init: {e}"))?;
    let cookie_key = Key::derive_from(&config.session_key);

    // CurseForge client is constructed only when CF_API_KEY is set; CF
    // support stays optional. Modrinth is API-key-free and always-on.
    let cf_client = match config.cf_api_key.as_deref() {
        Some(key) => Some(Arc::new(
            CurseForgeClient::new(key).context("constructing CurseForge client")?,
        )),
        None => None,
    };
    let mr_client = Arc::new(ModrinthClient::new().context("constructing Modrinth client")?);
    let snapshots_pvc = Arc::new(config.modpack_snapshots_pvc.clone());

    let state = AppState {
        kube,
        pool,
        mc_namespace: config.mc_namespace.clone(),
        mc_storage_class: config.mc_storage_class.clone(),
        mc_svc_type: config.mc_svc_type.clone(),
        node_host: config.node_host.clone(),
        loadbalancer_supported: config.loadbalancer_supported,
        capabilities_cache: anvil::routes::cluster::new_cache(),
        mc_versions_cache: anvil::routes::mc_versions::new_cache(),
        loader_version_cache: anvil::routes::runtimes::new_cache(),
        session_key: config.session_key.clone(),
        cookie_key,
        allowed_subs: config.allowed_subs.clone(),
        oidc,
        cf_client,
        mr_client,
        snapshots_pvc,
        modpack_poll_interval: Duration::from_secs(config.modpack_poll_interval_minutes * 60),
        update_locks: Arc::new(Mutex::new(HashSet::new())),
        update_phase_buses: Arc::new(Mutex::new(HashMap::new())),
        snapshot_pvc_lock: Arc::new(AsyncMutex::new(())),
        files_helper_image: config.files_helper_image.clone(),
    };

    // Hourly poller — covers both CF and Modrinth modpack rows. Skips CF
    // rows in-loop when cf_client is None.
    {
        let poller_state = state.clone();
        tokio::spawn(async move {
            modpack::poller::run(poller_state).await;
        });
    }

    let app = router(state)
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http());

    let listener = TcpListener::bind(config.bind_addr)
        .await
        .with_context(|| format!("failed to bind {}", config.bind_addr))?;

    event!(
        name: "anvil.startup",
        Level::INFO,
        bind.addr = %config.bind_addr,
        mc.namespace = config.mc_namespace,
        version = env!("CARGO_PKG_VERSION"),
        "anvil listening on {{bind.addr}}",
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("axum::serve failed")?;

    event!(name: "anvil.shutdown.complete", Level::INFO, "graceful shutdown complete");
    Ok(())
}

/// Sets up `tracing` with the supplied filter (defaults to `info` if unset
/// or unparseable).
fn init_tracing(default_filter: &str) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();
}

/// Resolves once SIGTERM or Ctrl-C arrives. Used to drive
/// `axum::serve::with_graceful_shutdown`.
async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }

    event!(name: "anvil.shutdown.signal", Level::INFO, "shutdown signal received");
}
