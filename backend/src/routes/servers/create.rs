//! `POST /api/servers` — create a managed Minecraft server.
//!
//! Validates the request, allocates a `NodePort` if requested, inserts a
//! `servers` row + audit entry, then synchronously creates the k8s
//! Secret, `StatefulSet` (replicas=0), and Service. Returns `202 Accepted`
//! with the new server's id+name. The user must call `POST /:id/start`
//! afterwards to bring up the pod.
//!
//! M5: the request now carries an optional `server_type` (defaults to
//! `"vanilla"`); when set to `"curseforge"`, the handler resolves the
//! latest `ServerFiles` file from the `CurseForge` API and persists the
//! provider config so the update orchestrator can re-instantiate it later.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::Utc;
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::{Secret, Service};
use kube::Api;
use kube::api::PostParams;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::AppState;
use crate::error::AppError;
use crate::k8s_builders::{
    BuildParams, build_headless_service, build_rcon_secret, build_service, build_statefulset,
    rcon_password,
};
use crate::modpack::curseforge::{AutoUpdateMode, Channel, Config as CfConfig};
use crate::modpack::{CurseForgeServerPack, ModpackProvider, ProviderContext, VanillaProvider};
use crate::validation::{
    validate_cpu_millicores, validate_exposure_mode, validate_mc_version, validate_memory_mi,
    validate_name, validate_storage_size_gi,
};

/// Lowest `NodePort` allocated by the panel.
const NODEPORT_MIN: i32 = 30_000;
/// Highest `NodePort` allocated by the panel (inclusive).
const NODEPORT_MAX: i32 = 30_099;
/// Default storage size (GiB) when the request omits the field.
const DEFAULT_STORAGE_SIZE_GI: i64 = 10;
/// Source kind discriminator persisted in `servers.source_kind`.
const SERVER_TYPE_VANILLA: &str = "vanilla";
/// Source kind discriminator for `CurseForge` `ServerFiles` servers.
const SERVER_TYPE_CURSEFORGE: &str = "curseforge";

/// Request body for `POST /api/servers`.
#[derive(Debug, Deserialize)]
pub struct CreateRequest {
    /// User-facing name (DNS-1123 label).
    pub name: String,
    /// Minecraft version. Required for vanilla; ignored on the `CurseForge` path
    /// (the chosen `ServerFiles` file's display name is stored instead).
    #[serde(default)]
    pub mc_version: Option<String>,
    /// Memory budget in MiB. Must be 1024–16384 in 1024-step.
    pub memory_mi: i64,
    /// CPU budget in millicores. Must be 250–16000.
    pub cpu_millicores: i64,
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
    /// `vanilla` (default) | `curseforge`.
    #[serde(default)]
    pub server_type: Option<String>,
    /// Required when `server_type == "curseforge"`.
    #[serde(default)]
    pub curseforge: Option<CurseForgeCreateConfig>,
}

/// Sub-form fields for the `CurseForge` path.
#[derive(Debug, Deserialize)]
pub struct CurseForgeCreateConfig {
    /// `CurseForge` project id (resolved via the `/modpack/curseforge/resolve`
    /// endpoint when the user pasted a URL).
    pub project_id: u32,
    /// Release channel filter (`release` default | `beta` | `alpha`).
    pub channel: Channel,
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
///   `exposure_mode_invalid` / `cf_disabled` / `cf_config_missing` /
///   `no_server_pack_files` / `cf_project_not_found`
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
    let CreateRequest {
        name,
        mc_version,
        memory_mi,
        cpu_millicores,
        exposure_mode,
        storage_class,
        storage_size_gi,
        server_type,
        curseforge,
    } = request;
    validate_name(&name)?;
    validate_memory_mi(memory_mi)?;
    validate_cpu_millicores(cpu_millicores)?;

    let server_type = server_type.unwrap_or_else(|| SERVER_TYPE_VANILLA.to_owned());
    if server_type != SERVER_TYPE_VANILLA && server_type != SERVER_TYPE_CURSEFORGE {
        return Err(AppError::BadRequest {
            code: "server_type_invalid",
            message: format!("server_type must be vanilla or curseforge, got {server_type:?}"),
        });
    }

    let exposure_mode =
        exposure_mode.map_or_else(|| state.mc_svc_type.to_lowercase(), |m| m.to_lowercase());
    validate_exposure_mode(&exposure_mode)?;

    if exposure_mode == "loadbalancer" && !state.loadbalancer_supported {
        return Err(AppError::LbUnavailable);
    }

    let storage_size_gi = storage_size_gi.unwrap_or(DEFAULT_STORAGE_SIZE_GI);
    validate_storage_size_gi(storage_size_gi)?;
    let storage_class = storage_class.filter(|s| !s.is_empty());
    let effective_storage_class = storage_class.clone().or_else(|| {
        if state.mc_storage_class.is_empty() {
            None
        } else {
            Some(state.mc_storage_class.clone())
        }
    });

    if name_exists(&state.pool, &name).await? {
        return Err(AppError::Conflict {
            code: "name_taken",
            message: format!("a server named {name:?} already exists"),
        });
    }

    let nodeport = if exposure_mode == "nodeport" {
        Some(allocate_nodeport(&state.pool).await?)
    } else {
        None
    };

    let id = Uuid::new_v4().to_string();
    let rcon_pwd = rcon_password();
    let now = Utc::now().timestamp();

    // Branch on server_type to resolve the provider, the version label, and the
    // source_config JSON to persist.
    let resolved = match server_type.as_str() {
        SERVER_TYPE_VANILLA => {
            // Vanilla requires an `mc_version`; CF rows don't.
            let mc_v = mc_version.ok_or_else(|| AppError::BadRequest {
                code: "mc_version_required",
                message: "mc_version is required for vanilla servers".to_owned(),
            })?;
            validate_mc_version(&state, &mc_v).await?;
            ResolvedSource {
                provider: Box::new(VanillaProvider::new()),
                mc_version: mc_v,
                source_kind: SERVER_TYPE_VANILLA,
                source_config: "{}".to_owned(),
            }
        }
        SERVER_TYPE_CURSEFORGE => resolve_curseforge(&state, curseforge).await?,
        _ => unreachable!("validated above"),
    };

    // Persist metadata + audit entry. If k8s create fails after this, the
    // SQLite row remains; DELETE handler tolerates missing k8s resources.
    insert_server(
        &state.pool,
        &id,
        &name,
        &resolved.mc_version,
        memory_mi,
        cpu_millicores,
        resolved.source_kind,
        &exposure_mode,
        storage_class.as_deref(),
        storage_size_gi,
        &resolved.source_config,
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
            "mc_version": resolved.mc_version,
            "memory_mi": memory_mi,
            "cpu_millicores": cpu_millicores,
            "exposure_mode": exposure_mode,
            "storage_class": storage_class,
            "storage_size_gi": storage_size_gi,
            "nodeport": nodeport,
            "source_kind": resolved.source_kind,
        })),
        now,
    )
    .await?;

    // Build the StatefulSet via provider-supplied image + command + env.
    let ctx = ProviderContext {
        server_id: &id,
        memory_mi,
    };
    let extra_env = resolved.provider.extra_env(&ctx);
    let command_owned = resolved.provider.launch_command();
    let build_params = BuildParams {
        id: &id,
        name: &name,
        namespace: &state.mc_namespace,
        mc_version: &resolved.mc_version,
        memory_mi,
        cpu_millicores,
        server_type: resolved.source_kind,
        image: resolved.provider.pod_image(),
        command: command_owned.as_deref(),
        extra_env: &extra_env,
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
    services
        .create(&pp, &build_headless_service(&build_params))
        .await?;
    stsets
        .create(&pp, &build_statefulset(&build_params))
        .await?;
    services.create(&pp, &build_service(&build_params)).await?;

    Ok((StatusCode::ACCEPTED, Json(CreateResponse { id, name })))
}

/// Materialised provider + persistence values for one create call.
struct ResolvedSource {
    provider: Box<dyn ModpackProvider>,
    mc_version: String,
    source_kind: &'static str,
    source_config: String,
}

/// Validates the `CurseForge` sub-form, hits the API to pick the newest
/// matching server-pack file, and produces the persistence payload.
async fn resolve_curseforge(
    state: &AppState,
    cfg: Option<CurseForgeCreateConfig>,
) -> Result<ResolvedSource, AppError> {
    let cf_client = state.cf_client.as_ref().ok_or(AppError::BadRequest {
        code: "cf_disabled",
        message: "CurseForge support is not enabled on this panel (CF_API_KEY missing)".to_owned(),
    })?;
    let cfg = cfg.ok_or(AppError::BadRequest {
        code: "cf_config_missing",
        message: "curseforge.{project_id, channel} required for server_type=curseforge".to_owned(),
    })?;

    // Materialize a temporary provider to drive the picker.
    let provisional = CurseForgeServerPack::new(CfConfig {
        project_id: cfg.project_id,
        channel: cfg.channel,
        version_skip: Vec::new(),
        force_version: None,
        current_version_id: 0,
        current_version_name: String::new(),
        auto_update_mode: AutoUpdateMode::Notify,
    });

    let files = cf_client
        .list_files(cfg.project_id)
        .await
        .map_err(|e| AppError::BadRequest {
            code: "cf_project_not_found",
            message: format!("CurseForge project {} unavailable: {e}", cfg.project_id),
        })?;
    let pick = provisional
        .pick_latest(&files)
        .ok_or(AppError::BadRequest {
            code: "no_server_pack_files",
            message: format!(
                "project {} has no server-pack files matching channel {:?}",
                cfg.project_id, cfg.channel
            ),
        })?;

    let stored_cfg = CfConfig {
        project_id: cfg.project_id,
        channel: cfg.channel,
        version_skip: Vec::new(),
        force_version: None,
        current_version_id: pick.id,
        current_version_name: pick.name.clone(),
        auto_update_mode: AutoUpdateMode::Notify,
    };
    let source_config =
        serde_json::to_string(&stored_cfg).map_err(|e| AppError::Internal(e.into()))?;

    Ok(ResolvedSource {
        provider: Box::new(CurseForgeServerPack::new(stored_cfg)),
        mc_version: pick.name,
        source_kind: SERVER_TYPE_CURSEFORGE,
        source_config,
    })
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
    cpu_millicores: i64,
    source_kind: &str,
    exposure_mode: &str,
    storage_class: Option<&str>,
    storage_size_gi: i64,
    source_config: &str,
    nodeport: Option<i32>,
    created_at: i64,
) -> Result<(), AppError> {
    let result = sqlx::query(
        "INSERT INTO servers (
            id, name, mc_version, memory_mi, cpu_millicores, server_type,
            exposure_mode, storage_class, storage_size_gi, source_config,
            source_kind, nodeport, created_at, last_started_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
    )
    .bind(id)
    .bind(name)
    .bind(mc_version)
    .bind(memory_mi)
    .bind(cpu_millicores)
    // Legacy `server_type` column kept in lockstep with `source_kind` so
    // callers reading the M2 schema continue to see the same value.
    .bind(source_kind)
    .bind(exposure_mode)
    .bind(storage_class)
    .bind(storage_size_gi)
    .bind(source_config)
    .bind(source_kind)
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
            2000,
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

    #[tokio::test]
    async fn insert_server_persists_curseforge_kind() {
        let pool = db::init("sqlite::memory:").await.unwrap();
        insert_server(
            &pool,
            "cf-1",
            "atm11",
            "ATM-11 4.4 Server",
            8192,
            4000,
            SERVER_TYPE_CURSEFORGE,
            "loadbalancer",
            Some("tank"),
            20,
            r#"{"project_id":1148445}"#,
            None,
            1,
        )
        .await
        .unwrap();
        let kind: String = sqlx::query_scalar("SELECT source_kind FROM servers WHERE id = ?")
            .bind("cf-1")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(kind, "curseforge");
    }
}
