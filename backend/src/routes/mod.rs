//! HTTP route definitions.

use axum::Router;
use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};

use crate::AppState;
use crate::auth::{handlers as auth, require_session};

pub mod catalog;
pub mod cluster;
pub mod health;
pub mod mc_versions;
pub mod runtimes;
pub mod servers;

/// Builds the stateful application router (production path).
///
/// Mounts every `/api/*` route, attaches the static-frontend fallback
/// (when a static-serve feature is active), and binds the supplied
/// [`AppState`] to the handlers that need it. The auth middleware is
/// applied to all `/api/*` routes except `/api/health`, `/api/auth/login`,
/// and `/api/auth/callback`.
pub fn router(state: AppState) -> Router {
    #[allow(
        unused_mut,
        reason = "mutated only when a static-serve feature is enabled"
    )]
    let mut api = api_routes(state.clone()).with_state(state);

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
#[allow(
    clippy::too_many_lines,
    reason = "flat list of route registrations; splitting would add indirection without clarity"
)]
fn api_routes(state: AppState) -> Router<AppState> {
    let public = Router::new()
        .route("/api/health", get(health::get))
        .route("/api/auth/login", get(auth::login))
        .route("/api/auth/callback", get(auth::callback));

    let protected = Router::new()
        .route("/api/auth/me", get(auth::me))
        .route("/api/auth/logout", post(auth::logout))
        .route(
            "/api/servers",
            get(servers::list).post(servers::create::handle),
        )
        .route(
            "/api/servers/{id}",
            get(servers::get::handle).delete(servers::delete::handle),
        )
        .route(
            "/api/servers/by-name/{name}",
            get(servers::get::handle_by_name),
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
        .route("/api/servers/{id}/update", post(servers::update::handle))
        .route(
            "/api/servers/{id}/update/stream",
            get(servers::update_stream::handle),
        )
        .route(
            "/api/servers/{id}/settings",
            axum::routing::patch(servers::settings::handle),
        )
        .route(
            "/api/servers/{id}/storage",
            axum::routing::patch(servers::storage::handle),
        )
        .route("/api/servers/{id}/mods", post(servers::mods::add_pending))
        .route(
            "/api/servers/{id}/mods/pending/{idx}",
            axum::routing::delete(servers::mods::remove_pending),
        )
        .route("/api/servers/{id}/mods/apply", post(servers::mods::apply))
        .route(
            "/api/servers/{id}/mods/apply/stream",
            get(servers::mods::apply_stream),
        )
        .route(
            "/api/servers/{id}/plugins",
            get(servers::plugins::list).post(servers::plugins::add_pending),
        )
        .route(
            "/api/servers/{id}/plugins/{filename}",
            axum::routing::delete(servers::plugins::remove_pending),
        )
        .route(
            "/api/servers/{id}/plugins/apply",
            post(servers::plugins::apply),
        )
        .route(
            "/api/servers/{id}/plugins/apply/stream",
            get(servers::plugins::apply_stream),
        )
        .route("/api/servers/{id}/metrics", get(servers::metrics::handle))
        .route(
            "/api/servers/{id}/players",
            get(servers::players::handle_get),
        )
        .route(
            "/api/servers/{id}/players/action",
            post(servers::players::handle_action),
        )
        .route(
            "/api/servers/{id}/players/broadcast",
            post(servers::players::handle_broadcast),
        )
        .route(
            "/api/servers/{id}/files",
            get(servers::files::list).put(servers::files::upload).layer(
                axum::extract::DefaultBodyLimit::max(servers::files::UPLOAD_CAP_USIZE),
            ),
        )
        .route("/api/servers/{id}/files/raw", get(servers::files::download))
        .route(
            "/api/servers/{id}/files/action",
            post(servers::files::action),
        )
        .route(
            "/api/servers/{id}/files/helper",
            axum::routing::delete(servers::files::kill_helper),
        )
        .route("/api/cluster/capabilities", get(cluster::handle))
        .route("/api/cluster/mc-versions", get(mc_versions::handle))
        .route(
            "/api/runtimes/{runtime}/versions",
            get(runtimes::handle_versions),
        )
        .route("/api/catalog/search", get(catalog::search))
        .route(
            "/api/catalog/projects/{provider}/{id}/versions",
            get(catalog::versions),
        )
        .route_layer(from_fn_with_state(state, require_session));

    public.merge(protected)
}
