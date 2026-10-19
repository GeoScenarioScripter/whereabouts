//! Dataset grouper (D-04): turns files into datasets and layers.
//!
//! A shapefile is its sidecars, a GeoPackage is one file with many layers, a raster is its image plus
//! `.aux.xml`/`.ovr`/world file/`.prj`. Never surfaces a `.dbf` or `.shx` as a standalone result.
//!
//! Spec: `docs/04-formats-and-crs.md` §3. Filled in by stage01 session 2.3.
