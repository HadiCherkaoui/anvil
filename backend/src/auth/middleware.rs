//! `require_session` axum middleware.
//!
//! Reads the `anvil_session` cookie, verifies the HS256 JWT, enforces
//! `ANVIL_ALLOWED_SUBS`, and attaches [`SessionClaims`] to the request
//! extensions so downstream handlers can pull the current user.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use axum_extra::extract::cookie::CookieJar;

use crate::auth::session::{verify, SESSION_COOKIE};
use crate::auth::types::SessionClaims;
use crate::error::AppError;
use crate::AppState;

/// Rejects requests that don't carry a valid session cookie.
///
/// # Errors
///
/// - [`AppError::Unauthorized`] (401) when the cookie is missing, has a bad
///   signature, or is expired.
/// - [`AppError::Forbidden`] (403) when the embedded subject is not in
///   `ANVIL_ALLOWED_SUBS` (and the allowlist is non-empty).
pub async fn require_session(
    State(state): State<AppState>,
    jar: CookieJar,
    mut req: Request,
    next: Next,
) -> Result<Response, AppError> {
    let cookie = jar.get(SESSION_COOKIE).ok_or(AppError::Unauthorized)?;
    let claims: SessionClaims = verify(&state.session_key, cookie.value())?;
    if !state.allowed_subs.is_empty() && !state.allowed_subs.iter().any(|s| s == &claims.sub) {
        return Err(AppError::Forbidden {
            code: "sub_not_allowed",
            message: format!("subject {} is not in ANVIL_ALLOWED_SUBS", claims.sub),
        });
    }
    req.extensions_mut().insert(claims);
    Ok(next.run(req).await)
}
