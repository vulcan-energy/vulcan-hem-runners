# Vulcan HEM FHS runners

Corresponding Source: [source tree at hem-fhs-preparation-2026-10-08-2](https://github.com/vulcan-energy/vulcan-hem-runners/tree/hem-fhs-preparation-2026-10-08-2). The complete source and build-material archives are linked from the [matching GitHub release](https://github.com/vulcan-energy/vulcan-hem-runners/releases/tag/hem-fhs-preparation-2026-10-08-2).

This repository contains the narrow FHS target runner source closure for that release.

## Scope

The release contains the shared `hem-target-core` and thin `hem_target_wrapper`, the `hem-target-runtime` TypeScript host and workers that prepare models and drive the runners in a browser or CLI, pristine pinned HEM and FHS upstream trees, the Community `vulcan-model-transform` and `vulcan-csv-codec` crates with their schema/default inputs and notices, and the offline-patched `jsonschema` dependency. It excludes the private Vulcan SAP/PV integration in `hem-batch-core`, `wasm_wrapper` Rust sources, and generic Free editor code; `hem-target-runtime` imports the Community geometry-editor packages and the `@repo/core` scenario helpers by name, and those are not part of this closure. The pinned upstream model sources remain included as runner dependencies.

The newly extracted first-party core, wrapper, TypeScript runtime and runner build helpers are licensed under AGPL-3.0-only, supplemented by Vulcan Origin Terms v1.0. The owner approved this scope on 2026-10-06. See `LICENSE`, `ADDITIONAL_TERMS.md`, `NOTICE.md`, `ATTRIBUTION.md` `TRADEMARKS.md`, and `FHS-MIT-LICENSE.md`. The unchanged proprietary root licence from the source repository is not included. The first-party licence grant is separate from the Community transform's own AGPL-3.0-only and Origin Terms.

The Community, upstream and vendored notices remain with their respective source trees. The approved FHS MIT authority remains the pinned Community decision for `c5ba2673fbd886cfe4fb528f61b376bdf406ebbd`, with canonical evidence revision `dd5ba73a19674d631da59b4924bb7dc2833fbb3b`.

## Source identity and build

`runner-source.json` binds the originating source commit to the exact exported file inventory hash; it records each exported file SHA-256 and pinned upstream revisions. The export hash is the build source-tree identity; the original root Git tree and prior candidate identity are retained as origin metadata. Verify the exact inventory before and after a build:

`node scripts/hem-targets/verify-runner-source-manifest.mjs --manifest runner-source.json --root . --phase before-build`

Use `scripts/build-fhs-target-wasm.sh /path/outside/this/repository` to reproduce the standalone FHS WASM build with the same target-feature flags and wasm-pack build-std settings as the source checkout's `fhs-target` lane. Cargo uses the neighboring `rust-hem-runner/.cargo-target` cache; generated package output and temporary state stay outside this repository. The helper uses the runner-scoped Rayon bridge and applies `scripts/hem-targets/licensing/runtime-header.js` to generated glue. It emits a local raw artifact; the root build script's hash injection and final artifact decoration are omitted. A successful raw build is not a qualification, registration, or release decision.

The exporter and verifier live under `scripts/hem-targets/` in the source checkout. Exporting writes local files only; it does not create commits, register artifacts, or publish them.
