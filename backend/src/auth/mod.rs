// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! OIDC authentication wiring.
//!
//! Login is Authorization-Code-with-PKCE against Authentik; the session
//! cookie carries an HS256 JWT signed with `ANVIL_SESSION_KEY`.

pub mod handlers;
pub mod middleware;
pub mod oidc;
pub mod session;
pub mod types;

pub use middleware::require_session;
pub use oidc::OidcState;
