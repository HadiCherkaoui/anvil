// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Authentik OIDC client with a 1h provider-metadata cache.
//!
//! Login = Authorization Code + PKCE; the cookie we issue afterwards is our
//! own HS256 JWT, not the Authentik ID token. JWKS is therefore consulted
//! only at `/callback` (where we verify Authentik's ID token before minting
//! our session token).

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::anyhow;
use openidconnect::core::{
    CoreAuthDisplay, CoreAuthenticationFlow, CoreClaimName, CoreClaimType, CoreClient,
    CoreClientAuthMethod, CoreGrantType, CoreIdTokenClaims, CoreJsonWebKey,
    CoreJweContentEncryptionAlgorithm, CoreJweKeyManagementAlgorithm, CoreResponseMode,
    CoreResponseType, CoreSubjectIdentifierType,
};
use openidconnect::{
    AdditionalProviderMetadata, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl,
    Nonce, PkceCodeChallenge, PkceCodeVerifier, ProviderMetadata, RedirectUrl, Scope,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::warn;

use crate::auth::types::OidcStateCookie;
use crate::error::AppError;

/// Provider metadata is refreshed (re-discovery + JWKS re-fetch) when older than this.
const METADATA_TTL: Duration = Duration::from_hours(1);

/// Authentik exposes `end_session_endpoint` in the discovery doc; teach
/// `openidconnect` about it via [`AdditionalProviderMetadata`].
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct AnvilExtras {
    end_session_endpoint: Option<String>,
}
impl AdditionalProviderMetadata for AnvilExtras {}

type AnvilProviderMetadata = ProviderMetadata<
    AnvilExtras,
    CoreAuthDisplay,
    CoreClientAuthMethod,
    CoreClaimName,
    CoreClaimType,
    CoreGrantType,
    CoreJweContentEncryptionAlgorithm,
    CoreJweKeyManagementAlgorithm,
    CoreJsonWebKey,
    CoreResponseMode,
    CoreResponseType,
    CoreSubjectIdentifierType,
>;

/// Cached, refreshable OIDC client material.
pub struct OidcState {
    issuer: IssuerUrl,
    client_id: ClientId,
    client_secret: ClientSecret,
    redirect_url: RedirectUrl,
    http: reqwest::Client,
    cache: RwLock<Option<Cached>>,
}

impl std::fmt::Debug for OidcState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcState")
            .field("issuer", &self.issuer.as_str())
            .field("client_id", &self.client_id.as_str())
            .field("redirect_url", &self.redirect_url.as_str())
            .finish_non_exhaustive()
    }
}

struct Cached {
    metadata: AnvilProviderMetadata,
    refreshed_at: Instant,
}

/// Output of [`OidcState::authorize_url`].
#[derive(Debug)]
pub struct AuthorizeUrl {
    /// Where to send the user (302).
    pub url: url::Url,
    /// Opaque payload to encrypt into a short-lived state cookie.
    pub state: OidcStateCookie,
}

/// Identity surfaced from the verified Authentik ID token.
#[derive(Debug)]
pub struct ExchangedIdentity {
    pub sub: String,
    pub name: String,
    pub email: String,
    pub picture: Option<String>,
}

impl OidcState {
    /// Builds a new OIDC client wrapper. Discovery is deferred to first use.
    ///
    /// # Errors
    ///
    /// Returns an error if the URLs are syntactically invalid or if the HTTP
    /// client cannot be built.
    pub fn new(
        issuer: String,
        client_id: String,
        client_secret: String,
        redirect_url: String,
    ) -> Result<Arc<Self>, AppError> {
        let http = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| AppError::Internal(anyhow!("reqwest build: {e}")))?;
        Ok(Arc::new(Self {
            issuer: IssuerUrl::new(issuer).map_err(invalid)?,
            client_id: ClientId::new(client_id),
            client_secret: ClientSecret::new(client_secret),
            redirect_url: RedirectUrl::new(redirect_url).map_err(invalid)?,
            http,
            cache: RwLock::new(None),
        }))
    }

    async fn metadata(&self) -> Result<AnvilProviderMetadata, AppError> {
        {
            let guard = self.cache.read().await;
            if let Some(c) = guard.as_ref()
                && c.refreshed_at.elapsed() < METADATA_TTL
            {
                return Ok(c.metadata.clone());
            }
        }
        let mut guard = self.cache.write().await;
        if let Some(c) = guard.as_ref()
            && c.refreshed_at.elapsed() < METADATA_TTL
        {
            return Ok(c.metadata.clone());
        }
        let metadata = AnvilProviderMetadata::discover_async(self.issuer.clone(), &self.http)
            .await
            .map_err(|e| AppError::Internal(anyhow!("OIDC discovery: {e}")))?;
        *guard = Some(Cached {
            metadata: metadata.clone(),
            refreshed_at: Instant::now(),
        });
        Ok(metadata)
    }

    fn build_client(
        &self,
        meta: AnvilProviderMetadata,
    ) -> openidconnect::core::CoreClient<
        openidconnect::EndpointSet,
        openidconnect::EndpointNotSet,
        openidconnect::EndpointNotSet,
        openidconnect::EndpointNotSet,
        openidconnect::EndpointMaybeSet,
        openidconnect::EndpointMaybeSet,
    > {
        CoreClient::from_provider_metadata(
            meta,
            self.client_id.clone(),
            Some(self.client_secret.clone()),
        )
        .set_redirect_uri(self.redirect_url.clone())
    }

    /// Returns Authentik's authorize URL plus the OIDC-state payload to stash
    /// in a short-lived encrypted cookie.
    ///
    /// # Errors
    ///
    /// Returns an error if metadata discovery fails.
    pub async fn authorize_url(&self) -> Result<AuthorizeUrl, AppError> {
        let meta = self.metadata().await?;
        let client = self.build_client(meta);
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let (url, csrf, nonce) = client
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .add_scope(Scope::new("openid".into()))
            .add_scope(Scope::new("email".into()))
            .add_scope(Scope::new("profile".into()))
            .set_pkce_challenge(challenge)
            .url();
        Ok(AuthorizeUrl {
            url,
            state: OidcStateCookie {
                csrf_state: csrf.secret().clone(),
                nonce: nonce.secret().clone(),
                pkce_verifier: verifier.secret().clone(),
            },
        })
    }

    /// Exchanges an authorization code (from `/callback`'s `?code=`) for a
    /// verified ID-token identity.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Unauthorized`] if Authentik refuses the exchange,
    /// the ID token signature doesn't verify, or the nonce doesn't match.
    /// Returns [`AppError::Internal`] for configuration / discovery errors.
    pub async fn exchange(
        &self,
        code: String,
        state: &OidcStateCookie,
    ) -> Result<ExchangedIdentity, AppError> {
        let meta = self.metadata().await?;
        let client = self.build_client(meta);
        let token = client
            .exchange_code(AuthorizationCode::new(code))
            .map_err(|e| AppError::Internal(anyhow!("exchange config: {e}")))?
            .set_pkce_verifier(PkceCodeVerifier::new(state.pkce_verifier.clone()))
            .request_async(&self.http)
            .await
            .map_err(|e| {
                warn!(exchange.error = %e, "OIDC token exchange failed");
                AppError::Unauthorized
            })?;
        let id_token = token
            .extra_fields()
            .id_token()
            .ok_or_else(|| AppError::Internal(anyhow!("no id_token in token response")))?;
        let claims = id_token
            .claims(
                &client.id_token_verifier(),
                &Nonce::new(state.nonce.clone()),
            )
            .map_err(|e| {
                warn!(claims.error = %e, "OIDC ID token claims verification failed");
                AppError::Unauthorized
            })?;
        Ok(extract_identity(claims))
    }

    /// Returns Authentik's `end_session_endpoint` if the discovery document
    /// advertises one.
    ///
    /// # Errors
    ///
    /// Returns an error if metadata discovery fails.
    pub async fn end_session_endpoint(&self) -> Result<Option<String>, AppError> {
        let meta = self.metadata().await?;
        Ok(meta.additional_metadata().end_session_endpoint.clone())
    }
}

fn extract_identity(claims: &CoreIdTokenClaims) -> ExchangedIdentity {
    let sub = claims.subject().as_str().to_owned();
    let email = claims
        .email()
        .map(|e| e.as_str().to_owned())
        .unwrap_or_default();
    let name = claims
        .name()
        .and_then(|n| n.get(None))
        .map(|n| n.as_str().to_owned())
        .or_else(|| claims.preferred_username().map(|p| p.as_str().to_owned()))
        .unwrap_or_else(|| sub.clone());
    let picture = claims
        .picture()
        .and_then(|p| p.get(None))
        .map(|p| p.as_str().to_owned());
    ExchangedIdentity {
        sub,
        name,
        email,
        picture,
    }
}

fn invalid(e: impl std::fmt::Display) -> AppError {
    AppError::Internal(anyhow!("invalid OIDC config: {e}"))
}
