# stage01 follow-up — Non-finite raster validation
Date: 2026-11-09 · Issue: #50 · Verification reached: V2 (local Windows); V3 CI pending

**Context:** release audit identified silent loss of non-finite raster numeric metadata. The owner requested the fix; this is an ad-hoc follow-up with core, catalog, indexer and documentation scope.

## What changed
- Core rejects present non-finite raster resolutions and any non-finite affine component with a field-specific `InvalidRecord`; dataset validation adds the layer ordinal.
- Catalog no longer silently filters numeric raster metadata. Invalid replacements fail before writes and leave the surrounding transaction usable.
- Shared extraction validates enriched layers before the catalog batch, reporting a non-retryable dataset error with no layers when invalid facts are found.
- Core tests cover NaN and both infinities in every numeric slot, finite extremes, missing values and text nodata. Catalog regression covers rejection, unchanged records/stats, continued batch writes and `"nan"` nodata round-trip.
- A temporary 1x1 TIFF with finite affine coefficients/corners produces an overflowing resolution; pipeline regression checks its error status, successful sibling datasets and restart persistence.

## Decisions proposed
- D-51 supersedes D-41's store-as-absent rule; decision/discussion and closure/handoff references are synchronized.

## Deviations / contract changes
- **Contract change:** `LayerRecord::validate` / `DatasetRecord::validate` now reject non-finite raster floats. Dependants updated together: core, catalog and indexer. No new positivity/dimension/invertibility rule; no schema migration or dependency change. Nodata stays verbatim text, including `"nan"`.
- Query execution is unchanged. Previously discarded values stay absent until re-extraction.

## Follow-ups
- Require review, green Windows/Linux final-head CI and integration into develop before closing #50. Release/main/tag and stage milestone closure remain separate.

## Verification
- Local Windows: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` passed. The workspace run passed 442 libtest/doctests, plus the standalone strict corpus agreement gate; 25 tests remain ignored under their existing prerequisites.
- End-to-end `overflowing_raster_resolution_does_not_abort_the_indexing_batch` passed, including both sibling datasets and restart persistence. Core/catalog regressions pass for all eight numeric slots and all three non-finite values.
- Committed corpus oracle passed against the existing baseline; no fixture, golden, threshold or reader implementation was changed.
- Not verified locally: Linux, elevated NTFS/VHD acceptance and dependency-policy checks. These remain final-head CI requirements. No new owner-volume scans are necessary for this ingestion-only invariant change; no V4 claim is made.
