//! Pure input validation for the create handler.
//!
//! Functions here perform no I/O and do not allocate beyond the `String`
//! that error messages require. They run before any kube or DB call so
//! invalid requests short-circuit with a 400 response.

use crate::AppState;
use crate::error::AppError;

/// Offline floor for Minecraft version validation.
///
/// The live source is the cached Mojang manifest in
/// `AppState::mc_versions_cache`; this fallback list keeps the panel
/// usable when the manifest endpoint is unreachable.
pub const KNOWN_MC_VERSIONS: &[&str] =
    &["1.20.4", "1.20.6", "1.21.0", "1.21.1", "1.21.3", "1.21.4"];

/// Allowed exposure modes for managed-server Services.
pub const KNOWN_EXPOSURE_MODES: &[&str] = &["loadbalancer", "nodeport", "clusterip"];

/// Minimum memory (MiB) the create handler accepts.
const MEMORY_MI_MIN: i64 = 1024;
/// Maximum memory (MiB) the create handler accepts.
const MEMORY_MI_MAX: i64 = 16_384;
/// Required step (MiB). Memory selectors in the UI snap to this grid.
const MEMORY_MI_STEP: i64 = 1024;

/// Minimum CPU (millicores). 250m starves the JVM; below this is a misconfiguration.
const CPU_MILLICORES_MIN: i64 = 250;
/// Maximum CPU (millicores). 16000m matches the cluster-profile ceiling.
const CPU_MILLICORES_MAX: i64 = 16_000;

/// Minimum PVC size (GiB).
const STORAGE_SIZE_GI_MIN: i64 = 10;
/// Maximum PVC size (GiB). Generous ceiling; anything more is a misconfig.
const STORAGE_SIZE_GI_MAX: i64 = 500;

/// Maximum length of a `CurseForge` slug.
const SLUG_MAX_LEN: usize = 200;

/// Maximum length of a `force_version` string.
const FORCE_VERSION_MAX_LEN: usize = 128;

/// Maximum entries in a `version_skip` list.
const VERSION_SKIP_MAX_LEN: usize = 50;

/// Loaders accepted by the modded `RuntimePicker` and the catalog facets.
const KNOWN_RUNTIMES: &[&str] = &["fabric", "forge", "neoforge", "paper"];

/// Catalog providers the search/versions endpoints recognise.
const KNOWN_CATALOG_PROVIDERS: &[&str] = &["curseforge", "modrinth"];

/// Maximum length of a free-text catalog search query.
const SEARCH_QUERY_MAX_LEN: usize = 100;

/// Maximum length of a mod jar filename.
const MOD_FILENAME_MAX_LEN: usize = 200;

/// Minimum Mojang username length. The official rule is 3..=16 ASCII.
const MC_USERNAME_MIN: usize = 3;
/// Maximum Mojang username length.
const MC_USERNAME_MAX: usize = 16;
/// Maximum kick / ban reason length, bytes.
const REASON_MAX_LEN: usize = 100;
/// Maximum chat message / broadcast length, bytes.
const CHAT_MAX_LEN: usize = 256;
/// Allowed gamemode discriminators.
const KNOWN_GAMEMODES: &[&str] = &["survival", "creative", "adventure", "spectator"];

/// Validates a server name against RFC 1123 label rules.
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] with `code = "name_invalid"` when the
/// name fails any of: 1–63 chars, lowercase ASCII alphanumerics + `-`,
/// must start with a letter, must end with a letter or digit.
pub fn validate_name(name: &str) -> Result<(), AppError> {
    let len = name.len();
    if !(1..=63).contains(&len) {
        return Err(invalid_name("must be 1–63 characters"));
    }
    let bytes = name.as_bytes();
    let valid_char = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-';
    if !bytes.iter().all(|&b| valid_char(b)) {
        return Err(invalid_name(
            "only lowercase letters, digits, and '-' are allowed",
        ));
    }
    // Indexing is safe: we already verified `len >= 1`.
    let first = bytes[0];
    let last = bytes[len - 1];
    if !first.is_ascii_lowercase() {
        return Err(invalid_name("must start with a lowercase letter"));
    }
    if !(last.is_ascii_lowercase() || last.is_ascii_digit()) {
        return Err(invalid_name("must end with a letter or digit"));
    }
    Ok(())
}

fn invalid_name(reason: &str) -> AppError {
    AppError::BadRequest {
        code: "name_invalid",
        message: format!("name invalid: {reason}"),
    }
}

/// Validates the `memory_mi` field against the supported range and step.
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] with `code = "memory_invalid"` when
/// the value is out of `[1024, 16384]` or is not a multiple of 1024.
pub fn validate_memory_mi(mi: i64) -> Result<(), AppError> {
    if !(MEMORY_MI_MIN..=MEMORY_MI_MAX).contains(&mi) || mi % MEMORY_MI_STEP != 0 {
        return Err(AppError::BadRequest {
            code: "memory_invalid",
            message: format!(
                "memory_mi must be in [{MEMORY_MI_MIN}..={MEMORY_MI_MAX}] in {MEMORY_MI_STEP}-Mi steps"
            ),
        });
    }
    Ok(())
}

/// Validates the `cpu_millicores` field against the supported range.
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] with `code = "cpu_millicores_invalid"` when
/// the value is out of `[250, 16000]`.
pub fn validate_cpu_millicores(m: i64) -> Result<(), AppError> {
    if !(CPU_MILLICORES_MIN..=CPU_MILLICORES_MAX).contains(&m) {
        return Err(AppError::BadRequest {
            code: "cpu_millicores_invalid",
            message: format!(
                "cpu_millicores must be in [{CPU_MILLICORES_MIN}..={CPU_MILLICORES_MAX}]"
            ),
        });
    }
    Ok(())
}

/// Returns `true` when `version` is in the offline floor.
///
/// Used by [`validate_mc_version`] as the fallback path; broken out so it
/// is unit-testable without an [`AppState`].
#[must_use]
pub fn is_known_mc_version_offline(version: &str) -> bool {
    KNOWN_MC_VERSIONS.contains(&version)
}

/// Validates that `version` is currently advertised by `/cluster/mc-versions`.
///
/// Consults the in-memory Mojang manifest cache; falls back to the offline
/// floor [`KNOWN_MC_VERSIONS`] when the cache is empty or stale (the cache
/// is populated lazily when the endpoint is first hit).
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] with `code = "mc_version_unknown"` if
/// the version is not in the live list AND not in the offline floor.
pub async fn validate_mc_version(state: &AppState, version: &str) -> Result<(), AppError> {
    if let Some(cached) = crate::routes::mc_versions::cached(&state.mc_versions_cache).await
        && cached.iter().any(|v| v == version)
    {
        return Ok(());
    }
    if is_known_mc_version_offline(version) {
        return Ok(());
    }
    Err(AppError::BadRequest {
        code: "mc_version_unknown",
        message: format!("mc_version {version:?} is not a known release"),
    })
}

/// Validates the `storage_size_gi` field against the supported range.
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] with `code = "storage_size_invalid"` when
/// the value is out of `[10, 500]`.
pub fn validate_storage_size_gi(gi: i64) -> Result<(), AppError> {
    if !(STORAGE_SIZE_GI_MIN..=STORAGE_SIZE_GI_MAX).contains(&gi) {
        return Err(AppError::BadRequest {
            code: "storage_size_invalid",
            message: format!(
                "storage_size_gi must be in [{STORAGE_SIZE_GI_MIN}..={STORAGE_SIZE_GI_MAX}]"
            ),
        });
    }
    Ok(())
}

/// Validates a `CurseForge` slug — non-blank, ≤ 200 characters after trimming.
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] with `code = "slug_invalid"`.
pub fn validate_slug(s: &str) -> Result<(), AppError> {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed.len() > SLUG_MAX_LEN {
        return Err(AppError::BadRequest {
            code: "slug_invalid",
            message: format!("slug must be 1..={SLUG_MAX_LEN} non-blank characters"),
        });
    }
    Ok(())
}

/// Validates `force_version` — `[A-Za-z0-9._-]{1,128}`.
///
/// Accepts ASCII alphanumerics plus `.`, `_`, `-`; bounded to 128 bytes.
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] with `code = "force_version_invalid"`.
pub fn validate_force_version(v: &str) -> Result<(), AppError> {
    if v.is_empty() || v.len() > FORCE_VERSION_MAX_LEN {
        return Err(AppError::BadRequest {
            code: "force_version_invalid",
            message: format!("force_version must be 1..={FORCE_VERSION_MAX_LEN} characters"),
        });
    }
    if !v
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(AppError::BadRequest {
            code: "force_version_invalid",
            message: "force_version may only contain [A-Za-z0-9._-]".to_owned(),
        });
    }
    Ok(())
}

/// Validates `version_skip` — at most 50 entries.
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] with `code = "version_skip_invalid"`.
pub fn validate_version_skip(list: &[String]) -> Result<(), AppError> {
    if list.len() > VERSION_SKIP_MAX_LEN {
        return Err(AppError::BadRequest {
            code: "version_skip_invalid",
            message: format!("version_skip must have ≤ {VERSION_SKIP_MAX_LEN} entries"),
        });
    }
    Ok(())
}

/// Validates a runtime discriminator (`fabric` | `forge` | `neoforge` | `paper`).
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] with `code = "runtime_invalid"`.
pub fn validate_runtime(r: &str) -> Result<(), AppError> {
    if KNOWN_RUNTIMES.contains(&r) {
        Ok(())
    } else {
        Err(AppError::BadRequest {
            code: "runtime_invalid",
            message: format!("runtime {r:?} not in {KNOWN_RUNTIMES:?}"),
        })
    }
}

/// Validates a Modrinth project id (8-char base62) or slug (`[a-z0-9_-]{1,40}`).
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] with `code = "modrinth_id_invalid"`.
pub fn validate_modrinth_id_or_slug(s: &str) -> Result<(), AppError> {
    let len = s.len();
    if (1..=40).contains(&len) {
        let is_id = len == 8 && s.chars().all(|c| c.is_ascii_alphanumeric());
        let is_slug = s
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
        if is_id || is_slug {
            return Ok(());
        }
    }
    Err(AppError::BadRequest {
        code: "modrinth_id_invalid",
        message: format!("modrinth id/slug {s:?} invalid"),
    })
}

/// Validates a catalog free-text search query — non-blank, ≤ 100 chars.
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] with `code = "search_query_invalid"`.
pub fn validate_search_query(q: &str) -> Result<(), AppError> {
    let trimmed = q.trim();
    if trimmed.is_empty() || trimmed.len() > SEARCH_QUERY_MAX_LEN {
        return Err(AppError::BadRequest {
            code: "search_query_invalid",
            message: format!("query must be 1..={SEARCH_QUERY_MAX_LEN} chars"),
        });
    }
    Ok(())
}

/// Validates a catalog provider discriminator (`curseforge` | `modrinth`).
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] with `code = "catalog_provider_invalid"`.
pub fn validate_catalog_provider(p: &str) -> Result<(), AppError> {
    if KNOWN_CATALOG_PROVIDERS.contains(&p) {
        Ok(())
    } else {
        Err(AppError::BadRequest {
            code: "catalog_provider_invalid",
            message: format!("provider {p:?} not in {KNOWN_CATALOG_PROVIDERS:?}"),
        })
    }
}

/// Validates a mod jar filename. Defends the sync Job's `rm` from path
/// injection at the DB-write layer.
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] with `code = "mod_filename_invalid"`
/// when the name contains `/`, doesn't end `.jar`, exceeds 200 bytes, or
/// uses characters outside `[A-Za-z0-9._+-]`.
pub fn validate_mod_filename(name: &str) -> Result<(), AppError> {
    let len = name.len();
    let ends_jar = std::path::Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("jar"));
    if !(1..=MOD_FILENAME_MAX_LEN).contains(&len)
        || name.contains('/')
        || !ends_jar
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
    {
        return Err(AppError::BadRequest {
            code: "mod_filename_invalid",
            message: format!(
                "filename {name:?} must be a basename ending .jar with [A-Za-z0-9._+-]"
            ),
        });
    }
    Ok(())
}

/// Validates that `mode` is in [`KNOWN_EXPOSURE_MODES`].
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] with `code = "exposure_mode_invalid"`.
pub fn validate_exposure_mode(mode: &str) -> Result<(), AppError> {
    if KNOWN_EXPOSURE_MODES.contains(&mode) {
        return Ok(());
    }
    Err(AppError::BadRequest {
        code: "exposure_mode_invalid",
        message: format!("exposure_mode {mode:?} not in {KNOWN_EXPOSURE_MODES:?}"),
    })
}

/// Validates a Mojang username (3–16 ASCII chars from `[A-Za-z0-9_]`).
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] with `code = "username_invalid"`.
pub fn validate_mc_username(s: &str) -> Result<&str, AppError> {
    let len = s.len();
    if !(MC_USERNAME_MIN..=MC_USERNAME_MAX).contains(&len)
        || !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(AppError::BadRequest {
            code: "username_invalid",
            message: format!(
                "username must be {MC_USERNAME_MIN}..={MC_USERNAME_MAX} chars from [A-Za-z0-9_]"
            ),
        });
    }
    Ok(s)
}

/// Validates a kick / ban reason. Empty is allowed (caller may omit
/// the reason). Rejects any control char (0x00..0x1F or 0x7F).
///
/// # Errors
///
/// `AppError::BadRequest` with `code = "reason_too_long"` or
/// `code = "reason_has_control_char"`.
pub fn validate_kick_reason(s: &str) -> Result<&str, AppError> {
    if s.len() > REASON_MAX_LEN {
        return Err(AppError::BadRequest {
            code: "reason_too_long",
            message: format!("reason must be ≤ {REASON_MAX_LEN} chars"),
        });
    }
    if s.chars().any(|c| (c as u32) < 0x20 || c == '\u{7f}') {
        return Err(AppError::BadRequest {
            code: "reason_has_control_char",
            message: "reason must not contain control characters".to_owned(),
        });
    }
    Ok(s)
}

/// Validates a chat message / broadcast body. Same shape as
/// [`validate_kick_reason`] with the chat-length cap.
///
/// # Errors
///
/// `AppError::BadRequest` with `code = "message_too_long"` or
/// `code = "message_has_control_char"`.
pub fn validate_chat_message(s: &str) -> Result<&str, AppError> {
    if s.len() > CHAT_MAX_LEN {
        return Err(AppError::BadRequest {
            code: "message_too_long",
            message: format!("message must be ≤ {CHAT_MAX_LEN} chars"),
        });
    }
    if s.chars().any(|c| (c as u32) < 0x20 || c == '\u{7f}') {
        return Err(AppError::BadRequest {
            code: "message_has_control_char",
            message: "message must not contain control characters".to_owned(),
        });
    }
    Ok(s)
}

/// Validates a gamemode discriminator.
///
/// # Errors
///
/// `AppError::BadRequest` with `code = "gamemode_invalid"`.
pub fn validate_gamemode(s: &str) -> Result<&str, AppError> {
    if KNOWN_GAMEMODES.contains(&s) {
        Ok(s)
    } else {
        Err(AppError::BadRequest {
            code: "gamemode_invalid",
            message: format!("gamemode {s:?} not in {KNOWN_GAMEMODES:?}"),
        })
    }
}

/// Validates that `s` parses as either an IPv4 or IPv6 literal.
///
/// # Errors
///
/// `AppError::BadRequest` with `code = "ip_invalid"`.
pub fn validate_ip_v4_or_v6(s: &str) -> Result<&str, AppError> {
    if s.parse::<std::net::IpAddr>().is_ok() {
        Ok(s)
    } else {
        Err(AppError::BadRequest {
            code: "ip_invalid",
            message: format!("{s:?} is not a valid IPv4 or IPv6 address"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_names_pass() {
        let long = "a".repeat(63);
        for n in ["smp", "survival", "a", "a1", "my-server", long.as_str()] {
            assert!(validate_name(n).is_ok(), "expected {n:?} to pass");
        }
    }

    #[test]
    fn invalid_names_fail() {
        let too_long = "a".repeat(64);
        for n in [
            "",
            "SMP",
            "has space",
            "-start",
            "end-",
            "1leading",
            too_long.as_str(),
        ] {
            assert!(validate_name(n).is_err(), "expected {n:?} to fail");
        }
    }

    #[test]
    fn valid_memory_passes() {
        for mi in [1024_i64, 2048, 4096, 6144, 8192, 16384] {
            assert!(validate_memory_mi(mi).is_ok());
        }
    }

    #[test]
    fn invalid_memory_fails() {
        for mi in [0_i64, 512, 1023, 1025, 17_000, -1] {
            assert!(validate_memory_mi(mi).is_err());
        }
    }

    #[test]
    fn valid_cpu_passes() {
        for m in [250_i64, 500, 1000, 2000, 4000, 8000, 16_000] {
            assert!(validate_cpu_millicores(m).is_ok());
        }
    }

    #[test]
    fn invalid_cpu_fails() {
        for m in [0_i64, 100, 249, 16_001, -250, 32_000] {
            assert!(validate_cpu_millicores(m).is_err());
        }
    }

    #[test]
    fn offline_versions_pass() {
        assert!(is_known_mc_version_offline("1.21.4"));
        assert!(is_known_mc_version_offline("1.20.4"));
    }

    #[test]
    fn offline_unknown_fails() {
        assert!(!is_known_mc_version_offline("1.7.10"));
        assert!(!is_known_mc_version_offline("garbage"));
    }

    #[test]
    fn storage_size_bounds() {
        assert!(validate_storage_size_gi(0).is_err());
        assert!(validate_storage_size_gi(9).is_err());
        assert!(validate_storage_size_gi(10).is_ok());
        assert!(validate_storage_size_gi(500).is_ok());
        assert!(validate_storage_size_gi(501).is_err());
    }

    #[test]
    fn slug_length_cap() {
        assert!(validate_slug("ok").is_ok());
        assert!(validate_slug("").is_err());
        assert!(validate_slug("   ").is_err());
        assert!(validate_slug(&"a".repeat(200)).is_ok());
        assert!(validate_slug(&"a".repeat(201)).is_err());
    }

    #[test]
    fn force_version_format() {
        assert!(validate_force_version("1.21.4").is_ok());
        assert!(validate_force_version("ATM-11_v3.2-final").is_ok());
        assert!(validate_force_version("").is_err());
        assert!(validate_force_version("bad version!").is_err());
        assert!(validate_force_version("contains space").is_err());
        assert!(validate_force_version(&"a".repeat(129)).is_err());
    }

    #[test]
    fn version_skip_cap() {
        let ok: Vec<String> = (0..50).map(|i| format!("v{i}")).collect();
        assert!(validate_version_skip(&ok).is_ok());
        let too_many: Vec<String> = (0..51).map(|i| format!("v{i}")).collect();
        assert!(validate_version_skip(&too_many).is_err());
    }

    #[test]
    fn known_exposure_modes_pass() {
        for m in KNOWN_EXPOSURE_MODES {
            assert!(validate_exposure_mode(m).is_ok());
        }
    }

    #[test]
    fn unknown_exposure_mode_fails() {
        assert!(validate_exposure_mode("LoadBalancer").is_err()); // case-sensitive
        assert!(validate_exposure_mode("hostport").is_err());
    }

    #[test]
    fn runtime_validator() {
        for r in KNOWN_RUNTIMES {
            assert!(validate_runtime(r).is_ok());
        }
        for r in ["", "vanilla", "FABRIC", "spongeforge"] {
            assert!(validate_runtime(r).is_err());
        }
    }

    #[test]
    fn modrinth_id_or_slug_validator() {
        assert!(validate_modrinth_id_or_slug("AANobbMI").is_ok());
        assert!(validate_modrinth_id_or_slug("sodium").is_ok());
        assert!(validate_modrinth_id_or_slug("more-than-eight-but-slug").is_ok());
        assert!(validate_modrinth_id_or_slug("").is_err());
        assert!(validate_modrinth_id_or_slug("UPPER").is_err());
        assert!(validate_modrinth_id_or_slug("space slug").is_err());
        assert!(validate_modrinth_id_or_slug(&"a".repeat(41)).is_err());
    }

    #[test]
    fn search_query_validator() {
        assert!(validate_search_query("sodium").is_ok());
        assert!(validate_search_query("").is_err());
        assert!(validate_search_query("    ").is_err());
        assert!(validate_search_query(&"a".repeat(101)).is_err());
    }

    #[test]
    fn catalog_provider_validator() {
        assert!(validate_catalog_provider("curseforge").is_ok());
        assert!(validate_catalog_provider("modrinth").is_ok());
        assert!(validate_catalog_provider("vanilla").is_err());
    }

    #[test]
    fn mod_filename_validator_accepts_realistic_names() {
        assert!(validate_mod_filename("sodium-fabric-0.5.13+mc1.21.1.jar").is_ok());
        assert!(validate_mod_filename("lithium-1.21.1.jar").is_ok());
    }

    #[test]
    fn mod_filename_validator_rejects_path_traversal() {
        assert!(validate_mod_filename("../etc/passwd").is_err());
        assert!(validate_mod_filename("a/b.jar").is_err());
        assert!(validate_mod_filename("sodium.zip").is_err());
        assert!(validate_mod_filename("").is_err());
        assert!(validate_mod_filename(&format!("{}.jar", "a".repeat(200))).is_err());
    }

    #[test]
    fn mc_username_accepts_real_examples() {
        for n in ["alice", "Bob_42", "x_y_z", "AAA", "abcdefghijklmnop"] {
            assert!(validate_mc_username(n).is_ok(), "expected {n:?} to pass");
        }
    }

    #[test]
    fn mc_username_rejects_bad_examples() {
        let too_long = "a".repeat(17);
        for n in [
            "",
            "ab",
            "has space",
            "has-dash",
            too_long.as_str(),
            "tab\there",
        ] {
            assert!(validate_mc_username(n).is_err(), "expected {n:?} to fail");
        }
    }

    #[test]
    fn kick_reason_bounds_and_chars() {
        assert!(validate_kick_reason("").is_ok());
        assert!(validate_kick_reason("legit reason").is_ok());
        assert!(validate_kick_reason(&"r".repeat(100)).is_ok());
        assert!(validate_kick_reason(&"r".repeat(101)).is_err());
        assert!(validate_kick_reason("with\nnewline").is_err());
        assert!(validate_kick_reason("with\rcarriage").is_err());
        assert!(validate_kick_reason("with\ttab").is_err());
    }

    #[test]
    fn chat_message_bounds_and_chars() {
        assert!(validate_chat_message("hi friends").is_ok());
        assert!(validate_chat_message(&"x".repeat(256)).is_ok());
        assert!(validate_chat_message(&"x".repeat(257)).is_err());
        assert!(validate_chat_message("with\nnewline").is_err());
    }

    #[test]
    fn gamemode_validator() {
        for m in ["survival", "creative", "adventure", "spectator"] {
            assert!(validate_gamemode(m).is_ok());
        }
        for m in ["", "Survival", "creative ", "spec", "0"] {
            assert!(validate_gamemode(m).is_err(), "expected {m:?} to fail");
        }
    }

    #[test]
    fn ip_validator() {
        for ip in ["10.0.0.5", "127.0.0.1", "::1", "2001:db8::1"] {
            assert!(validate_ip_v4_or_v6(ip).is_ok());
        }
        for ip in ["", "not.an.ip", "999.999.999.999", "10.0.0.0/24"] {
            assert!(validate_ip_v4_or_v6(ip).is_err());
        }
    }
}
