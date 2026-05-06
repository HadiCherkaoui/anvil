//! Transitive required-dependency resolver for Modrinth & `CurseForge` mods.
//!
//! Given a seed [`ModEntry`] the user just picked, resolves every required
//! dependency reachable in upstream metadata, excluding entries already
//! installed or already pending. Cycle-safe via a visited set; depth-capped.
//!
//! Optional, embedded, incompatible, etc. relations are dropped at
//! [`super::deps`] normalisation; only `Required` deps are followed.
//!
//! Per-step failures (network, missing version, parse) are logged and
//! skipped — the Add op for the seed still goes through.

use std::collections::{HashSet, VecDeque};

use anyhow::{anyhow, Result};
use tracing::{event, Level};

use super::deps::{from_curseforge, from_modrinth, DepKind, DependencySpec};
use super::modded::ModEntry;
use super::ModpackHttp;

/// How deep a chain of required deps the resolver follows before bailing.
///
/// Real-world dep chains rarely exceed 2–3 hops. Five gives a safety margin
/// while bounding the worst case (e.g. a misbehaving graph with a long
/// linear chain) to a manageable number of upstream calls.
const MAX_DEPTH: usize = 5;

/// Per-resolution state: server target + what the resolver should treat as
/// already-known. `installed` holds keys for mods already in `cfg.mods`;
/// `pending` holds keys for mods queued in `cfg.pending` as Add ops.
#[derive(Debug)]
pub struct ResolveContext<'a> {
    pub mc_version: &'a str,
    pub loader: &'a str,
    pub installed: HashSet<(String, String)>,
    pub pending: HashSet<(String, String)>,
}

/// Resolves the transitive required dependencies of `seed`.
///
/// Returns the new [`ModEntry`] values to append (in resolution order).
/// Already-installed / already-pending deps are skipped. Optional deps are
/// dropped. The seed itself is *not* in the returned vec — the caller is
/// expected to have already produced the seed's Add op.
///
/// # Errors
///
/// Returns an error only on contract violations (e.g. CF deps but no CF
/// client). Per-step upstream failures are logged and skipped instead of
/// propagating.
pub async fn resolve_required(
    seed: &ModEntry,
    ctx: &mut ResolveContext<'_>,
    http: &ModpackHttp<'_>,
) -> Result<Vec<ModEntry>> {
    let mut out: Vec<ModEntry> = Vec::new();
    let mut queue: VecDeque<(ModEntry, usize)> = VecDeque::new();
    let mut visited: HashSet<(String, String)> = HashSet::new();

    queue.push_back((seed.clone(), 0));
    visited.insert((seed.provider.clone(), seed.project_id.clone()));

    while let Some((cur, depth)) = queue.pop_front() {
        if depth >= MAX_DEPTH {
            event!(
                name: "anvil.deps.depth_cap",
                Level::WARN,
                project.id = %cur.project_id,
                "dep resolver depth cap reached; pruning",
            );
            continue;
        }

        let deps = match fetch_deps_for_entry(http, &cur).await {
            Ok(d) => d,
            Err(err) => {
                event!(
                    name: "anvil.deps.fetch_failed",
                    Level::WARN,
                    project.id = %cur.project_id,
                    err = %err,
                    "fetching deps for entry failed; skipping subtree",
                );
                continue;
            }
        };

        for spec in deps.into_iter().filter(|d| d.kind == DepKind::Required) {
            let key = (spec.provider.clone(), spec.project_id.clone());
            if ctx.installed.contains(&key)
                || ctx.pending.contains(&key)
                || !visited.insert(key.clone())
            {
                continue;
            }

            let entry = match resolve_one(http, &spec, ctx).await {
                Ok(e) => e,
                Err(err) => {
                    event!(
                        name: "anvil.deps.resolve_failed",
                        Level::WARN,
                        project.id = %spec.project_id,
                        err = %err,
                        "resolving dep entry failed; skipping",
                    );
                    continue;
                }
            };
            ctx.pending.insert(key);
            out.push(entry.clone());
            queue.push_back((entry, depth + 1));
        }
    }

    Ok(out)
}

async fn fetch_deps_for_entry(
    http: &ModpackHttp<'_>,
    entry: &ModEntry,
) -> Result<Vec<DependencySpec>> {
    match entry.provider.as_str() {
        "modrinth" => {
            let v = http.mr.version(&entry.version_id).await?;
            Ok(from_modrinth(&v.dependencies))
        }
        "curseforge" => {
            let cf = http.cf.ok_or_else(|| anyhow!("CF client unavailable"))?;
            let project_id: u32 = entry.project_id.parse()?;
            let file_id: u32 = entry.version_id.parse()?;
            let f = cf.file(project_id, file_id).await?;
            Ok(from_curseforge(&f.dependencies))
        }
        other => Err(anyhow!("unknown provider {other:?}")),
    }
}

async fn resolve_one(
    http: &ModpackHttp<'_>,
    spec: &DependencySpec,
    ctx: &ResolveContext<'_>,
) -> Result<ModEntry> {
    match spec.provider.as_str() {
        "modrinth" => resolve_one_modrinth(http, spec, ctx).await,
        "curseforge" => resolve_one_curseforge(http, spec, ctx).await,
        other => Err(anyhow!("unknown provider {other:?}")),
    }
}

async fn resolve_one_modrinth(
    http: &ModpackHttp<'_>,
    spec: &DependencySpec,
    ctx: &ResolveContext<'_>,
) -> Result<ModEntry> {
    let project = http.mr.project(&spec.project_id).await?;
    let version = if let Some(vid) = &spec.pinned_version_id {
        http.mr.version(vid).await?
    } else {
        let versions = http.mr.list_versions(&spec.project_id).await?;
        versions
            .iter()
            .filter(|v| v.loaders.iter().any(|l| l == ctx.loader))
            .filter(|v| v.game_versions.iter().any(|g| g == ctx.mc_version))
            .filter(|v| v.files.iter().any(|f| f.primary))
            .max_by(|a, b| a.date_published.cmp(&b.date_published))
            .cloned()
            .ok_or_else(|| anyhow!("no compatible Modrinth version"))?
    };
    let primary = version
        .files
        .iter()
        .find(|f| f.primary)
        .or_else(|| version.files.first())
        .ok_or_else(|| anyhow!("Modrinth version has no files"))?;
    Ok(ModEntry {
        provider: "modrinth".to_owned(),
        project_id: project.id,
        project_slug: project.slug,
        project_name: project.title,
        version_id: version.id.clone(),
        version_name: version.version_number.clone(),
        filename: primary.filename.clone(),
        download_url: primary.url.clone(),
        sha512: primary.hashes.sha512.clone(),
    })
}

async fn resolve_one_curseforge(
    http: &ModpackHttp<'_>,
    spec: &DependencySpec,
    ctx: &ResolveContext<'_>,
) -> Result<ModEntry> {
    let cf = http.cf.ok_or_else(|| anyhow!("CF client unavailable"))?;
    let project_id: u32 = spec.project_id.parse()?;
    let project = cf.project(project_id).await?;

    let file = if let Some(vid) = &spec.pinned_version_id {
        let file_id: u32 = vid.parse()?;
        cf.file(project_id, file_id).await?
    } else {
        let files = cf.list_files(project_id).await?;
        files
            .iter()
            .filter(|f| cf_file_matches(f, ctx.mc_version, ctx.loader))
            .max_by(|a, b| a.file_date.cmp(&b.file_date))
            .cloned()
            .ok_or_else(|| anyhow!("no compatible CurseForge version"))?
    };

    let download_url = file
        .download_url
        .clone()
        .ok_or_else(|| anyhow!("CurseForge file has no download URL"))?;
    let filename = if file.file_name.is_empty() {
        file.display_name.clone()
    } else {
        file.file_name.clone()
    };
    Ok(ModEntry {
        provider: "curseforge".to_owned(),
        project_id: project.id.to_string(),
        project_slug: project.slug,
        project_name: project.name,
        version_id: file.id.to_string(),
        version_name: file.display_name.clone(),
        filename,
        download_url,
        sha512: None,
    })
}

fn cf_file_matches(file: &super::cf_client::CfFile, mc_version: &str, loader: &str) -> bool {
    let mc_ok = file
        .game_versions
        .iter()
        .any(|v| v.eq_ignore_ascii_case(mc_version));
    let loader_ok = file
        .game_versions
        .iter()
        .any(|v| v.eq_ignore_ascii_case(loader));
    mc_ok && loader_ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modpack::cf_client::CfFile;

    fn cf(id: u32, gvs: &[&str]) -> CfFile {
        CfFile {
            id,
            display_name: format!("v{id}"),
            release_type: 1,
            is_server_pack: false,
            server_pack_file_id: None,
            download_url: Some(format!("https://example/{id}.jar")),
            file_date: format!("2026-01-{id:02}"),
            file_name: format!("{id}.jar"),
            game_versions: gvs.iter().map(|s| (*s).to_owned()).collect(),
            dependencies: Vec::new(),
        }
    }

    #[test]
    fn cf_file_matches_when_both_present() {
        let f = cf(1, &["1.21.4", "Forge"]);
        assert!(cf_file_matches(&f, "1.21.4", "Forge"));
        assert!(cf_file_matches(&f, "1.21.4", "forge"));
    }

    #[test]
    fn cf_file_skips_when_mc_missing() {
        let f = cf(1, &["1.20", "Forge"]);
        assert!(!cf_file_matches(&f, "1.21.4", "forge"));
    }

    #[test]
    fn cf_file_skips_when_loader_missing() {
        let f = cf(1, &["1.21.4"]);
        assert!(!cf_file_matches(&f, "1.21.4", "fabric"));
    }
}
