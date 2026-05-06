//! Normalised mod-dependency types shared across providers.
//!
//! Modrinth and `CurseForge` each describe upstream dependencies in their own
//! shape (see [`mr_client::MrDependency`] and [`cf_client::CfDependency`]).
//! [`DependencySpec`] is the panel-internal normalised form the dep-resolver
//! consumes, with kinds collapsed to the two we actually act on.
//!
//! [`mr_client::MrDependency`]: super::mr_client::MrDependency
//! [`cf_client::CfDependency`]: super::cf_client::CfDependency

use super::cf_client::CfDependency;
use super::mr_client::MrDependency;

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
#[must_use]
pub fn from_modrinth(deps: &[MrDependency]) -> Vec<DependencySpec> {
    deps.iter()
        .filter_map(|d| {
            let kind = match d.dependency_type.as_str() {
                "required" => DepKind::Required,
                "optional" => DepKind::Optional,
                _ => return None,
            };
            let project_id = d.project_id.clone()?;
            Some(DependencySpec {
                provider: "modrinth".to_owned(),
                project_id,
                pinned_version_id: d.version_id.clone(),
                kind,
            })
        })
        .collect()
}

/// Normalises `CurseForge` dependency entries.
#[must_use]
pub fn from_curseforge(deps: &[CfDependency]) -> Vec<DependencySpec> {
    deps.iter()
        .filter_map(|d| {
            let kind = match d.relation_type {
                3 => DepKind::Required,
                2 => DepKind::Optional,
                _ => return None,
            };
            Some(DependencySpec {
                provider: "curseforge".to_owned(),
                project_id: d.mod_id.to_string(),
                pinned_version_id: None,
                kind,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modrinth_filters_to_required_and_optional() {
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
        let out = from_modrinth(&raw);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, DepKind::Required);
        assert_eq!(out[0].project_id, "a");
        assert_eq!(out[1].kind, DepKind::Optional);
        assert_eq!(out[1].project_id, "c");
    }

    #[test]
    fn modrinth_drops_entries_without_project_id() {
        let raw = vec![MrDependency {
            version_id: Some("v".into()),
            project_id: None,
            file_name: None,
            dependency_type: "required".into(),
        }];
        assert!(from_modrinth(&raw).is_empty());
    }

    #[test]
    fn modrinth_pinned_version_is_kept() {
        let raw = vec![MrDependency {
            version_id: Some("ver-x".into()),
            project_id: Some("p".into()),
            file_name: None,
            dependency_type: "required".into(),
        }];
        let out = from_modrinth(&raw);
        assert_eq!(out[0].pinned_version_id.as_deref(), Some("ver-x"));
    }

    #[test]
    fn cf_relation_3_required_2_optional() {
        let raw = vec![
            CfDependency {
                mod_id: 1,
                relation_type: 3,
            },
            CfDependency {
                mod_id: 2,
                relation_type: 2,
            },
            CfDependency {
                mod_id: 3,
                relation_type: 5,
            },
            CfDependency {
                mod_id: 4,
                relation_type: 6,
            },
        ];
        let out = from_curseforge(&raw);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, DepKind::Required);
        assert_eq!(out[0].project_id, "1");
        assert_eq!(out[1].kind, DepKind::Optional);
        assert_eq!(out[1].project_id, "2");
    }
}
