//! Native GIS metadata readers (D-07): header readers, not a GIS library.
//!
//! Each reader performs bounded reads (a few hundred bytes or one metadata block) and reports format,
//! raw CRS definition, native bounds, geometry type, counts and raster facts. No GDAL, ever, in this
//! crate (GDAL only appears in `tests/oracle` and the optional Extended Format Pack).
//!
//! Spec: `docs/04-formats-and-crs.md` §1–2. Filled in by stage01 sessions 2.6, 3.1, 3.2.
