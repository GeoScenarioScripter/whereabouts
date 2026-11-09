//! Catalog error type.

use thiserror::Error;
use whereabouts_core::{CoreError, VolumeId};
use whereabouts_source::CheckpointError;

/// Errors returned by the catalog.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CatalogError {
    /// SQLite reported an error (I/O, constraint, busy, …).
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The record handed to the catalog breaks a core invariant (`DatasetRecord::validate`).
    #[error("invalid record: {0}")]
    InvalidRecord(#[from] CoreError),
    /// The volume is not registered (call `register_volume` first).
    #[error("unknown volume {0}")]
    UnknownVolume(VolumeId),
    /// The database was written by a different schema version than this build supports.
    #[error("catalog schema version {found} is not supported (this build: {supported})")]
    UnsupportedSchema {
        /// `PRAGMA user_version` of the database.
        found: i64,
        /// Schema version this build reads and writes.
        supported: i64,
    },
    /// A value doesn't fit the catalog's storage type (e.g. a `u64` above `i64::MAX`).
    #[error("value out of range for the catalog: {0}")]
    OutOfRange(String),
    /// Stored data doesn't decode into core types (unknown code, broken JSON, bad bounds, …).
    #[error("corrupt catalog: {0}")]
    Corrupt(String),
    /// The stored checkpoint bytes don't decode; the caller should rescan the volume.
    #[error("stored checkpoint is unreadable: {0}")]
    Checkpoint(#[from] CheckpointError),
}

/// Result alias for catalog operations.
pub type Result<T, E = CatalogError> = std::result::Result<T, E>;

pub(crate) fn corrupt(msg: impl Into<String>) -> CatalogError {
    CatalogError::Corrupt(msg.into())
}
