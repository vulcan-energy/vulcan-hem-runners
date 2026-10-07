<!-- SPDX-FileCopyrightText: 2026 Home Energy Foundry Limited and contributors -->
<!-- SPDX-License-Identifier: AGPL-3.0-only -->
<!-- Subject to Vulcan Origin Terms v1.0; see ADDITIONAL_TERMS.md. -->

# Vulcan HEM FHS runners: notices

Copyright © 2026 Home Energy Foundry Limited and contributors.

The newly extracted `hem-target-core`, `hem_target_wrapper`, and runner build
helpers are licensed under the GNU Affero General Public License version 3 only
(`AGPL-3.0-only`), supplemented by `ADDITIONAL_TERMS.md` (Vulcan Origin Terms
version 1.0). Their source files identify this scope with SPDX headers and an
explicit Origin Terms reference. The owner approved these terms on 2026-10-06.

This modified, unofficial FHS runner is not an official Home Energy Foundry
Limited build. Modification date: 2026-10-07. The Corresponding Source for
this release is identified by the immutable source tag
[`hem-fhs-mvp-2026-10-07`](https://github.com/vulcan-energy/vulcan-hem-runners/tree/hem-fhs-mvp-2026-10-07).
The complete source and build-material archives are linked from the matching
[GitHub release](https://github.com/vulcan-energy/vulcan-hem-runners/releases/tag/hem-fhs-mvp-2026-10-07).

## Included third-party material

- **Vulcan Community `vulcan-model-transform`** is AGPL-3.0-only with the
  Community's Vulcan Origin Terms. Its required attribution is in
  `ATTRIBUTION.md`; the exact additional terms and trademark notice are in
  `ADDITIONAL_TERMS.md` and `TRADEMARKS.md`.
- **Vulcan Community `vulcan-csv-codec`** is Apache-2.0. Preserve its source
  header and the Apache notice in the Community source tree.
- **Home Energy Model** is MIT at
  `62d3df705690f33b3fc3e905c9971d4f3743bf2e`.
- **Home Energy Model Future Homes Standard wrapper** is MIT at
  `c5ba2673fbd886cfe4fb528f61b376bdf406ebbd`; the canonical licence evidence
  revision is `dd5ba73a19674d631da59b4924bb7dc2833fbb3b`. The pinned upstream
  tree has no licence notice file, so its exact original MIT permission text is
  preserved in the source at
  `scripts/hem-targets/licensing/third-party/FHS-MIT.md` (SHA-256
  `af4b205d8259dd875442e4473dbd0d8ef638315d5dbb00a3d7d5fb1489f13cae`). The
  exporter materializes the same bytes as `FHS-MIT-LICENSE.md` at the runner
  repository root.
- **Vendored `jsonschema` 0.46.5** and Cargo registry dependencies retain the
  licence terms and notices supplied by their respective source packages.
- **Community schema/default assets** retain their existing source and notice
  context under `community/`.

This runner-specific licence grant does not change the licence of upstream,
Community, vendored, or registry components. Their own notices remain in force.
