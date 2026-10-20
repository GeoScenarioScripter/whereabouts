//! The `Source` abstraction (D-18): enumerate a file system and stream change events.
//!
//! v1 ships only the NTFS implementation (`whereabouts-ntfs`); SMB, cloud, PostGIS, macOS and Linux
//! sources plug in here later without touching the catalog or query layers.
//!
//! Spec: `docs/03-architecture.md` §2, §3, §9; contracts frozen by stage01 session 1.1.
//!
//! # Model
//! A source is **synchronous and poll-based**:
//! 1. [`Source::enumerate`] streams every entry into a [`RecordSink`] and returns a [`Checkpoint`]
//!    taken *before* enumeration started, so changes made during the enumeration are replayed by
//!    the first [`Source::changes`] call (events may therefore repeat state already enumerated;
//!    consumers must apply them idempotently).
//! 2. [`Source::changes`] returns everything since a checkpoint as a [`ChangeBatch`] plus the new
//!    checkpoint, or [`Changes::FullRescanRequired`] when the history is gone (journal wrapped,
//!    deleted or re-created, checkpoint from another volume/version). The caller commits a batch's
//!    checkpoint only after applying the batch.

#![warn(missing_docs)]

use std::fmt;
use std::ops::ControlFlow;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use whereabouts_core::{FileName, FileRecord, Frn};

/// A file system that can be enumerated and polled for changes.
pub trait Source: Send {
    /// Stable identity of the underlying volume/location, used to find its `VolumeId` in the
    /// catalog across runs.
    fn identity(&self) -> SourceIdentity;

    /// FRN of the root directory. Path reconstruction stops here (on NTFS the root is its own
    /// parent).
    fn root(&self) -> Frn;

    /// Streams every entry (files and directories, root included) into `sink`.
    ///
    /// Returns the checkpoint taken **before** enumeration began. If the sink returns
    /// `ControlFlow::Break`, enumeration stops with [`SourceError::Cancelled`].
    fn enumerate(&mut self, sink: &mut dyn RecordSink) -> Result<Checkpoint, SourceError>;

    /// Returns the changes since `since` without blocking for new ones (an empty batch means
    /// nothing changed). A checkpoint this source cannot continue from (other volume, unknown
    /// version, history overwritten) yields [`Changes::FullRescanRequired`], not an error.
    fn changes(&mut self, since: &Checkpoint) -> Result<Changes, SourceError>;
}

/// Receives records during [`Source::enumerate`].
pub trait RecordSink {
    /// Accepts one record; `Break` cancels the enumeration.
    fn accept(&mut self, record: FileRecord) -> ControlFlow<()>;
}

impl<F: FnMut(FileRecord) -> ControlFlow<()>> RecordSink for F {
    fn accept(&mut self, record: FileRecord) -> ControlFlow<()> {
        self(record)
    }
}

impl RecordSink for Vec<FileRecord> {
    fn accept(&mut self, record: FileRecord) -> ControlFlow<()> {
        self.push(record);
        ControlFlow::Continue(())
    }
}

/// Identifies the volume/location behind a source.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceIdentity {
    /// Source type, e.g. `"ntfs"`.
    pub kind: String,
    /// Stable key, unique per volume and unchanged across reboots and drive-letter changes
    /// (NTFS: the volume GUID path, e.g. `\\?\Volume{…}\`).
    pub key: String,
    /// Human-readable label, e.g. `"D:"`.
    pub label: String,
}

/// One change to the file system. Every variant carries the entry's state *after* the change
/// (for `Deleted`: its last known state), so consumers never need to look it up first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceEvent {
    /// A new entry appeared.
    Created(FileRecord),
    /// An entry was removed.
    Deleted(FileRecord),
    /// An entry was renamed and/or moved; `record` holds the new parent and name.
    Renamed {
        /// The entry after the rename.
        record: FileRecord,
        /// Parent directory before the rename.
        old_parent: Frn,
        /// Name before the rename.
        old_name: FileName,
    },
    /// An entry's content or attributes changed (sources coalesce per file and report it once
    /// per batch). Consumers compare content stamps before re-extracting.
    Modified(FileRecord),
}

impl SourceEvent {
    /// The entry the event is about (new state; last known state for `Deleted`).
    pub fn record(&self) -> &FileRecord {
        match self {
            SourceEvent::Created(r)
            | SourceEvent::Deleted(r)
            | SourceEvent::Modified(r)
            | SourceEvent::Renamed { record: r, .. } => r,
        }
    }
}

/// Events since a checkpoint, in the order they happened, and the checkpoint after them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeBatch {
    /// The events (possibly empty).
    pub events: Vec<SourceEvent>,
    /// Commit this once `events` are applied; pass it to the next [`Source::changes`] call.
    pub checkpoint: Checkpoint,
}

/// Result of [`Source::changes`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Changes {
    /// Incremental changes.
    Batch(ChangeBatch),
    /// The change history since the checkpoint is unavailable: call [`Source::enumerate`] again
    /// and reconcile.
    FullRescanRequired(RescanReason),
}

/// Why a full rescan is required.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RescanReason {
    /// The change journal no longer holds the checkpoint's position (it wrapped).
    JournalWrapped,
    /// The journal was deleted and re-created (its ID changed).
    JournalRecreated,
    /// The volume has no active change journal.
    JournalMissing,
    /// The checkpoint belongs to another volume, or its version/payload is not understood.
    CheckpointMismatch,
    /// Anything else, with a description.
    Other(String),
}

impl fmt::Display for RescanReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RescanReason::JournalWrapped => f.write_str("change journal wrapped"),
            RescanReason::JournalRecreated => f.write_str("change journal was re-created"),
            RescanReason::JournalMissing => f.write_str("no active change journal"),
            RescanReason::CheckpointMismatch => f.write_str("checkpoint does not match the source"),
            RescanReason::Other(s) => f.write_str(s),
        }
    }
}

/// An opaque, versioned position in a source's change history.
///
/// Only the source that produced it interprets `data` (NTFS: volume serial, journal ID, next
/// USN). Persisted by the catalog (`volumes` table) and embedded in name-index snapshots via
/// [`Checkpoint::to_bytes`].
///
/// Invariant: `data().len() <= MAX_DATA_LEN`, enforced by [`Checkpoint::new`],
/// [`Checkpoint::from_bytes`] and deserialisation, so `to_bytes` and `from_bytes` are exact
/// inverses.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "CheckpointRepr", into = "CheckpointRepr")]
pub struct Checkpoint {
    version: u32,
    data: Vec<u8>,
}

impl Checkpoint {
    /// Maximum payload size in bytes.
    pub const MAX_DATA_LEN: usize = 64 * 1024;
    const MAGIC: [u8; 4] = *b"WBCP";
    const HEADER_LEN: usize = 12;

    /// Creates a checkpoint with a source-defined payload format `version`.
    ///
    /// Fails with [`CheckpointError::TooLarge`] if `data` exceeds [`Checkpoint::MAX_DATA_LEN`].
    pub fn new(version: u32, data: Vec<u8>) -> Result<Self, CheckpointError> {
        if data.len() > Self::MAX_DATA_LEN {
            return Err(CheckpointError::TooLarge);
        }
        Ok(Checkpoint { version, data })
    }

    /// The payload format version chosen by the producing source.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// The source-defined payload.
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Self-describing encoding: `b"WBCP"`, version (u32 LE), payload length (u32 LE), payload.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::HEADER_LEN + self.data.len());
        out.extend_from_slice(&Self::MAGIC);
        out.extend_from_slice(&self.version.to_le_bytes());
        // Lossless: the invariant caps the length at MAX_DATA_LEN (64 KiB), far below u32::MAX.
        let len = self.data.len() as u32;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&self.data);
        out
    }

    /// Decodes [`Checkpoint::to_bytes`] output, validating magic, length and size limit.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, CheckpointError> {
        let header = bytes
            .get(..Self::HEADER_LEN)
            .ok_or(CheckpointError::Truncated)?;
        let (magic, rest) = header.split_at(4);
        if magic != Self::MAGIC {
            return Err(CheckpointError::BadMagic);
        }
        let (version, len) = rest.split_at(4);
        let version =
            u32::from_le_bytes(version.try_into().map_err(|_| CheckpointError::Truncated)?);
        let len = u32::from_le_bytes(len.try_into().map_err(|_| CheckpointError::Truncated)?);
        let len = usize::try_from(len).map_err(|_| CheckpointError::TooLarge)?;
        if len > Self::MAX_DATA_LEN {
            return Err(CheckpointError::TooLarge);
        }
        let payload = bytes.get(Self::HEADER_LEN..).unwrap_or_default();
        match payload.len().cmp(&len) {
            std::cmp::Ordering::Less => Err(CheckpointError::Truncated),
            std::cmp::Ordering::Greater => Err(CheckpointError::TrailingBytes),
            std::cmp::Ordering::Equal => Checkpoint::new(version, payload.to_vec()),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct CheckpointRepr {
    version: u32,
    data: Vec<u8>,
}

impl TryFrom<CheckpointRepr> for Checkpoint {
    type Error = CheckpointError;

    fn try_from(r: CheckpointRepr) -> Result<Self, CheckpointError> {
        Checkpoint::new(r.version, r.data)
    }
}

impl From<Checkpoint> for CheckpointRepr {
    fn from(c: Checkpoint) -> Self {
        CheckpointRepr {
            version: c.version,
            data: c.data,
        }
    }
}

/// A checkpoint could not be built or decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CheckpointError {
    /// Shorter than its header or declared length.
    #[error("checkpoint truncated")]
    Truncated,
    /// Does not start with the checkpoint magic.
    #[error("not a checkpoint (bad magic)")]
    BadMagic,
    /// Payload (given or declared) exceeds [`Checkpoint::MAX_DATA_LEN`].
    #[error("checkpoint payload too large")]
    TooLarge,
    /// Bytes after the declared payload.
    #[error("trailing bytes after checkpoint")]
    TrailingBytes,
}

/// A source operation failed. Recoverable history loss is [`Changes::FullRescanRequired`], not an
/// error.
#[derive(Debug, Error)]
pub enum SourceError {
    /// Access to the volume was denied (on NTFS: the process is not elevated).
    #[error("access denied: {0}")]
    AccessDenied(String),
    /// The location cannot be handled by this source (e.g. not an NTFS volume).
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The sink cancelled the enumeration.
    #[error("enumeration cancelled")]
    Cancelled,
    /// An I/O or OS error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use whereabouts_core::FileAttributes;

    fn rec(frn: u64, parent: u64, name: &str) -> FileRecord {
        FileRecord {
            frn: Frn(frn),
            parent: Frn(parent),
            name: FileName::from(name),
            attrs: FileAttributes::default(),
        }
    }

    /// In-memory source used to prove the trait is implementable and object-safe.
    struct Mem {
        files: Vec<FileRecord>,
        log: Vec<SourceEvent>,
    }

    impl Source for Mem {
        fn identity(&self) -> SourceIdentity {
            SourceIdentity {
                kind: "mem".into(),
                key: "mem-1".into(),
                label: "mem".into(),
            }
        }

        fn root(&self) -> Frn {
            Frn(5)
        }

        fn enumerate(&mut self, sink: &mut dyn RecordSink) -> Result<Checkpoint, SourceError> {
            let cp = Checkpoint::new(1, (self.log.len() as u64).to_le_bytes().to_vec()).unwrap();
            for f in &self.files {
                if sink.accept(f.clone()).is_break() {
                    return Err(SourceError::Cancelled);
                }
            }
            Ok(cp)
        }

        fn changes(&mut self, since: &Checkpoint) -> Result<Changes, SourceError> {
            let Ok(pos) = <[u8; 8]>::try_from(since.data()) else {
                return Ok(Changes::FullRescanRequired(
                    RescanReason::CheckpointMismatch,
                ));
            };
            let pos = usize::try_from(u64::from_le_bytes(pos)).unwrap();
            let Some(events) = self.log.get(pos..) else {
                return Ok(Changes::FullRescanRequired(RescanReason::JournalWrapped));
            };
            Ok(Changes::Batch(ChangeBatch {
                events: events.to_vec(),
                checkpoint: Checkpoint::new(1, (self.log.len() as u64).to_le_bytes().to_vec())
                    .unwrap(),
            }))
        }
    }

    #[test]
    fn source_trait_round_trip() {
        let mut src: Box<dyn Source> = Box::new(Mem {
            files: vec![rec(5, 5, ""), rec(6, 5, "a.shp")],
            log: vec![],
        });
        let mut seen = Vec::new();
        let cp = src.enumerate(&mut seen).unwrap();
        assert_eq!(seen.len(), 2);
        let mut n = 0;
        let mut stop_after_one = |_r: FileRecord| {
            n += 1;
            ControlFlow::Break(())
        };
        assert!(matches!(
            src.enumerate(&mut stop_after_one),
            Err(SourceError::Cancelled)
        ));
        assert_eq!(n, 1);
        match src.changes(&cp).unwrap() {
            Changes::Batch(b) => assert!(b.events.is_empty()),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            src.changes(&Checkpoint::new(1, vec![1]).unwrap()).unwrap(),
            Changes::FullRescanRequired(RescanReason::CheckpointMismatch)
        );
    }

    #[test]
    fn checkpoint_bytes_round_trip_and_validation() {
        let cp = Checkpoint::new(3, vec![1, 2, 3, 4, 5]).unwrap();
        let bytes = cp.to_bytes();
        assert_eq!(&bytes[..4], b"WBCP");
        assert_eq!(Checkpoint::from_bytes(&bytes).unwrap(), cp);
        assert_eq!(
            Checkpoint::from_bytes(&bytes[..11]),
            Err(CheckpointError::Truncated)
        );
        assert_eq!(
            Checkpoint::from_bytes(&bytes[..14]),
            Err(CheckpointError::Truncated)
        );
        let mut extra = bytes.clone();
        extra.push(0);
        assert_eq!(
            Checkpoint::from_bytes(&extra),
            Err(CheckpointError::TrailingBytes)
        );
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert_eq!(Checkpoint::from_bytes(&bad), Err(CheckpointError::BadMagic));
        let mut huge = bytes;
        huge[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            Checkpoint::from_bytes(&huge),
            Err(CheckpointError::TooLarge)
        );
        // Every prefix of a valid encoding is rejected without panicking.
        let enc = Checkpoint::new(7, vec![9; 40]).unwrap().to_bytes();
        for i in 0..enc.len() {
            assert!(Checkpoint::from_bytes(&enc[..i]).is_err());
        }
    }

    #[test]
    fn checkpoint_size_limit_holds_everywhere() {
        let max = Checkpoint::MAX_DATA_LEN;
        // At the limit: constructible, and bytes + serde round-trip exactly.
        let at = Checkpoint::new(2, vec![0xAB; max]).unwrap();
        assert_eq!(Checkpoint::from_bytes(&at.to_bytes()).unwrap(), at);
        let json = serde_json::to_string(&at).unwrap();
        assert_eq!(serde_json::from_str::<Checkpoint>(&json).unwrap(), at);
        // One over: rejected by new(), from_bytes() and deserialisation.
        assert_eq!(
            Checkpoint::new(2, vec![0xAB; max + 1]),
            Err(CheckpointError::TooLarge)
        );
        let mut over = at.to_bytes();
        over.push(0xAB);
        over[8..12].copy_from_slice(&u32::try_from(max + 1).unwrap().to_le_bytes());
        assert_eq!(
            Checkpoint::from_bytes(&over),
            Err(CheckpointError::TooLarge)
        );
        let json = serde_json::to_string(&serde_json::json!({
            "version": 2,
            "data": vec![0xABu8; max + 1],
        }))
        .unwrap();
        assert!(serde_json::from_str::<Checkpoint>(&json).is_err());
    }

    #[test]
    fn events_serde_round_trip() {
        let events = vec![
            SourceEvent::Created(rec(10, 5, "a.shp")),
            SourceEvent::Modified(rec(10, 5, "a.shp")),
            SourceEvent::Renamed {
                record: rec(10, 7, "b.shp"),
                old_parent: Frn(5),
                old_name: FileName::from("a.shp"),
            },
            SourceEvent::Deleted(rec(10, 7, "b.shp")),
        ];
        let batch = ChangeBatch {
            events,
            checkpoint: Checkpoint::new(1, vec![0xFF; 24]).unwrap(),
        };
        let json = serde_json::to_string(&Changes::Batch(batch.clone())).unwrap();
        assert_eq!(
            serde_json::from_str::<Changes>(&json).unwrap(),
            Changes::Batch(batch.clone())
        );
        assert_eq!(batch.events[2].record().name, FileName::from("b.shp"));
        for reason in [
            RescanReason::JournalWrapped,
            RescanReason::JournalRecreated,
            RescanReason::JournalMissing,
            RescanReason::CheckpointMismatch,
            RescanReason::Other("volume dismounted".into()),
        ] {
            let c = Changes::FullRescanRequired(reason);
            let json = serde_json::to_string(&c).unwrap();
            assert_eq!(serde_json::from_str::<Changes>(&json).unwrap(), c);
        }
        let id = SourceIdentity {
            kind: "ntfs".into(),
            key: r"\\?\Volume{x}\".into(),
            label: "D:".into(),
        };
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<SourceIdentity>(&json).unwrap(), id);
    }
}
