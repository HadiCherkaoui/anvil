//! Static frontend serving (feature-gated).
//!
//! This module is compiled in two flavours, picked at build time via Cargo
//! features:
//!
//! * `serve-dir` — serves `../frontend/out` from disk through
//!   `tower_http::services::ServeDir`. Useful in development: rebuild the
//!   frontend with `pnpm build` and the next request picks up the change
//!   without recompiling Rust.
//! * `embed` — bakes `../frontend/out` into the binary at compile time via
//!   `rust_embed::Embed`. The release container uses this so it ships as a
//!   single file with no on-disk dependency.
//!
//! Enabling **both** features simultaneously is a hard compile error
//! (`compile_error!`); enabling neither is allowed and means the static
//! routes are absent — used by `cargo test`.
//!
//! In every flavour, any GET that doesn't match a real asset falls back to
//! `index.html` so client-side routing in the SPA keeps working on direct
//! deep-links (e.g. `/servers/smp`).

#[cfg(all(feature = "serve-dir", feature = "embed"))]
compile_error!("features `serve-dir` and `embed` are mutually exclusive — pick exactly one");

#[cfg(feature = "serve-dir")]
mod serve_dir_impl {
    use axum::Router;
    use tower_http::services::{ServeDir, ServeFile};

    /// Filesystem path used in the dev `serve-dir` mode.
    ///
    /// Resolved relative to the process working directory. `cargo run` from
    /// `backend/` lands in `<repo>/frontend/out` — see ADR 0003.
    const FRONTEND_OUT: &str = "../frontend/out";

    /// Returns a router whose only job is to serve the static frontend
    /// bundle, falling back to `index.html` for unknown paths so client-side
    /// SPA routing works on direct URL access.
    ///
    /// `ServeDir::fallback` (rather than `not_found_service`) preserves the
    /// inner service's `200 OK` status — the SPA can't render correctly if
    /// every deep-link reaches the page with a `404` (browser dev tools and
    /// some kube probes treat it as a real failure).
    pub fn router() -> Router {
        let index = format!("{FRONTEND_OUT}/index.html");
        Router::new().fallback_service(ServeDir::new(FRONTEND_OUT).fallback(ServeFile::new(index)))
    }
}

#[cfg(feature = "embed")]
mod embed_impl {
    use axum::Router;
    use axum::body::Body;
    use axum::http::{HeaderValue, StatusCode, Uri, header};
    use axum::response::{IntoResponse, Response};
    use rust_embed::Embed;

    /// Embedded copy of the Next.js static export. The path is evaluated
    /// relative to `CARGO_MANIFEST_DIR` at compile time, so the directory
    /// **must exist** when building the release binary — even if empty,
    /// hence the placeholder `frontend/out/.gitkeep` in the repo.
    #[derive(Embed)]
    #[folder = "../frontend/out"]
    struct Assets;

    /// Returns a router whose fallback resolves any non-API GET to either
    /// the matching embedded asset or `index.html` (SPA fallback).
    pub fn router() -> Router {
        Router::new().fallback(serve_embedded)
    }

    /// Maps the request URI to an embedded asset, or to `index.html` when
    /// no asset matches (so client-side routing handles deep-links).
    async fn serve_embedded(uri: Uri) -> Response {
        // Strip the leading slash; rust-embed keys are repository-relative
        // paths without a leading separator.
        let path = uri.path().trim_start_matches('/');

        if path.is_empty() {
            return file_response("index.html");
        }

        Assets::get(path).map_or_else(|| file_response("index.html"), |_| file_response(path))
    }

    /// Builds a 200 response with the embedded asset's bytes and a guessed
    /// content type. Falls back to `404` if the asset is missing — should
    /// only happen if `index.html` itself wasn't bundled, which is a build
    /// error worth surfacing as 404 rather than a panic.
    fn file_response(path: &str) -> Response {
        let Some(file) = Assets::get(path) else {
            return (StatusCode::NOT_FOUND, "asset missing").into_response();
        };

        let mime = mime_guess::from_path(path).first_or_octet_stream();
        let mut response = Response::new(Body::from(file.data));
        if let Ok(value) = HeaderValue::from_str(mime.as_ref()) {
            response.headers_mut().insert(header::CONTENT_TYPE, value);
        }
        response
    }
}

// Re-exports are guarded by `not(other)` so that when someone misuses the
// crate with both features on, the only diagnostic they see is the
// `compile_error!` above — not a noisy E0428/E0252 about duplicate items.
#[cfg(all(feature = "serve-dir", not(feature = "embed")))]
pub use serve_dir_impl::router as static_router;

#[cfg(all(feature = "embed", not(feature = "serve-dir")))]
pub use embed_impl::router as static_router;
