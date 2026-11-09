# Wave 4 corpus coverage

The 13 cases in `scripts/wave4_cases.py` are built by `generate.py` inside the unchanged,
digest-pinned GDAL 3.13.3 / PROJ 9.8.1 image. They add:

| Follow-up | Committed coverage | Remaining limitation |
|---|---|---|
| #28 | LDID 0 without CPG, GDAL-created QIX, external TIFF OVR and MSK | SBN remains unverified: the pinned GDAL writer creates QIX, not an Esri SBN index. An explicitly licensed real SBN fixture is still needed. |
| #43 | PointZ without M, CP437 attribute names, DBF/SHX count mismatch | Lone DBF and MultiPatch remain GDAL-corpus gaps. Existing native tests cover them, but the current SHP corpus resolver assumes an SHP anchor; adding DBF requires a coordinated resolver/schema extension. MultiPatch needs explicit geometry-semantics review against GDAL before adding it to strict native rates. |
| #44 | Cassini EPSG:3068, Krovak EPSG:5514, NZMG EPSG:27200 | Requested 2–3 families supplied. Hotine and world projections remain covered by CRS fixtures, not new corpus datasets. |
| #61 | WKT2-preference GPKG, optional Z/M GPKG, 2d-gridded coverage, custom LCC GeoKeys, signed raster placement | Active WAL and read-only directory are runtime state, not portable committed files; existing `crates/readers/tests/geopackage.rs` covers them with temporary databases and Linux permissions. Signed-scale native byte tests also remain in `readers/src/tiff.rs`; GDAL may serialize a flipped affine as a transformation matrix. |

The external mask case deliberately omits mask overviews after generation: image overviews
remain in `.tif.ovr`, and the full-size `.tif.msk` is a supplied dataset member. No file
listing is learned from native output. Case membership is declared by the manifest, and the
separate grouper test verifies TIFF sidecar roles.

Corrupt count disagreement is tagged corrupt because the two physical record counts disagree;
the oracle compares status only per D-33. No ordinary failing case is excluded or retagged.
EPSG/WGS84 use the same `indexer::extract` call as indexing. The agreement gate rejects retryable
I/O failures even for corrupt cases and retains the full extraction error/partial diagnostic.
This distinguishes transient I/O/missing-member errors using the frozen extractor's `retry`
field; the extractor does not expose a finer non-retryable error enum.

The stricter 100% native baselines remain in force. CRS defaults enforce D-34's 98% minimum,
and measured per-format overrides lock in the current 100%. Coverage checks reject missing
committed data, missing adapters and omitted native-extent/EPSG/WGS84 checks. Only the unfetched
optional Natural Earth external sample may be skipped.
