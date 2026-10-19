# 03 — Architecture

Status: **Proposed** throughout (see `02-decisions.md` D-06 … D-20), except where marked Agreed.
Everything here is a design sketch to guide implementation — refine it as code lands, and keep the decision log in sync.

## 1. Process layout

```
┌──────────────────────────── UI process (unprivileged) ──────────────────────────┐
│  Tauri shell · TypeScript frontend · MapLibre GL map · virtualized result list  │
│  Custom protocol whereabouts://tiles/… served by the backend tile server        │
└───────────────────────────────▲──────────────────────────────────────────────────┘
                                │ named-pipe IPC (queries, results, status, events)
┌───────────────────────────────┴──── Indexer service (elevated) ─────────────────┐
│  Sources (D-18): NtfsVolumeSource → MFT enumeration + USN journal tail          │
│  Name index (in-memory, per volume) + snapshot persistence                      │
│  Dataset grouper (sidecars, FGDB folders, GPKG layers)                          │
│  Metadata job queue → native readers (D-07) → CRS normalizer (D-09)             │
│  Catalog: SQLite (WAL) + R*Tree;  Tile server + tile cache                      │
└───────────────────────────────▲──────────────────────────────────────────────────┘
                                │ spawn / kill / restart (only if Extended Format Pack installed)
┌───────────────────────────────┴──── GDAL fallback workers (optional) ───────────┐
│  GDAL/OGR for long-tail formats; per-file timeouts; crash-isolated              │
└──────────────────────────────────────────────────────────────────────────────────┘
```

**Open question:** whether the tile server lives in the service (has the catalog and file access) or the UI process's Tauri backend (closer to the protocol handler). Default assumption: protocol handler in the Tauri backend forwards tile requests over IPC to the service. Revisit if latency is a problem.

## 2. Proposed Rust workspace layout

Crates live in `crates/<short-name>` (e.g. `crates/nameindex` → package `whereabouts-nameindex`); the oracle harness is `tests/oracle` (D-24). Stage01 crates exist as stubs; `tiles`, `ipc`, `service` and `app/` are added by later stages.

| Crate | Responsibility |
|---|---|
| `whereabouts-core` | Shared types: `DatasetId`, `Dataset`, `Layer`, `Bounds`, `CrsRef`, errors |
| `whereabouts-source` | `Source` trait; event types (Created/Deleted/Renamed/Modified) |
| `whereabouts-ntfs` | `NtfsVolumeSource`: `FSCTL_ENUM_USN_DATA`, `FSCTL_QUERY_USN_JOURNAL`, `FSCTL_READ_USN_JOURNAL` |
| `whereabouts-nameindex` | Compact in-memory filename index, path reconstruction, substring/wildcard/regex scan, snapshot I/O |
| `whereabouts-datasets` | Dataset grouping rules (file sets → datasets/layers) |
| `whereabouts-readers` | Native format readers (header/metadata; later full-geometry reads for previews) |
| `whereabouts-crs` | WKT1/ESRI WKT/WKT2/PROJJSON/GeoKey parsing, EPSG identification, bounds → WGS84 (proj4rs) |
| `whereabouts-catalog` | SQLite schema, migrations, R*Tree, query building |
| `whereabouts-query` | Search syntax parser → name-index scan + catalog query plan |
| `whereabouts-tiles` | MVT encoder, raster warping, tile cache |
| `whereabouts-ipc` | Named-pipe protocol (serde messages) |
| `whereabouts-service` | The elevated indexer service binary |
| `whereabouts-indexer` | Pipeline shared by the CLI and the service: source → name index → grouper → extraction queue → `extract` (readers + CRS) → catalog; checkpoints (D-24) |
| `whereabouts-cli` | M0 CLI (index, inspect, query) |
| `app/` (Tauri) | UI process and frontend |
| `tests/oracle` | GDAL-diff harness (dev/CI only) |

## 3. Indexing pipeline

### 3.1 Initial enumeration (per NTFS volume)
1. Open the volume handle (`\\.\C:`) — requires elevation (hence the service).
2. Enumerate all records with `FSCTL_ENUM_USN_DATA` → `(FRN, parent FRN, name, attributes)`. Raw `$MFT` parsing is a later optimization (faster, gives sizes/timestamps in one pass, but must handle attribute lists, fixups, fragmented runs).
3. Build the name index: store **(FRN, parent, name)**, not full paths; reconstruct paths on demand by walking parent links to the root.
4. Feed candidate GIS files (by extension and by folder signature, e.g. `*.gdb` directories) to the dataset grouper.

### 3.2 Staying live
- `FSCTL_QUERY_USN_JOURNAL` → journal ID and next USN; `FSCTL_READ_USN_JOURNAL` → stream of create/delete/rename (old+new name)/data-change reasons.
- Apply to the name index; emit `Source` events; the grouper maps events to affected datasets and enqueues re-extraction.
- Persist snapshot + last processed USN + journal ID. On restart, replay from the saved USN.
- **Full rescan** when the journal ID changed, the journal was deleted/disabled, or the saved USN has been overwritten (journal wrapped).

### 3.3 Edge cases to handle
- **Hard links:** one file, several names/parents.
- **Non-NTFS local drives** (FAT/exFAT, ReFS partially): need a walker + `ReadDirectoryChangesW` fallback (v1 inclusion is Open).
- Debounce bursts of changes (e.g. a tool rewriting a shapefile writes several sidecars).

### 3.4 Name index memory layout (guidelines)
- One contiguous **string pool** per volume; entries reference names by `u32` offset/length — no per-file heap strings, no pointers.
- Keep a **case-folded copy** (or fold at build time) so comparisons are cheap.
- Store the **extension offset** so `ext:` filters are O(1) per entry.
- Search is a (multithreaded) linear scan over the pool — scanning millions of short strings in RAM takes milliseconds. Sorted arrays for name/size/date sort orders. n-gram/trigram index only if needed later.

### 3.5 Metadata extraction
- Background, prioritized queue (e.g., recently modified and user-visible items first).
- Native readers read **only the bytes needed** (bounded reads). GeoJSON/KML need streaming scans — apply size thresholds/timeouts and record "extent pending/approximate" if needed.
- Output per dataset/layer: format, CRS (native definition + identified EPSG if any), native bounds, WGS84 bounds (densified transform), simplified footprint, geometry type, feature/point count, raster width/height/bands/dtype/resolution/nodata/overviews, field list.
- Record extraction status and errors per dataset (`ok`, `partial`, `unsupported`, `error:<reason>`), and the source mtime/size so re-extraction happens only on change.

## 4. Catalog schema (sketch)

```sql
-- datasets: one logical dataset (shapefile set, GPKG file, FGDB folder, GeoTIFF + sidecars, LAS file…)
CREATE TABLE datasets (
  id            INTEGER PRIMARY KEY,
  volume_id     INTEGER NOT NULL,
  kind          TEXT NOT NULL,        -- 'vector' | 'raster' | 'pointcloud' | 'container' | 'cad'
  format        TEXT NOT NULL,        -- 'shp' | 'gpkg' | 'gtiff' | 'las' | 'fgb' | 'gpq' | 'geojson' | 'kml' | 'dxf' | 'fgdb' …
  primary_path  TEXT NOT NULL,        -- path of the anchor file/folder (rebuilt from FRN chain; cached)
  name          TEXT NOT NULL,
  total_size    INTEGER,
  mtime         INTEGER,
  status        TEXT NOT NULL,        -- 'pending' | 'ok' | 'partial' | 'unsupported' | 'error'
  status_detail TEXT,
  extracted_at  INTEGER
);

CREATE TABLE dataset_files (          -- every member file and its role
  dataset_id INTEGER NOT NULL REFERENCES datasets(id),
  frn        INTEGER NOT NULL,
  role       TEXT NOT NULL            -- 'main' | 'shx' | 'dbf' | 'prj' | 'cpg' | 'qix' | 'aux_xml' | 'world_file' | 'ovr' …
);

CREATE TABLE layers (                 -- 1 for simple formats; N for GPKG/FGDB
  id            INTEGER PRIMARY KEY,
  dataset_id    INTEGER NOT NULL REFERENCES datasets(id),
  name          TEXT,
  geometry_type TEXT,                 -- 'Point' | 'LineString' | 'Polygon' | 'Multi…' | NULL for rasters
  feature_count INTEGER,
  crs_id        INTEGER REFERENCES crs(id),
  minx REAL, miny REAL, maxx REAL, maxy REAL,  -- native CRS bounds
  footprint     BLOB,                 -- simplified WGS84 footprint (WKB)
  fields_json   TEXT
);

CREATE TABLE rasters (
  layer_id  INTEGER PRIMARY KEY REFERENCES layers(id),
  width INTEGER, height INTEGER, bands INTEGER, dtype TEXT,
  res_x REAL, res_y REAL, nodata TEXT, has_overviews INTEGER
);

CREATE TABLE crs (
  id        INTEGER PRIMARY KEY,
  epsg      INTEGER,                  -- NULL if unidentified
  name      TEXT,
  definition TEXT NOT NULL,           -- original WKT / PROJJSON / GeoKey summary
  def_kind  TEXT NOT NULL             -- 'wkt1_esri' | 'wkt1' | 'wkt2' | 'projjson' | 'geokeys'
);

-- WGS84 bbox per layer for the coarse spatial filter
CREATE VIRTUAL TABLE layer_extents USING rtree(id, minx, maxx, miny, maxy);
```

Tile cache lives in a separate SQLite file: `tiles(dataset_id, layer_id, source_mtime, z, x, y, format, data)`.

## 5. Query pipeline

1. Parse the search string (syntax design is Open — Everything-like: space = AND, `|` = OR, `!` = NOT, wildcards, `regex:`, plus GIS filters such as `crs:2193`, `geom:polygon`, `res:<1m`, `format:gpkg`, `in:"Wellington"`).
2. Name terms → scan the in-memory name index → candidate FRNs → map to datasets.
3. Spatial term (map extent, drawn box, or gazetteer place) → `layer_extents` R*Tree filter → exact intersection against `footprint` with the `geo` crate (D-16).
4. Attribute filters → SQL on `datasets`/`layers`/`rasters`/`crs`.
5. Intersect, rank (name relevance, spatial relevance such as overlap ratio, recency), page, stream to UI.

Goal: typing feels instant; spatial queries should not noticeably slow the result list.

## 6. Preview / tile pipeline (D-13, D-14)

**Vectors**
- Read features with native readers (`shapefile`, `geozero` for GPKG WKB, `flatgeobuf`, `geoarrow`/`parquet`, streaming GeoJSON/KML).
- Use built-in spatial indexes where available (FlatGeobuf, GPKG R-tree; shapefile `.qix`/`.sbn` if present, else build one) to read only features touching the tile.
- Reproject → Web Mercator, clip to tile + buffer, simplify per zoom, encode **MVT**. Cache.

**Rasters**
- Decode with the `tiff` crate (LZW/Deflate/PackBits first; JPEG-in-TIFF, ZSTD, WebP, LERC need extra work).
- **Warp** per tile: tile pixel → lon/lat → source CRS → source pixel; nearest/bilinear sampling.
- Use internal overviews (COG/pyramids) when zoomed out; generate and cache overviews if missing (slow — background).
- Rendering semantics: nodata masks, palettes, min/max stretch + colormap for DEMs/float bands.

**Point clouds**
- `las` + `laz` (pure Rust); decimate by zoom; render with deck.gl `PointCloudLayer` (or a MapLibre custom layer).

**Hand-off:** "Open in QGIS / ArcGIS Pro", "Reveal in Explorer", "Copy path".

## 7. Frontend

- Tauri + TypeScript; framework Open (Solid/Svelte are light; React has the largest ecosystem).
- MapLibre GL JS: footprints layer, drag-box query, hover/select linked with the result list, preview layers from `whereabouts://tiles/…`.
- Offline basemap from a bundled **PMTiles** file; offline place search from a bundled **GeoNames** subset.
- Virtualized list for large result sets.

## 8. Packaging

- Tauri bundler → MSI (WiX). Installer registers the elevated indexer service; the UI never needs admin.
- Core is a static Rust build — no GDAL, no `proj.db` (D-07, D-09).
- Extended Format Pack (GDAL fallback) is a separate optional download, built via vcpkg with pinned versions.

## 9. Future-proofing hooks (do not implement in v1, but don't block them)

- `Source` trait implementations: SMB share (crawl + watch, no USN over SMB), S3 via `object_store`, STAC catalogs, PostGIS read-only, macOS (`getattrlistbulk` + FSEvents), Linux (`getdents64` + fanotify/inotify).
- GDAL `/vsis3/` and `/vsicurl/` for remote COG/GeoParquet headers (via the optional pack) — or native HTTP range reads for COG/GeoParquet/FlatGeobuf.
- Project-file parsers (`.qgz`/`.qgs`, `.aprx`, `.mxd`) to link projects ↔ datasets.
