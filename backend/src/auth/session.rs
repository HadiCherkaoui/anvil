//! Session JWT mint + verify (HS256, signed with `ANVIL_SESSION_KEY`).

use crate::auth::types::SessionClaims;
use crate::error::AppError;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};

/// Cookie name for the session JWT.
pub const SESSION_COOKIE: &str = "anvil_session";

/// Cookie name for the short-lived encrypted OIDC-state cookie.
pub const OIDC_STATE_COOKIE: &str = "anvil_oidc_state";

/// Session lifetime — 8 hours.
pub const SESSION_TTL_SECS: i64 = 8 * 60 * 60;

/// OIDC-state cookie lifetime — 10 minutes (the user has that long to finish login).
pub const OIDC_STATE_TTL_SECS: i64 = 10 * 60;

/// Mints an HS256-signed JWT carrying [`SessionClaims`].
///
/// # Errors
///
/// Returns [`AppError::Internal`] if `jsonwebtoken` fails to encode (extremely
/// unlikely; would indicate a serializer bug).
pub fn mint(key: &[u8], claims: &SessionClaims) -> Result<String, AppError> {
    encode(&Header::default(), claims, &EncodingKey::from_secret(key))
        .map_err(|e| AppError::Internal(anyhow::anyhow!("jwt encode: {e}")))
}

/// Verifies an HS256-signed session JWT and returns the embedded claims.
///
/// # Errors
///
/// Returns [`AppError::Unauthorized`] if the signature is wrong, the token is
/// expired, or the encoding is otherwise invalid. The middleware translates
/// this into a 401.
pub fn verify(key: &[u8], token: &str) -> Result<SessionClaims, AppError> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_required_spec_claims(&["exp"]);
    validation.validate_nbf = true;
    let data = decode::<SessionClaims>(token, &DecodingKey::from_secret(key), &validation)
        .map_err(|_| AppError::Unauthorized)?;
    // Reject tokens issued in the future (clock-skew tolerance: 60s). A
    // token with a far-future `iat` would otherwise sit valid until its
    // `exp` — an attacker who minted one once could reuse it indefinitely
    // even after key rotation if they replay before exp.
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    if data.claims.iat > now + 60 {
        return Err(AppError::Unauthorized);
    }
    Ok(data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> i64 {
        jiff::Timestamp::now().as_second()
    }

    fn fixture(exp_offset: i64) -> SessionClaims {
        SessionClaims {
            sub: "u-1".into(),
            name: "Hadi".into(),
            email: "hadi@example.test".into(),
            picture: None,
            iat: now(),
            exp: now() + exp_offset,
        }
    }

    #[test]
    fn round_trip_succeeds() {
        let key = vec![0x42_u8; 32];
        let token = mint(&key, &fixture(60)).unwrap();
        let claims = verify(&key, &token).unwrap();
        assert_eq!(claims.sub, "u-1");
        assert_eq!(claims.email, "hadi@example.test");
    }

    #[test]
    fn wrong_key_fails() {
        let token = mint(&[0x41_u8; 32], &fixture(60)).unwrap();
        assert!(matches!(
            verify(&[0x42_u8; 32], &token),
            Err(AppError::Unauthorized)
        ));
    }

    #[test]
    fn expired_token_fails() {
        // jsonwebtoken's default leeway is 60s — go an hour into the past to
        // be unambiguously expired.
        let key = vec![0x42_u8; 32];
        let token = mint(&key, &fixture(-3600)).unwrap();
        assert!(matches!(verify(&key, &token), Err(AppError::Unauthorized)));
    }

    #[test]
    fn tampered_signature_fails() {
        let key = vec![0x42_u8; 32];
        let token = mint(&key, &fixture(60)).unwrap();
        let mut bytes = token.into_bytes();
        let last = bytes.last_mut().unwrap();
        *last = if *last == b'A' { b'B' } else { b'A' };
        let tampered = String::from_utf8(bytes).unwrap();
        assert!(matches!(
            verify(&key, &tampered),
            Err(AppError::Unauthorized)
        ));
    }
}
