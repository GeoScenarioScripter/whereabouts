//! [`NtfsVolumeSource`]: the `Source` implementation for one local NTFS volume (Windows only).
//!
//! - [`Source::enumerate`] queries the journal first (that position is the checkpoint, D-29),
//!   then streams a synthesised root record followed by every `FSCTL_ENUM_USN_DATA` record.
//! - [`Source::changes`] checks the checkpoint against the volume and journal
//!   ([`checkpoint::assess`]), reads the journal with non-blocking `FSCTL_READ_USN_JOURNAL`
//!   calls and folds the records into events ([`Coalescer`]).
//!
//! Nothing here writes to the volume; in particular no journal is ever created (D-31). A volume
//! without an active journal can be enumerated, but every `changes` call then asks for a full
//! rescan until someone creates a journal.

use std::io;
use std::ops::ControlFlow;

use whereabouts_core::{FileAttributes, FileName, FileRecord, Frn};
use whereabouts_source::{
    ChangeBatch, Changes, Checkpoint, CheckpointError, RecordSink, RescanReason, Source,
    SourceError, SourceIdentity,
};

use crate::checkpoint::{self, JournalPosition, NtfsCheckpoint};
use crate::error::{Error, ParseError, Result};
use crate::journal::{Coalescer, ReadStep, file_record, read_step};
use crate::record::{JournalInfo, RecordIter};
use crate::volume::{EnumOptions, Volume};

/// `SourceIdentity::kind` of NTFS volumes.
pub const SOURCE_KIND: &str = "ntfs";

/// Options for [`NtfsVolumeSource`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceOptions {
    /// Output buffer for `FSCTL_ENUM_USN_DATA` (see [`EnumOptions::buffer_size`]).
    pub enum_buffer_size: usize,
    /// Output buffer per `FSCTL_READ_USN_JOURNAL` call in bytes (clamped to 4 KiB ..= 16 MiB).
    pub journal_buffer_size: usize,
    /// One [`Source::changes`] call stops reading once it has consumed this many bytes of
    /// journal records, so a long backlog arrives as several batches of bounded size. Call
    /// again until a batch comes back empty.
    pub max_batch_bytes: u64,
}

impl Default for SourceOptions {
    fn default() -> Self {
        Self {
            enum_buffer_size: EnumOptions::default().buffer_size,
            journal_buffer_size: 64 << 10,
            max_batch_bytes: 32 << 20,
        }
    }
}

/// Counters from the last [`Source::changes`] call (for logs and benchmarks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChangeStats {
    /// `FSCTL_READ_USN_JOURNAL` calls.
    pub reads: u64,
    /// Journal records folded.
    pub records: u64,
    /// Bytes of journal output consumed.
    pub bytes: u64,
    /// Events produced.
    pub events: u64,
    /// `true` when the batch stopped at [`SourceOptions::max_batch_bytes`] (more is waiting).
    pub truncated: bool,
}

/// The [`Source`] for one local NTFS volume, addressed by drive letter (mount points and
/// volume GUID paths are #25). Requires an elevated process.
#[derive(Debug)]
pub struct NtfsVolumeSource {
    volume: Volume,
    root: Frn,
    identity: SourceIdentity,
    options: SourceOptions,
    last_changes: ChangeStats,
}

impl NtfsVolumeSource {
    /// Opens a volume (`"C:"`, `"D:\"`, …) with default options.
    ///
    /// # Errors
    /// Everything [`Volume::open`] reports, plus [`Error::Os`] if the root directory or the
    /// volume GUID path cannot be read.
    pub fn open(volume: &str) -> Result<Self> {
        Self::open_with(volume, SourceOptions::default())
    }

    /// Opens a volume with explicit options.
    ///
    /// # Errors
    /// As [`NtfsVolumeSource::open`].
    pub fn open_with(volume: &str, options: SourceOptions) -> Result<Self> {
        let volume = Volume::open(volume)?;
        let root = Frn(volume.root_frn()?);
        let identity = SourceIdentity {
            kind: SOURCE_KIND.to_owned(),
            key: volume.volume_guid_path()?,
            label: format!("{}:", volume.letter()),
        };
        Ok(Self {
            volume,
            root,
            identity,
            options,
            last_changes: ChangeStats::default(),
        })
    }

    /// The underlying volume handle.
    #[must_use]
    pub fn volume(&self) -> &Volume {
        &self.volume
    }

    /// Counters from the last [`Source::changes`] call.
    #[must_use]
    pub fn last_change_stats(&self) -> ChangeStats {
        self.last_changes
    }

    /// The record reported for the root directory, which enumeration never returns (D-31): its
    /// own parent, an empty name, the directory attribute.
    #[must_use]
    pub fn root_record(&self) -> FileRecord {
        FileRecord {
            frn: self.root,
            parent: self.root,
            name: FileName::from(""),
            attrs: FileAttributes(FileAttributes::DIRECTORY),
        }
    }

    /// The current journal state; a journal being deleted counts as none.
    fn journal(&self) -> Result<Option<JournalInfo>> {
        match self.volume.query_journal() {
            Err(Error::JournalDeleteInProgress { .. }) => Ok(None),
            other => other,
        }
    }

    fn checkpoint_for(&self, journal: Option<JournalPosition>) -> Result<Checkpoint, SourceError> {
        NtfsCheckpoint {
            volume_serial: self.volume.info().serial_number,
            journal,
            pending_renames: Vec::new(),
        }
        .to_checkpoint()
        .map_err(checkpoint_error)
    }

    /// Reads the journal from `pos`, folding records into `coalescer`. Returns the USN to resume
    /// from, or the reason a rescan is required.
    fn read_from(
        &mut self,
        pos: JournalPosition,
        coalescer: &mut Coalescer,
    ) -> Result<std::result::Result<i64, RescanReason>, SourceError> {
        let words = self.options.journal_buffer_size.clamp(4 << 10, 16 << 20) / 8;
        let mut buf = vec![0_u64; words];
        let mut usn = pos.next_usn;
        let mut stats = ChangeStats::default();
        let volume = format!("{}:", self.volume.letter());
        loop {
            let reply = match self.volume.read_journal(usn, pos.journal_id, &mut buf) {
                Ok(reply) => reply,
                Err(Error::JournalEntryDeleted { .. }) => {
                    return Ok(Err(RescanReason::JournalWrapped));
                }
                Err(Error::JournalNotActive { .. } | Error::JournalDeleteInProgress { .. }) => {
                    return Ok(Err(RescanReason::JournalMissing));
                }
                Err(e) => return Err(source_error(e)),
            };
            stats.reads += 1;
            let parse = |source| Error::Parse {
                what: "FSCTL_READ_USN_JOURNAL output",
                volume: volume.clone(),
                source,
            };
            match read_step(usn, reply).map_err(|e| source_error(parse(e)))? {
                ReadStep::Records { next_usn } => {
                    for item in RecordIter::new(&reply[8..], 8) {
                        match item {
                            Ok(rec) => coalescer.push(&rec),
                            Err(e @ ParseError::FileIdTooWide { .. }) => {
                                return Err(wide_file_id(&volume, &e));
                            }
                            Err(e) if e.is_record_local() => {
                                // Skipping it would lose a change; the index converges only
                                // through a rescan. Never seen from a real kernel.
                                tracing::warn!(%volume, error = %e, "malformed journal record");
                                return Ok(Err(RescanReason::Other(format!(
                                    "malformed USN journal record on {volume}: {e}"
                                ))));
                            }
                            Err(e) => return Err(source_error(parse(e))),
                        }
                    }
                    usn = next_usn;
                    stats.bytes += reply.len() as u64;
                    if stats.bytes >= self.options.max_batch_bytes {
                        stats.truncated = true;
                        break;
                    }
                }
                ReadStep::Advance { next_usn } => usn = next_usn,
                ReadStep::CaughtUp => break,
                ReadStep::Stalled { next_usn } => {
                    return Err(SourceError::Io(io::Error::other(format!(
                        "reading the USN journal of {volume} stalled at USN {next_usn} \
                         (started at {usn})"
                    ))));
                }
            }
        }
        stats.records = coalescer.records();
        self.last_changes = stats;
        Ok(Ok(usn))
    }
}

impl Source for NtfsVolumeSource {
    fn identity(&self) -> SourceIdentity {
        self.identity.clone()
    }

    fn root(&self) -> Frn {
        self.root
    }

    fn enumerate(&mut self, sink: &mut dyn RecordSink) -> Result<Checkpoint, SourceError> {
        // The checkpoint is the journal position *before* enumeration (D-29).
        let journal = self.journal().map_err(source_error)?;
        let checkpoint = self.checkpoint_for(journal.map(|j| JournalPosition {
            journal_id: j.journal_id,
            next_usn: j.next_usn,
        }))?;
        if sink.accept(self.root_record()).is_break() {
            return Err(SourceError::Cancelled);
        }
        let volume = format!("{}:", self.volume.letter());
        let root = self.root.0;
        let mut enumerator = self.volume.enumerate_with(EnumOptions {
            buffer_size: self.options.enum_buffer_size,
            ..EnumOptions::default()
        });
        let (mut records, mut skipped) = (0_u64, 0_u64);
        while let Some(batch) = enumerator.next_batch().map_err(source_error)? {
            for item in batch {
                match item {
                    Ok(rec) if rec.frn == root => {}
                    Ok(rec) => {
                        records += 1;
                        if let ControlFlow::Break(()) = sink.accept(file_record(&rec)) {
                            return Err(SourceError::Cancelled);
                        }
                    }
                    Err(e @ ParseError::FileIdTooWide { .. }) => {
                        return Err(wide_file_id(&volume, &e));
                    }
                    Err(e) if e.is_record_local() => {
                        skipped += 1;
                        tracing::warn!(%volume, error = %e, "skipping USN record");
                    }
                    Err(source) => {
                        return Err(source_error(Error::Parse {
                            what: "FSCTL_ENUM_USN_DATA record",
                            volume,
                            source,
                        }));
                    }
                }
            }
        }
        let stats = enumerator.stats();
        tracing::info!(
            %volume, records, skipped, batches = stats.batches,
            kernel_ms = stats.kernel_time.as_millis() as u64,
            journal = journal.is_some(), "enumerated volume"
        );
        Ok(checkpoint)
    }

    fn changes(&mut self, since: &Checkpoint) -> Result<Changes, SourceError> {
        let checkpoint = match NtfsCheckpoint::from_checkpoint(since) {
            Ok(cp) => cp,
            Err(e) => {
                tracing::info!(error = %e, "checkpoint not usable; full rescan");
                return Ok(Changes::FullRescanRequired(
                    RescanReason::CheckpointMismatch,
                ));
            }
        };
        let journal = self.journal().map_err(source_error)?;
        let serial = self.volume.info().serial_number;
        let pos = match checkpoint::assess(&checkpoint, serial, journal.as_ref()) {
            Ok(pos) => pos,
            Err(reason) => return Ok(Changes::FullRescanRequired(reason)),
        };
        let mut coalescer = Coalescer::new(checkpoint.pending_renames);
        let next_usn = match self.read_from(pos, &mut coalescer)? {
            Ok(usn) => usn,
            Err(reason) => return Ok(Changes::FullRescanRequired(reason)),
        };
        let (events, pending_renames) = coalescer.finish();
        self.last_changes.events = events.len() as u64;
        let checkpoint = NtfsCheckpoint {
            volume_serial: serial,
            journal: Some(JournalPosition {
                journal_id: pos.journal_id,
                next_usn,
            }),
            pending_renames,
        }
        .to_checkpoint()
        .map_err(checkpoint_error)?;
        Ok(Changes::Batch(ChangeBatch { events, checkpoint }))
    }
}

/// Maps a volume error onto the `Source` contract's error.
fn source_error(e: Error) -> SourceError {
    match e {
        Error::NotElevated { .. } => SourceError::AccessDenied(e.to_string()),
        Error::InvalidVolume(_)
        | Error::NotNtfs { .. }
        | Error::NotLocal { .. }
        | Error::Unsupported { .. } => SourceError::Unsupported(e.to_string()),
        other => SourceError::Io(io::Error::other(other)),
    }
}

/// 128-bit file IDs (ReFS-style `FILE_ID_128`) do not fit `Frn(u64)` (#24): refuse the volume
/// rather than truncate IDs.
fn wide_file_id(volume: &str, e: &ParseError) -> SourceError {
    SourceError::Unsupported(format!(
        "{volume} returned a 128-bit file ID, which does not fit a 64-bit FRN ({e})"
    ))
}

fn checkpoint_error(e: CheckpointError) -> SourceError {
    SourceError::Io(io::Error::other(e))
}

impl From<Error> for SourceError {
    fn from(e: Error) -> Self {
        source_error(e)
    }
}
