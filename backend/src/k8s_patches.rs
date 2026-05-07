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

/// Field manager identifier used on every server-side-apply patch the panel
/// emits. Sharing a single name keeps ownership consistent across handlers
/// (start, stop, settings, storage, orchestrator) — k8s tracks per-field
/// ownership by manager name.
pub const ANVIL_FIELD_MANAGER: &str = "anvil";

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
