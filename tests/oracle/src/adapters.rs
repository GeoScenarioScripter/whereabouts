//! The adapter registry: one entry per format that has a native reader, mapping a corpus case to
//! a [`NativeRecord`].
//!
//! Empty until the reader sessions land (stage01: 2.6 shapefile, 3.1 geopackage, 3.2 geotiff;
//! 4.3 later switches them to the full `indexer::extract` path for EPSG/WGS84). Each adds **one
//! line** to [`ADAPTERS`] plus its `read` function (in its own module here), listing only the
//! [`Check`]s it is ready to be held to.

mod native;
mod shapefile;

use std::path::{Path, PathBuf};

use crate::compare::Check;
use crate::manifest::Case;
use crate::record::NativeRecord;

/// What an adapter gets: the manifest entry plus resolved paths.
#[derive(Debug, Clone)]
pub struct ResolvedCase<'a> {
    /// The manifest entry.
    pub case: &'a Case,
    /// The directory holding the case's files.
    pub dir: PathBuf,
}

impl<'a> ResolvedCase<'a> {
    /// Resolve `case` against its directory.
    pub fn new(case: &'a Case, dir: PathBuf) -> Self {
        Self { case, dir }
    }

    /// Absolute path of the anchor file.
    pub fn anchor_path(&self) -> PathBuf {
        self.dir.join(&self.case.anchor)
    }

    /// Absolute paths of every file in the case (anchor included), in manifest order.
    pub fn file_paths(&self) -> Vec<PathBuf> {
        self.case.files.iter().map(|f| self.dir.join(f)).collect()
    }

    /// Path of a case file with the given extension (case-insensitive, e.g. `"prj"`), if listed.
    pub fn file_with_extension(&self, ext: &str) -> Option<PathBuf> {
        self.case
            .files
            .iter()
            .find(|f| {
                Path::new(f)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case(ext))
            })
            .map(|f| self.dir.join(f))
    }
}

/// Reads one case with a native reader. `None` = the adapter declines the case (counted as
/// skipped, e.g. a sub-variant it does not handle yet); a reader *error* is `Some` with
/// [`NativeStatus::Error`](crate::record::NativeStatus::Error), never `None` and never a panic.
pub type ReadFn = fn(&ResolvedCase<'_>) -> Option<NativeRecord>;

/// One registered adapter.
#[derive(Debug, Clone, Copy)]
pub struct Adapter {
    /// Format directory name it handles (see [`KNOWN_FORMATS`](crate::manifest::KNOWN_FORMATS)).
    pub format: &'static str,
    /// The checks this adapter is held to (others are not compared).
    pub checks: &'static [Check],
    /// The reader entry point.
    pub read: ReadFn,
}

/// The registry. One line per format; keep sorted by format.
pub static ADAPTERS: &[Adapter] = &[
    // 3.1: Adapter { format: "geopackage", checks: &[...], read: geopackage::read },
    // 3.2: Adapter { format: "geotiff", checks: &[...], read: geotiff::read },
    Adapter {
        format: "shapefile",
        checks: shapefile::CHECKS,
        read: shapefile::read,
    },
];

/// The registered adapters.
pub fn registry() -> &'static [Adapter] {
    ADAPTERS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_most_one_adapter_per_format() {
        let mut formats: Vec<_> = registry().iter().map(|a| a.format).collect();
        formats.sort_unstable();
        let n = formats.len();
        formats.dedup();
        assert_eq!(formats.len(), n, "duplicate adapter format");
        for f in formats {
            assert!(
                crate::manifest::KNOWN_FORMATS.contains(&f),
                "adapter for unknown format {f}"
            );
        }
    }
}
