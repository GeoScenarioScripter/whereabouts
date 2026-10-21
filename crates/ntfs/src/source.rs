//! [`NtfsVolumeSource`]: the `Source` implementation for one local NTFS volume (Windows only).
//!
//! - [`Source::enumerate`] queries the journal first (that position is the checkpoint, D-29),
//!   then streams a synthesised root record followed by every `FSCTL_ENUM_USN_DATA` record.
//! - [`Source::changes`] checks the checkpoint against the volume and journal
//!   ([`checkpoint::assess`]), reads the journal with non-blocking `FSCTL_READ_USN_JOURNAL`
//!   calls ([`read_batch`], pure) and folds the records into events ([`Coalescer`]).
//!
//! Nothing here writes to the volume; in particular no journal is ever created (D-31). A volume
//! without an active journal can be enumerated, but every `changes` call then asks for a full
//! rescan until someone creates a journal.
//!
//! Only genuine OS failures become `SourceError::Io`. Anything wrong with the journal's
//! *content* (malformed or stalled replies) becomes `FullRescanRequired(Other)`, because the
//! same read would fail again on every poll.

use std::io;
use std::ops::ControlFlow;
use std::time::Duration;

use whereabouts_core::{FileAttributes, FileName, FileRecord, Frn};
use whereabouts_source::{
    ChangeBatch, Changes, Checkpoint, CheckpointError, RecordSink, RescanReason, Source,
    SourceError, SourceIdentity,
};

use crate::checkpoint::{self, JournalPosition, NtfsCheckpoint};
use crate::error::{Error, ParseError, Result, win32};
use crate::journal::{BatchEnd, Coalescer, JournalReader, file_record, read_batch};
use crate::record::JournalInfo;
use crate::volume::{EnumOptions, Volume};

/// `SourceIdentity::kind` of NTFS volumes.
pub const SOURCE_KIND: &str = "ntfs";

/// Options for [`NtfsVolumeSource`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceOptions {
    /// Output buffer for `FSCTL_ENUM_USN_DATA` (see [`EnumOptions::buffer_size`]).
    pub enum_buffer_size: usize,
    /// Output buffer per `FSCTL_READ_USN_JOURNAL` call in bytes, clamped to 1 KiB ..= 16 MiB
    /// (1 KiB holds the largest `USN_RECORD_V2` plus the reply header).
    pub journal_buffer_size: usize,
    /// Soft cap per [`Source::changes`] call: once this many bytes of journal records have been
    /// read **and** the batch holds at least one event, the batch ends and the rest waits for
    /// the next call. A truncated batch is therefore never empty; call `changes` again until a
    /// batch comes back empty.
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

/// Counters from the last [`Source::changes`] call (for logs and benchmarks). Reset at the start
/// of every call, so after a rescan or an error they describe that call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChangeStats {
    /// `FSCTL_READ_USN_JOURNAL` calls.
    pub reads: u64,
    /// Journal records folded.
    pub records: u64,
    /// Bytes of journal records consumed.
    pub bytes: u64,
    /// Events produced.
    pub events: u64,
    /// `true` when the batch stopped at [`SourceOptions::max_batch_bytes`] (more is waiting).
    pub truncated: bool,
}

/// Counters from the last [`Source::enumerate`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EnumerateStats {
    /// Records passed to the sink (the synthesised root included).
    pub records: u64,
    /// Records skipped because the kernel returned them malformed (logged at `error`). A
    /// non-zero count means the enumeration is incomplete.
    pub skipped: u64,
    /// `FSCTL_ENUM_USN_DATA` calls that returned data.
    pub batches: u64,
    /// Time spent inside the kernel.
    pub kernel_time: Duration,
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
    last_enumerate: EnumerateStats,
}

/// [`JournalReader`] over the volume, with one reusable 8-byte-aligned buffer.
struct VolumeReader<'v> {
    volume: &'v Volume,
    journal_id: u64,
    buf: Vec<u64>,
}

impl JournalReader for VolumeReader<'_> {
    type Error = Error;

    fn read(&mut self, start_usn: i64) -> Result<&[u8]> {
        self.volume
            .read_journal(start_usn, self.journal_id, &mut self.buf)
    }
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
            last_enumerate: EnumerateStats::default(),
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

    /// Counters from the last [`Source::enumerate`] call, including skipped malformed records.
    #[must_use]
    pub fn last_enumerate_stats(&self) -> EnumerateStats {
        self.last_enumerate
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

    fn label(&self) -> String {
        format!("{}:", self.volume.letter())
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

    /// Maps a failed journal read: lost history becomes a rescan reason, anything else stays an
    /// error. `ERROR_INVALID_PARAMETER` is what the read returns when `journal_id` is no longer
    /// the active journal (re-created between `assess` and the read), so the journal is queried
    /// again to tell that apart from a genuine failure.
    fn read_failure(&self, e: Error, journal_id: u64) -> Result<RescanReason, SourceError> {
        match e {
            Error::JournalEntryDeleted { .. } => Ok(RescanReason::JournalWrapped),
            Error::JournalNotActive { .. } | Error::JournalDeleteInProgress { .. } => {
                Ok(RescanReason::JournalMissing)
            }
            Error::Os {
                code: win32::ERROR_INVALID_PARAMETER,
                ..
            } => match self.journal() {
                Ok(None) => Ok(RescanReason::JournalMissing),
                Ok(Some(j)) if j.journal_id != journal_id => Ok(RescanReason::JournalRecreated),
                _ => Err(source_error(e)),
            },
            other => Err(source_error(other)),
        }
    }

    /// The body of [`Source::changes`] once the checkpoint is decoded.
    fn changes_since(&mut self, checkpoint: NtfsCheckpoint) -> Result<Changes, SourceError> {
        let journal = self.journal().map_err(source_error)?;
        let serial = self.volume.info().serial_number;
        let pos = match checkpoint::assess(&checkpoint, serial, journal.as_ref()) {
            Ok(pos) => pos,
            Err(reason) => return Ok(Changes::FullRescanRequired(reason)),
        };
        let mut coalescer = Coalescer::new(checkpoint.pending_renames);
        let words = self.options.journal_buffer_size.clamp(1 << 10, 16 << 20) / 8;
        let mut reader = VolumeReader {
            volume: &self.volume,
            journal_id: pos.journal_id,
            buf: vec![0_u64; words],
        };
        let read = match read_batch(
            &mut reader,
            pos.next_usn,
            self.options.max_batch_bytes,
            &mut coalescer,
        ) {
            Ok(read) => read,
            Err(e) => {
                return self
                    .read_failure(e, pos.journal_id)
                    .map(Changes::FullRescanRequired);
            }
        };
        self.last_changes.reads = read.reads;
        self.last_changes.bytes = read.bytes;
        self.last_changes.records = coalescer.records();
        match read.end {
            BatchEnd::CaughtUp => {}
            BatchEnd::Truncated => self.last_changes.truncated = true,
            BatchEnd::Rescan(reason) => return Ok(Changes::FullRescanRequired(reason)),
            BatchEnd::Unsupported(what) => {
                return Err(SourceError::Unsupported(format!(
                    "{}: {what} (#24)",
                    self.label()
                )));
            }
        }
        // StartUsn = 0 is special to the kernel: "from the first record still in the journal",
        // with no ERROR_JOURNAL_ENTRY_DELETED if the journal wrapped after `assess` saw
        // FirstUsn = 0. For any other start the kernel itself refuses purged USNs, so only this
        // case needs the second look.
        if pos.next_usn == 0 {
            match self.journal().map_err(source_error)? {
                None => return Ok(Changes::FullRescanRequired(RescanReason::JournalMissing)),
                Some(j) if j.journal_id != pos.journal_id => {
                    return Ok(Changes::FullRescanRequired(RescanReason::JournalRecreated));
                }
                Some(j) if j.first_usn > pos.next_usn => {
                    return Ok(Changes::FullRescanRequired(RescanReason::JournalWrapped));
                }
                Some(_) => {}
            }
        }
        let (events, pending_renames) = coalescer.finish();
        self.last_changes.events = events.len() as u64;
        let checkpoint = NtfsCheckpoint {
            volume_serial: serial,
            journal: Some(JournalPosition {
                journal_id: pos.journal_id,
                next_usn: read.next_usn,
            }),
            pending_renames,
        }
        .to_checkpoint()
        .map_err(checkpoint_error)?;
        Ok(Changes::Batch(ChangeBatch { events, checkpoint }))
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
        self.last_enumerate = EnumerateStats::default();
        // The checkpoint is the journal position *before* enumeration (D-29).
        let journal = self.journal().map_err(source_error)?;
        let checkpoint = self.checkpoint_for(journal.map(|j| JournalPosition {
            journal_id: j.journal_id,
            next_usn: j.next_usn,
        }))?;
        if sink.accept(self.root_record()).is_break() {
            return Err(SourceError::Cancelled);
        }
        let volume = self.label();
        let root = self.root.0;
        let mut stats = EnumerateStats {
            records: 1,
            ..EnumerateStats::default()
        };
        let mut enumerator = self.volume.enumerate_with(EnumOptions {
            buffer_size: self.options.enum_buffer_size,
            ..EnumOptions::default()
        });
        let outcome = (|| {
            while let Some(batch) = enumerator.next_batch().map_err(source_error)? {
                for item in batch {
                    match item {
                        Ok(rec) if rec.frn == root => {}
                        Ok(rec) => {
                            stats.records += 1;
                            if let ControlFlow::Break(()) = sink.accept(file_record(&rec)) {
                                return Err(SourceError::Cancelled);
                            }
                        }
                        Err(e @ ParseError::FileIdTooWide { .. }) => {
                            return Err(SourceError::Unsupported(format!(
                                "{volume} returned a 128-bit file ID, which does not fit a \
                                 64-bit FRN ({e}) (#24)"
                            )));
                        }
                        Err(e) if e.is_record_local() => {
                            stats.skipped += 1;
                            tracing::error!(
                                %volume, error = %e,
                                "skipping a malformed record: this entry is missing from the index"
                            );
                        }
                        Err(source) => {
                            return Err(source_error(Error::Parse {
                                what: "FSCTL_ENUM_USN_DATA record",
                                volume: volume.clone(),
                                source,
                            }));
                        }
                    }
                }
            }
            Ok(())
        })();
        let kernel = enumerator.stats();
        stats.batches = kernel.batches;
        stats.kernel_time = kernel.kernel_time;
        self.last_enumerate = stats;
        outcome?;
        if stats.skipped > 0 {
            tracing::error!(
                %volume, skipped = stats.skipped,
                "enumeration skipped malformed records; the index is incomplete"
            );
        }
        tracing::info!(
            %volume, records = stats.records, skipped = stats.skipped, batches = stats.batches,
            kernel_ms = stats.kernel_time.as_millis() as u64,
            journal = journal.is_some(), "enumerated volume"
        );
        Ok(checkpoint)
    }

    fn changes(&mut self, since: &Checkpoint) -> Result<Changes, SourceError> {
        self.last_changes = ChangeStats::default();
        match NtfsCheckpoint::from_checkpoint(since) {
            Ok(cp) => self.changes_since(cp),
            Err(e) => {
                tracing::info!(error = %e, "checkpoint not usable; full rescan");
                Ok(Changes::FullRescanRequired(
                    RescanReason::CheckpointMismatch,
                ))
            }
        }
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

fn checkpoint_error(e: CheckpointError) -> SourceError {
    SourceError::Io(io::Error::other(e))
}

impl From<Error> for SourceError {
    fn from(e: Error) -> Self {
        source_error(e)
    }
}
