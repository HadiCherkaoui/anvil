//! Strategic-merge patches we apply to managed `StatefulSet`s outside the
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

/// Strategic-merges the `mc` container's `env` array on a managed `StatefulSet`.
///
/// k8s strategic-merge keys env entries by `name`, so this patch updates /
/// inserts every entry in `env` and leaves unrelated entries on the resource
/// untouched. Pass the *full* env block when you need a deterministic state
/// (e.g. settings PATCH rebuilds and resends every entry) — passing a partial
/// list only mutates the listed names.
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
    let patch = json!({
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
    stsets
        .patch(
            &resource_name,
            &PatchParams::default(),
            &Patch::Strategic(&patch),
        )
        .await
        .with_context(|| format!("patching env on StatefulSet {resource_name}"))?;
    Ok(())
}
