//! `OpenAPI` document root for the anvil API.
//!
//! [`ApiDoc`] is the entry point consumed by `utoipa-axum`'s
//! `OpenApiRouter::with_openapi(ApiDoc::openapi())` call in the router.
//! All annotated handlers accumulate into this spec via `routes!(...)`.

/// Root `OpenAPI` document for the anvil panel API.
///
/// Populated at startup by the `utoipa-axum` router; individual handler
/// annotations drive the path/component registration automatically.
#[derive(Debug, utoipa::OpenApi)]
#[openapi(info(title = "anvil API", version = "1.0.0"))]
pub struct ApiDoc;
