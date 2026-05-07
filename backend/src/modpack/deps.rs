//! Normalised mod-dependency types for the Modrinth-only mod path.
//!
//! Individual mods are sourced exclusively from Modrinth (the catalog
//! search restricts `type=mod` and `type=plugin` to Modrinth, and
//! Modrinth deps can only point at other Modrinth projects). `CurseForge`
//! is reserved for modpack-level downloads — that path bypasses this
//! resolver entirely.
//!
//! [`MrDependency`]: super::mr_client::MrDependency

use super::mr_client::{ModrinthClient, MrDependency};

/// Kind of dependency the resolver acts on.
///
/// Modrinth `"required"` and CF `relationType=3` map to [`DepKind::Required`];
/// Modrinth `"optional"` and CF `relationType=2` map to [`DepKind::Optional`].
/// Other kinds (incompatible, embedded, tool, include) are dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepKind {
    Required,
    Optional,
}

/// Normalised dependency descriptor.
///
/// `provider` matches `ModEntry::provider` (`"modrinth"` | `"curseforge"`).
/// `pinned_version_id` is honoured when set; otherwise the resolver picks
/// the newest version compatible with the server's `(mc_version, loader)`.
#[derive(Debug, Clone)]
pub struct DependencySpec {
    pub provider: String,
    pub project_id: String,
    pub pinned_version_id: Option<String>,
    pub kind: DepKind,
}

/// Normalises Modrinth dependency entries.
///
/// Modrinth lets a dependency carry `project_id`, `version_id`, both, or
/// neither (Modrinth API spec). Mods that pin a *minimum* version of a dep
/// frequently use `version_id` alone — JEI's `fabric-api ≥ 0.140.3+26.1`
/// pin is one such case. The previous synchronous `from_modrinth`
/// silently dropped those, leaving the resolver blind to the dep and
/// the boot to fail with `HARD_DEP_NO_CANDIDATE`.
///
/// This async variant resolves the version-id-only case via
/// `/v2/version/{id}` (one extra HTTP call per such dep) so the project
/// id is always present. Entries with neither id are still dropped —
/// nothing actionable to do with them.
///
/// # Errors
///
/// Returns the underlying Modrinth client error from the version lookup.
pub async fn from_modrinth(
    client: &ModrinthClient,
    deps: &[MrDependency],
) -> anyhow::Result<Vec<DependencySpec>> {
    let mut out = Vec::with_capacity(deps.len());
    for d in deps {
        let kind = match d.dependency_type.as_str() {
            "required" => DepKind::Required,
            "optional" => DepKind::Optional,
            _ => continue,
        };
        let project_id = match (d.project_id.as_deref(), d.version_id.as_deref()) {
            (Some(p), _) => p.to_owned(),
            (None, Some(vid)) => match client.version(vid).await {
                Ok(v) => v.project_id,
                Err(err) => {
                    tracing::warn!(
                        version_id = vid,
                        error = %err,
                        "failed to resolve Modrinth version-only dep; skipping",
                    );
                    continue;
                }
            },
            (None, None) => continue,
        };
        out.push(DependencySpec {
            provider: "modrinth".to_owned(),
            project_id,
            pinned_version_id: d.version_id.clone(),
            kind,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client() -> ModrinthClient {
        ModrinthClient::new().expect("ModrinthClient::new")
    }

    #[tokio::test]
    async fn modrinth_filters_to_required_and_optional() {
        let raw = vec![
            MrDependency {
                version_id: None,
                project_id: Some("a".into()),
                file_name: None,
                dependency_type: "required".into(),
            },
            MrDependency {
                version_id: None,
                project_id: Some("b".into()),
                file_name: None,
                dependency_type: "incompatible".into(),
            },
            MrDependency {
                version_id: None,
                project_id: Some("c".into()),
                file_name: None,
                dependency_type: "optional".into(),
            },
            MrDependency {
                version_id: None,
                project_id: Some("d".into()),
                file_name: None,
                dependency_type: "embedded".into(),
            },
        ];
        let client = test_client();
        let out = from_modrinth(&client, &raw).await.expect("ok");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, DepKind::Required);
        assert_eq!(out[0].project_id, "a");
        assert_eq!(out[1].kind, DepKind::Optional);
        assert_eq!(out[1].project_id, "c");
    }

    #[tokio::test]
    async fn modrinth_pinned_version_is_kept() {
        let raw = vec![MrDependency {
            version_id: Some("ver-x".into()),
            project_id: Some("p".into()),
            file_name: None,
            dependency_type: "required".into(),
        }];
        let client = test_client();
        let out = from_modrinth(&client, &raw).await.expect("ok");
        assert_eq!(out[0].pinned_version_id.as_deref(), Some("ver-x"));
    }
}
