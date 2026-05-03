//! `/api/modpack/*` — provider-side helper endpoints.
//!
//! Currently exposes only `GET /api/modpack/curseforge/resolve?slug=…` so the
//! New Server modal can paste a `CurseForge` URL and get back a project id.

use axum::Json;
use axum::extract::{Query, State};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::AppError;
use crate::validation::validate_slug;

/// Query string for the resolve endpoint.
#[derive(Debug, Deserialize)]
pub struct ResolveQuery {
    /// `CurseForge` URL slug (`"all-the-mods-11"`).
    pub slug: String,
}

/// Response body for the resolve endpoint.
#[derive(Debug, Serialize)]
pub struct ResolveResponse {
    pub project_id: u32,
    pub name: String,
    pub slug: String,
}

/// Handler for `GET /api/modpack/curseforge/resolve?slug=…`.
///
/// # Errors
///
/// - 400 `cf_disabled` if `CF_API_KEY` is unset
/// - 400 `cf_url_unparseable` if the slug is empty
/// - 400 `cf_project_not_found` if no MC project matches the slug
pub async fn resolve_curseforge(
    State(state): State<AppState>,
    Query(q): Query<ResolveQuery>,
) -> Result<Json<ResolveResponse>, AppError> {
    let cf = state.cf_client.as_ref().ok_or(AppError::BadRequest {
        code: "cf_disabled",
        message: "CurseForge support is not enabled on this panel".to_owned(),
    })?;
    validate_slug(&q.slug)?;

    let project = cf
        .resolve_slug(q.slug.trim())
        .await
        .map_err(|e| AppError::BadRequest {
            code: "cf_project_not_found",
            message: format!("no project with slug {:?}: {e}", q.slug),
        })?;

    Ok(Json(ResolveResponse {
        project_id: project.id,
        name: project.name,
        slug: project.slug,
    }))
}
