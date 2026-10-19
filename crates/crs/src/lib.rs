//! Pure-Rust CRS layer (D-09).
//!
//! Parses ESRI WKT1 / OGC WKT1 / WKT2 / PROJJSON / GeoTIFF GeoKeys into an internal model, identifies
//! EPSG codes (authority → name match → parameter match), and transforms densified bounds to WGS84
//! with `proj4rs`. Metres of error from ignoring datum grids are acceptable: this is a search index.
//!
//! Spec: `docs/04-formats-and-crs.md` §4. Filled in by stage01 sessions 1.4 and 2.4.
