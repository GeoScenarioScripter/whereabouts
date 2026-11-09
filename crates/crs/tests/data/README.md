# CRS parser fixtures

Test inputs for `whereabouts-crs` (stage01 session 1.4). Expectations live in
`../fixtures.rs` (`CASES`); robustness tests in `../robustness.rs` truncate and mutate every
file here.

## Layout

- `<authority>-<code>/` — one directory per CRS, one file per encoding:
  - `esri.prj` — ESRI WKT1 (what ArcGIS/QGIS write to `.prj`)
  - `wkt1.txt` — OGC WKT1 as written by GDAL/PROJ (`WKT1_GDAL`)
  - `wkt2_2019.txt` — WKT2:2019 (ISO 19162:2019 / OGC 18-010r7)
  - `wkt2_2015.txt` — WKT2:2015 (ISO 19162:2015 / OGC 12-063r5), a few CRSs only
  - `projjson.json` — PROJJSON
  A variant is missing where PROJ cannot express that CRS in that encoding (EPSG:4978 has no
  ESRI WKT, EPSG:8857 has no WKT1).
- `handwritten/` — synthetic definitions for dialect quirks the authority exports don't show.
  The file-name prefix gives the dialect (`esri-`, `wkt1-`, `wkt2_2019-`, `projjson-`).

## Provenance and licences

**Authority-derived files** (`epsg-*/`, `esri-*/` except the `wkt2_2015.txt` files) were
downloaded unmodified from <https://spatialreference.org/ref/> on 2026-10-19 by `fetch.sh`
(site built with PROJ 9.9.0). spatialreference.org generates them from the PROJ database:

- EPSG definitions come from the **EPSG Geodetic Parameter Dataset**, © IOGP, used under the
  EPSG terms of use (<https://epsg.org/terms-of-use.html>), which permit copying and
  redistribution with attribution. The files are as exported by PROJ and are not presented as
  EPSG data themselves.
- `esri-*` definitions (ESRI:54009, 54030, 54034, 102003) and all `esri.prj` encodings come from
  Esri's projection engine definitions as imported into PROJ from
  <https://github.com/Esri/projection-engine-db-doc> (Apache License 2.0).
- PROJ itself is MIT-licensed.

**`wkt2_2015.txt` files** (EPSG 4326, 4978, 32633, 2227, 3031, 7405) were hand-converted by the
Whereabouts contributors to WKT2:2015 syntax (`GEODCRS`, `BASEGEODCRS`, top-level
`AREA`/`BBOX`, no `USAGE`/`ENSEMBLE`; `epsg-3031` also uses `()` brackets, long keywords and
the generic `UNIT`) from the EPSG-derived WKT2:2019 text above.

**`handwritten/`** files were written for this test suite by the Whereabouts contributors and
are under the repository licence (MIT OR Apache-2.0). Numeric values in
`wkt1-towgs84.txt`, `wkt2_2019-boundcrs.txt` and `projjson-boundcrs.json` (OSGB 1936 → WGS 84
Helmert parameters) and `wkt1-parentheses-lowercase.txt` (NAD27 / UTM 17N) are EPSG-derived.

## Refreshing

`sh crates/crs/tests/data/fetch.sh` re-downloads the authority-derived files (needs `curl`).
Keep its code list in sync with `CASES` in `../fixtures.rs`; the
`fixture_table_matches_the_data_directory` test fails if a directory has no table entry.
