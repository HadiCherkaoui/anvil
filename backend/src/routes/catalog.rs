//! `/api/catalog/*` — unified `CurseForge` + Modrinth catalog.
//!
//! `GET /api/catalog/search` fans out to Modrinth (always-on) + `CurseForge`
//! (when configured) for modpacks; Modrinth-only for individual mods.
//! `GET /api/catalog/projects/{provider}/{id}/versions` lists installable
//! versions of one project, filtered by the requested loader / mc.

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::error::AppError;
use crate::modpack::cf_client::{CfFile, CfProject};
use crate::modpack::mr_client::{MrFile, MrSearchHit, MrVersion, SearchQuery};
use crate::validation::{
    validate_catalog_provider, validate_modrinth_id_or_slug, validate_runtime,
    validate_search_query,
};

/// `GET /api/catalog/search` query.
#[derive(Debug, Deserialize)]
pub struct SearchParams {
    /// `mod` | `modpack` | `plugin`
    #[serde(rename = "type")]
    pub kind: String,
    pub q: String,
    #[serde(default)]
    pub loader: Option<String>,
    #[serde(default)]
    pub mc: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
}

/// One row in the merged catalog response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CatalogHit {
    pub provider: &'static str,
    pub project_id: String,
    pub slug: String,
    pub name: String,
    pub summary: String,
    pub icon_url: Option<String>,
    pub downloads: u64,
    pub follows: u64,
    pub project_type: String,
    pub loaders: Vec<String>,
    pub game_versions: Vec<String>,
    pub author: Option<String>,
    pub updated: String,
}

/// Response body for `GET /api/catalog/search`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SearchResponse {
    pub results: Vec<CatalogHit>,
}

/// `GET /api/catalog/search` handler.
///
/// # Errors
///
/// - 400 `catalog_type_invalid` if `type` is not `mod`, `modpack`, or `plugin`.
/// - 400 `search_query_invalid` if `q` is blank or too long.
/// - 400 `runtime_invalid` if `loader` is set to an unknown value.
#[utoipa::path(
    get,
    path = "/api/catalog/search",
    params(
        ("type" = String, Query, description = "Project type: mod | modpack | plugin"),
        ("q" = String, Query, description = "Search query string"),
        ("loader" = Option<String>, Query, description = "Filter by loader (e.g. fabric, forge)"),
        ("mc" = Option<String>, Query, description = "Filter by Minecraft version"),
        ("limit" = Option<u32>, Query, description = "Max results (1–50, default 20)"),
        ("offset" = Option<u32>, Query, description = "Pagination offset (default 0)")
    ),
    responses(
        (status = 200, description = "Merged catalog search results", body = SearchResponse),
        (status = 400, description = "Invalid query parameters")
    ),
    tag = "catalog"
)]
pub async fn search(
    State(state): State<AppState>,
    Query(p): Query<SearchParams>,
) -> Result<Json<SearchResponse>, AppError> {
    if p.kind != "mod" && p.kind != "modpack" && p.kind != "plugin" {
        return Err(AppError::BadRequest {
            code: "catalog_type_invalid",
            message: format!("type must be mod, modpack, or plugin, got {:?}", p.kind),
        });
    }
    validate_search_query(&p.q)?;
    if let Some(l) = p.loader.as_deref() {
        validate_runtime(l)?;
    }

    let limit = p.limit.unwrap_or(20).clamp(1, 50);
    let offset = p.offset.unwrap_or(0);

    let mut results: Vec<CatalogHit> = Vec::new();

    // Modrinth — mod, modpack, and plugin queries all hit it. CurseForge
    // does not host Bukkit-style plugins meaningfully, so plugin queries
    // are Modrinth-only.
    let mr_q = SearchQuery {
        query: &p.q,
        project_type: match p.kind.as_str() {
            "mod" => "mod",
            "plugin" => "plugin",
            _ => "modpack",
        },
        loader: p.loader.as_deref(),
        game_version: p.mc.as_deref(),
        limit,
        offset,
    };
    match state.mr_client.search(&mr_q).await {
        Ok(hits) => results.extend(hits.into_iter().map(modrinth_hit_to_catalog)),
        Err(e) => tracing::warn!(error = %e, "modrinth search failed"),
    }

    // CurseForge — only for modpacks, only when configured. Full-text
    // search via /mods/search?searchFilter=...&classId=4471 (modpacks).
    if p.kind == "modpack"
        && let Some(cf) = state.cf_client.as_ref()
    {
        match cf.search(&p.q, limit, offset).await {
            Ok(hits) => results.extend(hits.into_iter().map(cf_project_to_catalog)),
            Err(e) => tracing::warn!(error = %e, "curseforge search failed"),
        }
    }

    results.sort_by_key(|r| std::cmp::Reverse(r.downloads));
    Ok(Json(SearchResponse { results }))
}

/// `GET /api/catalog/projects/{provider}/{id}/versions` query.
#[derive(Debug, Deserialize)]
pub struct VersionsParams {
    #[serde(default)]
    pub loader: Option<String>,
    #[serde(default)]
    pub mc: Option<String>,
}

/// One version row in the catalog response.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CatalogVersion {
    pub version_id: String,
    pub version_name: String,
    pub channel: String,
    pub loaders: Vec<String>,
    pub game_versions: Vec<String>,
    pub date_published: String,
    pub primary_filename: String,
    pub primary_url: String,
    pub primary_sha512: Option<String>,
}

/// Response body for the versions endpoint.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VersionsResponse {
    pub versions: Vec<CatalogVersion>,
}

/// `GET /api/catalog/projects/{provider}/{id}/versions` handler.
///
/// # Errors
///
/// - 400 `catalog_provider_invalid` if `provider` is not `curseforge` or `modrinth`.
/// - 400 `runtime_invalid` if `loader` is set to an unknown value.
/// - 400 `cf_id_invalid` if the CF id is not numeric.
/// - 400 `cf_disabled` if a CF query lands while CF is disabled.
/// - 400 `modrinth_unavailable` / `cf_unavailable` on upstream errors.
#[utoipa::path(
    get,
    path = "/api/catalog/projects/{provider}/{id}/versions",
    params(
        ("provider" = String, Path, description = "Catalog provider: modrinth | curseforge"),
        ("id" = String, Path, description = "Project id or slug"),
        ("loader" = Option<String>, Query, description = "Filter by loader"),
        ("mc" = Option<String>, Query, description = "Filter by Minecraft version")
    ),
    responses(
        (status = 200, description = "Installable versions for the project", body = VersionsResponse),
        (status = 400, description = "Invalid provider, id, or upstream error")
    ),
    tag = "catalog"
)]
pub async fn versions(
    State(state): State<AppState>,
    Path((provider, id)): Path<(String, String)>,
    Query(p): Query<VersionsParams>,
) -> Result<Json<VersionsResponse>, AppError> {
    validate_catalog_provider(&provider)?;
    if let Some(l) = p.loader.as_deref() {
        validate_runtime(l)?;
    }

    let versions = match provider.as_str() {
        "modrinth" => {
            validate_modrinth_id_or_slug(&id)?;
            let raw =
                state
                    .mr_client
                    .list_versions(&id)
                    .await
                    .map_err(|e| AppError::BadRequest {
                        code: "modrinth_unavailable",
                        message: format!("modrinth list_versions: {e}"),
                    })?;
            raw.iter()
                .filter(|v| {
                    p.loader
                        .as_deref()
                        .is_none_or(|l| v.loaders.iter().any(|x| x == l))
                })
                .filter(|v| {
                    p.mc.as_deref()
                        .is_none_or(|mc| v.game_versions.iter().any(|x| x == mc))
                })
                .filter_map(|v| {
                    let primary = v.files.iter().find(|f| f.primary)?;
                    Some(mr_version_to_catalog(v, primary))
                })
                .collect()
        }
        "curseforge" => {
            let project_id: u32 = id.parse().map_err(|_| AppError::BadRequest {
                code: "cf_id_invalid",
                message: format!("CurseForge id must be numeric, got {id:?}"),
            })?;
            let cf = state.cf_client.as_ref().ok_or(AppError::BadRequest {
                code: "cf_disabled",
                message: "CurseForge support is not enabled".to_owned(),
            })?;
            let files = cf
                .list_files(project_id)
                .await
                .map_err(|e| AppError::BadRequest {
                    code: "cf_unavailable",
                    message: format!("curseforge list_files: {e}"),
                })?;
            files.iter().map(cf_file_to_catalog).collect()
        }
        _ => unreachable!("validated above"),
    };

    Ok(Json(VersionsResponse { versions }))
}

fn modrinth_hit_to_catalog(h: MrSearchHit) -> CatalogHit {
    CatalogHit {
        provider: "modrinth",
        project_id: h.project_id,
        slug: h.slug,
        name: h.title,
        summary: h.description,
        icon_url: h.icon_url,
        downloads: h.downloads,
        follows: h.follows,
        project_type: h.project_type,
        loaders: h
            .display_categories
            .into_iter()
            .filter(|c| {
                matches!(
                    c.as_str(),
                    "fabric" | "forge" | "neoforge" | "paper" | "quilt"
                )
            })
            .collect(),
        game_versions: h.versions,
        author: Some(h.author),
        updated: h.date_modified,
    }
}

fn cf_project_to_catalog(p: CfProject) -> CatalogHit {
    use std::collections::BTreeSet;

    let mut loaders: BTreeSet<&'static str> = BTreeSet::new();
    let mut game_versions: BTreeSet<String> = BTreeSet::new();
    for f in &p.latest_files_indexes {
        if let Some(name) = f.mod_loader.and_then(cf_loader_id_to_name) {
            loaders.insert(name);
        }
        if !f.game_version.is_empty() {
            game_versions.insert(f.game_version.clone());
        }
    }

    CatalogHit {
        provider: "curseforge",
        project_id: p.id.to_string(),
        slug: p.slug,
        name: p.name,
        summary: p.summary,
        icon_url: p.logo.map(|l| l.url),
        downloads: p.download_count,
        follows: p.thumbs_up_count,
        project_type: "modpack".to_owned(),
        loaders: loaders.into_iter().map(str::to_owned).collect(),
        game_versions: game_versions.into_iter().collect(),
        author: p.authors.into_iter().next().map(|a| a.name),
        updated: p.date_modified,
    }
}

/// Maps `CurseForge` `modLoader` enum ids to the loader names the panel uses.
/// Per the public `CurseForge` schema: 1 `Forge`, 4 `Fabric`, 5 `Quilt`,
/// 6 `NeoForge`. Unknown ids are dropped silently — the catalog UI just
/// shows fewer chips.
fn cf_loader_id_to_name(id: u8) -> Option<&'static str> {
    match id {
        1 => Some("forge"),
        4 => Some("fabric"),
        5 => Some("quilt"),
        6 => Some("neoforge"),
        _ => None,
    }
}

fn mr_version_to_catalog(v: &MrVersion, primary: &MrFile) -> CatalogVersion {
    CatalogVersion {
        version_id: v.id.clone(),
        version_name: v.name.clone(),
        channel: v.version_type.clone(),
        loaders: v.loaders.clone(),
        game_versions: v.game_versions.clone(),
        date_published: v.date_published.clone(),
        primary_filename: primary.filename.clone(),
        primary_url: primary.url.clone(),
        primary_sha512: primary.hashes.sha512.clone(),
    }
}

fn cf_file_to_catalog(f: &CfFile) -> CatalogVersion {
    let channel = match f.release_type {
        1 => "release",
        2 => "beta",
        _ => "alpha",
    };
    // CF interleaves loader labels and MC versions in one array — split
    // them so the UI chips match the Modrinth shape (lowercase loaders).
    let (loaders, game_versions): (Vec<String>, Vec<String>) = f
        .game_versions
        .iter()
        .cloned()
        .partition(|v| matches!(v.as_str(), "Forge" | "Fabric" | "NeoForge" | "Quilt"));
    CatalogVersion {
        version_id: f.id.to_string(),
        version_name: f.display_name.clone(),
        channel: channel.to_owned(),
        loaders: loaders.into_iter().map(|l| l.to_lowercase()).collect(),
        game_versions,
        date_published: f.file_date.clone(),
        primary_filename: f.display_name.clone(),
        primary_url: f.download_url.clone().unwrap_or_default(),
        primary_sha512: None,
    }
}
