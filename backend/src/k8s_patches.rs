//! Server-side-apply patches we apply to managed `StatefulSet`s outside the
//! reconcile path.
//!
//! Currently a single helper — the orchestrator and the settings handler both
//! need to update the `mc` container's env without recreating the resource.
//! The function targets the `mc` container by name; every managed server has
//! exactly one such container (see `k8s_builders::build_statefulset`).

use anyhow::{Context as _, Result};
use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::core::v1::EnvVar;
use kube::Api;
use kube::api::{Patch, PatchParams};
use serde_json::json;

use crate::server_properties::ServerProperties;

/// Field manager identifier used on every server-side-apply patch the panel
/// emits. Sharing a single name keeps ownership consistent across handlers
/// (start, stop, settings, storage, orchestrator) — k8s tracks per-field
/// ownership by manager name.
pub const ANVIL_FIELD_MANAGER: &str = "anvil";

/// Returns `provider_env` with the server's persisted `server.properties`
/// env overrides appended.
///
/// Every code path that applies the `mc` container env MUST route through
/// this. Because [`patch_statefulset_env`] makes `anvil` the authoritative
/// owner of `containers[name=mc].env` (keyed by entry name), a patch that
/// sends provider-only env silently strips the property overrides a prior
/// `PATCH /settings` applied. The modpack update, version change, and
/// restore paths all rebuild env from a provider that has no notion of
/// `server.properties`, so they merge them back here.
///
/// An absent server row or invalid `properties` JSON yields the provider
/// env unchanged / property defaults.
///
/// # Errors
///
/// Returns the `sqlx` error if the `properties` lookup fails.
pub async fn with_properties_env(
    pool: &sqlx::SqlitePool,
    server_id: &str,
    provider_env: &[EnvVar],
) -> Result<Vec<EnvVar>> {
    let mut env = provider_env.to_vec();
    let row: Option<(String,)> = sqlx::query_as("SELECT properties FROM servers WHERE id = ?")
        .bind(server_id)
        .fetch_optional(pool)
        .await
        .context("loading server.properties for env apply")?;
    if let Some((json,)) = row {
        let props: ServerProperties = serde_json::from_str(&json).unwrap_or_default();
        env.extend(props.to_env());
    }
    Ok(env)
}

/// Server-side-applies the `mc` container's full `env` array on a managed
/// `StatefulSet`.
///
/// Strategic-merge silently dropped removals (the panel's previous shape) —
/// k8s merges arrays by `name` and never observes a removal. Server-side
/// apply with `force: true` makes the panel the authoritative owner of
/// `containers[name=mc].env`, so any entry omitted from `env` is removed.
/// Callers pass the *full* env block.
///
/// # Errors
///
/// Returns the `kube::Error` raised by the underlying `patch` call (e.g. 404
/// when the `StatefulSet` does not exist, 403 when RBAC blocks the verb).
pub async fn patch_statefulset_env(
    client: &kube::Client,
    ns: &str,
    server_id: &str,
    env: &[EnvVar],
) -> Result<()> {
    let stsets: Api<StatefulSet> = Api::namespaced(client.clone(), ns);
    let resource_name = format!("mc-{server_id}");
    // Apply patches must include enough identifying state for the API
    // server to resolve the target — apiVersion+kind+name on the
    // resource, plus the container `name` so strategic-merge keys onto
    // the right container. The env block is the authoritative slice.
    let apply = json!({
        "apiVersion": "apps/v1",
        "kind": "StatefulSet",
        "metadata": { "name": resource_name },
        "spec": {
            "template": {
                "spec": {
                    "containers": [
                        {
                            "name": "mc",
                            "env": env,
                        }
                    ]
                }
            }
        }
    });
    let pp = PatchParams::apply(ANVIL_FIELD_MANAGER).force();
    stsets
        .patch(&resource_name, &pp, &Patch::Apply(&apply))
        .await
        .with_context(|| format!("patching env on StatefulSet {resource_name}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn seed(pool: &sqlx::SqlitePool, id: &str, properties: &str) {
        sqlx::query(
            "INSERT INTO servers
                (id, name, mc_version, memory_mi, source_kind, exposure_mode,
                 storage_size_gi, source_config, created_at, properties)
             VALUES (?, ?, '1.21.4', 4096, 'vanilla', 'clusterip', 10, '{}', 0, ?)",
        )
        .bind(id)
        .bind(id)
        .bind(properties)
        .execute(pool)
        .await
        .expect("seed server");
    }

    #[tokio::test]
    async fn appends_properties_overrides_to_provider_env() {
        let pool = crate::db::init("sqlite::memory:").await.expect("migrate");
        seed(&pool, "s1", r#"{"difficulty":"hard"}"#).await;
        let provider = vec![EnvVar {
            name: "MAX_MEMORY".to_owned(),
            value: Some("4096M".to_owned()),
            ..EnvVar::default()
        }];
        let full = with_properties_env(&pool, "s1", &provider)
            .await
            .expect("merge");
        assert!(full.iter().any(|e| e.name == "MAX_MEMORY"));
        assert!(
            full.iter()
                .any(|e| e.name == "DIFFICULTY" && e.value.as_deref() == Some("hard")),
            "server.properties override must survive an env apply",
        );
    }

    #[tokio::test]
    async fn absent_row_returns_provider_env_unchanged() {
        let pool = crate::db::init("sqlite::memory:").await.expect("migrate");
        let provider = vec![EnvVar {
            name: "VERSION".to_owned(),
            value: Some("1.21.4".to_owned()),
            ..EnvVar::default()
        }];
        let full = with_properties_env(&pool, "missing", &provider)
            .await
            .expect("merge");
        assert_eq!(full.len(), 1);
    }
}
