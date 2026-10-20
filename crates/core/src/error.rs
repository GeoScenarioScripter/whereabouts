//! Errors raised when constructing or decoding core types.

use thiserror::Error;

/// An invariant of a core type was violated, or a stored code could not be decoded.
///
/// Raised by validating constructors (`Bounds::new`, `Dataset::new`, …), by `FromStr` impls of the
/// code enums, and by serde when deserialising invalid data.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CoreError {
    /// Native bounds are non-finite or inverted.
    #[error("invalid bounds: {0}")]
    InvalidBounds(String),
    /// A WGS84 extent has no parts or a part lies outside [-180, 180] × [-90, 90].
    #[error("invalid WGS84 extent: {0}")]
    InvalidExtent(String),
    /// A dataset's member list violates its invariants (see `Dataset::new`).
    #[error("invalid dataset: {0}")]
    InvalidDataset(String),
    /// A file name's byte representation is not well-formed WTF-8.
    #[error("invalid file name: {0}")]
    InvalidFileName(String),
    /// A `DatasetRecord`/`LayerRecord` violates its documented invariants (see `validate`).
    #[error("invalid record: {0}")]
    InvalidRecord(String),
    /// An `ExtractionStatus` was decoded from an inconsistent code/detail pair.
    #[error("invalid extraction status: {0}")]
    InvalidStatus(String),
    /// A string code does not name any variant of the given enum.
    #[error("unknown {kind} code {code:?}")]
    UnknownCode {
        /// The enum the code was decoded for, e.g. `"Format"`.
        kind: &'static str,
        /// The offending code.
        code: String,
    },
}
