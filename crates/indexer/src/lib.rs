//! The indexing pipeline, shared by the M0 CLI and (from M1) the elevated indexer service.
//!
//! Source enumeration/changes → name index → dataset grouper → prioritized extraction queue →
//! native readers + CRS normalization/transform (`extract`) → catalog; plus checkpoint persistence,
//! restart replay and the full-rescan path.
//!
//! Spec: `docs/03-architecture.md` §3. Filled in by stage01 sessions 3.3 and 4.2.

#![warn(missing_docs)]

mod extract;
mod pipeline;

pub use extract::{ExtractedMetadata, extract};
pub use pipeline::{IndexError, IndexOptions, IndexProgress, Indexer};
