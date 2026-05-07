//! HTTP handlers for the four `/api/auth/*` endpoints.
//!
//! - `GET  /api/auth/login`    → 302 to Authentik authorize URL
//! - `GET  /api/auth/callback` → exchange code, mint session JWT, set cookie, 302 to `/`
//! - `GET  /api/auth/me`       → JSON body from request-extension claims
//! - `POST /api/auth/logout`   → clear cookie, return Authentik end-session URL

use axum::Extension;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::header;
use axum::response::{IntoResponse, Redirect, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar, PrivateCookieJar, SameSite};
use serde::Deserialize;
use serde_json::json;
use time::{Duration, OffsetDateTime};

use crate::AppState;
use crate::auth::oidc::ExchangedIdentity;
use crate::auth::session::{
    OIDC_STATE_COOKIE, OIDC_STATE_TTL_SECS, SESSION_COOKIE, SESSION_TTL_SECS, mint,
};
use crate::auth::types::{MeResponse, OidcStateCookie, SessionClaims};
use crate::error::AppError;

fn session_cookie(value: String) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, value))
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(Duration::seconds(SESSION_TTL_SECS))
        .build()
}

fn state_cookie(value: String) -> Cookie<'static> {
    Cookie::build((OIDC_STATE_COOKIE, value))
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .path("/api/auth")
        .max_age(Duration::seconds(OIDC_STATE_TTL_SECS))
        .build()
}

fn removal(name: &'static str, path: &'static str) -> Cookie<'static> {
    Cookie::build((name, ""))
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .path(path)
        .build()
}

/// `GET /api/auth/login` — redirect to Authentik with PKCE.
///
/// # Errors
///
/// Returns [`AppError`] if OIDC discovery fails or the state cookie cannot
/// be serialised.
pub async fn login(
    State(state): State<AppState>,
    private_jar: PrivateCookieJar,
) -> Result<(PrivateCookieJar, Redirect), AppError> {
    let auth = state.oidc.authorize_url().await?;
    let payload = serde_json::to_string(&auth.state)
        .map_err(|e| AppError::Internal(anyhow::anyhow!("serialize oidc state: {e}")))?;
    let jar = private_jar.add(state_cookie(payload));
    Ok((jar, Redirect::to(auth.url.as_str())))
}

/// Query string of `GET /api/auth/callback`.
#[derive(Debug, Deserialize)]
pub struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// `GET /api/auth/callback` — exchange code, mint session JWT, set cookie.
///
/// On every error path we attach a removal `Set-Cookie` for
/// [`OIDC_STATE_COOKIE`] so the short-lived state doesn't sit for the full
/// 10-minute TTL after a failed exchange.
///
/// # Errors
///
/// - [`AppError::Unauthorized`] for missing/invalid state, code, or ID token.
/// - [`AppError::Forbidden`] when the subject is not in `ANVIL_ALLOWED_SUBS`.
/// - [`AppError::Internal`] for serialisation or discovery failures.
pub async fn callback(
    State(state): State<AppState>,
    private_jar: PrivateCookieJar,
    public_jar: CookieJar,
    Query(params): Query<CallbackParams>,
) -> Response {
    match callback_inner(state, private_jar, public_jar, params).await {
        Ok(ok) => ok.into_response(),
        Err(err) => attach_state_removal(err.into_response()),
    }
}

async fn callback_inner(
    state: AppState,
    private_jar: PrivateCookieJar,
    public_jar: CookieJar,
    params: CallbackParams,
) -> Result<(PrivateCookieJar, CookieJar, Redirect), AppError> {
    if let Some(err) = params.error {
        return Err(AppError::Forbidden {
            code: "oidc_provider_error",
            message: err,
        });
    }
    let code = params.code.ok_or(AppError::Unauthorized)?;
    let csrf = params.state.ok_or(AppError::Unauthorized)?;
    let cookie = private_jar
        .get(OIDC_STATE_COOKIE)
        .ok_or(AppError::Unauthorized)?;
    let stored: OidcStateCookie =
        serde_json::from_str(cookie.value()).map_err(|_| AppError::Unauthorized)?;
    if stored.csrf_state != csrf {
        return Err(AppError::Unauthorized);
    }

    let identity: ExchangedIdentity = state.oidc.exchange(code, &stored).await?;

    if !state.allowed_subs.is_empty() && !state.allowed_subs.iter().any(|s| s == &identity.sub) {
        return Err(AppError::Forbidden {
            code: "sub_not_allowed",
            message: format!("subject {} is not in ANVIL_ALLOWED_SUBS", identity.sub),
        });
    }

    let now = OffsetDateTime::now_utc().unix_timestamp();
    let claims = SessionClaims {
        sub: identity.sub,
        name: identity.name,
        email: identity.email,
        picture: identity.picture,
        iat: now,
        exp: now + SESSION_TTL_SECS,
    };
    let token = mint(&state.session_key, &claims)?;

    let private_jar = private_jar.remove(removal(OIDC_STATE_COOKIE, "/api/auth"));
    let public_jar = public_jar.add(session_cookie(token));
    Ok((private_jar, public_jar, Redirect::to("/")))
}

/// Appends a removal `Set-Cookie` for [`OIDC_STATE_COOKIE`] (path
/// `/api/auth`, http-only, secure, `SameSite=Lax`) so a callback failure
/// doesn't leave the encrypted state cookie sitting client-side for its
/// full 10-minute TTL.
fn attach_state_removal(mut resp: Response) -> Response {
    let cookie = removal(OIDC_STATE_COOKIE, "/api/auth");
    if let Ok(value) = cookie.to_string().parse() {
        resp.headers_mut().append(header::SET_COOKIE, value);
    }
    resp
}

/// `GET /api/auth/me` — current user from the request extension.
#[allow(clippy::unused_async, reason = "axum handlers are uniformly async")]
pub async fn me(Extension(claims): Extension<SessionClaims>) -> Json<MeResponse> {
    Json(MeResponse::from(&claims))
}

/// `POST /api/auth/logout` — clear the session cookie and report Authentik's
/// end-session URL so the frontend can navigate there.
///
/// # Errors
///
/// Returns [`AppError`] if Authentik discovery fails (so we can't surface the
/// end-session endpoint).
pub async fn logout(
    State(state): State<AppState>,
    public_jar: CookieJar,
) -> Result<(CookieJar, Json<serde_json::Value>), AppError> {
    let endpoint = state.oidc.end_session_endpoint().await?;
    let logout_url = endpoint.unwrap_or_else(|| "/".to_owned());
    let cleared = public_jar.remove(removal(SESSION_COOKIE, "/"));
    Ok((cleared, Json(json!({ "logoutUrl": logout_url }))))
}
