# Test corpus + GDAL oracle records

GIS datasets for the Whereabouts readers, plus what GDAL says about each one (D-08, D-25).
GDAL only ever runs **inside a pinned container** to produce these files; it is never a Rust
dependency (D-07). Normal CI needs no GDAL: `cargo test -p whereabouts-oracle` reads the
committed records.

```
tests/corpus/
  manifest.toml                 one [[case]] per dataset (id, format, files, tags, provenance, licence)
  data/<format>/<id>/…          committed data (≤ 10 MB total; byte-exact, see .gitattributes)
  external/<format>/<id>/…      large real-world samples, fetched on demand (gitignored)
  expected/<id>.json            GDAL oracle record per case (committed, including external cases)
  scripts/
    generate.sh                 runs generate.py + golden.py in the pinned GDAL image (Docker)
    generate.py                 builds every origin = "synthetic" case
    golden.py                   writes expected/<id>.json
    fetch-external.sh / .ps1    downloads + sha256-verifies external samples
    external-files.txt          sha256, destination, immutable URL per external file
```

## Regenerating

```sh
tests/corpus/scripts/generate.sh                 # everything (needs Docker, Linux containers)
tests/corpus/scripts/generate.sh <case-id>...    # just these cases (+ their goldens)
tests/corpus/scripts/generate.sh --goldens-only  # keep data, rewrite goldens
tests/corpus/scripts/fetch-external.sh           # optional: external samples first (or .ps1)
```

No Docker? Run the **Oracle goldens** workflow (`.github/workflows/oracle-golden.yml`,
`workflow_dispatch`) on your branch and commit the `oracle-corpus` artifact. The same workflow
runs on every PR that touches `scripts/` or the manifest and fails if the regenerated output
differs from what is committed — the corpus is reproducible byte for byte.

Determinism: one image pinned by digest, one platform (`linux/amd64`), no wall-clock time (DBF
dates and GeoPackage `last_change` pinned), closed-form coordinates and pixels, goldens with
stable key order and floats rounded to 9 decimals. Bumping GDAL = edit `GDAL_IMAGE` in
`generate.sh`, regenerate, review the diff like code.

## Adding a case

1. Add a `[[case]]` to `manifest.toml` (`origin = "synthetic"`, tags from the vocabulary below).
2. Add a `@builder("<id>")` function to `generate.py`.
3. `generate.sh <id>` — it fails if the files produced differ from the manifest's `files`.
4. `cargo test -p whereabouts-oracle` (integrity tests check manifest ↔ data ↔ goldens).

Real-world data: only with a redistributable licence and an immutable source URL. Small samples
are committed (`origin = "real-world"`, `provenance` = URL at a pinned revision, optional
`[case.sha256]`); large ones get `location = "external"` and lines in `external-files.txt`.

Adding a **format** (Tier 1 beyond SHP/GPKG/GeoTIFF): add its name to `KNOWN_FORMATS`
(`tests/oracle/src/manifest.rs`), a reader function in `golden.py` (`READERS`), builders, cases.

## Tag vocabulary

| Tag | Meaning |
|---|---|
| `crs:utm`, `crs:transverse-mercator`, `crs:nztm`, `crs:lcc`, `crs:albers`, `crs:webmercator`, `crs:polar-stereo`, `crs:geographic` | projection family |
| `crs:esri-wkt`, `crs:no-authority`, `crs:custom-name`, `crs:custom`, `crs:us-feet` | CRS definition quirks |
| `crs:none` | no usable CRS |
| `crs:antimeridian`, `crs:contains-pole` | WGS84 extent edge cases |
| `enc:cpg`, `enc:ldid`, `enc:utf8`, `enc:cp1251`, `enc:cp936` | DBF encoding signal / encoding |
| `geom:point`, `geom:line`, `geom:polygon`, `geom:multipolygon`, `geom:z`, `geom:m` | geometry |
| `empty`, `multi-layer` | layer shape |
| `gpkg:stale-bounds`, `gpkg:missing-bounds`, `gpkg:attributes`, `gpkg:tiles` | GeoPackage |
| `raster`, `tiff:north-up`, `tiff:rotated`, `tiff:pixel-is-point`, `tiff:bigtiff`, `tiff:cog`, `tiff:overviews`, `tiff:compressed`, `tiff:aux-xml`, `tiff:worldfile`, `tiff:nodata`, `tiff:nodata-nan`, `tiff:multiband`, `tiff:no-georef` | raster |
| `corrupt` (+ `corrupt:truncated`, `corrupt:bad-header`, `corrupt:wrong-format`) | deliberately broken: readers must return `Error`/`Partial`, never panic |
| `real-world`, `large` | third-party data |

## Oracle record (`expected/<id>.json`, schema 1)

Mirrored by the serde types in `tests/oracle/src/record.rs` (`deny_unknown_fields`, so the two
cannot drift silently).

| Field | Meaning |
|---|---|
| `oracle` | GDAL + PROJ versions |
| `status` / `error` / `messages` | `ok` or `error` (GDAL refused); GDAL warnings, paths stripped |
| `layers[]` | GPKG tables in name order; one layer (file stem) for SHP/GeoTIFF |
| `kind` | `vector`, `attributes`, `raster`, `tiles` |
| `geometry_type`, `has_z`, `has_m` | flattened OGR type name (`Point`, `LineString`, `Polygon`, `Multi…`) + Z/M |
| `feature_count` | exact (`GetFeatureCount(force)`) |
| `extent` | **true** native extent: union of geometry envelopes; for rasters, bbox of the 4 pixel corners of the GDAL geotransform (PixelIsArea convention, even for PixelIsPoint files) |
| `declared_extent` | what the file claims: SHP header, `gpkg_contents` (may be stale, NULL, or zeros for empty SHP) |
| `raster` | width, height, band_count, data_types, nodata (`"nan"`/`"inf"` as strings), geotransform, resolution, `georef_source` (`geotiff_tags`/`aux_xml`/`worldfile`/`gpkg_tile_matrix`), pixel_is_point, overview_count, bigtiff, layout, compression |
| `fields` | attribute name/type/width (vector + attributes) |
| `encoding` | SHP only: GDAL's chosen DBF encoding, `.cpg` value, LDID byte |
| `crs` | `status` (`identified`/`unidentified`/`missing`), `source` (`prj`/`gpkg_srs`/`geokeys`/`aux_xml`/`none`), `raw` (the definition exactly as the file carries it; `null` for GeoKeys), `raw_kind` (`wkt1`/`wkt1_esri`/`wkt2`/`geokeys`), `declared_authority` (GPKG), `wkt2`, `epsg` + `epsg_method` (`authority`/`auto_identify`/`find_matches` ≥ 70 confidence, run on the raw definition) + `candidates` |
| `wgs84_extent`, `crosses_antimeridian` | `OCTTransformBounds`, 21 points per edge; `west > east` when crossing 180° |

Notes for reader sessions:
- A world-file raster's `.prj` is not read by GDAL's GTiff driver; `golden.py` parses it with OSR
  (`crs.source = "prj"`) because that is what a Whereabouts user expects.
- GDAL is more forgiving than a bounded header reader should be (e.g. `shp_corrupt_bad_dbf`,
  `tif_corrupt_truncated` open with warnings), so corrupt cases are compared on status only.
