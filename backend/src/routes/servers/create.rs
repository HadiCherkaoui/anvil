//! `POST /api/servers` — create a managed Minecraft server.
//!
//! Validates the request, allocates a `NodePort` if requested, inserts a
//! `servers` row + audit entry inside a transaction, then synchronously
//! creates the k8s Secret, `StatefulSet` (replicas=0), and Service. Returns
//! `202 Accepted` with the new server's id+name. The user must call
//! `POST /:id/start` afterwards to bring up the pod.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::{Secret, Service};
use kube::api::PostParams;
use kube::Api;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::error::AppError;
use crate::k8s_builders::{
    build_rcon_secret, build_service, build_statefulset, rcon_password, BuildParams,
};
use crate::validation::{
    validate_exposure_mode, validate_mc_version, validate_memory_mi, validate_name,
};
use crate::AppState;

/// Lowest `NodePort` allocated by the panel.
const NODEPORT_MIN: i32 = 30_000;
/// Highest `NodePort` allocated by the panel (inclusive).
const NODEPORT_MAX: i32 = 30_099;
/// Default storage size (GiB) when the request omits the field.
const DEFAULT_STORAGE_SIZE_GI: i64 = 10;
/// Server type for M2 — only vanilla.
const SERVER_TYPE_VANILLA: &str = "vanilla";

/// Request body for `POST /api/servers`.
#[derive(Debug, Deserialize)]
pub struct CreateRequest {
    /// User-facing name (DNS-1123 label).
    pub name: String,
    /// Minecraft version. Must be in `KNOWN_MC_VERSIONS`.
    pub mc_version: String,
    /// Memory budget in MiB. Must be 1024–16384 in 1024-step.
    pub memory_mi: i64,
    /// `loadbalancer` | `nodeport` | `clusterip`. Defaults to the cluster
    /// configuration in `state.mc_svc_type`.
    #[serde(default)]
    pub exposure_mode: Option<String>,
    /// PVC `StorageClass`. `None`/missing => use chart default. Empty string
    /// is treated the same as missing.
    #[serde(default)]
    pub storage_class: Option<String>,
    /// PVC size in GiB. Defaults to 10.
    #[serde(default)]
    pub storage_size_gi: Option<i64>,
}

/// Response body for `POST /api/servers`.
#[derive(Debug, Serialize)]
pub struct CreateResponse {
    pub id: String,
    pub name: String,
}

/// Handler for `POST /api/servers`.
///
/// # Errors
///
/// - 400 `name_invalid` / `memory_invalid` / `mc_version_unknown` /
///   `exposure_mode_invalid`
/// - 502 `lb_unavailable` if `exposure_mode=loadbalancer` and the cluster
///   doesn't support it
/// - 409 `name_taken` if the user-facing name is already in use
/// - 409 `nodeport_range_exhausted` if all 100 `NodePorts` are allocated
/// - 500 on k8s or DB failure
#[allow(
    clippy::too_many_lines,
    reason = "linear orchestration: validate -> reserve -> persist -> create k8s; splitting it up adds noise"
)]
pub async fn handle(
    State(state): State<AppState>,
    Json(request): Json<CreateRequest>,
) -> Result<(StatusCode, Json<CreateResponse>), AppError> {
    // Validate request fields and resolve defaults.
    let CreateRequest {
        name,
        mc_version,
        memory_mi,
        exposure_mode,
        storage_class,
        storage_size_gi,
    } = request;
    validate_name(&name)?;
    validate_memory_mi(memory_mi)?;
    validate_mc_version(&mc_version)?;

    let exposure_mode =
        exposure_mode.map_or_else(|| state.mc_svc_type.to_lowercase(), |m| m.to_lowercase());
    validate_exposure_mode(&exposure_mode)?;

    if exposure_mode == "loadbalancer" && !state.loadbalancer_supported {
        return Err(AppError::LbUnavailable);
    }

    let storage_size_gi = storage_size_gi.unwrap_or(DEFAULT_STORAGE_SIZE_GI);
    if storage_size_gi <= 0 || storage_size_gi > 500 {
        return Err(AppError::BadRequest {
            code: "storage_size_invalid",
            message: format!("storage_size_gi must be in [1..=500], got {storage_size_gi}"),
        });
    }
    // The SQLite column is nullable; an empty string in the request maps to None.
    let storage_class = storage_class.filter(|s| !s.is_empty());
    // Effective StorageClass for the StatefulSet: request override, then chart default,
    // then None (k8s cluster default).
    let effective_storage_class = storage_class.clone().or_else(|| {
        if state.mc_storage_class.is_empty() {
            None
        } else {
            Some(state.mc_storage_class.clone())
        }
    });

    // Reject duplicate names early so we don't have to roll back k8s state.
    if name_exists(&state.pool, &name).await? {
        return Err(AppError::Conflict {
            code: "name_taken",
            message: format!("a server named {name:?} already exists"),
        });
    }

    // Pre-allocate a NodePort if needed.
    let nodeport = if exposure_mode == "nodeport" {
        Some(allocate_nodeport(&state.pool).await?)
    } else {
        None
    };

    let id = Uuid::new_v4().to_string();
    let rcon_pwd = rcon_password();
    let now = Utc::now().timestamp();
    let source_config = json!({}).to_string();

    // Persist metadata + audit entry. If k8s create fails after this, the
    // SQLite row remains; DELETE handler tolerates missing k8s resources.
    insert_server(
        &state.pool,
        &id,
        &name,
        &mc_version,
        memory_mi,
        SERVER_TYPE_VANILLA,
        &exposure_mode,
        storage_class.as_deref(),
        storage_size_gi,
        &source_config,
        nodeport,
        now,
    )
    .await?;
    insert_audit(
        &state.pool,
        &id,
        "created",
        Some(json!({
            "name": name,
            "mc_version": mc_version,
            "memory_mi": memory_mi,
            "exposure_mode": exposure_mode,
            "storage_class": storage_class,
            "storage_size_gi": storage_size_gi,
            "nodeport": nodeport,
        })),
        now,
    )
    .await?;

    // Create k8s objects synchronously: Secret first so the StatefulSet's
    // RCON env var resolves on first scale-up; StatefulSet next; Service
    // last.
    let build_params = BuildParams {
        id: &id,
        name: &name,
        namespace: &state.mc_namespace,
        mc_version: &mc_version,
        memory_mi,
        server_type: SERVER_TYPE_VANILLA,
        exposure_mode: &exposure_mode,
        storage_class: effective_storage_class.as_deref(),
        storage_size_gi,
        nodeport,
        created_at: now,
    };

    let pp = PostParams::default();
    let secrets: Api<Secret> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let stsets: Api<StatefulSet> = Api::namespaced(state.kube.clone(), &state.mc_namespace);
    let services: Api<Service> = Api::namespaced(state.kube.clone(), &state.mc_namespace);

    secrets
        .create(&pp, &build_rcon_secret(&id, &state.mc_namespace, &rcon_pwd))
        .await?;
    stsets
        .create(&pp, &build_statefulset(&build_params))
        .await?;
    services.create(&pp, &build_service(&build_params)).await?;

    Ok((StatusCode::ACCEPTED, Json(CreateResponse { id, name })))
}

/// Returns `true` iff a row with `name` exists in `servers`.
async fn name_exists(pool: &SqlitePool, name: &str) -> Result<bool, AppError> {
    let row: Option<i64> = sqlx::query_scalar("SELECT 1 FROM servers WHERE name = ?")
        .bind(name)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

/// Returns the lowest unused `NodePort` in the configured range.
async fn allocate_nodeport(pool: &SqlitePool) -> Result<i32, AppError> {
    let rows: Vec<i64> = sqlx::query_scalar(
        "SELECT nodeport FROM servers WHERE nodeport IS NOT NULL ORDER BY nodeport ASC",
    )
    .fetch_all(pool)
    .await?;
    let used: std::collections::BTreeSet<i32> = rows
        .into_iter()
        .filter_map(|n| i32::try_from(n).ok())
        .collect();
    for candidate in NODEPORT_MIN..=NODEPORT_MAX {
        if !used.contains(&candidate) {
            return Ok(candidate);
        }
    }
    Err(AppError::Conflict {
        code: "nodeport_range_exhausted",
        message: format!("all NodePorts in [{NODEPORT_MIN}..={NODEPORT_MAX}] are allocated"),
    })
}

/// Persists a new row in `servers`.
///
/// The `name_exists` pre-check is racy with concurrent creates; the
/// `UNIQUE NOT NULL` constraint on `servers.name` is the durable
/// guarantee. A UNIQUE violation here is mapped back to
/// [`AppError::Conflict`] so the client still sees `409 name_taken`
/// rather than a misleading 500.
#[allow(clippy::too_many_arguments)]
async fn insert_server(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    mc_version: &str,
    memory_mi: i64,
    server_type: &str,
    exposure_mode: &str,
    storage_class: Option<&str>,
    storage_size_gi: i64,
    source_config: &str,
    nodeport: Option<i32>,
    created_at: i64,
) -> Result<(), AppError> {
    let result = sqlx::query(
        "INSERT INTO servers (
            id, name, mc_version, memory_mi, server_type, exposure_mode,
            storage_class, storage_size_gi, source_config, nodeport,
            created_at, last_started_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
    )
    .bind(id)
    .bind(name)
    .bind(mc_version)
    .bind(memory_mi)
    .bind(server_type)
    .bind(exposure_mode)
    .bind(storage_class)
    .bind(storage_size_gi)
    .bind(source_config)
    .bind(nodeport.map(i64::from))
    .bind(created_at)
    .execute(pool)
    .await;

    match result {
        Ok(_) => Ok(()),
        Err(sqlx::Error::Database(err)) if err.is_unique_violation() => Err(AppError::Conflict {
            code: "name_taken",
            message: format!("a server named {name:?} already exists"),
        }),
        Err(other) => Err(AppError::DbUnavailable(other)),
    }
}

/// Persists an audit log entry. Used by every mutating handler.
pub(crate) async fn insert_audit(
    pool: &SqlitePool,
    server_id: &str,
    action: &str,
    details: Option<serde_json::Value>,
    ts: i64,
) -> Result<(), AppError> {
    let details_text = details.map(|v| v.to_string());
    sqlx::query(
        "INSERT INTO audit_log (ts, server_id, action, details, actor)
         VALUES (?, ?, ?, ?, NULL)",
    )
    .bind(ts)
    .bind(server_id)
    .bind(action)
    .bind(details_text)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn insert_dummy(pool: &SqlitePool, id: &str, name: &str, nodeport: Option<i32>) {
        insert_server(
            pool,
            id,
            name,
            "1.21.4",
            4096,
            SERVER_TYPE_VANILLA,
            "nodeport",
            None,
            10,
            "{}",
            nodeport,
            0,
        )
        .await
        .expect("insert");
    }

    #[tokio::test]
    async fn name_exists_returns_false_on_empty_db() {
        let pool = db::init("sqlite::memory:").await.unwrap();
        assert!(!name_exists(&pool, "smp").await.unwrap());
    }

    #[tokio::test]
    async fn name_exists_returns_true_after_insert() {
        let pool = db::init("sqlite::memory:").await.unwrap();
        insert_dummy(&pool, "id-1", "smp", None).await;
        assert!(name_exists(&pool, "smp").await.unwrap());
    }

    #[tokio::test]
    async fn allocate_nodeport_picks_lowest_on_empty_db() {
        let pool = db::init("sqlite::memory:").await.unwrap();
        let port = allocate_nodeport(&pool).await.unwrap();
        assert_eq!(port, 30_000);
    }

    #[tokio::test]
    async fn allocate_nodeport_skips_used_ports() {
        let pool = db::init("sqlite::memory:").await.unwrap();
        insert_dummy(&pool, "id-a", "a", Some(30_000)).await;
        insert_dummy(&pool, "id-b", "b", Some(30_001)).await;
        insert_dummy(&pool, "id-d", "d", Some(30_003)).await;
        let port = allocate_nodeport(&pool).await.unwrap();
        assert_eq!(port, 30_002);
    }

    #[tokio::test]
    async fn allocate_nodeport_exhausted_returns_conflict() {
        let pool = db::init("sqlite::memory:").await.unwrap();
        // Fill the entire 30_000..=30_099 range.
        for (i, port) in (NODEPORT_MIN..=NODEPORT_MAX).enumerate() {
            insert_dummy(&pool, &format!("id-{i}"), &format!("s{i}"), Some(port)).await;
        }
        let err = allocate_nodeport(&pool).await.expect_err("must fail");
        match err {
            AppError::Conflict { code, .. } => assert_eq!(code, "nodeport_range_exhausted"),
            other => panic!("expected Conflict, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn insert_audit_round_trips_details() {
        let pool = db::init("sqlite::memory:").await.unwrap();
        insert_audit(
            &pool,
            "srv-1",
            "created",
            Some(serde_json::json!({ "name": "smp", "memory_mi": 4096 })),
            1_700_000_000,
        )
        .await
        .unwrap();

        let row: (i64, String, String, Option<String>) = sqlx::query_as(
            "SELECT ts, server_id, action, details FROM audit_log WHERE server_id = ?",
        )
        .bind("srv-1")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.0, 1_700_000_000);
        assert_eq!(row.1, "srv-1");
        assert_eq!(row.2, "created");
        let details = row.3.expect("details");
        assert!(details.contains("\"memory_mi\":4096"));
    }
}
