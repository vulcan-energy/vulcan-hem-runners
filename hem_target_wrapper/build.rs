// SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors
// SPDX-License-Identifier: AGPL-3.0-only
// This file is subject to Vulcan Origin Terms v1.0; see scripts/hem-targets/licensing/ADDITIONAL_TERMS.md.

fn main() {
    for name in [
        "VULCAN_WASM_BUILD_ID",
        "VULCAN_SOURCE_GIT_COMMIT",
        "VULCAN_SOURCE_TREE_SHA256",
        "VULCAN_SOURCE_TREE_DIRTY",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
        if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32") {
            assert!(
                std::env::var(name).is_ok_and(|value| !value.is_empty()),
                "Build this WASM through scripts/deploy/build-wasm.sh fhs-target: missing {name}"
            );
        }
    }
}
