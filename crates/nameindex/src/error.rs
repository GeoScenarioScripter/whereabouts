//! Error types.

use thiserror::Error;
use whereabouts_source::CheckpointError;

/// Why a record could not be inserted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum IndexError {
    /// The name is longer than [`crate::MAX_NAME_BYTES`] (WTF-8 bytes).
    #[error("name too long ({0} bytes)")]
    NameTooLong(usize),
    /// The name is not well-formed WTF-8, or raw UTF-16 had an odd byte length.
    #[error("invalid name encoding")]
    InvalidName,
    /// The index reached its `u32` entry or pool limit; [`crate::NameIndex::compact`] may help.
    #[error("index capacity exceeded")]
    CapacityExceeded,
}

/// Why a path could not be reconstructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum PathError {
    /// No live entry with this id.
    #[error("no such entry")]
    NotFound,
    /// An ancestor is not in the index (parent never seen, or removed).
    #[error("entry is orphaned (an ancestor is missing)")]
    Orphan,
    /// The parent chain loops, or is deeper than [`crate::MAX_DEPTH`].
    #[error("parent chain has a cycle")]
    Cycle,
}

/// Why a snapshot could not be written or loaded. Any error on load means "rescan".
#[derive(Debug, Error)]
pub enum SnapshotError {
    /// Reading or writing failed.
    #[error("snapshot I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Not a name-index snapshot.
    #[error("not a name-index snapshot (bad magic)")]
    BadMagic,
    /// Written by an incompatible format version.
    #[error("unsupported snapshot version {0}")]
    UnsupportedVersion(u32),
    /// The file ends early.
    #[error("snapshot truncated")]
    Truncated,
    /// The CRC-32 over the snapshot does not match its trailer.
    #[error("snapshot checksum mismatch (stored {stored:#010x}, computed {computed:#010x})")]
    ChecksumMismatch {
        /// CRC stored in the trailer.
        stored: u32,
        /// CRC computed over the bytes read.
        computed: u32,
    },
    /// Bytes follow the trailer.
    #[error("trailing bytes after snapshot")]
    TrailingBytes,
    /// The embedded checkpoint is invalid.
    #[error("invalid embedded checkpoint: {0}")]
    Checkpoint(#[from] CheckpointError),
    /// The content is structurally invalid (bad name, duplicate FRN, wrong count, …).
    #[error("corrupt snapshot: {0}")]
    Corrupt(String),
}
