# Corpus-consumer test updates — applied at wave 4 review

The orchestrator authorized these scope extensions on 2026-11-09; they are implemented in
PR #67, with no removed read-budget assertions, rate reductions or case-specific oracle skips.

- `crates/readers/tests/shapefile_corpus.rs` now handles a DBF anchor when no SHP exists, records
  exact Partial outcomes for SHX/DBF count disagreement and the documented CP437 fallback,
  asserts MultiPatch declared-layer semantics and lone-DBF non-spatial metadata, and retains
  the <64 KiB budget for every fixture. SBN/SBX roles are resolved explicitly.
- `crates/crs/tests/g2.rs` adds the exact expected text-only skip for binary custom GeoKeys and
  one missing-CRS entry (lone DBF; the genuine upstream SBN county sample supplies its PRJ). The full
  extraction oracle still exercises GeoKeys; no CRS rate or disagreement assertion is weakened.
- A separately authorized production fix changes MultiPatch's claimed PolyhedralSurface Z to
  existing `Geometry(false,false)`, meaning unknown declared layer geometry. Individual patches
  can contain TINs, collections and polygons. No geometry content is scanned; false Z/M flags
  describe the unknown declared layer, not the dimensions of the patches. The generic oracle
  alias `Geometry` -> GDAL `Unknown` applies to all formats and preserves specific types.

Source: [GDAL RFC 64](https://gdal.org/en/stable/development/rfc/rfc64_triangle_polyhedralsurface_tin.html).
The frozen enums/signatures/storage strings are unchanged. Core edits clarify rustdoc only.
D-49 is proposed in the session note for the orchestrator to record.
