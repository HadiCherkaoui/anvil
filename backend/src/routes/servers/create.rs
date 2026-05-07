//! `POST /api/servers` — create a managed Minecraft server.
//!
//! Validates the request, allocates a `NodePort` if requested, inserts a
//! `servers` row + audit entry, then synchronously creates the k8s
//! Secret, `StatefulSet` (replicas=0), and Service. Returns `202 Accepted`
//! with the new server's id+name. The user must call `POST /:id/start`
//! afterwards to bring up the pod.
//!
//! M5: the request carries an optional `source_kind` (defaults to
//! `"vanilla"`); when set to `"curseforge"`, the handler resolves the
//! latest `ServerFiles` file from the `CurseForge` API and persists the
//! provider config so the update orchestrator can re-instantiate it later.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::Utc;
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::{PersistentVolumeClaim, Secret, Service};
use kube::Api;
use kube::api::PostParams;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::SqlitePool;
use uuid::Uuid;

use std::collections::HashSet;

use crate::AppState;
use crate::error::AppError;
use crate::k8s_builders::{
    BuildParams, build_data_pvc, build_headless_service, build_rcon_secret, build_service,
    build_statefulset, rcon_password,
};
use crate::modpack::curseforge::{AutoUpdateMode, Channel, Config as CfConfig};
use crate::modpack::dep_resolver::{ResolveContext, resolve_required};
use crate::modpack::guard::UpdateGuard;
use crate::modpack::modded::{Config as ModdedConfig, ModEntry, ModdedRuntime, PendingOp, Runtime};
use crate::modpack::modrinth::{Config as MrPackConfig, ModrinthServerPack};
use crate::modpack::mods_apply::{self, SyncTarget};
use crate::modpack::paper::{Config as PaperConfig, PaperServerProvider};
use crate::modpack::{
    CurseForgeServerPack, ModpackHttp, ModpackProvider, ProviderContext, VanillaProvider,
};
use crate::server_properties::ServerProperties;
use crate::validation::{
    validate_exposure_mode, validate_mc_version, validate_memory_mi, validate_mod_filename,
    validate_modrinth_id_or_slug, validate_name, validate_runtime, validate_storage_size_gi,
};

/// Lowest `NodePort` allocated by the panel.
const NODEPORT_MIN: i32 = 30_000;
/// Highest `NodePort` allocated by the panel (inclusive).
const NODEPORT_MAX: i32 = 30_099;
/// Default storage size (GiB) when the request omits the field.
const DEFAULT_STORAGE_SIZE_GI: i64 = 10;
/// Source kind discriminator persisted in `servers.source_kind`.
const SOURCE_KIND_VANILLA: &str = "vanilla";
/// Source kind discriminator for `CurseForge` `ServerFiles` servers.
const SOURCE_KIND_CURSEFORGE: &str = "curseforge";
/// Source kind discriminator for Modrinth `.mrpack` servers.
const SOURCE_KIND_MODRINTH: &str = "modrinth";
/// Source kind discriminator for `modded` (Fabric/Forge/NeoForge) servers.
const SOURCE_KIND_MODDED: &str = "modded";
/// Source kind discriminator for Paper servers (Bukkit-API plugin host).
const SOURCE_KIND_PAPER: &str = "paper";

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
    /// `vanilla` (default) | `curseforge` | `modrinth` | `modded` | `paper`.
    #[serde(default)]
    pub source_kind: Option<String>,
    /// Required when `source_kind == "curseforge"`.
    #[serde(default)]
    pub curseforge: Option<CurseForgeCreateConfig>,
    /// Required when `source_kind == "modrinth"`.
    #[serde(default)]
    pub modrinth: Option<ModrinthCreateConfig>,
    /// Required when `source_kind == "modded"`.
    #[serde(default)]
    pub modded: Option<ModdedCreateConfig>,
    /// Optional sub-form for the `paper` source kind. When present and
    /// `initial_plugins` is non-empty, the create handler folds them into
    /// `pending_plugins` and spawns the apply Job post-create.
    #[serde(default)]
    pub paper: Option<PaperCreateConfig>,
    /// Curated subset of `server.properties` overrides. Missing => vanilla
    /// defaults; itzg overlays the resulting env onto `server.properties`
    /// on every pod start.
    #[serde(default)]
    pub properties: Option<ServerProperties>,
}

/// Sub-form fields for the Paper plugin pre-pick path.
#[derive(Debug, Deserialize)]
pub struct PaperCreateConfig {
    /// Initial plugin selection picked at create-time. Required deps are
    /// resolved upstream and appended to `pending_plugins`.
    #[serde(default)]
    pub initial_plugins: Vec<ModEntry>,
}

/// Sub-form fields for the Modrinth modpack path.
#[derive(Debug, Deserialize)]
pub struct ModrinthCreateConfig {
    /// Modrinth project id (8-char base62) or slug.
    pub project_id: String,
    pub channel: Channel,
}

/// Sub-form fields for the modded (runtime + manual modlist) path.
#[derive(Debug, Deserialize)]
pub struct ModdedCreateConfig {
    /// `fabric` | `forge` | `neoforge`.
    pub runtime: String,
    /// Initial mod selection picked at create-time. Folded into `pending`
    /// as Add ops so the Mods tab shows "N pending — apply now" on first load.
    #[serde(default)]
    pub initial_mods: Vec<ModEntry>,
    /// Forge / `NeoForge` loader version chosen from the cascading picker.
    /// `None` keeps itzg's default (`*_VERSION=LATEST`); fabric ignores it.
    #[serde(default)]
    pub loader_version: Option<String>,
}

/// Sub-form fields for the `CurseForge` path.
#[derive(Debug, Deserialize)]
pub struct CurseForgeCreateConfig {
    /// `CurseForge` project id (picked via the catalog browse sheet).
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
        exposure_mode,
        storage_class,
        storage_size_gi,
        source_kind,
        curseforge,
        modrinth,
        modded,
        paper,
        properties,
    } = request;
    validate_name(&name)?;
    validate_memory_mi(memory_mi)?;

    let source_kind = source_kind.unwrap_or_else(|| SOURCE_KIND_VANILLA.to_owned());
    if !matches!(
        source_kind.as_str(),
        SOURCE_KIND_VANILLA
            | SOURCE_KIND_CURSEFORGE
            | SOURCE_KIND_MODRINTH
            | SOURCE_KIND_MODDED
            | SOURCE_KIND_PAPER
    ) {
        return Err(AppError::BadRequest {
            code: "source_kind_invalid",
            message: format!("source_kind {source_kind:?} not supported"),
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

    // Properties default to vanilla MC values when the form omits them.
    let properties = properties.unwrap_or_default();
    properties.validate()?;
    let properties_json =
        serde_json::to_string(&properties).map_err(|e| AppError::Internal(e.into()))?;

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

    let id = Uuid::new_v4().to_string();
    let rcon_pwd = rcon_password();
    let now = Utc::now().timestamp();

    // Branch on source_kind to resolve the provider, the version label, and the
    // source_config JSON to persist.
    let resolved = match source_kind.as_str() {
        SOURCE_KIND_VANILLA => {
            // Vanilla requires an `mc_version`; CF rows don't.
            let mc_v = mc_version.ok_or_else(|| AppError::BadRequest {
                code: "mc_version_required",
                message: "mc_version is required for vanilla servers".to_owned(),
            })?;
            validate_mc_version(&state, &mc_v).await?;
            ResolvedSource {
                provider: Box::new(VanillaProvider::new()),
                mc_version: mc_v,
                source_kind: SOURCE_KIND_VANILLA,
                source_config: "{}".to_owned(),
                initial_pending_mods: 0,
            }
        }
        SOURCE_KIND_CURSEFORGE => resolve_curseforge(&state, curseforge).await?,
        SOURCE_KIND_MODRINTH => resolve_modrinth(&state, modrinth).await?,
        SOURCE_KIND_MODDED => resolve_modded(&state, mc_version, modded).await?,
        SOURCE_KIND_PAPER => resolve_paper(&state, mc_version, paper).await?,
        _ => unreachable!("validated above"),
    };

    // Persist metadata + audit entry. If k8s create fails after this, the
    // SQLite row remains; DELETE handler tolerates missing k8s resources.
    // NodePort allocation + insert happen inside a single transaction so
    // two concurrent creates can't pick the same port.
    let nodeport = insert_server_with_nodeport(
        &state.pool,
        &id,
        &name,
        &resolved.mc_version,
        memory_mi,
        resolved.source_kind,
        &exposure_mode,
        storage_class.as_deref(),
        storage_size_gi,
        &resolved.source_config,
        &properties_json,
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
            "exposure_mode": exposure_mode,
            "storage_class": storage_class,
            "storage_size_gi": storage_size_gi,
            "nodeport": nodeport,
            "source_kind": resolved.source_kind,
            "properties": &properties,
        })),
        now,
    )
    .await?;

    // Build the StatefulSet via provider-supplied image + command + env.
    let ctx = ProviderContext {
        server_id: &id,
        memory_mi,
    };
    let mut extra_env = resolved.provider.extra_env(&ctx);
    extra_env.extend(properties.to_env());
    let command_owned = resolved.provider.launch_command();
    let build_params = BuildParams {
        id: &id,
        name: &name,
        namespace: &state.mc_namespace,
        mc_version: &resolved.mc_version,
        memory_mi,
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
    let pvcs: Api<PersistentVolumeClaim> = Api::namespaced(state.kube.clone(), &state.mc_namespace);

    // Track which k8s resources we successfully created so we can roll
    // them back if a later step fails. Without rollback, a half-failed
    // create leaves orphans the user can only clean up via DELETE (which
    // succeeds because the SQLite row exists). Order: secret → headless
    // svc → STS → PVC → public svc. Rollback walks in reverse and is
    // 404-tolerant — a resource that never landed is fine to "delete".
    #[derive(Default)]
    struct Created {
        secret: bool,
        headless: bool,
        sts: bool,
        pvc: bool,
        public_svc: bool,
    }
    let mut created = Created::default();
    let resource_name = format!("mc-{id}");
    let pvc_name = format!("data-{resource_name}-0");
    let headless_name = format!("{resource_name}-headless");
    let secret_name = format!("{resource_name}-rcon");

    let create_result: Result<(), AppError> = async {
        secrets
            .create(&pp, &build_rcon_secret(&id, &state.mc_namespace, &rcon_pwd))
            .await?;
        created.secret = true;
        services
            .create(&pp, &build_headless_service(&build_params))
            .await?;
        created.headless = true;
        stsets
            .create(&pp, &build_statefulset(&build_params))
            .await?;
        created.sts = true;
        // Pre-create the data PVC so create-time Jobs (mod/plugin sync) can
        // mount /data before the StatefulSet has ever scaled to 1. The
        // StatefulSet adopts the matching name on first scale-up.
        pvcs.create(&pp, &build_data_pvc(&build_params)).await?;
        created.pvc = true;
        let public_svc = build_service(&build_params)?;
        services.create(&pp, &public_svc).await?;
        created.public_svc = true;
        Ok(())
    }
    .await;

    if let Err(err) = create_result {
        // Reverse-order best-effort cleanup. 404s are tolerated: the
        // resource we tried to create may have failed before the API
        // accepted it, in which case the delete also legitimately 404s.
        let dp = kube::api::DeleteParams::default();
        if created.public_svc {
            tolerate_404(services.delete(&resource_name, &dp).await, "public_svc", &id);
        }
        if created.pvc {
            tolerate_404(pvcs.delete(&pvc_name, &dp).await, "pvc", &id);
        }
        if created.sts {
            tolerate_404(stsets.delete(&resource_name, &dp).await, "sts", &id);
        }
        if created.headless {
            tolerate_404(services.delete(&headless_name, &dp).await, "headless", &id);
        }
        if created.secret {
            tolerate_404(secrets.delete(&secret_name, &dp).await, "secret", &id);
        }
        return Err(err);
    }

    // C#10: when the user pre-picked mods at create time, kick the same
    // apply task the manual `POST /:id/mods/apply` endpoint spawns. This
    // mirrors the manual flow without going through the HTTP handler.
    // Failure to acquire the lock is logged + dropped; the pending ops
    // remain in source_config and the user can apply manually. Paper
    // plugins follow the same pattern with `SyncTarget::Plugins`.
    if resolved.initial_pending_mods > 0
        && (resolved.source_kind == SOURCE_KIND_MODDED || resolved.source_kind == SOURCE_KIND_PAPER)
    {
        let target = if resolved.source_kind == SOURCE_KIND_PAPER {
            SyncTarget::Plugins
        } else {
            SyncTarget::Mods
        };
        if let Some(guard) = UpdateGuard::try_acquire(
            &id,
            state.update_locks.clone(),
            state.update_phase_buses.clone(),
            state.update_errors.clone(),
        ) {
            let task_state = state.clone();
            let task_id = id.clone();
            tokio::spawn(async move {
                mods_apply::run(task_state, task_id, guard, target).await;
            });
        } else {
            tracing::warn!(
                server.id = %id,
                "apply guard unavailable on create; user can apply manually",
            );
        }
    }

    Ok((StatusCode::ACCEPTED, Json(CreateResponse { id, name })))
}

/// Materialised provider + persistence values for one create call.
struct ResolvedSource {
    provider: Box<dyn ModpackProvider>,
    mc_version: String,
    source_kind: &'static str,
    source_config: String,
    /// Number of pending mod ops staged for the apply Job spawned post-create.
    /// Non-zero only on the modded path with a non-empty `initial_mods`.
    initial_pending_mods: usize,
}

/// Validates the `CurseForge` sub-form, hits the API to pick the newest
/// matching server-pack file, and produces the persistence payload.
async fn resolve_curseforge(
    state: &AppState,
    cfg: Option<CurseForgeCreateConfig>,
) -> Result<ResolvedSource, AppError> {
    if state.cf_client.is_none() {
        return Err(AppError::BadRequest {
            code: "cf_disabled",
            message: "CurseForge support is not enabled on this panel (CF_API_KEY missing)"
                .to_owned(),
        });
    }
    let cfg = cfg.ok_or(AppError::BadRequest {
        code: "cf_config_missing",
        message: "curseforge.{project_id, channel} required for source_kind=curseforge".to_owned(),
    })?;

    // Resolve the project's slug now (itzg's mc-image-helper rejects calls
    // without --slug; we ship it as CF_SLUG via extra_env). One round-trip
    // is fine — create is rare.
    let cf_client = state.cf_client.as_deref().ok_or(AppError::BadRequest {
        code: "cf_disabled",
        message: "CurseForge support is not enabled on this panel (CF_API_KEY missing)".to_owned(),
    })?;
    let project = cf_client
        .project(cfg.project_id)
        .await
        .map_err(|e| AppError::BadRequest {
            code: "cf_project_not_found",
            message: format!("CurseForge project {} unavailable: {e}", cfg.project_id),
        })?;

    // Materialize a temporary provider to drive the picker; `latest()`
    // returns the newest CLIENT file id whose linked server pack lets
    // itzg drive the install.
    let provisional = CurseForgeServerPack::new(CfConfig {
        project_id: cfg.project_id,
        slug: project.slug.clone(),
        channel: cfg.channel,
        version_skip: Vec::new(),
        force_version: None,
        current_version_id: 0,
        current_version_name: String::new(),
        auto_update_mode: AutoUpdateMode::Notify,
    });

    let http = ModpackHttp {
        cf: state.cf_client.as_deref(),
        mr: state.mr_client.as_ref(),
    };
    let pick = provisional
        .latest(&http)
        .await
        .map_err(|e| AppError::BadRequest {
            code: "cf_project_not_found",
            message: format!("CurseForge project {} unavailable: {e}", cfg.project_id),
        })?
        .ok_or(AppError::BadRequest {
            code: "no_server_pack_files",
            message: format!(
                "project {:?} has no client files with a linked server pack matching channel {:?} \
                 — itzg's AUTO_CURSEFORGE path needs a manifest-bearing client file",
                project.slug, cfg.channel
            ),
        })?;

    let pick_id_u32: u32 = pick.id.parse().map_err(|_| {
        AppError::Internal(anyhow::anyhow!(
            "CF pick id {:?} not numeric — provider returned a non-CF id",
            pick.id
        ))
    })?;
    let stored_cfg = CfConfig {
        project_id: cfg.project_id,
        slug: project.slug,
        channel: cfg.channel,
        version_skip: Vec::new(),
        force_version: None,
        current_version_id: pick_id_u32,
        current_version_name: pick.name.clone(),
        auto_update_mode: AutoUpdateMode::Notify,
    };
    let source_config =
        serde_json::to_string(&stored_cfg).map_err(|e| AppError::Internal(e.into()))?;

    Ok(ResolvedSource {
        provider: Box::new(CurseForgeServerPack::new(stored_cfg)),
        mc_version: pick.name,
        source_kind: SOURCE_KIND_CURSEFORGE,
        source_config,
        initial_pending_mods: 0,
    })
}

/// Validates the Modrinth sub-form and resolves the latest matching version.
async fn resolve_modrinth(
    state: &AppState,
    cfg: Option<ModrinthCreateConfig>,
) -> Result<ResolvedSource, AppError> {
    let cfg = cfg.ok_or(AppError::BadRequest {
        code: "modrinth_config_missing",
        message: "modrinth.{project_id, channel} required for source_kind=modrinth".to_owned(),
    })?;
    validate_modrinth_id_or_slug(&cfg.project_id)?;

    let provisional = ModrinthServerPack::new(MrPackConfig {
        project_id: cfg.project_id.clone(),
        channel: cfg.channel,
        version_skip: Vec::new(),
        force_version: None,
        current_version_id: String::new(),
        current_version_name: String::new(),
        auto_update_mode: AutoUpdateMode::Notify,
    });
    let http = ModpackHttp {
        cf: state.cf_client.as_deref(),
        mr: state.mr_client.as_ref(),
    };
    let pick = provisional
        .latest(&http)
        .await
        .map_err(|e| AppError::BadRequest {
            code: "modrinth_unavailable",
            message: format!("modrinth lookup: {e}"),
        })?
        .ok_or(AppError::BadRequest {
            code: "no_modpack_versions",
            message: format!("project {:?} has no matching versions", cfg.project_id),
        })?;

    let stored_cfg = MrPackConfig {
        project_id: cfg.project_id,
        channel: cfg.channel,
        version_skip: Vec::new(),
        force_version: None,
        current_version_id: pick.id.clone(),
        current_version_name: pick.name.clone(),
        auto_update_mode: AutoUpdateMode::Notify,
    };
    let source_config =
        serde_json::to_string(&stored_cfg).map_err(|e| AppError::Internal(e.into()))?;

    Ok(ResolvedSource {
        provider: Box::new(ModrinthServerPack::new(stored_cfg)),
        mc_version: pick.name,
        source_kind: SOURCE_KIND_MODRINTH,
        source_config,
        initial_pending_mods: 0,
    })
}

/// Builds a modded `ResolvedSource`. Initial mods are folded into `pending`
/// as Add ops so the Mods tab shows "N pending — apply now" on first load.
/// Required dependencies of every initial mod are resolved upstream and
/// appended to `pending`, so the apply Job spawned post-create installs
/// them in one pass.
async fn resolve_modded(
    state: &AppState,
    mc_version: Option<String>,
    cfg: Option<ModdedCreateConfig>,
) -> Result<ResolvedSource, AppError> {
    let cfg = cfg.ok_or(AppError::BadRequest {
        code: "modded_config_missing",
        message: "modded.{runtime} required for source_kind=modded".to_owned(),
    })?;
    validate_runtime(&cfg.runtime)?;
    let runtime = match cfg.runtime.as_str() {
        "fabric" => Runtime::Fabric,
        "forge" => Runtime::Forge,
        "neoforge" => Runtime::NeoForge,
        other => {
            return Err(AppError::BadRequest {
                code: "runtime_invalid",
                message: format!("runtime {other:?} not allowed for modded servers"),
            });
        }
    };
    let mc_v = mc_version.ok_or(AppError::BadRequest {
        code: "mc_version_required",
        message: "mc_version is required for modded servers".to_owned(),
    })?;
    for m in &cfg.initial_mods {
        validate_mod_filename(&m.filename)?;
    }

    let loader_lower = match runtime {
        Runtime::Fabric => "fabric",
        Runtime::Forge => "forge",
        Runtime::NeoForge => "neoforge",
    };
    let resolved_extras =
        resolve_initial_extras(state, &cfg.initial_mods, &mc_v, loader_lower).await;

    let mut pending: Vec<PendingOp> =
        Vec::with_capacity(cfg.initial_mods.len() + resolved_extras.len());
    for m in &cfg.initial_mods {
        pending.push(PendingOp::Add {
            mod_entry: m.clone(),
        });
    }
    for dep in resolved_extras {
        pending.push(PendingOp::Add { mod_entry: dep });
    }
    let pending_count = pending.len();
    let loader_version = cfg.loader_version.filter(|s| !s.is_empty());
    let stored = ModdedConfig {
        runtime,
        mc_version: mc_v.clone(),
        loader_version,
        mods: Vec::new(),
        pending,
        auto_update_mode: crate::modpack::modded::AutoUpdateMode::default(),
    };
    let source_config = serde_json::to_string(&stored).map_err(|e| AppError::Internal(e.into()))?;
    Ok(ResolvedSource {
        provider: Box::new(ModdedRuntime::new(stored)),
        mc_version: mc_v,
        source_kind: SOURCE_KIND_MODDED,
        source_config,
        initial_pending_mods: pending_count,
    })
}

/// Resolves the union of required deps for every seed in `seeds`. Each seed
/// pre-populates the pending set so the resolver doesn't add it back. First
/// resolution wins on conflicts.
async fn resolve_initial_extras(
    state: &AppState,
    seeds: &[ModEntry],
    mc_version: &str,
    loader: &str,
) -> Vec<ModEntry> {
    if seeds.is_empty() {
        return Vec::new();
    }
    let mut pending: HashSet<(String, String)> = seeds
        .iter()
        .map(|m| (m.provider.clone(), m.project_id.clone()))
        .collect();
    let http = ModpackHttp {
        cf: state.cf_client.as_deref(),
        mr: state.mr_client.as_ref(),
    };
    let mut extras: Vec<ModEntry> = Vec::new();
    for seed in seeds {
        let mut ctx = ResolveContext {
            mc_version,
            loader,
            installed: HashSet::new(),
            pending: pending.clone(),
        };
        match resolve_required(seed, &mut ctx, &http).await {
            Ok(deps) => {
                for dep in deps {
                    let key = (dep.provider.clone(), dep.project_id.clone());
                    if pending.insert(key) {
                        extras.push(dep);
                    }
                }
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    seed.project_id = %seed.project_id,
                    "initial dep resolver failed for seed; continuing",
                );
            }
        }
    }
    extras
}

/// Builds a Paper `ResolvedSource`. Paper-build pin is left to itzg's default
/// (latest stable) for B; the catalog UI can offer a picker in a follow-up.
async fn resolve_paper(
    state: &AppState,
    mc_version: Option<String>,
    paper_cfg: Option<PaperCreateConfig>,
) -> Result<ResolvedSource, AppError> {
    let mc_v = mc_version.ok_or(AppError::BadRequest {
        code: "mc_version_required",
        message: "mc_version is required for paper servers".to_owned(),
    })?;
    validate_mc_version(state, &mc_v).await?;

    // PaperMC ships builds for a subset of MC versions. itzg's TYPE=PAPER
    // bombs out at boot when the version is unsupported (`Requested version
    // 26.1 is not available`); reject up front instead.
    if !crate::routes::papermc::is_supported(&state.papermc_cache, &mc_v).await {
        return Err(AppError::BadRequest {
            code: "paper_unsupported_version",
            message: format!("paper does not ship builds for MC {mc_v}"),
        });
    }

    let initial_plugins = paper_cfg.map(|p| p.initial_plugins).unwrap_or_default();
    for p in &initial_plugins {
        validate_mod_filename(&p.filename)?;
    }
    let resolved_extras = resolve_initial_extras(state, &initial_plugins, &mc_v, "paper").await;

    let mut pending_plugins: Vec<ModEntry> =
        Vec::with_capacity(initial_plugins.len() + resolved_extras.len());
    for p in &initial_plugins {
        if !pending_plugins.iter().any(|x| x.filename == p.filename) {
            pending_plugins.push(p.clone());
        }
    }
    for dep in resolved_extras {
        if !pending_plugins.iter().any(|x| x.filename == dep.filename) {
            pending_plugins.push(dep);
        }
    }
    let pending_count = pending_plugins.len();

    let stored = PaperConfig {
        mc_version: mc_v.clone(),
        paper_build: None,
        plugins: Vec::new(),
        pending_plugins,
        auto_update_mode: crate::modpack::modded::AutoUpdateMode::default(),
    };
    let source_config = serde_json::to_string(&stored).map_err(|e| AppError::Internal(e.into()))?;
    Ok(ResolvedSource {
        provider: Box::new(PaperServerProvider::new(stored)),
        mc_version: mc_v,
        source_kind: SOURCE_KIND_PAPER,
        source_config,
        initial_pending_mods: pending_count,
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
///
/// Kept for the unit tests that exercise the picker against an in-memory
/// pool; the production path uses [`allocate_nodeport_tx`] inside the
/// create transaction so SELECT + INSERT are atomic.
#[cfg(test)]
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

/// Persists a new server row, allocating a `NodePort` inside the same
/// transaction when `exposure_mode == "nodeport"`. Bundling the SELECT
/// + INSERT prevents two concurrent creates from picking the same port.
///
/// Returns the allocated port (or `None` when not in nodeport mode).
#[allow(clippy::too_many_arguments)]
async fn insert_server_with_nodeport(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    mc_version: &str,
    memory_mi: i64,
    source_kind: &str,
    exposure_mode: &str,
    storage_class: Option<&str>,
    storage_size_gi: i64,
    source_config: &str,
    properties_json: &str,
    created_at: i64,
) -> Result<Option<i32>, AppError> {
    // One retry: a UNIQUE-violation on a future `nodeport` index, or any
    // other transient conflict, gives us a chance to re-allocate.
    for attempt in 0..2 {
        let mut tx = pool.begin().await?;

        let nodeport = if exposure_mode == "nodeport" {
            Some(allocate_nodeport_tx(&mut tx).await?)
        } else {
            None
        };

        let result = sqlx::query(
            "INSERT INTO servers (
                id, name, mc_version, memory_mi,
                exposure_mode, storage_class, storage_size_gi, source_config,
                source_kind, properties, nodeport, created_at, last_started_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
        )
        .bind(id)
        .bind(name)
        .bind(mc_version)
        .bind(memory_mi)
        .bind(exposure_mode)
        .bind(storage_class)
        .bind(storage_size_gi)
        .bind(source_config)
        .bind(source_kind)
        .bind(properties_json)
        .bind(nodeport.map(i64::from))
        .bind(created_at)
        .execute(&mut *tx)
        .await;

        match result {
            Ok(_) => {
                tx.commit().await?;
                return Ok(nodeport);
            }
            Err(sqlx::Error::Database(err)) if err.is_unique_violation() => {
                drop(tx);
                let msg = err.message().to_ascii_lowercase();
                if msg.contains("nodeport") && attempt == 0 {
                    continue;
                }
                if msg.contains("nodeport") {
                    return Err(AppError::Conflict {
                        code: "port_conflict",
                        message: "NodePort already allocated; retry".to_owned(),
                    });
                }
                return Err(AppError::Conflict {
                    code: "name_taken",
                    message: format!("a server named {name:?} already exists"),
                });
            }
            Err(other) => {
                drop(tx);
                return Err(AppError::DbUnavailable(other));
            }
        }
    }
    unreachable!("loop returns or errors")
}

/// Transactional variant of [`allocate_nodeport`]. Same semantics; the
/// SELECT runs against the open transaction so a concurrent insert can't
/// race past it before our INSERT lands.
async fn allocate_nodeport_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
) -> Result<i32, AppError> {
    let rows: Vec<i64> = sqlx::query_scalar(
        "SELECT nodeport FROM servers WHERE nodeport IS NOT NULL ORDER BY nodeport ASC",
    )
    .fetch_all(&mut **tx)
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
/// Kept for the unit tests that pre-seed rows with explicit ports; the
/// production path goes through [`insert_server_with_nodeport`].
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
async fn insert_server(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    mc_version: &str,
    memory_mi: i64,
    source_kind: &str,
    exposure_mode: &str,
    storage_class: Option<&str>,
    storage_size_gi: i64,
    source_config: &str,
    properties_json: &str,
    nodeport: Option<i32>,
    created_at: i64,
) -> Result<(), AppError> {
    let result = sqlx::query(
        "INSERT INTO servers (
            id, name, mc_version, memory_mi,
            exposure_mode, storage_class, storage_size_gi, source_config,
            source_kind, properties, nodeport, created_at, last_started_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
    )
    .bind(id)
    .bind(name)
    .bind(mc_version)
    .bind(memory_mi)
    .bind(exposure_mode)
    .bind(storage_class)
    .bind(storage_size_gi)
    .bind(source_config)
    .bind(source_kind)
    .bind(properties_json)
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

/// Logs a kube delete result issued during create-rollback. 404s mean the
/// resource was never durably created and we silently move on; transport
/// errors are logged so the operator can clean them up if needed (the
/// SQLite row remains and a follow-up DELETE will retry the steps).
fn tolerate_404<T>(result: Result<T, kube::Error>, kind: &'static str, server_id: &str) {
    match result {
        Ok(_) => {}
        Err(kube::Error::Api(err)) if err.code == 404 => {}
        Err(other) => {
            tracing::warn!(
                server.id = %server_id,
                resource_kind = kind,
                error = %other,
                "rollback delete failed",
            );
        }
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
            SOURCE_KIND_VANILLA,
            "nodeport",
            None,
            10,
            "{}",
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
            SOURCE_KIND_CURSEFORGE,
            "loadbalancer",
            Some("tank"),
            20,
            r#"{"project_id":1148445}"#,
            "{}",
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
