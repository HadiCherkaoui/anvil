//! Transitive required-dependency resolver for Modrinth mods.
//!
//! Given a seed [`ModEntry`] the user just picked, resolves every required
//! dependency reachable in upstream metadata, excluding entries already
//! installed or already pending. Cycle-safe via a visited set; depth-capped.
//!
//! Optional, embedded, incompatible, etc. relations are dropped at
//! [`super::deps`] normalisation; only `Required` deps are followed.
//!
//! Modrinth-only by design: individual mods are sourced exclusively from
//! Modrinth (catalog `type=mod` is Modrinth, Modrinth deps point only at
//! other Modrinth projects), so the resolver doesn't need a `CurseForge`
//! branch. CF stays load-bearing for modpack-level downloads.
//!
//! Per-step failures (network, missing version, parse) are logged and
//! skipped — the Add op for the seed still goes through.

use std::collections::{HashSet, VecDeque};

use anyhow::{Result, anyhow};
use tracing::{Level, event};

use super::ModpackHttp;
use super::deps::{DepKind, DependencySpec, from_modrinth};
use super::modded::ModEntry;

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
    if entry.provider != "modrinth" {
        // Mods only come from Modrinth in the catalog flow; anything
        // else is a stale/legacy row we can't resolve deps for. Returning
        // empty lets the seed's Add op still go through.
        return Ok(Vec::new());
    }
    let v = http.mr.version(&entry.version_id).await?;
    from_modrinth(http.mr, &v.dependencies).await
}

async fn resolve_one(
    http: &ModpackHttp<'_>,
    spec: &DependencySpec,
    ctx: &ResolveContext<'_>,
) -> Result<ModEntry> {
    if spec.provider != "modrinth" {
        return Err(anyhow!("non-modrinth dep providers not supported"));
    }
    resolve_one_modrinth(http, spec, ctx).await
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
