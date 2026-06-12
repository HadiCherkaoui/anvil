//! `OpenAPI` document root for the anvil API.
//!
//! [`ApiDoc`] is the entry point consumed by `utoipa-axum`'s
//! `OpenApiRouter::with_openapi(ApiDoc::openapi())` call in the router.
//! All annotated handlers accumulate into this spec via `routes!(...)`.

#[derive(Debug, utoipa::OpenApi)]
#[openapi(info(title = "anvil API", version = "1.0.0"))]
pub struct ApiDoc;
