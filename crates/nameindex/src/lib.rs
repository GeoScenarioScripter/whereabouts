//! Compact in-memory filename index for one volume.
//!
//! Holds every `(FRN, parent, name, attributes)` a `Source` reports, rebuilds paths on demand,
//! answers case-insensitive substring / wildcard / extension queries with a parallel linear scan,
//! and persists itself as a checksummed binary snapshot carrying the source's `Checkpoint`.
//! Spec: `docs/03-architecture.md` §3.4 and D-10; built in stage01 session 2.1.
//!
//! # Layout
//! - **Entry columns** (struct of arrays, indexed by [`EntryId`]): FRN (8 B), parent entry (4),
//!   name offset (4) + length (2), folded segment (4), extension offset (2), attributes (4),
//!   live flag (1). No per-file heap allocation, no pointers.
//! - **Name pool**: every original name back to back, lossless WTF-8 (D-28).
//! - **Folded pool**: every case-folded name ([`fold_char`]) followed by a NUL, plus a segment
//!   table (start offset + owning entry, 8 B). Substring search is one SIMD `memmem` pass over
//!   this pool; a hit maps back to its entry by binary search on segment starts.
//! - **FRN map**: a dense `u32` table indexed by MFT record number (4 B per MFT record) with a
//!   hash-map overflow for anything else.
//! - **Root**: seeded at construction from the source's root FRN, because NTFS enumeration never
//!   returns it (D-31).
//!
//! # Mutation
//! [`NameIndex::insert`] is an upsert keyed by FRN, so replayed events are idempotent; renames
//! append the new name and leave a tombstoned segment; [`NameIndex::remove`] tombstones the entry.
//! [`NameIndex::compact`] drops tombstones (and invalidates [`EntryId`]s). Records may arrive
//! before their parent (enumeration order is MFT order): they wait in a pending table and are
//! linked when the parent appears. Entries whose parent never appears (NTFS metadata under
//! `$Extend`, which enumeration skips) stay orphans: their path is [`PathError::Orphan`], they
//! are found by name but never by path.
//!
//! # Paths
//! [`NameIndex::path`] walks parent links to the root, guarded against missing ancestors and
//! cycles (any chain deeper than [`MAX_DEPTH`]). Paths are volume-relative with a leading `\`.
//!
//! Hard links: one name per FRN (the last one reported), as documented for M0.

#![warn(missing_docs)]

mod error;
mod fold;
mod frnmap;
mod index;
mod search;
mod snapshot;
#[cfg(feature = "synth")]
pub mod synth;

pub use error::{IndexError, PathError, SnapshotError};
pub use fold::{fold_char, fold_str};
pub use index::{EntryId, IndexStats, MAX_DEPTH, MAX_NAME_BYTES, NameIndex, SEPARATOR};
pub use search::{KindFilter, MatchMode, Query, SearchResults};
pub use snapshot::{SNAPSHOT_VERSION, Snapshot};
