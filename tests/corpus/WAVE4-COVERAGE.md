# Wave 4 corpus coverage

The 15 synthetic cases in `scripts/wave4_cases.py` are built by `generate.py` inside the unchanged,
digest-pinned GDAL 3.13.3 / PROJ 9.8.1 image. One additional upstream SBN fixture is pinned by
revision and SHA256 in the manifest, with its licence retained under `licences/`. They add:

| Follow-up | Committed coverage | Remaining limitation |
|---|---|---|
| #28 | LDID 0 without CPG, GDAL-created QIX, upstream GDAL SBN/SBX, external TIFF OVR and MSK | SBN/SBX are grouped members, not native spatial-index inputs; the fixture checks metadata and grouping. |
| #43 | PointZ without M, CP437 attribute names, DBF/SHX count mismatch, lone DBF, MultiPatch | CP437 retains the explicit D-42 fallback disagreement below; no field-name agreement is claimed. |
| #44 | Cassini EPSG:3068, Krovak EPSG:5514, NZMG EPSG:27200 | Requested 2–3 families supplied. Hotine and world projections remain covered by CRS fixtures, not new corpus datasets. |
| #61 | WKT2-preference GPKG, optional Z/M GPKG, 2d-gridded coverage, custom LCC GeoKeys, signed raster placement | Active WAL and read-only directory are runtime state, not portable committed files; existing `crates/readers/tests/geopackage.rs` covers them with temporary databases and Linux permissions. Signed-scale native byte tests also remain in `readers/src/tiff.rs`; GDAL may serialize a flipped affine as a transformation matrix. |

The external mask case deliberately omits mask overviews after generation: image overviews
remain in `.tif.ovr`, and the full-size `.tif.msk` is a supplied dataset member. No file
listing is learned from native output. Case membership is declared by the manifest, and the
separate grouper test verifies TIFF sidecar roles.

The SBN/SBX fixture comes from OSGeo/GDAL `autotest/ogr/data/shp/gjpoint.*`, revision
`15c79ff06cbc32a6631bdf303b956aa7210c7ad3`; `licences/GDAL-LICENSE.TXT` retains its upstream
MIT-style licence. It is third-party regression data, not a newly generated Esri index.
The lone DBF is an attribute-only dataset with two records and no invented spatial metadata.

MultiPatch reports the declared layer as unconstrained `Geometry`, normalized to GDAL's
`Unknown` in the generic oracle adapter. Its false dimensional flags describe the unknown
declared layer type; actual patches can contain Z and their coordinates are not scanned.
[GDAL RFC64](https://gdal.org/en/stable/development/rfc/rfc64_triangle_polyhedralsurface_tin.html)
specifies this unknown layer type because individual features may have different surface types.
The fixture still checks exact count, bounds and CRS, with the strict native 1.0 baseline.

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

CP437 field names are an explicit known disagreement under D-42/#42: GDAL decodes `café`,
native reports `caf‚` with a Partial fallback diagnostic. A focused comparison asserts both
values and the diagnostic; generic native/CRS rates do not claim field-name agreement.
