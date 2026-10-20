//! Native GIS metadata readers (D-07): header readers, not a GIS library.
//!
//! Each reader performs bounded reads (a few hundred bytes or one metadata block) and reports
//! raw CRS definition, native bounds, geometry type, counts, fields and raster facts, following
//! the [`FormatReader`] contract. No GDAL, ever, in this crate (GDAL only appears in
//! `tests/oracle` and the optional Extended Format Pack).
//!
//! Spec: `docs/04-formats-and-crs.md` §1–2; stage01 sessions 2.6 (shapefile, this registry),
//! 3.1 (GeoPackage), 3.2 (GeoTIFF).
//!
//! # Use
//! - [`read`] dispatches a [`ResolvedDataset`] to the reader registered for its format.
//! - [`reader_for`] / [`readers`] expose the registry itself.
//! - [`measure`] reports the bytes a reader read on the current thread (tests assert the bound;
//!   benchmarks report it).
//!
//! # Adding a reader
//! Write the module (`src/<format>.rs`) with a unit struct implementing [`FormatReader`], do all
//! file access through the crate's bounded `io::SourceFile` (so budgets and [`measure`] see it),
//! and add **one line** to `READERS` below. Each [`Format`] may be claimed by one reader only
//! (a unit test enforces it).

mod dbf;
mod encoding;
mod io;
mod prj;
mod shapefile;

use std::path::Path;

use whereabouts_core::{
    DatasetMetadata, Format, FormatReader, ReadError, ReadLimits, ResolvedDataset,
};

pub use dbf::DbfReader;
pub use io::{IoStats, measure};
pub use shapefile::ShapefileReader;

/// The reader registry: **one line per reader**, sorted by reader name. Sessions adding a reader
/// add exactly one line here (3.1: GeoPackage, 3.2: GeoTIFF).
static READERS: &[&dyn FormatReader] = &[&DbfReader, &ShapefileReader];

/// Every registered reader, in registry order.
pub fn readers() -> &'static [&'static dyn FormatReader] {
    READERS
}

/// The reader for `format`, if one is registered.
pub fn reader_for(format: Format) -> Option<&'static dyn FormatReader> {
    READERS
        .iter()
        .copied()
        .find(|r| r.formats().contains(&format))
}

/// Reads `dataset` with the reader registered for its format, within `limits`.
///
/// Formats without a native reader yet give [`ReadError::Unsupported`] (status `unsupported`).
pub fn read(dataset: &ResolvedDataset, limits: &ReadLimits) -> Result<DatasetMetadata, ReadError> {
    match reader_for(dataset.format) {
        Some(reader) => reader.read(dataset, limits),
        None => Err(ReadError::Unsupported(format!(
            "no native reader for {} datasets yet",
            dataset.format
        ))),
    }
}

/// Layer name for single-layer file formats: the file stem (`a.b.shp` → `a.b`), lossy for
/// names that are not valid Unicode.
pub(crate) fn layer_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use whereabouts_core::ExtractionStatus;

    use super::*;

    #[test]
    fn each_format_has_at_most_one_reader() {
        let mut seen = BTreeSet::new();
        for reader in readers() {
            assert!(
                !reader.formats().is_empty(),
                "{} claims no format",
                reader.name()
            );
            for format in reader.formats() {
                assert!(seen.insert(*format), "{format} claimed twice");
            }
        }
        let names: Vec<_> = readers().iter().map(|r| r.name()).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted, "keep READERS sorted by name");
    }

    #[test]
    fn dispatch_by_format() {
        assert_eq!(reader_for(Format::Shapefile).unwrap().name(), "shapefile");
        assert_eq!(reader_for(Format::Dbf).unwrap().name(), "dbf");
        assert!(reader_for(Format::NameOnly).is_none());
        let ds = ResolvedDataset {
            format: Format::Kml,
            anchor: PathBuf::from("does-not-matter.kml"),
            sidecars: Vec::new(),
        };
        let err = read(&ds, &ReadLimits::default()).unwrap_err();
        assert_eq!(err.status(), ExtractionStatus::Unsupported);
    }

    #[test]
    fn layer_names_are_stems() {
        assert_eq!(layer_name(Path::new("dir/roads.shp")), "roads");
        assert_eq!(layer_name(Path::new("a.b.SHP")), "a.b");
        assert_eq!(layer_name(Path::new("")), "");
    }
}
