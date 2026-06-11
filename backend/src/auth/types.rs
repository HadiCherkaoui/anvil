//! Shared OIDC/session types.

use serde::{Deserialize, Serialize};

/// Claims encoded into the session JWT.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionClaims {
    /// Authentik user UUID. Stable across logins.
    pub sub: String,
    /// Display name (Authentik `name` claim, falling back to `preferred_username`).
    pub name: String,
    /// Email address (may be empty if Authentik did not return one).
    pub email: String,
    /// Optional avatar URL.
    pub picture: Option<String>,
    /// Issued-at, seconds since the Unix epoch.
    pub iat: i64,
    /// Expiry, seconds since the Unix epoch.
    pub exp: i64,
}

/// Wire shape of `GET /api/auth/me`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MeResponse {
    pub sub: String,
    pub name: String,
    pub email: String,
    pub picture: Option<String>,
}

impl From<&SessionClaims> for MeResponse {
    fn from(c: &SessionClaims) -> Self {
        Self {
            sub: c.sub.clone(),
            name: c.name.clone(),
            email: c.email.clone(),
            picture: c.picture.clone(),
        }
    }
}

/// Encrypted cookie payload spanning `/api/auth/login` → `/api/auth/callback`.
/// Stored under `anvil_oidc_state` via [`PrivateCookieJar`].
///
/// [`PrivateCookieJar`]: axum_extra::extract::cookie::PrivateCookieJar
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcStateCookie {
    /// Random CSRF token Authentik echoes in `?state=`.
    pub csrf_state: String,
    /// Random nonce embedded in the ID token; verified during exchange.
    pub nonce: String,
    /// PKCE code verifier; exchanged with the code at `/callback`.
    pub pkce_verifier: String,
}
