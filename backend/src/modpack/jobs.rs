//! Backup, swap, and restore Job builders.
//!
//! All three Jobs mount the per-server data PVC (`data-mc-{id}-0`); backup
//! and restore additionally mount the shared `mc-snapshots` PVC. Each Job
//! has `backoffLimit: 0` so failures surface immediately to the orchestrator
//! (we own retry semantics — the Job's job is to run once and report).

use std::collections::BTreeMap;

use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    Container, EnvVar, EnvVarSource, PersistentVolumeClaimVolumeSource, PodSpec, PodTemplateSpec,
    SecretKeySelector, Volume, VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use crate::k8s::{LABEL_SERVER, MANAGED_BY_LABEL, MANAGED_BY_VALUE};

/// Auto-cleanup window for completed Jobs. 10min gives the orchestrator
/// time to read final status before the API server reaps them.
const JOB_TTL_SECONDS: i32 = 600;

/// Image used for backup/restore — busybox ships `tar` + `sh` and is tiny.
const BUSYBOX_IMAGE: &str = "busybox:1.36";

/// Image used for swap — alpine because we need `curl` + `unzip` available
/// via `apk add`. Pinned to a recent stable tag.
const ALPINE_IMAGE: &str = "alpine:3.20";

/// How many tar backups the GC tail of the backup Job keeps per server.
///
/// 3 fits ATM-11's ~5–10 GB-per-archive footprint into a 100 GiB shared
/// PVC across ~5 servers. Hardcoded per the M5 plan (no UI control).
const BACKUP_KEEP_COUNT: usize = 3;

/// Builds the backup Job.
///
/// Tars `/data` into `/snap/mc-{id}/mc-{id}-{ts}.tgz` on the shared snapshots
/// PVC, then GCs old archives down to the newest [`BACKUP_KEEP_COUNT`]. The
/// data PVC is mounted read-only — the `StatefulSet` must be scaled to 0
/// first so the RWO mount is available.
#[must_use]
pub fn build_backup_job(server_id: &str, ts: i64, namespace: &str, snapshots_pvc: &str) -> Job {
    let resource_name = format!("mc-{server_id}");
    let pvc_name = format!("data-{resource_name}-0");
    let job_name = format!("backup-{resource_name}-{ts}");
    let archive_path = format!("/snap/{resource_name}/{resource_name}-{ts}.tgz");
    // Tar, then `ls -t` newest-first, drop the first N (lines after) — busybox
    // `xargs -r` skips the rm when the list is empty.
    let gc_skip = BACKUP_KEEP_COUNT + 1;
    let cmd = format!(
        "set -eu; mkdir -p /snap/{resource_name}; tar czf {archive_path} -C /data .; \
         echo backup wrote {archive_path}; \
         cd /snap/{resource_name} && ls -t | tail -n +{gc_skip} | xargs -r rm -f"
    );

    let container = Container {
        name: "tar".to_owned(),
        image: Some(BUSYBOX_IMAGE.to_owned()),
        command: Some(vec!["sh".to_owned(), "-c".to_owned(), cmd]),
        volume_mounts: Some(vec![
            VolumeMount {
                name: "data".to_owned(),
                mount_path: "/data".to_owned(),
                read_only: Some(true),
                ..VolumeMount::default()
            },
            VolumeMount {
                name: "snap".to_owned(),
                mount_path: "/snap".to_owned(),
                ..VolumeMount::default()
            },
        ]),
        ..Container::default()
    };

    job(
        &job_name,
        namespace,
        labels(server_id, "backup"),
        container,
        vec![data_volume(&pvc_name), snapshots_volume(snapshots_pvc)],
    )
}

/// Builds the restore Job.
///
/// Wipes `/data/*`, then untars `/snap/mc-{id}/mc-{id}-{ts}.tgz` back into
/// `/data`. Use case: rolling back after a failed update.
#[must_use]
pub fn build_restore_job(server_id: &str, ts: i64, namespace: &str, snapshots_pvc: &str) -> Job {
    let resource_name = format!("mc-{server_id}");
    let pvc_name = format!("data-{resource_name}-0");
    let job_name = format!("restore-{resource_name}-{ts}");
    let archive_path = format!("/snap/{resource_name}/{resource_name}-{ts}.tgz");
    // `find /data -mindepth 1 -delete` rather than `rm -rf /data/*` so dotfiles
    // (e.g. `.eulafailures`) get cleaned up too. busybox find supports `-delete`.
    let cmd = format!(
        "set -eu; find /data -mindepth 1 -delete; tar xzf {archive_path} -C /data; \
         echo restore from {archive_path} done"
    );

    let container = Container {
        name: "untar".to_owned(),
        image: Some(BUSYBOX_IMAGE.to_owned()),
        command: Some(vec!["sh".to_owned(), "-c".to_owned(), cmd]),
        volume_mounts: Some(vec![
            VolumeMount {
                name: "data".to_owned(),
                mount_path: "/data".to_owned(),
                ..VolumeMount::default()
            },
            VolumeMount {
                name: "snap".to_owned(),
                mount_path: "/snap".to_owned(),
                read_only: Some(true),
                ..VolumeMount::default()
            },
        ]),
        ..Container::default()
    };

    job(
        &job_name,
        namespace,
        labels(server_id, "restore"),
        container,
        vec![data_volume(&pvc_name), snapshots_volume(snapshots_pvc)],
    )
}

/// Builds the swap Job.
///
/// Preserves user-owned files, wipes pack-owned dirs, downloads the new
/// `ServerFiles` zip, unpacks, restores preserved files, then re-merges the
/// panel-managed `server.properties` keys (RCON enable + port + password).
#[must_use]
pub fn build_swap_job(
    server_id: &str,
    ts: i64,
    namespace: &str,
    download_url: &str,
    preserve_list: &[&str],
    wipe_list: &[&str],
    rcon_secret_name: &str,
) -> Job {
    let resource_name = format!("mc-{server_id}");
    let pvc_name = format!("data-{resource_name}-0");
    let job_name = format!("swap-{resource_name}-{ts}");

    let env = vec![
        env_kv("CF_DOWNLOAD_URL", download_url),
        env_kv("PRESERVE_LIST", &preserve_list.join("\n")),
        env_kv("WIPE_LIST", &wipe_list.join("\n")),
        env_secret("RCON_PASSWORD", rcon_secret_name, "password"),
    ];

    let container = Container {
        name: "swap".to_owned(),
        image: Some(ALPINE_IMAGE.to_owned()),
        command: Some(vec![
            "sh".to_owned(),
            "-c".to_owned(),
            SWAP_SCRIPT.to_owned(),
        ]),
        env: Some(env),
        volume_mounts: Some(vec![VolumeMount {
            name: "data".to_owned(),
            mount_path: "/data".to_owned(),
            ..VolumeMount::default()
        }]),
        ..Container::default()
    };

    job(
        &job_name,
        namespace,
        labels(server_id, "swap"),
        container,
        vec![data_volume(&pvc_name)],
    )
}

/// Builds the mod-sync Job.
///
/// Wipes any `/data/{target_dir}/*.jar` not in `keep_filenames`, then
/// downloads any `desired_urls` line whose filename isn't yet present.
/// Verifies sha512 when supplied. The data PVC is the only mount; no
/// snapshots PVC needed. `target_dir` is `"mods"` for modded servers and
/// `"plugins"` for Paper servers.
#[must_use]
pub fn build_mod_sync_job(
    server_id: &str,
    ts: i64,
    namespace: &str,
    target_dir: &str,
    keep_filenames: &[&str],
    desired_urls: &[(&str, &str, Option<&str>)],
) -> Job {
    let resource_name = format!("mc-{server_id}");
    let pvc_name = format!("data-{resource_name}-0");
    let job_name = format!("mod-sync-{resource_name}-{ts}");

    let keep = keep_filenames.join("\n");
    let desired = desired_urls
        .iter()
        .map(|(filename, url, sha)| format!("{filename}\t{url}\t{}", sha.unwrap_or("")))
        .collect::<Vec<_>>()
        .join("\n");

    let env = vec![
        env_kv("TARGET_DIR", target_dir),
        env_kv("KEEP_FILENAMES", &keep),
        env_kv("DESIRED_URLS", &desired),
    ];

    let container = Container {
        name: "sync".to_owned(),
        image: Some(ALPINE_IMAGE.to_owned()),
        command: Some(vec![
            "sh".to_owned(),
            "-c".to_owned(),
            MOD_SYNC_SCRIPT.to_owned(),
        ]),
        env: Some(env),
        volume_mounts: Some(vec![VolumeMount {
            name: "data".to_owned(),
            mount_path: "/data".to_owned(),
            ..VolumeMount::default()
        }]),
        ..Container::default()
    };

    job(
        &job_name,
        namespace,
        labels(server_id, "mod-sync"),
        container,
        vec![data_volume(&pvc_name)],
    )
}

/// Inline shell script the mod-sync container runs.
///
/// Reads `$TARGET_DIR` for the data-relative subdir to manage (`mods` or
/// `plugins`). The same script handles modded mods and Paper plugins.
const MOD_SYNC_SCRIPT: &str = r#"
set -eu
apk add --no-cache curl >/dev/null

DEST="/data/$TARGET_DIR"
mkdir -p "$DEST"

# 1. Build the keep-set in a temp file.
echo "$KEEP_FILENAMES" > /tmp/keep.txt

# 2. Remove any jar in $DEST that isn't in the keep set.
for jar in "$DEST"/*.jar; do
  [ -e "$jar" ] || continue
  base=$(basename "$jar")
  if ! grep -qxF "$base" /tmp/keep.txt; then
    echo "removing $base"
    rm -f "$jar"
  fi
done

# 3. Download every DESIRED_URLS line whose filename isn't yet present.
echo "$DESIRED_URLS" | while IFS="$(printf '\t')" read -r filename url sha; do
  [ -z "$filename" ] && continue
  target="$DEST/$filename"
  if [ -e "$target" ]; then
    continue
  fi
  echo "fetching $filename"
  curl -fL "$url" -o "$target.tmp"
  if [ -n "$sha" ]; then
    echo "$sha  $target.tmp" | sha512sum -c -
  fi
  mv "$target.tmp" "$target"
done

echo "mod-sync complete ($TARGET_DIR)"
"#;

/// Inline shell script the swap container runs.
///
/// Exit-on-error (`set -eu`); preserve / wipe / download / unpack / restore /
/// remerge. The two design-review fixes are baked in:
///
///   1. `cp -a /tmp/preserved/. /data/` (dot, not glob) so an empty
///      preserved dir does not break the cp.
///   2. `ensure_kv` writes server.properties whether the file already exists
///      or not — the modpack's `startserver.sh` may not have created it yet
///      on first install.
const SWAP_SCRIPT: &str = r#"
set -eu
apk add --no-cache curl unzip findutils >/dev/null

cd /data
mkdir -p /tmp/preserved

# 1. Preserve user data (handle missing gracefully).
echo "$PRESERVE_LIST" | while IFS= read -r pat; do
  [ -z "$pat" ] && continue
  for src in /data/$pat; do
    [ -e "$src" ] || continue
    cp -a "$src" /tmp/preserved/
  done
done

# 2. Wipe pack-owned dirs and stale shell scripts / installer jars.
echo "$WIPE_LIST" | while IFS= read -r pat; do
  [ -z "$pat" ] && continue
  rm -rf /data/$pat
done
find /data -maxdepth 1 -name "*.sh" -delete
find /data -maxdepth 1 -name "neoforge-*-installer.jar" -delete
find /data -maxdepth 1 -name "forge-*-installer.jar" -delete

# 3. Download + unpack the new ServerFiles zip.
curl -fL "$CF_DOWNLOAD_URL" -o /tmp/pack.zip
unzip -o /tmp/pack.zip -d /data

# 4. Restore preserved (preserved values WIN on collision).
if [ -d /tmp/preserved ] && [ "$(ls -A /tmp/preserved)" ]; then
  cp -a /tmp/preserved/. /data/
fi

# 5. Re-merge our managed server.properties keys (handle file-absent case).
SP=/data/server.properties
touch "$SP"
ensure_kv () {
  KEY="$1"; VAL="$2"
  if grep -q "^${KEY}=" "$SP"; then
    sed -i "s|^${KEY}=.*|${KEY}=${VAL}|" "$SP"
  else
    echo "${KEY}=${VAL}" >> "$SP"
  fi
}
ensure_kv enable-rcon true
ensure_kv "rcon.port" 25575
ensure_kv "rcon.password" "$RCON_PASSWORD"
ensure_kv "query.port" 25565
ensure_kv "server-port" 25565

# 6. eula.txt always true (we accepted at server-create time).
echo "eula=true" > /data/eula.txt

# 7. Make the pack's launcher executable; some packs ship without +x.
find /data -maxdepth 1 -name "*.sh" -exec chmod +x {} +

echo "swap complete"
"#;

/// Common Job constructor.
fn job(
    name: &str,
    namespace: &str,
    labels: BTreeMap<String, String>,
    container: Container,
    volumes: Vec<Volume>,
) -> Job {
    Job {
        metadata: ObjectMeta {
            name: Some(name.to_owned()),
            namespace: Some(namespace.to_owned()),
            labels: Some(labels.clone()),
            ..ObjectMeta::default()
        },
        spec: Some(JobSpec {
            backoff_limit: Some(0),
            ttl_seconds_after_finished: Some(JOB_TTL_SECONDS),
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels),
                    ..ObjectMeta::default()
                }),
                spec: Some(PodSpec {
                    restart_policy: Some("Never".to_owned()),
                    containers: vec![container],
                    volumes: Some(volumes),
                    ..PodSpec::default()
                }),
            },
            ..JobSpec::default()
        }),
        status: None,
    }
}

fn labels(server_id: &str, kind: &str) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    m.insert(MANAGED_BY_LABEL.to_owned(), MANAGED_BY_VALUE.to_owned());
    m.insert(LABEL_SERVER.to_owned(), server_id.to_owned());
    m.insert("app.anvil.io/job-kind".to_owned(), kind.to_owned());
    m
}

fn data_volume(pvc_name: &str) -> Volume {
    Volume {
        name: "data".to_owned(),
        persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
            claim_name: pvc_name.to_owned(),
            ..PersistentVolumeClaimVolumeSource::default()
        }),
        ..Volume::default()
    }
}

fn snapshots_volume(pvc_name: &str) -> Volume {
    Volume {
        name: "snap".to_owned(),
        persistent_volume_claim: Some(PersistentVolumeClaimVolumeSource {
            claim_name: pvc_name.to_owned(),
            ..PersistentVolumeClaimVolumeSource::default()
        }),
        ..Volume::default()
    }
}

fn env_kv(name: &str, value: &str) -> EnvVar {
    EnvVar {
        name: name.to_owned(),
        value: Some(value.to_owned()),
        value_from: None,
    }
}

fn env_secret(name: &str, secret_name: &str, key: &str) -> EnvVar {
    EnvVar {
        name: name.to_owned(),
        value: None,
        value_from: Some(EnvVarSource {
            secret_key_ref: Some(SecretKeySelector {
                name: secret_name.to_owned(),
                key: key.to_owned(),
                optional: Some(false),
            }),
            ..EnvVarSource::default()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_job_name_includes_server_id_and_ts() {
        let j = build_backup_job("abc123", 1_700_000_000, "mc", "mc-snapshots");
        assert_eq!(
            j.metadata.name.as_deref(),
            Some("backup-mc-abc123-1700000000")
        );
        assert_eq!(j.metadata.namespace.as_deref(), Some("mc"));
    }

    #[test]
    fn backup_job_mounts_data_read_only() {
        let j = build_backup_job("abc", 1, "mc", "mc-snapshots");
        let vmounts = j.spec.unwrap().template.spec.unwrap().containers[0]
            .volume_mounts
            .clone()
            .unwrap();
        let data = vmounts.iter().find(|m| m.name == "data").unwrap();
        assert_eq!(data.read_only, Some(true));
    }

    #[test]
    fn backup_job_no_retries() {
        let j = build_backup_job("abc", 1, "mc", "mc-snapshots");
        assert_eq!(j.spec.unwrap().backoff_limit, Some(0));
    }

    #[test]
    fn swap_job_carries_preserve_and_wipe_lists_in_env() {
        let j = build_swap_job(
            "abc",
            1,
            "mc",
            "https://example.com/p.zip",
            &["world*", "ops.json"],
            &["mods", "config"],
            "mc-abc-rcon",
        );
        let env = j.spec.unwrap().template.spec.unwrap().containers[0]
            .env
            .clone()
            .unwrap();
        let preserve = env.iter().find(|e| e.name == "PRESERVE_LIST").unwrap();
        let wipe = env.iter().find(|e| e.name == "WIPE_LIST").unwrap();
        assert!(preserve.value.as_deref().unwrap().contains("world*"));
        assert!(wipe.value.as_deref().unwrap().contains("mods"));
    }

    #[test]
    fn swap_job_pulls_rcon_password_from_secret() {
        let j = build_swap_job("abc", 1, "mc", "url", &[], &[], "mc-abc-rcon");
        let env = j.spec.unwrap().template.spec.unwrap().containers[0]
            .env
            .clone()
            .unwrap();
        let rcon = env.iter().find(|e| e.name == "RCON_PASSWORD").unwrap();
        let sk = rcon
            .value_from
            .as_ref()
            .unwrap()
            .secret_key_ref
            .as_ref()
            .unwrap();
        assert_eq!(sk.name, "mc-abc-rcon");
    }

    #[test]
    fn swap_script_uses_dot_notation_for_cp() {
        // Guard against the "cp -a /tmp/preserved/* /data/" regression that
        // breaks when /tmp/preserved is empty.
        assert!(SWAP_SCRIPT.contains("cp -a /tmp/preserved/. /data/"));
        assert!(!SWAP_SCRIPT.contains("cp -a /tmp/preserved/* /data/"));
    }

    #[test]
    fn swap_script_writes_server_properties_unconditionally() {
        // ensure_kv must touch the file first so the rewrite works on first boot.
        assert!(SWAP_SCRIPT.contains("touch \"$SP\""));
        assert!(SWAP_SCRIPT.contains("ensure_kv enable-rcon true"));
        assert!(SWAP_SCRIPT.contains("ensure_kv \"rcon.password\""));
    }

    #[test]
    fn restore_job_wipes_then_untars() {
        let j = build_restore_job("abc", 1, "mc", "mc-snapshots");
        let cmd = j.spec.unwrap().template.spec.unwrap().containers[0]
            .command
            .clone()
            .unwrap();
        let script = cmd.last().unwrap();
        assert!(script.contains("find /data -mindepth 1 -delete"));
        assert!(script.contains("tar xzf"));
    }

    #[test]
    fn mod_sync_job_carries_keep_and_desired_in_env() {
        let j = build_mod_sync_job(
            "abc",
            1,
            "mc",
            "mods",
            &["sodium.jar", "lithium.jar"],
            &[("iris.jar", "https://example/iris.jar", Some("ffff"))],
        );
        let env = j.spec.unwrap().template.spec.unwrap().containers[0]
            .env
            .clone()
            .unwrap();
        let keep = env.iter().find(|e| e.name == "KEEP_FILENAMES").unwrap();
        let desired = env.iter().find(|e| e.name == "DESIRED_URLS").unwrap();
        assert!(keep.value.as_deref().unwrap().contains("sodium.jar"));
        assert!(keep.value.as_deref().unwrap().contains("lithium.jar"));
        assert!(
            desired
                .value
                .as_deref()
                .unwrap()
                .contains("iris.jar\thttps://example/iris.jar\tffff")
        );
    }

    #[test]
    fn mod_sync_job_uses_data_pvc_only() {
        let j = build_mod_sync_job("abc", 1, "mc", "mods", &[], &[]);
        let v = j.spec.unwrap().template.spec.unwrap().volumes.unwrap();
        assert_eq!(v.len(), 1);
        let data = v.iter().find(|x| x.name == "data").unwrap();
        let pvc = data.persistent_volume_claim.as_ref().unwrap();
        assert_eq!(pvc.claim_name, "data-mc-abc-0");
    }

    #[test]
    fn mod_sync_job_name_includes_server_id_and_ts() {
        let j = build_mod_sync_job("abc", 1_700_000_000, "mc", "mods", &[], &[]);
        assert_eq!(
            j.metadata.name.as_deref(),
            Some("mod-sync-mc-abc-1700000000")
        );
    }

    #[test]
    fn mod_sync_job_target_dir_is_passed_via_env() {
        let j = build_mod_sync_job("abc", 1, "mc", "plugins", &[], &[]);
        let env = j.spec.unwrap().template.spec.unwrap().containers[0]
            .env
            .clone()
            .unwrap();
        let td = env.iter().find(|e| e.name == "TARGET_DIR").unwrap();
        assert_eq!(td.value.as_deref(), Some("plugins"));
    }

    #[test]
    fn mod_sync_script_uses_target_dir_var() {
        // Guard the script never reverts to a hardcoded /data/mods path.
        assert!(MOD_SYNC_SCRIPT.contains("DEST=\"/data/$TARGET_DIR\""));
        assert!(!MOD_SYNC_SCRIPT.contains("/data/mods/*.jar"));
    }
}
