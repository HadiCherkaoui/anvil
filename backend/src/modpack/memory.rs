// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! JVM memory + GC env shared by every itzg-based provider.
//!
//! itzg's image reads `INIT_MEMORY` and `MAX_MEMORY` to set `-Xms` / `-Xmx`.
//! Sharing the helper keeps the math + GC flags in one place — both at boot
//! (provider `extra_env`) and at runtime (settings PATCH that re-patches the
//! `StatefulSet` env, see `routes/servers/settings.rs`).

use k8s_openapi::api::core::v1::EnvVar;

use super::vanilla::env_kv;

/// Initial JVM heap size in MiB given a max budget.
///
/// itzg's image sets `-Xms` from `INIT_MEMORY` and `-Xmx` from `MAX_MEMORY`;
/// when `INIT_MEMORY` matches `MAX_MEMORY` the JVM commits the full heap up
/// front and never returns pages to the OS, leaving idle pods sitting at the
/// configured ceiling. A quarter-of-max start (floor 1 GiB) lets the heap
/// commit lazily as mods load — paired with [`IDLE_GC_OPTS`] so the heap
/// also shrinks back during long idles.
#[must_use]
pub fn init_memory_mi(max_mi: i64) -> i64 {
    // Clamp lower bound to 512 MiB but never exceed the max budget; small
    // pods (e.g. memory_mi == 256) would otherwise get init > max and refuse
    // to start.
    (max_mi / 4).max(512).min(max_mi)
}

/// JVM `-XX:` flags that let G1 release committed heap to the OS during
/// long idles. Without these, G1 only grows the heap toward `-Xmx`; with
/// them, every 30s of idle the JVM runs a concurrent collection that can
/// return unused regions to the OS, so an idle pod's RSS tracks live-set
/// rather than peak heap. JEP 346 — supported on Java 12+.
pub const IDLE_GC_OPTS: &str = "-XX:+G1PeriodicGCInvokesConcurrent -XX:G1PeriodicGCInterval=30000";

/// Builds the `INIT_MEMORY` / `MAX_MEMORY` / `JVM_XX_OPTS` env triple every
/// itzg-based provider stamps onto its `mc` container.
#[must_use]
pub fn build_memory_env(memory_mi: i64) -> Vec<EnvVar> {
    vec![
        env_kv("INIT_MEMORY", &format!("{}M", init_memory_mi(memory_mi))),
        env_kv("MAX_MEMORY", &format!("{memory_mi}M")),
        env_kv("JVM_XX_OPTS", IDLE_GC_OPTS),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_memory_env_4096() {
        let env = build_memory_env(4096);
        let init = env.iter().find(|e| e.name == "INIT_MEMORY").unwrap();
        let max = env.iter().find(|e| e.name == "MAX_MEMORY").unwrap();
        let gc = env.iter().find(|e| e.name == "JVM_XX_OPTS").unwrap();
        assert_eq!(init.value.as_deref(), Some("1024M")); // 4096/4 == 1024
        assert_eq!(max.value.as_deref(), Some("4096M"));
        assert!(gc.value.is_some());
    }

    #[test]
    fn init_memory_floor_at_512() {
        // 1024/4 == 256, floor to 512
        assert_eq!(init_memory_mi(1024), 512);
    }

    #[test]
    fn init_memory_scales_above_4_gib() {
        let env = build_memory_env(8192); // 8192/4 == 2048
        let init = env.iter().find(|e| e.name == "INIT_MEMORY").unwrap();
        assert_eq!(init.value.as_deref(), Some("2048M"));
    }

    #[test]
    fn init_memory_clamped_to_max_for_tiny_budgets() {
        // Floor (512) would otherwise exceed max (256), preventing JVM start.
        assert_eq!(init_memory_mi(256), 256);
        assert_eq!(init_memory_mi(400), 400);
        assert_eq!(init_memory_mi(512), 512);
    }
}
