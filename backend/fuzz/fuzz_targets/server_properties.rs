// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Fuzz the user-editable `server.properties` JSON input path.
//!
//! Minecraft server properties are edited through Anvil's Properties tab and
//! persisted as JSON, then decoded with `serde` and run through
//! [`ServerProperties::validate`] and [`ServerProperties::to_env`] on every
//! save and pod start. Those functions process untrusted input, so a panic in
//! any of them is a denial-of-service vector. This target drives arbitrary
//! bytes through that exact chain to surface panics, overflows, or aborts.
//!
//! Run locally with a nightly toolchain:
//!
//! ```sh
//! cd backend && cargo +nightly fuzz run server_properties
//! ```
#![no_main]

use anvil::server_properties::ServerProperties;
use libfuzzer_sys::fuzz_target;

// The deserialize step rejects most inputs; only well-formed JSON that maps
// onto `ServerProperties` reaches `validate`/`to_env`, which is exactly the
// surface worth exercising. Results are intentionally discarded — the target
// asserts nothing beyond "this input must never panic or abort".
fuzz_target!(|data: &[u8]| {
    if let Ok(properties) = serde_json::from_slice::<ServerProperties>(data) {
        let _ = properties.validate();
        let _ = properties.to_env();
    }
});
