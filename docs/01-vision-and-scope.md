# 01 — Vision and Scope

## One-liner

**Whereabouts: the whereabouts of your data.** Instantly find GIS datasets on your machine by name *and* by place — draw a box on a map, or type a place name, and see every dataset that covers it.

## The problem

GIS file systems are notoriously messy:

- Shapefiles, GeoTIFFs, GeoPackages, point clouds, CAD files and File Geodatabases scattered across drives, with names like `final_v3_clip.shp`.
- The question users actually have — *"what data do I have for this area?"* — is not answerable by any ordinary file search, because location lives **inside** the files (extent, CRS), not in their names.
- Generic tools (voidtools Everything, Windows Search) are fast at names but know nothing about geography. GIS desktop apps (QGIS browser, ArcGIS Pro catalog) know geography but are not fast, global, machine-wide search tools.

## Positioning

> Not "Everything, but for GIS files" — **"Everything, plus *where*."**

The owner's framing (Agreed): the things that make this hard — dataset abstraction, geospatial metadata indexing, and later network/cloud sources — are not obstacles, they **are the key features** and the product's moat.

## Target users

- GIS analysts, surveyors, remote-sensing and planning professionals on Windows workstations.
- Vendor-neutral: should be equally useful to **QGIS** users and **ArcGIS Pro** users.

## Prior art (and the gap)

| Product | What it does | Why the gap remains |
|---|---|---|
| voidtools Everything | Instant filename search via NTFS MFT + USN journal | No geospatial awareness; closed-source (SDK and `es.exe` CLI are public) |
| ArcMap/ArcGIS Desktop Search (10.x) | Indexed GIS content search with "within current extent" filter, text-based place search | Tied to ArcGIS, heavyweight setup, not a standalone instant tool |
| ArcGIS Server search services | Server-built index of org GIS content on file shares/DBs | Enterprise server product |
| Voyager (alta4 / Voyager Search) | Indexes many GIS formats locally and across servers; text + spatial search, faceted browsing | Enterprise software, not a lightweight instant desktop utility |

**The gap:** a fast, lightweight, vendor-neutral desktop tool with Everything-like responsiveness plus spatial search. (Validate the current competitive landscape before launch — see open questions.)

## Version 1 — goals (scope)

Status: scope boundaries **Agreed**; feature details **Proposed** unless noted.

1. **Local files only.** (Agreed) Local drives on the user's machine. No network shares, no cloud in v1.
2. **Instant filename search** across local NTFS volumes using the MFT/USN technique (Everything-style).
3. **Dataset abstraction.** (Agreed — "a must") Results are *datasets*, not files: a shapefile = its sidecars; a File Geodatabase = a folder; a GeoPackage = one file with many layers.
4. **Geospatial metadata indexing.** (Agreed) At minimum CRS and bounds; also geometry type, feature/point counts, raster size/resolution/bands where available.
5. **Spatial search:**
   - draw a box / use the current map extent;
   - type a place name (offline gazetteer);
   - two-stage matching: bbox filter, then exact test against a stored simplified footprint.
6. **Filters:** format, CRS/EPSG, geometry type, raster resolution, feature count, size, date modified (and date fields in metadata where present).
7. **Map view** showing dataset footprints, linked with the result list (hover/select highlights both ways).
8. **Previews** of datasets on the map — faithful placement and sensible default styling (vectors first; rasters and point clouds as described in the roadmap).
9. **Hand-off actions:** open in QGIS / ArcGIS Pro, reveal in Explorer, copy path.
10. **Works offline:** bundled low-zoom basemap and gazetteer.

## Non-goals (v1)

- **Not a GIS.** No editing, geoprocessing, or QGIS-grade cartography. Preview fidelity bar = "right place, sensible default style, fast" (Proposed, D-13).
- No network shares (SMB/NAS), no cloud (S3, COG over HTTP, STAC), no database sources (PostGIS) — **Deferred**, see roadmap.
- No PostgreSQL dependency (D-11).
- No GDAL in the core (D-07).
- No macOS/Linux in v1 (Proposed; architecture keeps the door open).
- Full-text search *inside* attribute tables is not a v1 goal.

## Provisioned (post-v1) — summary

Detail in [05-roadmap.md](05-roadmap.md).

- **Network shares and cloud sources** (Agreed as future; "maybe the first when local features are done").
- Existing **PostGIS databases as an indexable source**.
- **Project-file linkage**: which `.qgz`/`.aprx`/`.mxd` uses a dataset; broken-link detection.
- **Extended Format Pack** (optional GDAL plugin) for the long tail.
- macOS/Linux ports.
- A **team/enterprise catalog server** (where PostgreSQL/PostGIS would belong).
- Thumbnails, attribute peeks, catalog analytics.

## Naming

- Final pick (current): **Whereabouts** (Agreed as current pick). Chosen over Gazetteer, Dowser, Graticule, Geodex, Cognita, Intersects, etc.
- Still to do: trademark (software class 9), domains, GitHub org, winget/Microsoft Store, crates.io name checks — see open questions.
