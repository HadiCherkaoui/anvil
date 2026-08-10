// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Backup, restore, and mod-sync Job builders.
//!
//! Each Job mounts the per-server data PVC (`data-mc-{id}-0`); backup and
//! restore additionally mount the shared `mc-snapshots` PVC. Each Job has
//! `backoffLimit: 0` so failures surface immediately to the orchestrator
//! (we own retry semantics — the Job's job is to run once and report).

use std::collections::BTreeMap;

use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    Container, EnvVar, PersistentVolumeClaimVolumeSource, PodSpec, PodTemplateSpec, Volume,
    VolumeMount,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use crate::k8s::{LABEL_SERVER, MANAGED_BY_LABEL, MANAGED_BY_VALUE};

/// Auto-cleanup window for completed Jobs. 10min gives the orchestrator
/// time to read final status before the API server reaps them.
const JOB_TTL_SECONDS: i32 = 600;

/// How many tar backups the GC tail of the orchestrator's auto-backup Job
/// keeps per server.
///
/// 3 fits ATM-11's ~5–10 GB-per-archive footprint into a 100 GiB shared
/// PVC across ~5 servers. Manual backups opt out of GC entirely by passing
/// `gc_keep = None`.
pub const BACKUP_KEEP_COUNT: usize = 3;

/// Max chars of `archive_id` we splice into a Job name. Job names propagate
/// to the auto-injected `batch.kubernetes.io/job-name` label whose value
/// is bounded by the 63-char DNS-1035 label limit; with the longest prefix
/// (`restore-mc-{36-char-uuid}-`) that leaves 15 chars for the suffix. Cap
/// at 12 to keep a small safety margin for future prefix changes.
const ARCHIVE_ID_NAME_CAP: usize = 12;

/// Returns `archive_id` truncated to [`ARCHIVE_ID_NAME_CAP`] chars for use
/// in Job metadata. Path / DB still consume the full id.
fn name_suffix(archive_id: &str) -> &str {
    archive_id.get(..ARCHIVE_ID_NAME_CAP).unwrap_or(archive_id)
}

/// Builds the backup Job.
///
/// Tars `/data` into `/snap/mc-{id}/{subdir}/{archive_id}.tgz` on the shared
/// snapshots PVC. When `gc_keep = Some(n)`, prunes the newest-n in `subdir`
/// after writing; when `None`, no GC runs (manual backups own retention).
/// The data PVC is mounted read-only — the `StatefulSet` must be scaled to
/// 0 first so the RWO mount is available.
#[must_use]
pub fn build_backup_job(
    server_id: &str,
    archive_id: &str,
    namespace: &str,
    snapshots_pvc: &str,
    subdir: &str,
    gc_keep: Option<usize>,
    busybox_image: &str,
) -> Job {
    let resource_name = format!("mc-{server_id}");
    let pvc_name = format!("data-{resource_name}-0");
    let job_name = format!("backup-{resource_name}-{}", name_suffix(archive_id));
    let archive_path = format!("/snap/{resource_name}/{subdir}/{archive_id}.tgz");
    // Tar, then optionally `ls -t` newest-first and drop the first N — busybox
    // `xargs -r` skips the rm when the list is empty.
    let gc_cmd = match gc_keep {
        Some(keep) => format!(
            " && cd /snap/{resource_name}/{subdir} && ls -t | tail -n +{} | xargs -r rm -f",
            keep + 1
        ),
        None => String::new(),
    };
    let cmd = format!(
        "set -eu; mkdir -p /snap/{resource_name}/{subdir}; tar czf {archive_path} -C /data .; \
         echo backup wrote {archive_path}{gc_cmd}"
    );

    let container = Container {
        name: "tar".to_owned(),
        image: Some(busybox_image.to_owned()),
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
/// Wipes `/data/*`, then untars `/snap/mc-{id}/{subdir}/{archive_id}.tgz`
/// back into `/data`. Use cases: orchestrator rollback (`subdir = "auto"`)
/// and manual restore (`subdir = "manual"`).
#[must_use]
pub fn build_restore_job(
    server_id: &str,
    archive_id: &str,
    namespace: &str,
    snapshots_pvc: &str,
    subdir: &str,
    busybox_image: &str,
) -> Job {
    let resource_name = format!("mc-{server_id}");
    let pvc_name = format!("data-{resource_name}-0");
    let job_name = format!("restore-{resource_name}-{}", name_suffix(archive_id));
    let archive_path = format!("/snap/{resource_name}/{subdir}/{archive_id}.tgz");
    // `find /data -mindepth 1 -delete` rather than `rm -rf /data/*` so dotfiles
    // (e.g. `.eulafailures`) get cleaned up too. busybox find supports `-delete`.
    let cmd = format!(
        "set -eu; find /data -mindepth 1 -delete; tar xzf {archive_path} -C /data; \
         echo restore from {archive_path} done"
    );

    let container = Container {
        name: "untar".to_owned(),
        image: Some(busybox_image.to_owned()),
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

/// Builds the mod-sync Job.
///
/// Wipes any `/data/{target_dir}/*.jar` not in `keep_filenames`, then
/// downloads any `desired_urls` line whose filename isn't yet present.
/// Verifies sha512 when supplied. The data PVC is the only mount; no
/// snapshots PVC needed. `target_dir` is `"mods"` for modded servers and
/// `"plugins"` for Paper servers. `image` must provide `apk add curl`
/// (alpine — see `mcDefaults.alpineImage`).
#[must_use]
pub fn build_mod_sync_job(
    server_id: &str,
    ts: i64,
    namespace: &str,
    target_dir: &str,
    keep_filenames: &[&str],
    desired_urls: &[(&str, &str, Option<&str>)],
    image: &str,
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
        image: Some(image.to_owned()),
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

# Pick a hash tool once: prefer sha512, fall back to sha1, else verify-skip.
HASH_LEN=0
HASH_BIN=""
if command -v sha512sum >/dev/null 2>&1; then
  HASH_BIN="sha512sum"; HASH_LEN=128
elif command -v sha1sum >/dev/null 2>&1; then
  HASH_BIN="sha1sum"; HASH_LEN=40
else
  echo "warning: no sha512sum/sha1sum found; skipping hash verification"
fi

verify_hash() {
  # $1 path, $2 expected hex
  expected="$2"
  [ -z "$HASH_BIN" ] && return 0
  # Refuse to verify when the digest length doesn't match the picked tool
  # (e.g. sha512 expected but only sha1sum available).
  exp_len=$(printf %s "$expected" | wc -c)
  if [ "$exp_len" -ne "$HASH_LEN" ]; then
    echo "warning: digest length $exp_len does not match $HASH_BIN ($HASH_LEN); skipping verify for $1"
    return 0
  fi
  actual=$("$HASH_BIN" "$1" | awk '{print $1}')
  [ "$actual" = "$expected" ]
}

# 3. Download/refresh every DESIRED_URLS line. Existing files are
# re-hashed and re-downloaded on mismatch (catches partial / corrupted
# previous syncs); curl uses retries so transient network blips don't
# fail the whole Job.
echo "$DESIRED_URLS" | while IFS="$(printf '\t')" read -r filename url sha; do
  [ -z "$filename" ] && continue
  target="$DEST/$filename"
  if [ -e "$target" ] && [ -n "$sha" ]; then
    if verify_hash "$target" "$sha"; then
      continue
    fi
    echo "hash mismatch on existing $filename; re-downloading"
    rm -f "$target"
  elif [ -e "$target" ] && [ -z "$sha" ]; then
    # No expected hash, trust the filename and skip the re-download.
    continue
  fi
  echo "fetching $filename"
  curl -fL --retry 3 --retry-delay 2 --connect-timeout 30 --max-time 600 "$url" -o "$target.tmp"
  if [ -n "$sha" ] && [ -n "$HASH_BIN" ]; then
    if ! verify_hash "$target.tmp" "$sha"; then
      echo "ERROR: hash verification failed for $filename"
      rm -f "$target.tmp"
      exit 1
    fi
  fi
  mv "$target.tmp" "$target"
done

echo "mod-sync complete ($TARGET_DIR)"
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
                    // The sync container runs `curl` on a user-influenced URL;
                    // don't mount the namespace default ServiceAccount token so
                    // a file:// fetch can't exfiltrate cluster credentials.
                    automount_service_account_token: Some(false),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn extract_command(j: &Job) -> String {
        j.spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers[0]
            .command
            .as_ref()
            .unwrap()[2]
            .clone()
    }

    const TEST_BUSYBOX: &str = "busybox:test";
    const TEST_MOD_SYNC: &str = "alpine:test";

    #[test]
    fn backup_job_name_includes_server_id_and_archive_id() {
        let j = build_backup_job(
            "abc123",
            "1700000000",
            "mc",
            "mc-snapshots",
            "auto",
            Some(3),
            TEST_BUSYBOX,
        );
        assert_eq!(
            j.metadata.name.as_deref(),
            Some("backup-mc-abc123-1700000000")
        );
        assert_eq!(j.metadata.namespace.as_deref(), Some("mc"));
    }

    #[test]
    fn backup_job_mounts_data_read_only() {
        let j = build_backup_job(
            "abc",
            "1",
            "mc",
            "mc-snapshots",
            "auto",
            Some(3),
            TEST_BUSYBOX,
        );
        let vmounts = j.spec.unwrap().template.spec.unwrap().containers[0]
            .volume_mounts
            .clone()
            .unwrap();
        let data = vmounts.iter().find(|m| m.name == "data").unwrap();
        assert_eq!(data.read_only, Some(true));
    }

    #[test]
    fn backup_job_no_retries() {
        let j = build_backup_job(
            "abc",
            "1",
            "mc",
            "mc-snapshots",
            "auto",
            Some(3),
            TEST_BUSYBOX,
        );
        assert_eq!(j.spec.unwrap().backoff_limit, Some(0));
    }

    #[test]
    fn auto_backup_keeps_gc() {
        let j = build_backup_job(
            "abc",
            "1700000000",
            "mc",
            "mc-snapshots",
            "auto",
            Some(3),
            TEST_BUSYBOX,
        );
        let cmd = extract_command(&j);
        assert!(cmd.contains("xargs -r rm -f"));
        assert!(cmd.contains("/snap/mc-abc/auto/1700000000.tgz"));
        assert!(cmd.contains("cd /snap/mc-abc/auto"));
    }

    #[test]
    fn manual_backup_does_not_emit_gc_command() {
        let j = build_backup_job(
            "abc",
            "bk-uuid",
            "mc",
            "mc-snapshots",
            "manual",
            None,
            TEST_BUSYBOX,
        );
        let cmd = extract_command(&j);
        assert!(!cmd.contains("xargs"), "manual backup must not GC: {cmd}");
        assert!(cmd.contains("/snap/mc-abc/manual/bk-uuid.tgz"));
    }

    #[test]
    fn restore_job_wipes_then_untars() {
        let j = build_restore_job("abc", "1", "mc", "mc-snapshots", "auto", TEST_BUSYBOX);
        let script = extract_command(&j);
        assert!(script.contains("find /data -mindepth 1 -delete"));
        assert!(script.contains("tar xzf /snap/mc-abc/auto/1.tgz"));
    }

    #[test]
    fn restore_job_subdir_path_is_honoured() {
        let j = build_restore_job(
            "abc",
            "bk-uuid",
            "mc",
            "mc-snapshots",
            "manual",
            TEST_BUSYBOX,
        );
        let script = extract_command(&j);
        assert!(script.contains("/snap/mc-abc/manual/bk-uuid.tgz"));
        assert_eq!(j.metadata.name.as_deref(), Some("restore-mc-abc-bk-uuid"));
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
            TEST_MOD_SYNC,
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
        let j = build_mod_sync_job("abc", 1, "mc", "mods", &[], &[], TEST_MOD_SYNC);
        let v = j.spec.unwrap().template.spec.unwrap().volumes.unwrap();
        assert_eq!(v.len(), 1);
        let data = v.iter().find(|x| x.name == "data").unwrap();
        let pvc = data.persistent_volume_claim.as_ref().unwrap();
        assert_eq!(pvc.claim_name, "data-mc-abc-0");
    }

    #[test]
    fn mod_sync_job_name_includes_server_id_and_ts() {
        let j = build_mod_sync_job("abc", 1_700_000_000, "mc", "mods", &[], &[], TEST_MOD_SYNC);
        assert_eq!(
            j.metadata.name.as_deref(),
            Some("mod-sync-mc-abc-1700000000")
        );
    }

    #[test]
    fn mod_sync_job_target_dir_is_passed_via_env() {
        let j = build_mod_sync_job("abc", 1, "mc", "plugins", &[], &[], TEST_MOD_SYNC);
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

    // Regression: a UUID-shaped server_id plus a `bk-{32hex}` manual
    // archive_id used to produce an 82-char Job name; k8s rejected the
    // create at admission because the auto-injected
    // `batch.kubernetes.io/job-name` label exceeded the 63-byte cap.
    const REAL_UUID: &str = "b6ed52f6-741f-4639-8bba-754a45d75367";
    const REAL_BK: &str = "bk-d6577c07038b4b20912de48089220ec1";

    #[test]
    fn backup_job_name_fits_label_limit_with_uuid_and_manual_id() {
        let j = build_backup_job(
            REAL_UUID,
            REAL_BK,
            "mc",
            "mc-snapshots",
            "manual",
            None,
            TEST_BUSYBOX,
        );
        let name = j.metadata.name.as_deref().unwrap();
        assert!(
            name.len() <= 63,
            "job name must fit DNS-1035 label cap: {} chars: {name}",
            name.len()
        );
    }

    #[test]
    fn restore_job_name_fits_label_limit_with_uuid_and_manual_id() {
        let j = build_restore_job(
            REAL_UUID,
            REAL_BK,
            "mc",
            "mc-snapshots",
            "manual",
            TEST_BUSYBOX,
        );
        let name = j.metadata.name.as_deref().unwrap();
        assert!(
            name.len() <= 63,
            "job name must fit DNS-1035 label cap: {} chars: {name}",
            name.len()
        );
    }

    #[test]
    fn backup_archive_path_keeps_full_archive_id_even_when_name_truncates() {
        // The Job NAME is shortened to fit the label cap, but the path on
        // the snapshots PVC must still match the DB row's snapshot_path
        // (`manual/{archive_id}.tgz`) verbatim — otherwise restore reads
        // from a different file than the one we wrote.
        let j = build_backup_job(
            REAL_UUID,
            REAL_BK,
            "mc",
            "mc-snapshots",
            "manual",
            None,
            TEST_BUSYBOX,
        );
        let cmd = extract_command(&j);
        assert!(
            cmd.contains(&format!("/snap/mc-{REAL_UUID}/manual/{REAL_BK}.tgz")),
            "archive path must use the full archive_id: {cmd}"
        );
    }
}
