//! Pure input validation for the create handler.
//!
//! Functions here perform no I/O and do not allocate beyond the `String`
//! that error messages require. They run before any kube or DB call so
//! invalid requests short-circuit with a 400 response.

use crate::error::AppError;

/// Minecraft versions Anvil offers in the UI.
///
/// Hardcoded per CLAUDE.md anti-overengineering: there are only ~10 we
/// care about and a discovery service is out of scope. Bumping this
/// list is a one-line code change in M3.
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

/// Validates that `version` is in [`KNOWN_MC_VERSIONS`].
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] with `code = "mc_version_unknown"` if
/// the version is not in the allow-list.
pub fn validate_mc_version(version: &str) -> Result<(), AppError> {
    if KNOWN_MC_VERSIONS.contains(&version) {
        return Ok(());
    }
    Err(AppError::BadRequest {
        code: "mc_version_unknown",
        message: format!("mc_version {version:?} not in {:?}", KNOWN_MC_VERSIONS),
    })
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
        message: format!("exposure_mode {mode:?} not in {:?}", KNOWN_EXPOSURE_MODES),
    })
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
    fn known_versions_pass() {
        assert!(validate_mc_version("1.21.4").is_ok());
        assert!(validate_mc_version("1.20.4").is_ok());
    }

    #[test]
    fn unknown_version_fails() {
        assert!(validate_mc_version("1.7.10").is_err());
        assert!(validate_mc_version("garbage").is_err());
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
}
