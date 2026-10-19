# 05 — Roadmap

Two horizons: **Version 1** (local, Windows, core value) and **Provisioned** features (designed-for now, built later).
Milestone breakdown is **Proposed**; the v1 scope boundary and the "network & cloud next" ordering are **Agreed**.

**Delivery:** each milestone is delivered as one stage (M0 = stage01 … M4 = stage05), planned and executed per [`08-development-workflow.md`](08-development-workflow.md). Stage plans and status: [`stages/README.md`](stages/README.md).

---

## Version 1 goals (recap)

Local-only (Agreed) · instant filename search · dataset abstraction (Agreed) · geospatial metadata index — CRS, bounds, etc. (Agreed) · spatial search (box / extent / place name) · GIS filters · map with footprints · previews · open-in-QGIS/ArcGIS hand-off · offline-capable.

Details: [01-vision-and-scope.md](01-vision-and-scope.md).

## v1 milestones

### M0 — CLI indexer prototype (D-20) → [stage01](stages/stage01/PLAN.md)
Prove the risky core before any UI.
- [ ] `whereabouts-ntfs`: enumerate one volume via `FSCTL_ENUM_USN_DATA`; tail the USN journal; persist snapshot + last USN + journal ID; detect when a full rescan is needed.
- [ ] `whereabouts-nameindex`: compact string-pool index; path reconstruction from FRN chains; substring/wildcard search; benchmark against Everything on the same volume.
- [ ] `whereabouts-datasets`: grouping rules for shapefile, GPKG, GeoTIFF+sidecars, single-file formats.
- [ ] `whereabouts-readers`: Shapefile, GeoPackage, GeoTIFF first.
- [ ] `whereabouts-crs`: WKT1/ESRI parsing, EPSG identification, densified bounds → WGS84.
- [ ] `whereabouts-catalog`: SQLite schema + R*Tree.
- [ ] `whereabouts-cli`: `index`, `inspect <path>`, `query "<text>" --bbox …`.
- [ ] `tests/oracle`: GDAL-diff harness + initial corpus.

**Exit criteria:** indexes a real GIS workstation volume; filename search feels instant; extents/CRS agree with GDAL on the corpus within tolerance; survives journal wrap and service restart.

### M1 — Indexer service + complete Tier 1 readers
- [ ] Elevated Windows service; named-pipe IPC (`whereabouts-ipc`).
- [ ] Remaining Tier 1 readers: FlatGeobuf, GeoParquet, LAS/LAZ, world-file rasters, GeoJSON, KML/KMZ, DXF.
- [ ] Simplified footprints + two-stage spatial query (D-16).
- [ ] Prioritized background extraction queue with status/error recording; debounce change bursts.
- [ ] Hard links; (Open) non-NTFS local drive fallback.

### M2 — Desktop UI (Tauri + MapLibre)
- [ ] Search box with Everything-style syntax + GIS filters (syntax design is Open).
- [ ] Virtualized results list (datasets/layers, format, CRS, size, date, status).
- [ ] Map: footprints, drag-box and current-extent search, list ↔ map highlight.
- [ ] Offline basemap (PMTiles) + offline gazetteer place search (GeoNames subset).
- [ ] Hand-off actions: Open in QGIS / ArcGIS Pro, Reveal in Explorer, Copy path.

### M3 — Previews (local tile server, D-14)
- [ ] Tauri custom protocol `whereabouts://tiles/…` → service.
- [ ] Vector MVT tiling with reprojection, clipping, simplification; tile cache.
- [ ] Raster preview: decode (common codecs), warp to Web Mercator, overviews, nodata, default stretch/colormap.
- [ ] Point-cloud preview with decimation (deck.gl) — may slip to v1.x.

### M4 — Tier 2 + polish + release
- [ ] File Geodatabase native reader (metadata; previews may follow later).
- [ ] Performance passes (memory layout, multithreaded scan, query latency).
- [ ] MSI installer (service registration), auto-update strategy (Open), crash reporting (Open).
- [ ] Name availability checks done; branding.

---

## Provisioned (post-v1) features

Ordered by current expectation. Items 1–2 are **Agreed** as future direction ("maybe the first when local features are done"); the rest are **Proposed** candidates.

1. **Network shares (SMB/NAS)** — `Source` impl with crawling + change watching (no USN journal over SMB); scheduled re-crawl; offline-tolerant catalog entries. Most enterprise GIS data lives here — likely the hook for a paid tier.
2. **Cloud sources** — S3/Azure/GCS via `object_store`; COG / GeoParquet / FlatGeobuf header reads via HTTP range requests; **STAC** catalogs as a source.
3. **PostGIS as a source** — connect read-only to users' databases; index geometry tables/views as datasets (`geometry_columns`, `ST_EstimatedExtent`).
4. **Project-file linkage** — parse `.qgz`/`.qgs`, `.aprx`, `.mxd`(?) to show which projects use a dataset, and detect broken links after data moves.
5. **Extended Format Pack** — optional GDAL fallback plugin (D-08) for Tier 3 formats; optionally `gdal3.js` for long-tail previews.
6. **Richer previews** — thumbnails in results, attribute table peek, FGDB geometry previews.
7. **macOS / Linux** — `Source` impls (`getattrlistbulk` + FSEvents; `getdents64` + fanotify/inotify); Tauri already cross-platform.
8. **Team / enterprise catalog server** — shared index for an organization's NAS + cloud; this is where PostgreSQL/PostGIS belongs (D-11).
9. **Catalog analytics** — e.g., area by CRS/year/format (DuckDB considered, but its spatial extension bundles GDAL).
10. **Content search** — attribute values / metadata documents (ISO 19115 XML, `.shp.xml`).
11. **CLI and SDK** — `es.exe`-style CLI for scripting; IPC SDK for other tools (QGIS plugin, ArcGIS Pro add-in).
