# `hem-target-core`

This is the shared implementation boundary for the pinned FHS target runner.
It holds target preparation, FHS execution glue and an in-memory output writer
so the existing runner and thin target wrapper use the same behavior. The owner
approved `AGPL-3.0-only` supplemented by Vulcan Origin Terms v1.0 for this
extracted source on 2026-10-06. This does not itself authorize publication.

## Dependency boundary

- Geometry CSV conversion remains implemented by the canonical
  `community/crates/vulcan-model-transform` crate. This crate calls its
  `transform_geometry_csv` API and does not copy or reimplement the converter.
- Execution calls the pinned HEM and FHS upstream crates at the revisions
  recorded in `community/PATH_RIGHTS.md`. It does not depend on the legacy
  `hem-batch-core` crate.
- This crate has no `hem_pv_micro` dependency. The FHS wrapper boundary does
  not include the legacy batch orchestration or SAP calculator/converter code.

`target_scenario.rs` contains the narrow scenario operations already used by
the target preparation seam: supported snippet merges, compliance-field repair,
cooling assignment, event expansion, schema finalization and structural-change
checks. Its merge helper retains the existing supported category behavior. The
old `sap_xml` and `additional_outputs` no-op match arms were omitted because
both fell through to the existing no-op default; no SAP handling was moved.
`hem-batch-core` re-exports the moved functions so existing call sites retain
one implementation.

## Source and licence provenance

`Cargo.toml` declares `AGPL-3.0-only`. In this source checkout, the exact GNU
AGPL version 3 text and Vulcan Origin Terms v1.0 are under
`scripts/hem-targets/licensing/`. The runner exporter copies them to the source
distribution root as `LICENSE` and `ADDITIONAL_TERMS.md`, alongside `NOTICE.md`,
`ATTRIBUTION.md`, and `TRADEMARKS.md`. First-party AGPL coverage comes from the
owner's explicit 2026-10-06 decision, independently of the Community transform
dependency.

The Community transform dependency retains its own `AGPL-3.0-only` and Vulcan
Origin Terms notices and existing Community path approval. Any distribution
that includes that covered material must carry the required Vulcan Community,
Home Energy Foundry, origin-page, copyright/licence, corresponding-source and
non-affiliation notices in `community/ADDITIONAL_TERMS.md` and
`community/ATTRIBUTION.md`. The pinned HEM/FHS upstream MIT authority is the
settled decision in `community/PATH_RIGHTS.md`: HEM
`62d3df705690f33b3fc3e905c9971d4f3743bf2e`; FHS
`c5ba2673fbd886cfe4fb528f61b376bdf406ebbd`; canonical FHS MIT evidence revision
`dd5ba73a19674d631da59b4924bb7dc2833fbb3b`. Preserve the corresponding upstream
licence notices with any source or binary distribution.

The machine-readable relocation record is
[`relocation-provenance.json`](relocation-provenance.json). It records the root
revision, SHA-256 of each relocated file before the separately approved licence
headers were added, post-approval hashes of the moved source files, and exact
pre-move source-file hashes from the retained `tmp/hem-release-final-20261006/final-checkpoint/`
archive (or `git show` for unchanged files). The Community checkout was dirty;
its observed HEAD is therefore not a complete transform source snapshot. Capture
and hash the exact Community source/export tree before qualifying or releasing a
runner.
