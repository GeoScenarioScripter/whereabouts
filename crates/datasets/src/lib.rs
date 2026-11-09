//! Dataset grouper (D-04): turns files into datasets.
//!
//! A shapefile is its sidecars, a GeoPackage is one file (its `-wal`/`-shm`/`-journal` files are
//! ignored), a raster is its image plus `.aux.xml`/`.ovr`/`.msk`/world file/`.prj`, a File
//! Geodatabase is a `*.gdb` folder. `.dbf`/`.shx` members, orphan sidecars and FGDB internal
//! files are never surfaced as standalone results; a `.dbf` without a `.shp` is a non-spatial
//! table (`Format::Dbf`). Layers come later, from the readers.
//!
//! Spec: `docs/04-formats-and-crs.md` §3; stage01 session 2.3. The grouper reads names only,
//! never file contents.
//!
//! # API
//! - [`DirectoryView`]: what the grouper reads (`for_each_child(dir)`, `entry(frn)`), with
//!   [`MemoryTree`] as an in-memory implementation that applies `SourceEvent`s.
//! - [`Grouper`]: per-volume state. [`Grouper::full_scan`] groups a whole tree (and reconciles
//!   after a rescan); [`Grouper::apply_batch`] takes change events and returns a
//!   [`GroupDelta`]: `added`/`changed`/`modified` (the dirty set to extract), `renamed` (anchor
//!   renamed or moved, no re-extraction), `removed`, and `moved_dirs`.
//! - [`is_relevant_name`]: whether a file name can affect grouping at all (cheap event filter).
//!
//! Dataset IDs are (volume, anchor FRN) (D-27); names are WTF-8 bytes (D-28).

#![warn(missing_docs)]

mod fold;
mod grouper;
mod rules;
mod view;

pub use grouper::{GroupDelta, Grouper};
pub use rules::is_relevant_name;
pub use view::{DirectoryView, Entry, MemoryTree};
