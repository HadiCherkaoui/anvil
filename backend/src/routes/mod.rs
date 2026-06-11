//! HTTP route definitions.

use axum::Json;
use axum::Router;
use axum::middleware::from_fn_with_state;
use axum::routing::get;
use utoipa::OpenApi as _;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::AppState;
use crate::auth::{handlers as auth, require_session};
use crate::openapi::ApiDoc;

pub mod catalog;
pub mod cluster;
pub mod health;
pub mod mc_versions;
pub mod papermc;
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
    let mut api = api_routes(state);

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
///
/// Builds an [`OpenApiRouter`] for the annotated pilot handlers and plain
/// [`axum::Router`] `.route()` calls for everything else. Splits the spec
/// out at the end and mounts `GET /api/openapi.json` on the axum router.
#[allow(
    clippy::too_many_lines,
    reason = "flat list of route registrations; splitting would add indirection without clarity"
)]
fn api_routes(state: AppState) -> Router {
    // Public routes: health is annotated; login + callback redirect and are
    // left as plain .route() entries for now (auth handlers have no schema).
    let public = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(health::get))
        .route("/api/auth/login", get(auth::login))
        .route("/api/auth/callback", get(auth::callback));

    // Protected routes: all annotated JSON handlers use .routes(routes!(...))
    // so their #[utoipa::path] metadata accumulates into the OpenApi spec.
    // Same-path + different-method pairs are merged in one routes!() call.
    // SSE/stream handlers and binary upload/download stay as plain .route()
    // because they must not appear in the JSON spec.
    let protected = OpenApiRouter::new()
        // ── servers list + create (/api/servers) ─────────────────────────
        .routes(routes!(servers::list, servers::create::handle))
        // ── server by id ─────────────────────────────────────────────────
        .routes(routes!(servers::get::handle))
        .routes(routes!(servers::get::handle_by_name))
        .routes(routes!(servers::delete::handle))
        // ── server lifecycle ─────────────────────────────────────────────
        .routes(routes!(servers::start::handle))
        .routes(routes!(servers::stop::handle))
        .routes(routes!(servers::restart::handle))
        // ── rcon / update / version / settings / storage ─────────────────
        .routes(routes!(servers::rcon::handle))
        .routes(routes!(servers::update::handle))
        .routes(routes!(servers::version::handle))
        .routes(routes!(servers::settings::handle))
        .routes(routes!(servers::storage::handle))
        // ── mods ─────────────────────────────────────────────────────────
        .routes(routes!(servers::mods::add_pending))
        .routes(routes!(servers::mods::remove_pending))
        .routes(routes!(servers::mods::apply))
        // SSE stream — no #[utoipa::path], must stay plain .route().
        .route(
            "/api/servers/{id}/mods/apply/stream",
            get(servers::mods::apply_stream),
        )
        // ── plugins (GET+POST on same path merge into one spec entry) ─────
        .routes(routes!(
            servers::plugins::list,
            servers::plugins::add_pending
        ))
        .routes(routes!(servers::plugins::remove_pending))
        .routes(routes!(servers::plugins::apply))
        // SSE stream — no #[utoipa::path], must stay plain .route().
        .route(
            "/api/servers/{id}/plugins/apply/stream",
            get(servers::plugins::apply_stream),
        )
        // ── backups (POST+GET on same path merge into one spec entry) ─────
        .routes(routes!(servers::backups::create, servers::backups::list))
        .routes(routes!(servers::backups::restore))
        .routes(routes!(servers::backups::delete))
        // ── metrics / players ────────────────────────────────────────────
        .routes(routes!(servers::metrics::handle))
        .routes(routes!(servers::players::handle_get))
        .routes(routes!(servers::players::handle_action))
        .routes(routes!(servers::players::handle_broadcast))
        // ── logs ─────────────────────────────────────────────────────────
        .routes(routes!(servers::logs::handle))
        // SSE stream — no #[utoipa::path], keep plain.
        .route(
            "/api/servers/{id}/logs/stream",
            get(servers::logs_stream::handle),
        )
        // SSE stream — no #[utoipa::path], keep plain.
        .route(
            "/api/servers/{id}/update/stream",
            get(servers::update_stream::handle),
        )
        // ── files ─────────────────────────────────────────────────────────
        // list + action + kill_helper are annotated JSON handlers.
        .routes(routes!(servers::files::list))
        .routes(routes!(servers::files::action))
        .routes(routes!(servers::files::kill_helper))
        // upload (PUT multipart) and download (GET binary) are unannotated.
        .route(
            "/api/servers/{id}/files",
            axum::routing::put(servers::files::upload).layer(axum::extract::DefaultBodyLimit::max(
                servers::files::UPLOAD_CAP_USIZE,
            )),
        )
        .route("/api/servers/{id}/files/raw", get(servers::files::download))
        // ── cluster / mc-versions / papermc / runtimes ───────────────────
        .routes(routes!(cluster::handle))
        .routes(routes!(mc_versions::handle))
        .routes(routes!(papermc::handle))
        .routes(routes!(runtimes::handle_versions))
        // ── catalog ───────────────────────────────────────────────────────
        .routes(routes!(catalog::search))
        .routes(routes!(catalog::versions))
        // ── auth (me + logout) ────────────────────────────────────────────
        .routes(routes!(auth::me))
        .routes(routes!(auth::logout))
        .route_layer(from_fn_with_state(state.clone(), require_session));

    // Merge public + protected, then split to extract the accumulated spec.
    let (router, api) = public.merge(protected).with_state(state).split_for_parts();

    // Mount the OpenAPI spec at a stable JSON endpoint. The spec is captured
    // into the closure by value so the router owns it with no Arc needed.
    router.route("/api/openapi.json", get(move || async move { Json(api) }))
}
