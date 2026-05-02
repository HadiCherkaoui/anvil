//! HTTP route definitions.

use axum::routing::{get, post};
use axum::Router;

use crate::AppState;

pub mod cluster;
pub mod health;
pub mod servers;

/// Builds the stateful application router (production path).
///
/// Mounts every `/api/*` route, attaches the static-frontend fallback
/// (when a static-serve feature is active), and binds the supplied
/// [`AppState`] to the handlers that need it.
pub fn router(state: AppState) -> Router {
    #[allow(
        unused_mut,
        reason = "mutated only when a static-serve feature is enabled"
    )]
    let mut api = api_routes().with_state(state);

    // Both-features-on is a hard error in `static_serve.rs`; this guard
    // ensures we don't *also* trip a missing-symbol error from the merge.
    #[cfg(any(
        all(feature = "serve-dir", not(feature = "embed")),
        all(feature = "embed", not(feature = "serve-dir")),
    ))]
    {
        api = api.merge(crate::static_serve::static_router());
    }

    api
}

/// Builds the application router *without* state, for tests that hit only
/// state-free endpoints (e.g. `/api/health`).
///
/// Stateful routes are not mounted on this router — calling them would
/// fail to compile, which is the point.
pub fn stateless_router() -> Router {
    Router::new().route("/api/health", get(health::get))
}

/// Internal: routes that exercise [`AppState`].
fn api_routes() -> Router<AppState> {
    Router::new()
        .route("/api/health", get(health::get))
        .route(
            "/api/servers",
            get(servers::list).post(servers::create::handle),
        )
        .route(
            "/api/servers/{id}",
            get(servers::get::handle).delete(servers::delete::handle),
        )
        .route("/api/servers/{id}/start", post(servers::start::handle))
        .route("/api/servers/{id}/stop", post(servers::stop::handle))
        .route("/api/servers/{id}/restart", post(servers::restart::handle))
        .route("/api/servers/{id}/logs", get(servers::logs::handle))
        .route(
            "/api/servers/{id}/logs/stream",
            get(servers::logs_stream::handle),
        )
        .route("/api/servers/{id}/rcon", post(servers::rcon::handle))
        .route("/api/cluster/capabilities", get(cluster::handle))
}
