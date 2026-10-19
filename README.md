# Whereabouts

**The whereabouts of your data.** Instant search for GIS datasets on your Windows machine — by name *and* by place.

> Not "Everything, but for GIS files" — **Everything, plus *where*.**

Whereabouts is in the spirit of voidtools *Everything*: it reads the NTFS Master File Table and follows the USN change journal to keep a machine-wide filename index that answers as you type. On top of that it understands GIS data: it groups sidecar files into **datasets** (a shapefile is its `.shp/.shx/.dbf/.prj`, a GeoPackage is many layers, a File Geodatabase is a folder), reads each dataset's **extent and coordinate reference system** with fast native header readers, and lets you ask *"what data do I have for this area?"* by drawing a box on a map or typing a place name.

> **Status: pre-alpha, under active design and development.** Nothing is usable yet. Stage 01 (a command-line indexer prototype) is in progress — see [the stage plan](docs/stages/README.md).

## Planned for v1

- Instant filename search across local NTFS volumes (MFT + USN journal).
- Dataset abstraction: results are datasets and layers, not loose sidecar files.
- Geospatial metadata index: CRS (with EPSG identification), bounds, geometry type, feature counts, raster size/resolution.
- Spatial search: drag a box, use the current map extent, or type a place name (offline gazetteer).
- Filters: format, CRS/EPSG, geometry type, resolution, size, date.
- Map view with dataset footprints and previews; hand-off to QGIS / ArcGIS Pro.
- Works offline. No GDAL, no database server — a small native install.

Formats (v1): Shapefile, GeoPackage, GeoTIFF/COG, world-file rasters, FlatGeobuf, GeoParquet, LAS/LAZ, GeoJSON, KML/KMZ, DXF; File Geodatabase later in v1. Network shares and cloud storage are planned after v1.

## How it's built

A Rust workspace (indexer, readers, CRS, SQLite + R*Tree catalog) with a Tauri + MapLibre desktop UI planned. Design docs live in [`docs/`](docs/README.md): [vision & scope](docs/01-vision-and-scope.md), [decisions](docs/02-decisions.md), [architecture](docs/03-architecture.md), [formats & CRS](docs/04-formats-and-crs.md), [roadmap](docs/05-roadmap.md).

```sh
# Requires Rust (pinned via rust-toolchain.toml) and, on Windows, the MSVC build tools.
cargo build
cargo test --workspace
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md) (the project guide for humans and coding agents alike).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
