# 04 — Formats, Dataset Grouping, and CRS

Status: **Proposed** (D-04 dataset abstraction and D-05 metadata indexing are **Agreed**; the approach below is Proposed).

Principle: **we need header readers, not a GIS library.** For indexing, each dataset needs only a handful of facts — extent, CRS, geometry type, counts, raster size/resolution — and for most common formats those live in the first few hundred bytes or a small metadata block.

## 1. Support tiers

| Tier | Meaning | Formats |
|---|---|---|
| **Tier 1 (v1, native)** | Native Rust readers in the core | Shapefile, GeoPackage, FlatGeobuf, GeoParquet, LAS/LAZ, GeoTIFF/COG, world-file rasters, GeoJSON, KML/KMZ, DXF |
| **Tier 2 (native, after Tier 1 is solid)** | High-value, harder native readers | File Geodatabase (OpenFileGDB spec) — the most valuable "hard" format for Esri users |
| **Tier 3 (long tail, optional)** | Via the optional GDAL fallback plugin (Extended Format Pack) | ECW, MrSID, JPEG2000, NetCDF/HDF, GRIB, Esri Grid, ERDAS IMG, and others |
| **Name-only** | Indexed by filename only, no geo metadata | DWG (needs proprietary ODA libraries even in GDAL), and anything unsupported |

The original v1 format wish list from discussion: SHP, GPKG, GeoTIFF/COG, GeoJSON, KML, LAS/LAZ, FGDB, DWG/DXF. FGDB moved to Tier 2 (sequenced after Tier 1); DWG is name-only.

## 2. Where the metadata lives (per format)

| Format | Metadata location | Effort | Notes |
|---|---|---|---|
| Shapefile | `.shp` 100-byte header: bbox + shape type. `.dbf` header: record count + field list. `.prj`: WKT (often ESRI flavour, often without EPSG). `.cpg`: DBF encoding | Easy | Feature count from `.dbf` (or `.shx` length) |
| GeoPackage | SQLite: `gpkg_contents` (min_x/min_y/max_x/max_y, srs_id), `gpkg_spatial_ref_sys` (definitions), `gpkg_geometry_columns` | Easy | `gpkg_contents` bounds are optional and can be stale → fall back to the layer's R-tree (`rtree_<table>_<geom>`) min/max. Many layers per file. Open read-only |
| FlatGeobuf | Header: envelope, geometry type, feature count, CRS | Easy | Has a packed Hilbert R-tree — great for previews |
| GeoParquet | Parquet footer `geo` metadata JSON: bbox, CRS as PROJJSON, geometry types | Easy | Needs the `parquet` crate (footer only for indexing) |
| LAS/LAZ | Public header: bounds, point count. CRS in VLRs/EVLRs (GeoTIFF keys or OGC WKT) | Easy | LAZ header is uncompressed |
| GeoTIFF / COG | TIFF tags: ModelTiepoint, ModelPixelScale or ModelTransformation, GeoKeyDirectory (usually a direct EPSG code) | Moderate | Handle BigTIFF, rotation/shear, PixelIsArea vs PixelIsPoint, `.aux.xml` and `.ovr` sidecars |
| World-file rasters | `.tfw`/`.jgw`/`.pgw`/`.wld` (6-parameter affine) + `.prj` + image dimensions from the image header | Easy | |
| GeoJSON | Always WGS84 (RFC 7946). Extent requires streaming the whole file unless a top-level `bbox` exists | Easy code, slow on big files | Apply size threshold/timeout; legacy `crs` member may appear in old files |
| KML / KMZ | Always WGS84; extent requires streaming XML. KMZ = zip | Easy code, slow on big files | |
| DXF | Header `$EXTMIN` / `$EXTMAX` | Easy | **No CRS** — store extent in native units, flag "unknown CRS"; exclude from WGS84 spatial index unless user assigns CRS (Open) |
| File Geodatabase | Folder with `.gdbtable`/`.gdbtablx`; system table `GDB_Items` holds layer definitions incl. extents and CRS (XML) | Hard-ish (weeks) | Follow the published OpenFileGDB specification |

## 3. Dataset grouping rules (D-04)

The grouper turns files into datasets and layers. Initial rules:

- **Shapefile:** anchor `name.shp`; members with the same stem: `.shx`, `.dbf`, `.prj`, `.cpg`, `.sbn`, `.sbx`, `.qix`, `.shp.xml`, `.qmd`. A `.dbf` alone is a non-spatial table (not a spatial result).
- **File Geodatabase:** any directory named `*.gdb` containing `a00000001.gdbtable` → one dataset; its feature classes/rasters are layers. Never surface its internal files.
- **GeoPackage:** one `.gpkg` file → one dataset; each `gpkg_contents` row (features, tiles, attributes) → a layer. Ignore `-wal`/`-shm` files as members.
- **GeoTIFF / rasters:** anchor image; members: `.aux.xml`, `.ovr`, `.msk`, world file, `.prj`.
- **LAS/LAZ, FlatGeobuf, GeoParquet, GeoJSON, KML/KMZ, DXF:** single-file datasets.
- **Esri Grid / other folder formats (Tier 3):** folder with signature files (e.g. `hdr.adf`) → one dataset.
- Changes to any member file mark the whole dataset dirty for re-extraction.
- Renames/moves of the anchor update the dataset path without re-extraction if content mtime/size is unchanged.

## 4. CRS strategy (D-09)

**CRS is the real hard part, not file formats.**

Inputs to parse:
- ESRI-flavoured WKT1 (`.prj` files; frequently no `AUTHORITY` codes),
- OGC WKT1 and WKT2,
- PROJJSON (GeoParquet),
- GeoTIFF GeoKeys (often a direct EPSG code),
- GeoPackage `gpkg_spatial_ref_sys` (organization + id, plus WKT).

Steps:
1. Parse to an internal CRS model (datum/ellipsoid, projection method + parameters, units, axis order).
2. **Identify** EPSG when possible: direct authority code → else exact name match against an embedded table → else parameter matching (projection method, params, datum, units).
3. Store both the original definition and the identified EPSG (nullable).
4. **Transform bounds to WGS84** with `proj4rs` using an embedded EPSG definitions table (e.g. via a crate such as `crs-definitions`, or generated from the EPSG database — its terms permit redistribution with attribution):
   - densify bbox edges (e.g. 21 points per edge) before transforming;
   - detect and split antimeridian-crossing extents;
   - clamp polar/out-of-domain results sensibly.
5. Build the simplified WGS84 footprint for D-16.

Precision bar: metres of error from ignoring datum grid shifts are acceptable — this is a search index.

Coverage expectation: projections covering nearly all real data — UTM/Transverse Mercator, Lambert Conformal Conic, Albers, Web Mercator, polar stereographic, geographic. **Validate against a real corpus.** If coverage is insufficient, fall back to PROJ via the `proj` crate (still far lighter to package than GDAL).

Unidentified/unsupported CRS: keep native bounds, mark `crs_status = unknown`, exclude from the spatial index, still searchable by name and attributes.

## 5. Testing: GDAL as oracle (D-08)

- Maintain a **real-world corpus** of files (including nasty edge cases: ESRI WKT quirks, rotated GeoTIFFs, PixelIsPoint, BigTIFF, odd DBF encodings, stale GPKG bounds, antimeridian data, LAS with WKT vs GeoKey CRS).
- In CI: run `gdalinfo -json` / `ogrinfo -json -so` across the corpus and diff against native readers — extents (with tolerance), CRS identification, geometry types, counts, raster sizes.
- Track a per-format agreement rate; regressions fail the build.
- GDAL is **dev/CI-only** here — it is never linked into the core binary.

## 6. Preview readiness by format (for the roadmap)

| Format | Full read for previews (native) |
|---|---|
| FlatGeobuf, GeoPackage | Easy — built-in spatial indexes |
| Shapefile | Easy — use `.qix`/`.sbn` if present, otherwise build an index |
| GeoParquet | Moderate — row-group bbox stats help |
| GeoJSON/KML | Easy but slow for big files — build a temporary index/cache |
| GeoTIFF/COG | Moderate–hard — decoding codecs + warping + overviews |
| LAS/LAZ | Moderate — decimation + deck.gl |
| FGDB | Hard — full geometry decoding |
| Tier 3 | Via GDAL plugin, or `gdal3.js` (WASM) optional preview pack |
